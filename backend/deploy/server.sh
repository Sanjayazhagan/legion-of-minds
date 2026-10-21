#!/usr/bin/env bash
# Runs on the VM, started by deploy.ps1 over ssh. Installs what the server
# needs (first deploy only), builds it from the uploaded source, sets up
# Postgres and the env file, and (re)starts it as a systemd service.
#
#   bash server.sh <app_dir> <port> <public_host> [domain]
#
# Reads /tmp/legion-deploy.tar.gz (source) and, if present,
# /tmp/legion-deploy.env (secrets from the laptop, merged into the env file).
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
else
  set_default BIND_ADDR "0.0.0.0:$PORT"
  set_default PUBLIC_URL "http://$HOST:$PORT"
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
    client_max_body_size 8m;
    location / {
        proxy_pass http://127.0.0.1:$PORT;
        proxy_set_header Host \$host;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
    }
}
NGINX
      $SUDO ln -sf "$conf" /etc/nginx/sites-enabled/$SERVICE
      $SUDO nginx -t
      $SUDO systemctl reload nginx
    fi
    if ! $SUDO test -d "/etc/letsencrypt/live/$DOMAIN"; then
      command -v certbot >/dev/null 2>&1 || $APT install -y -qq certbot python3-certbot-nginx >/dev/null
      $SUDO certbot --nginx -d "$DOMAIN" --non-interactive --agree-tos --register-unsafely-without-email --redirect
    fi
  else
    # Nothing on ports 80/443 yet: Caddy gets and renews the certificate itself.
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
