#!/usr/bin/env bash
# Open CGagentHarness in your browser on macOS: start Ollama with a 32768-token
# window if it is not running, open the app if it is not running, find the
# loopback port the app's backend chose, and hand that exact origin to
# start-web.sh --desktop-url (the same path as the app's globe button).
#   scripts/start-macos.sh                       # app mode (default)
#   scripts/start-macos.sh --standalone [port]   # no app: start-web.sh runs `serve` itself
#   scripts/start-macos.sh --trust-cert          # app mode, and trust the home's certificate in your login keychain
#   scripts/start-macos.sh --no-ollama ...       # leave Ollama alone
# Env: CGAH_APP  app bundle (default /Applications, then ~/Applications)
# Run scripts/install-macos.sh first. Full guide: docs/INSTALL.md.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
loopback=127.0.0.1 # DevSkim: ignore DS162092 because the app and Ollama listen on loopback only.
ollama_api=http://$loopback:11434 # DevSkim: ignore DS137138 because Ollama's default endpoint is plain HTTP on loopback.
sidecar='/Contents/MacOS/cgagentharness desktop$'

mode=app ollama=1 trust=0
while [ "$#" -gt 0 ]; do
  case $1 in
    --standalone) mode=standalone ;;
    --no-ollama) ollama=0 ;;
    --trust-cert) trust=1 ;;
    -h | --help) sed -n '2,11p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    --*) echo "start-macos: unknown option: $1" >&2; exit 3 ;;
    *) break ;;
  esac
  shift
done
[ "$mode" = standalone ] || [ "$#" -eq 0 ] || { echo 'start-macos: a port is only valid with --standalone' >&2; exit 3; }
[ "$mode" = app ] || [ "$trust" = 0 ] || { echo 'start-macos: --trust-cert needs app mode (the server must be running)' >&2; exit 3; }
[ "$(uname -s)" = Darwin ] || { echo 'start-macos: macOS only; elsewhere run scripts/start-web.sh' >&2; exit 3; }

say() { printf '==> %s\n' "$*"; }
# --noproxy: an http_proxy/ALL_PROXY must never answer for a loopback service.
ollama_up() { curl --noproxy '*' --silent --fail --max-time 2 -o /dev/null "$ollama_api/api/version"; }
# PIDs of this user's app backends. A translocated or copied app runs from
# another path, so match the bundle layout, not one install location.
app_backends() { pgrep -u "$(id -u)" -f "$sidecar" || true; }
# Run a server in its own session, so the Ctrl-C that stops a later
# foreground command (start-web.sh) does not reach it. macOS has no setsid(1).
# shellcheck disable=SC2016 # perl expands its own variables.
detached() { nohup /usr/bin/perl -MPOSIX -e 'POSIX::setsid(); exec @ARGV or die "exec $ARGV[0]: $!\n"' -- "$@"; }
# The Ollama CLI on PATH, else the one inside Ollama.app.
ollama_cli() {
  command -v ollama 2>/dev/null && return
  for c in /Applications/Ollama.app/Contents/Resources/ollama "$HOME/Applications/Ollama.app/Contents/Resources/ollama"; do
    if [ -x "$c" ]; then printf '%s\n' "$c"; return; fi
  done
}

if [ "$ollama" = 1 ]; then
  if ollama_up; then
    say "Ollama is running at $ollama_api (its context window was fixed when it started)"
  elif [ -d /Applications/Ollama.app ] || [ -d "$HOME/Applications/Ollama.app" ]; then
    # GUI apps read launchctl's environment. Keep a value the operator chose.
    if [ -z "$(launchctl getenv OLLAMA_CONTEXT_LENGTH 2>/dev/null || true)" ]; then
      launchctl setenv OLLAMA_CONTEXT_LENGTH 32768
    fi
    say "starting Ollama (OLLAMA_CONTEXT_LENGTH=$(launchctl getenv OLLAMA_CONTEXT_LENGTH))"
    # open waits while a first launch shows macOS's "downloaded from the
    # internet" prompt. Never block on it or let it hold our output open.
    open -a Ollama </dev/null >/dev/null 2>&1 &
    for _ in $(seq 1 30); do ollama_up && break; sleep 1; done
    ollama_up || echo 'start-macos: Ollama did not answer yet; answer any first-launch prompt, chat waits for it (docs/MODELS.md)' >&2
  elif cli=$(ollama_cli) && [ -n "$cli" ]; then
    # CLI-only install (no app): run its server. An exported value wins.
    log=$HOME/Library/Logs/ollama-serve.log
    say "starting $cli serve (OLLAMA_CONTEXT_LENGTH=${OLLAMA_CONTEXT_LENGTH:-32768}; log $log)"
    OLLAMA_CONTEXT_LENGTH=${OLLAMA_CONTEXT_LENGTH:-32768} detached "$cli" serve </dev/null >>"$log" 2>&1 &
    for _ in $(seq 1 30); do ollama_up && break; sleep 1; done
    ollama_up || echo "start-macos: Ollama did not answer yet; see $log (docs/MODELS.md)" >&2
  else
    echo 'start-macos: Ollama is not installed; run scripts/install-macos.sh or point chat at another model (docs/MODELS.md)' >&2
  fi
fi

if [ "$mode" = standalone ]; then
  # One server owns a home: its owner keeps <home>/.server.lock open. Ask who
  # holds the target home's lock instead of guessing from the environment, so
  # an app on a custom home neither blocks nor slips past this check.
  home=${CGAGENTHARNESS_HOME:-$HOME/.CGagentHarness}
  holder=$(lsof -t -- "$home/.server.lock" 2>/dev/null | head -n1 || true)
  if [ -n "$holder" ]; then
    echo "start-macos: $home is already owned by pid $holder ($(ps -o command= -p "$holder" 2>/dev/null || echo unknown))." >&2
    echo 'Use app mode (no --standalone) for the app, quit that owner, or set CGAGENTHARNESS_HOME to another absolute home.' >&2
    exit 3
  fi
  exec bash "$root/scripts/start-web.sh" "$@"
fi

if [ -z "$(app_backends)" ]; then
  app=${CGAH_APP:-}
  if [ -z "$app" ]; then
    for candidate in "/Applications/CG Agent Harness.app" "$HOME/Applications/CG Agent Harness.app"; do
      if [ -d "$candidate" ]; then app=$candidate; break; fi
    done
  fi
  [ -n "$app" ] && [ -d "$app" ] || { echo 'start-macos: the app is not installed; run scripts/install-macos.sh or set CGAH_APP' >&2; exit 3; }
  say "opening $app"
  open "$app" </dev/null >/dev/null 2>&1 &
fi

# The backend binds port 0 and tells only the app which port it got, so read
# the listening socket of the app's own backend process.
port='' pids=''
for _ in $(seq 1 120); do
  pids=$(app_backends)
  if [ "$(printf '%s\n' "$pids" | grep -c .)" -gt 1 ]; then
    echo 'start-macos: more than one app backend is running (one per home); use the globe button in the app you want' >&2
    exit 3
  fi
  if [ -n "$pids" ]; then
    # lsof exits 1 until the new backend listens; under pipefail that must not end the script.
    port=$(lsof -nP -a -p "$pids" -iTCP@"$loopback" -sTCP:LISTEN -Fn 2>/dev/null | sed -n "s/^n$loopback:\([0-9]*\)\$/\1/p" | head -n1 || true)
    [ -z "$port" ] || break
  fi
  sleep 0.5
done
[ -n "$port" ] || { echo 'start-macos: the app backend did not start listening; open Harness → Setup and recovery in the app' >&2; exit 2; }

# A plain-HTTP request succeeds only on an explicit legacy-HTTP home; a TLS
# listener drops it. Probing HTTPS instead would need a TLS 1.3 client, which
# the LibreSSL curl on macOS 12 is not.
if curl --noproxy '*' --silent --max-time 3 -o /dev/null "http://$loopback:$port/"; then # DevSkim: ignore DS137138 because this only detects a legacy-HTTP home on loopback.
  origin=http://$loopback:$port # DevSkim: ignore DS137138 because an explicit legacy-HTTP home serves plain HTTP on loopback.
else
  origin=https://$loopback:$port
  # Browsers do not trust the home's generated certificate. Export only its
  # public half with the app's own backend; nothing is trusted unless asked.
  # Its argv is "<bundle>/Contents/MacOS/cgagentharness desktop" (app_backends matched it).
  backend=$(ps -o command= -p "$pids" || true)
  backend=${backend% desktop}
  # A new process does not inherit the app's environment, so name the home
  # that backend actually owns: the one whose .server.lock it holds open.
  home=$(lsof -nP -a -p "$pids" -Fn 2>/dev/null | sed -n 's|^n\(/.*\)/\.server\.lock$|\1|p' | head -n1 || true)
  pem=$(mktemp)
  trap 'rm -f "$pem"' EXIT
  if [ -z "$home" ]; then
    echo "start-macos: cannot tell which home pid $pids owns; certificate trust not checked (docs/SECURE_RESEARCH.md)" >&2
  elif CGAGENTHARNESS_HOME=$home "$backend" tls certificate >"$pem" 2>/dev/null && [ -s "$pem" ]; then
    if security verify-cert -c "$pem" -p ssl -s "$loopback" -L -q >/dev/null 2>&1; then
      say "browser trust: this home's certificate is trusted for $loopback"
    elif [ "$trust" = 1 ]; then
      say "trusting this home's certificate for TLS in your login keychain (macOS asks for your password)"
      security add-trusted-cert -r trustRoot -p ssl -k "$HOME/Library/Keychains/login.keychain-db" "$pem" \
        || echo "start-macos: the certificate was not trusted; the browser will warn" >&2
    else
      say 'browser trust: not trusted yet; your browser will warn. Rerun with --trust-cert, or see docs/SECURE_RESEARCH.md#https-and-native-trust'
    fi
  else
    echo "start-macos: could not export the certificate of $home; see docs/SECURE_RESEARCH.md" >&2
  fi
fi
bash "$root/scripts/start-web.sh" --desktop-url "$origin"
