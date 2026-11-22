#!/usr/bin/env bash
# Runs on the VM, started by deploy.ps1 over ssh. Installs what the server
# needs (first deploy only), builds it from the uploaded source, sets up
# Postgres and the env file, and (re)starts it as a systemd service.
#
#   bash server.sh <app_dir> <port> <public_host> [domain]
#
# Reads /tmp/legion-deploy.tar.gz (source) and, if present,
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
need=()
command -v cc >/dev/null 2>&1 || need+=(build-essential)
command -v pkg-config >/dev/null 2>&1 || need+=(pkg-config)
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

# A Rust release build needs more memory than the smallest VMs have.
mem_mb=$(awk '/MemTotal/ {print int($2/1024)}' /proc/meminfo)
swap_mb=$(awk '/SwapTotal/ {print int($2/1024)}' /proc/meminfo)
if [ "$mem_mb" -lt 3000 ] && [ "$swap_mb" -lt 1000 ] && [ ! -f /swapfile ]; then
  say "Only ${mem_mb} MB of memory: adding a 2 GB swap file for the build"
  $SUDO fallocate -l 2G /swapfile
  $SUDO chmod 600 /swapfile
  $SUDO mkswap /swapfile >/dev/null
  $SUDO swapon /swapfile
  echo '/swapfile none swap sw 0 0' | $SUDO tee -a /etc/fstab >/dev/null
fi

# --- Rust ----------------------------------------------------------------
if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
if ! command -v cargo >/dev/null 2>&1; then
  say "Installing Rust (rustup, minimal profile)"
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null
  . "$HOME/.cargo/env"
fi

# --- Source and build ----------------------------------------------------
mkdir -p "$APP/src"
rm -rf "$APP/src/backend" "$APP/src/game"
tar -xzf /tmp/legion-deploy.tar.gz -C "$APP/src"
rm -f /tmp/legion-deploy.tar.gz
say "Building the server (the first build takes several minutes)..."
# The target folder lives outside src so later builds only redo what changed.
(cd "$APP/src/backend" && CARGO_TARGET_DIR="$APP/target" cargo build --release --locked -p bag-server)
install -m 755 "$APP/target/release/bag-server" "$APP/bag-server.new"
# Keep the last working server, in case the new one needs rolling back.
[ -f "$APP/bag-server" ] && cp -f "$APP/bag-server" "$APP/bag-server.prev"
mv -f "$APP/bag-server.new" "$APP/bag-server"
say "Build complete."

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
  rm -f /tmp/legion-deploy.env /tmp/legion-deploy.env.clean
  say "Merged deploy.env into $ENV_FILE"
fi

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
