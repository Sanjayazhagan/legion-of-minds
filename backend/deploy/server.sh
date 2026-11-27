#!/usr/bin/env bash
# Runs on the VM, started by deploy.ps1 over ssh. Installs what the server
# needs (first deploy only), puts the new server in place (the ready-made one
# GitHub built, or built here from the uploaded source if there is none),
# sets up Postgres and the env file, and (re)starts it as a systemd service.
#
#   bash server.sh <app_dir> <port> <public_host> [domain] [size]
#
# size: auto (default: from this machine's memory and disk), small, medium,
# large or keep (don't touch the sizing settings).
#
# Reads /tmp/legion-deploy.tar.gz (source, plus backend/deploy-bin with the
# ready-made servers when deploy.ps1 got them) and, if present,
# /tmp/legion-deploy.env (secrets from the laptop, merged into the env file).
#
# The VM also runs the portfolio (pm2, its own folder, maybe nginx). This
# script never touches pm2 or the portfolio folder, refuses a port or domain
# something else already uses, and only adds its own nginx site.
set -euo pipefail

APP="$1"
PORT="$2"
HOST="$3"
DOMAIN="${4:-}"
SIZE="${5:-auto}"
SERVICE=legion-server
ENV_FILE="$APP/server.env"
SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"
PG="sudo -u postgres"
APT="$SUDO env DEBIAN_FRONTEND=noninteractive apt-get"

say() { echo "  $*"; }
fail() { echo "  ERROR: $*" >&2; exit 1; }
port_busy() { ss -ltnH "( sport = :$1 )" 2>/dev/null | grep -q .; }

# --- Packages (first deploy only) ---------------------------------------
# Compilers are only installed if the server has to be built here.
need=()
command -v psql >/dev/null 2>&1 || need+=(postgresql)
command -v curl >/dev/null 2>&1 || need+=(curl)
command -v openssl >/dev/null 2>&1 || need+=(openssl)
if [ ${#need[@]} -gt 0 ]; then
  say "Installing: ${need[*]}"
  $APT update -qq
  $APT install -y -qq "${need[@]}" >/dev/null
fi
$SUDO systemctl enable --now postgresql >/dev/null 2>&1 || true

# --- Don't collide with the portfolio or anything else --------------------
# The port must be free, unless it's our own server already running there.
if port_busy "$PORT" && ! $SUDO systemctl is-active --quiet $SERVICE; then
  fail "Port $PORT is already used by another program on the VM (the portfolio?). Pick another one: .\deploy.ps1 -Port 8081"
fi
if [ -n "$DOMAIN" ] && command -v nginx >/dev/null 2>&1; then
  # Another nginx site (for example the portfolio's) already answers for
  # this domain: adding ours would fight over it.
  other=$($SUDO grep -rlE "server_name[^;]*[[:space:]]$DOMAIN[[:space:];]" /etc/nginx/sites-enabled/ /etc/nginx/conf.d/ 2>/dev/null | grep -v "/$SERVICE\$" || true)
  [ -z "$other" ] || fail "$DOMAIN is already used by another nginx site ($other). Use a separate subdomain for the game, like game.yourdomain.com."
fi

# --- Back up the database before anything changes -------------------------
# Like the portfolio's pre-deploy backup. The new server may add tables when
# it starts, so keep a copy from before. Keeps the newest 10.
if $PG psql -qtAc "SELECT 1 FROM pg_database WHERE datname='legion'" 2>/dev/null | grep -q 1; then
  BK="$APP/db-backups"
  mkdir -p "$BK"
  chmod 700 "$BK"
  TS=$(date +%Y%m%d-%H%M%S)
  if $PG pg_dump -Fc legion > "$BK/legion-$TS.dump"; then
    chmod 600 "$BK/legion-$TS.dump"
    say "Backed up the database -> $BK/legion-$TS.dump"
    say "(restore with: sudo -u postgres pg_restore --clean -d legion $BK/legion-$TS.dump)"
  else
    rm -f "$BK/legion-$TS.dump"
    fail "Database backup failed, so nothing was changed. The server keeps running."
  fi
  ls -1t "$BK"/legion-*.dump 2>/dev/null | tail -n +11 | xargs -r rm -f || true
else
  say "No database yet (first deploy): nothing to back up."
fi

# --- Building on the VM (only when there is no ready-made server) ---------
build_on_vm() {
need=()
command -v cc >/dev/null 2>&1 || need+=(build-essential)
command -v pkg-config >/dev/null 2>&1 || need+=(pkg-config)
if [ ${#need[@]} -gt 0 ]; then
  say "Installing: ${need[*]}"
  $APT update -qq
  $APT install -y -qq "${need[@]}" >/dev/null
fi

# A Rust release build (and even installing Rust) needs more memory than
# the smallest VMs have, and the portfolio is using some of it too. Make sure
# memory plus swap is at least about 5 GB; the kernel otherwise kills the
# install or build ("Killed").
mem_mb=$(awk '/MemTotal/ {print int($2/1024)}' /proc/meminfo)
swap_mb=$(awk '/SwapTotal/ {print int($2/1024)}' /proc/meminfo)
want_mb=5000
if [ $((mem_mb + swap_mb)) -lt $want_mb ] && [ ! -f /swapfile.legion ]; then
  add_gb=$(( (want_mb - mem_mb - swap_mb + 1023) / 1024 ))
  [ "$add_gb" -ge 2 ] || add_gb=2
  free_gb=$(df -BG --output=avail / | tail -n 1 | tr -dc '0-9')
  if [ "${free_gb:-0}" -lt $((add_gb + 3)) ]; then
    fail "Only ${mem_mb} MB memory + ${swap_mb} MB swap, and not enough free disk (${free_gb} GB) for a ${add_gb} GB swap file. Free some disk space on the VM."
  fi
  say "Only ${mem_mb} MB memory + ${swap_mb} MB swap: adding a ${add_gb} GB swap file for the build"
  $SUDO fallocate -l "${add_gb}G" /swapfile.legion
  $SUDO chmod 600 /swapfile.legion
  $SUDO mkswap /swapfile.legion >/dev/null
  $SUDO swapon /swapfile.legion
  echo '/swapfile.legion none swap sw 0 0' | $SUDO tee -a /etc/fstab >/dev/null
fi
# Fewer parallel compile jobs on a small VM, so the build doesn't run out of
# memory: one per 2 GB of RAM, at least 1.
jobs=$(( mem_mb / 2048 ))
[ "$jobs" -ge 1 ] || jobs=1
cpus=$(nproc)
[ "$jobs" -le "$cpus" ] || jobs=$cpus

# --- Rust ----------------------------------------------------------------
if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
if ! command -v rustup >/dev/null 2>&1 && ! command -v cargo >/dev/null 2>&1; then
  say "Installing Rust (rustup, minimal profile)"
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none >/dev/null
  . "$HOME/.cargo/env"
fi
# rustup can be left without a working default toolchain: none set ("rustup
# could not choose a version of cargo to run"), or half installed after an
# install was killed ("error reading rustc version"). Reinstall stable, small.
if command -v rustup >/dev/null 2>&1 && ! cargo --version >/dev/null 2>&1; then
  say "Setting up Rust stable (minimal profile)"
  rustup set profile minimal >/dev/null 2>&1 || true
  if ! rustup toolchain install stable --profile minimal >/dev/null; then
    rustup toolchain uninstall stable >/dev/null 2>&1 || true
    rustup toolchain install stable --profile minimal >/dev/null
  fi
  rustup default stable >/dev/null
  if ! cargo --version >/dev/null 2>&1; then
    # Still broken (for example a half-installed toolchain): start it over.
    rustup toolchain uninstall stable >/dev/null 2>&1 || true
    rustup toolchain install stable --profile minimal >/dev/null
    rustup default stable >/dev/null
  fi
fi
cargo --version >/dev/null 2>&1 || fail "cargo doesn't run on the VM. Run 'rustup toolchain uninstall stable' there, then deploy again."
say "Using $(cargo --version), building with $jobs job(s)"

say "Building the server on the VM (the first build takes several minutes)..."
# The target folder lives outside src so later builds only redo what changed.
(cd "$APP/src/backend" && CARGO_TARGET_DIR="$APP/target" CARGO_BUILD_JOBS="$jobs" cargo build --release --locked -j "$jobs" -p bag-server)
install -m 755 "$APP/target/release/bag-server" "$APP/bag-server.new"
say "Build complete."
}

# --- Unpack ---------------------------------------------------------------
mkdir -p "$APP/src"
rm -rf "$APP/src/backend" "$APP/src/game"
tar -xzf /tmp/legion-deploy.tar.gz -C "$APP/src"
rm -f /tmp/legion-deploy.tar.gz

# --- Ready-made server from GitHub, or build here ----------------------------
# deploy.ps1 puts the servers GitHub Actions built (.github/workflows/
# server.yml) in backend/deploy-bin. Use the one for this processor if it
# runs here; otherwise fall back to building from the uploaded source.
BIN="$APP/src/backend/deploy-bin"
arch=$(uname -m)
[ "$arch" != arm64 ] || arch=aarch64
prebuilt="$BIN/bag-server-$arch"
use_prebuilt=false
if [ -f "$prebuilt" ]; then
  chmod 755 "$prebuilt"
  # Started with no settings it must stop with "DATABASE_URL is required".
  # That proves it loads and runs on this VM (right processor, new enough
  # system libraries) without touching the running server.
  out=$(env -i timeout 10 "$prebuilt" 2>&1 || true)
  case "$out" in
    *DATABASE_URL*) use_prebuilt=true ;;
    *) say "The ready-made server does not run on this VM ($arch): ${out:-no output}"
       say "Building here instead." ;;
  esac
elif [ -d "$BIN" ]; then
  say "No ready-made server for this VM's processor ($arch): building here instead."
fi

if $use_prebuilt; then
  install -m 755 "$prebuilt" "$APP/bag-server.new"
  commit=$(head -c 7 "$BIN/COMMIT" 2>/dev/null || true)
  say "Using the server GitHub built for $arch (commit ${commit:-unknown}). Nothing to compile."
else
  build_on_vm
fi
# Keep the last working server, in case the new one needs rolling back.
[ -f "$APP/bag-server" ] && cp -f "$APP/bag-server" "$APP/bag-server.prev"
mv -f "$APP/bag-server.new" "$APP/bag-server"

# --- Env file ------------------------------------------------------------
touch "$ENV_FILE"
chmod 600 "$ENV_FILE"
has() { grep -q "^$1=" "$ENV_FILE"; }
set_default() { has "$1" || echo "$1=$2" >> "$ENV_FILE"; }
set_value() { sed -i "/^$1=/d" "$ENV_FILE"; echo "$1=$2" >> "$ENV_FILE"; }

if [ -f /tmp/legion-deploy.env ]; then
  # Same merge as the portfolio: only the keys in deploy.env are replaced or
  # added; everything else already on the VM stays.
  tr -d '\r' < /tmp/legion-deploy.env | grep -E '^[A-Za-z_][A-Za-z0-9_]*=' > /tmp/legion-deploy.env.clean || true
  cut -d= -f1 /tmp/legion-deploy.env.clean | while read -r k; do sed -i "/^$k=/d" "$ENV_FILE"; done
  cat /tmp/legion-deploy.env.clean >> "$ENV_FILE"
  # Remember which keys deploy.env set, so the sizing below leaves them be.
  cut -d= -f1 /tmp/legion-deploy.env.clean > /tmp/legion-deploy.env.keys
  rm -f /tmp/legion-deploy.env /tmp/legion-deploy.env.clean
  say "Merged deploy.env into $ENV_FILE"
fi

# --- Sizing: how much this machine takes ---------------------------------
# A 1 GB VM (Azure B1) must refuse 1024-neuron brains and keep little battle
# log; a 24 GB Oracle Ampere box can take everything. Picked from this
# machine on every deploy, so moving or resizing the VM follows by itself.
# A value set in deploy.env always wins (and isn't touched here).
AUTO_KEYS="$APP/auto-size.keys"   # keys this script set, so it may change them
pinned() { [ -f /tmp/legion-deploy.env.keys ] && grep -qx "$1" /tmp/legion-deploy.env.keys; }
size_set() {
  # Only change a key this script set before, or one nobody set at all.
  # A key deploy.env set is also marked: drop it from deploy.env later and
  # the sizing takes it over again.
  if pinned "$1"; then
    grep -qx "$1" "$AUTO_KEYS" 2>/dev/null || echo "$1" >> "$AUTO_KEYS"
    say "  $1 from deploy.env: $(grep "^$1=" "$ENV_FILE" | tail -n 1 | cut -d= -f2-) (this machine: $2)"; return
  fi
  if has "$1" && ! grep -qx "$1" "$AUTO_KEYS" 2>/dev/null; then
    say "  $1 kept as set by hand in $ENV_FILE: $(grep "^$1=" "$ENV_FILE" | tail -n 1 | cut -d= -f2-) (delete that line to size it automatically)"; return
  fi
  set_value "$1" "$2"
  grep -qx "$1" "$AUTO_KEYS" 2>/dev/null || echo "$1" >> "$AUTO_KEYS"
  say "  $1=$2"
}
if [ "$SIZE" != keep ]; then
  mem_mb=$(awk '/MemTotal/ {print int($2/1024)}' /proc/meminfo)
  cpus=$(nproc)
  disk_gb=$(df -BG --output=size "$APP" | tail -n 1 | tr -dc '0-9')
  if [ "$SIZE" = auto ]; then
    if [ "$mem_mb" -ge 12000 ] && [ "$cpus" -ge 3 ]; then SIZE=large
    elif [ "$mem_mb" -ge 3500 ] && [ "$cpus" -ge 2 ]; then SIZE=medium
    else SIZE=small; fi
  fi
  # gb_day: battle log a day at ~1,000 raids (100 daily players) if every
  # army had the biggest brains allowed (measured 2026-11-25: 4, ~10 and
  # ~33 MB a raid). Real armies mix sizes, so this is the worst case.
  case "$SIZE" in
    # 1-2 GB, 1-2 cores (README "How many players").
    small)  brain=256;  pool=6;  gb_day=4 ;;
    # 4-8 GB, 2+ cores: 512 brains are ~3-4 s and ~350 MB at 10 at once.
    medium) brain=512;  pool=8;  gb_day=10 ;;
    # 12+ GB, 3+ cores (Oracle Ampere A1 4 cores / 24 GB): everything.
    large)  brain=1024; pool=12; gb_day=33 ;;
    *) fail "Unknown size '$SIZE' (use auto, small, medium, large or keep)." ;;
  esac
  # Battle log days: half the disk for raids at the worst case, 3 to 14.
  days=$(( ${disk_gb:-0} / 2 / gb_day ))
  [ "$days" -ge 3 ] || days=3
  [ "$days" -le 14 ] || days=14
  say "Sizing for this machine (${mem_mb} MB memory, ${cpus} cores, ${disk_gb} GB disk): $SIZE"
  size_set MAX_ONLINE_BRAIN "$brain"
  size_set DB_POOL "$pool"
  size_set KEEP_RAIDS_DAYS "$days"
fi
rm -f /tmp/legion-deploy.env.keys

if ! has DATABASE_URL; then
  # First deploy: a database and a user with a random password, made here
  # so the password never leaves the VM.
  pw=$(openssl rand -hex 24)
  if $PG psql -qtAc "SELECT 1 FROM pg_roles WHERE rolname='legion'" | grep -q 1; then
    $PG psql -qc "ALTER ROLE legion WITH LOGIN PASSWORD '$pw'"
  else
    $PG psql -qc "CREATE ROLE legion WITH LOGIN PASSWORD '$pw'"
  fi
  if ! $PG psql -qtAc "SELECT 1 FROM pg_database WHERE datname='legion'" | grep -q 1; then
    $PG createdb -O legion legion
  fi
  echo "DATABASE_URL=postgres://legion:$pw@127.0.0.1/legion" >> "$ENV_FILE"
  say "Created the Postgres database 'legion' (password stored only in $ENV_FILE)"
fi

if [ -n "$DOMAIN" ]; then
  # Behind the HTTPS proxy: only reachable through it.
  set_value BIND_ADDR "127.0.0.1:$PORT"
  set_value PUBLIC_URL "https://$DOMAIN"
  # Rate limits read the client's address from the proxy's X-Forwarded-For.
  set_value TRUST_PROXY true
else
  set_default BIND_ADDR "0.0.0.0:$PORT"
  set_default PUBLIC_URL "http://$HOST:$PORT"
  set_value TRUST_PROXY false
fi
set_default RUST_LOG "info,sqlx=warn"

# --- Service -------------------------------------------------------------
$SUDO tee /etc/systemd/system/$SERVICE.service >/dev/null <<UNIT
[Unit]
Description=Legion of Minds game server
After=network-online.target postgresql.service
Wants=network-online.target

[Service]
User=$(id -un)
WorkingDirectory=$APP
EnvironmentFile=$ENV_FILE
# Fewer malloc arenas: battle threads otherwise each keep their own, and
# memory stays high long after a burst of raids (matters on a 1 GB VM).
Environment=MALLOC_ARENA_MAX=2
ExecStart=$APP/bag-server
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
UNIT
$SUDO systemctl daemon-reload
$SUDO systemctl enable $SERVICE >/dev/null 2>&1
$SUDO systemctl restart $SERVICE
say "Service $SERVICE restarted."

# --- Firewall on the VM itself -------------------------------------------
# Oracle Cloud's Ubuntu images ship iptables rules that reject every port
# but 22, on top of the cloud's own security list (opened in the Oracle
# console). Azure's images have no such rules. Open only what players need:
# 80 and 443 with a domain (certbot needs 80 too), otherwise the server port.
# Rules go before the final REJECT and are saved so they survive a reboot.
if [ -n "$DOMAIN" ]; then open_ports="80 443"; else open_ports="$PORT"; fi
opened=""
for ipt in iptables ip6tables; do
  command -v $ipt >/dev/null 2>&1 || continue
  reject=$($SUDO $ipt -L INPUT --line-numbers -n 2>/dev/null | awk '$2=="REJECT" {print $1; exit}')
  [ -n "$reject" ] || continue
  for p in $open_ports; do
    if ! $SUDO $ipt -C INPUT -p tcp -m state --state NEW --dport "$p" -j ACCEPT 2>/dev/null; then
      $SUDO $ipt -I INPUT "$reject" -p tcp -m state --state NEW --dport "$p" -j ACCEPT
      opened="$opened $ipt:$p"
    fi
  done
done
if [ -n "$opened" ]; then
  if command -v netfilter-persistent >/dev/null 2>&1; then
    $SUDO netfilter-persistent save >/dev/null 2>&1 || true
  elif [ -d /etc/iptables ]; then
    $SUDO sh -c 'iptables-save > /etc/iptables/rules.v4' || true
    command -v ip6tables-save >/dev/null 2>&1 && $SUDO sh -c 'ip6tables-save > /etc/iptables/rules.v6' || true
  fi
  say "Opened in the VM's own firewall:$opened"
fi
# ufw (off on Azure and Oracle images, but some people turn it on).
if command -v ufw >/dev/null 2>&1 && $SUDO ufw status 2>/dev/null | grep -q "Status: active"; then
  for p in $open_ports; do $SUDO ufw allow "$p/tcp" >/dev/null; done
  say "Allowed $open_ports in ufw"
fi

# --- HTTPS (only with a domain) -------------------------------------------
if [ -n "$DOMAIN" ]; then
  if command -v nginx >/dev/null 2>&1; then
    # nginx is already there (for example, serving the portfolio): add a site.
    conf=/etc/nginx/sites-available/$SERVICE
    if [ ! -f "$conf" ]; then
      $SUDO tee "$conf" >/dev/null <<NGINX
server {
    listen 80;
    server_name $DOMAIN;
    client_max_body_size 40m;
    location / {
        proxy_pass http://127.0.0.1:$PORT;
        proxy_set_header Host \$host;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
    }
}
NGINX
      $SUDO ln -sf "$conf" /etc/nginx/sites-enabled/$SERVICE
      # Check the whole nginx setup before reloading, so a mistake here can
      # never take the portfolio down: undo our site and stop instead.
      if ! $SUDO nginx -t; then
        $SUDO rm -f /etc/nginx/sites-enabled/$SERVICE "$conf"
        fail "nginx rejected the new site, so it was removed again. The portfolio is unchanged."
      fi
      $SUDO systemctl reload nginx
    fi
    # Sites made before big brains allowed only 8 MB uploads: raise it.
    if $SUDO grep -q "client_max_body_size 8m;" "$conf"; then
      $SUDO sed -i 's/client_max_body_size 8m;/client_max_body_size 40m;/' "$conf"
      $SUDO nginx -t && $SUDO systemctl reload nginx
    fi
    if ! $SUDO test -d "/etc/letsencrypt/live/$DOMAIN"; then
      command -v certbot >/dev/null 2>&1 || $APT install -y -qq certbot python3-certbot-nginx >/dev/null
      $SUDO certbot --nginx -d "$DOMAIN" --non-interactive --agree-tos --register-unsafely-without-email --redirect
    fi
  else
    # No nginx: Caddy gets and renews the certificate itself, but only if
    # nothing else (like the portfolio) already holds ports 80 and 443.
    if ! command -v caddy >/dev/null 2>&1 && { port_busy 80 || port_busy 443; }; then
      fail "Ports 80/443 are used by another program and nginx isn't installed. Set up HTTPS for $DOMAIN in that program, pointing at 127.0.0.1:$PORT."
    fi
    command -v caddy >/dev/null 2>&1 || $APT install -y -qq caddy >/dev/null
    if ! $SUDO grep -q "^$DOMAIN " /etc/caddy/Caddyfile 2>/dev/null; then
      printf '\n%s {\n    reverse_proxy 127.0.0.1:%s\n}\n' "$DOMAIN" "$PORT" | $SUDO tee -a /etc/caddy/Caddyfile >/dev/null
    fi
    $SUDO systemctl reload caddy || $SUDO systemctl restart caddy
  fi
  say "HTTPS set up for $DOMAIN"
fi

# --- Check ---------------------------------------------------------------
for _ in $(seq 1 20); do
  if curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
    url=$(grep '^PUBLIC_URL=' "$ENV_FILE" | tail -n 1 | cut -d= -f2-)
    say "Server is up. Players connect to: $url"
    say "OAuth redirect URLs: $url/v1/auth/google/callback and $url/v1/auth/discord/callback"
    say "Sign-ins turned on: $(curl -fsS "http://127.0.0.1:$PORT/v1/auth/providers")"
    exit 0
  fi
  sleep 1
done
echo "The server did not answer on port $PORT. Last log lines:" >&2
$SUDO journalctl -u $SERVICE -n 40 --no-pager >&2 || true
exit 1
