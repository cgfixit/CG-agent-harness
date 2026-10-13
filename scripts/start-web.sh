#!/usr/bin/env bash
# Start the CGagentHarness console on loopback and open it in a browser.
# Usage: scripts/start-web.sh [port]
# Env:   CGAGENTHARNESS_HOME  state directory (default ~/.CGagentHarness)
#        CGAH_BIN             backend binary (default target/release/cgagentharness)
#        NO_OPEN=1            start the server without opening a browser
set -euo pipefail

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

# Fresh homes serve HTTPS; only a literal `enabled: true` under tls: is on, so
# an existing config with TLS off (or quoted "true") is reached over HTTP.
scheme=https
cfg=$home/config.yaml
if [ -f "$cfg" ]; then
  tls=$(awk '/^tls:/ {t=1; next} t && /^[^ #]/ {t=0} t && $1=="enabled:" {print $2; exit}' "$cfg")
  [ "${tls:-true}" = true ] || scheme=http
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
[ "$scheme" = https ] && echo "First visit: trust the local certificate (docs/SECURE_RESEARCH.md); sign in admin / admin on a fresh home."

if [ -z "${NO_OPEN:-}" ]; then
  for opener in xdg-open open wslview; do
    if command -v "$opener" >/dev/null 2>&1; then
      "$opener" "$url" >/dev/null 2>&1 || true
      break
    fi
  done
fi

wait "$server"
