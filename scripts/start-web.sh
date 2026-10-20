#!/usr/bin/env bash
# Start the CGagentHarness console on loopback and open it in a browser.
# Usage: scripts/start-web.sh [port]
#        scripts/start-web.sh --desktop-url http[s]://127.0.0.1:port
# Env:   CGAGENTHARNESS_HOME  state directory (default ~/.CGagentHarness)
#        CGAH_BIN             backend binary (default target/release/cgagentharness)
#        NO_OPEN=1            start the server without opening a browser
set -euo pipefail

# The native app already owns its home/server. Open only the exact loopback
# origin supplied by the shell; never compete for the same home lock or stop it.
if [ "${1:-}" = --desktop-url ]; then
  if [ "$#" -ne 2 ] || [[ ! "$2" =~ ^https?://127\.0\.0\.1:([0-9]{1,5})$ ]]; then
    echo 'start-web: expected an owned desktop loopback URL' >&2; exit 3
  fi
  desktop_port=${BASH_REMATCH[1]}
  if [ "$desktop_port" -lt 1 ] || [ "$desktop_port" -gt 65535 ]; then
    echo 'start-web: invalid desktop port' >&2; exit 3
  fi
  (exec 3<>"/dev/tcp/127.0.0.1/$desktop_port") 2>/dev/null || { # DevSkim: ignore DS162092 because the probe targets the owned loopback server only.
    echo 'start-web: desktop server is unavailable; reopen the app' >&2; exit 2;
  }
  echo "CGagentHarness console: $2/"
  echo 'This server is owned by the app. Keep the app running; sign in separately in the browser.'
  /usr/bin/open -- "$2/"
  exit 0
fi

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
bin=${CGAH_BIN:-$root/target/release/cgagentharness}
home=${CGAGENTHARNESS_HOME:-$HOME/.CGagentHarness}
port=${1:-${PORT:-8790}}
loopback=127.0.0.1 # DevSkim: ignore DS162092 because the console binds and is reached on loopback only.

case $port in
  '' | *[!0-9]*) echo "start-web: port must be a number" >&2; exit 3 ;;
esac
if [ "$port" -lt 1024 ] || [ "$port" -gt 65535 ]; then
  echo "start-web: port must be 1024-65535" >&2
  exit 3
fi

port_open() { (exec 3<>"/dev/tcp/$loopback/$1") 2>/dev/null; }

if port_open "$port"; then
  echo "start-web: $loopback:$port is already in use; pass another port" >&2
  exit 3
fi

if [ ! -x "$bin" ]; then
  if [ -n "${CGAH_BIN:-}" ]; then
    echo "start-web: CGAH_BIN is not executable: $bin" >&2
    exit 3
  fi
  (cd "$root" && cargo build --release --locked)
fi

# Fresh homes serve HTTPS. An existing config serves HTTPS only with a literal
# `enabled: true` under tls:; a missing key (a home seeded before TLS) is HTTP.
scheme=https
cfg=$home/config.yaml
if [ -f "$cfg" ]; then
  tls=$(awk '/^tls:/ {t=1; next} t && /^[^ #]/ {t=0} t && $1=="enabled:" {print $2; exit}' "$cfg")
  [ "${tls:-false}" = true ] || scheme=http
fi
url=$scheme://$loopback:$port/

"$bin" serve --port "$port" &
server=$!
trap 'kill "$server" 2>/dev/null || true' EXIT INT TERM

for _ in $(seq 1 100); do
  kill -0 "$server" 2>/dev/null || { wait "$server"; exit $?; }
  port_open "$port" && break
  sleep 0.2
done
port_open "$port" || { echo "start-web: server did not open port $port" >&2; exit 2; }

echo "CGagentHarness console: $url"
[ "$scheme" = https ] && echo "First visit: trust the local certificate (docs/SECURE_RESEARCH.md)."
echo 'A fresh home opens Set your password for admin; existing accounts sign in with their own password.'

if [ -z "${NO_OPEN:-}" ]; then
  for opener in xdg-open open wslview; do
    if command -v "$opener" >/dev/null 2>&1; then
      "$opener" "$url" >/dev/null 2>&1 || true
      break
    fi
  done
fi

wait "$server"
