#!/usr/bin/env bash
# Open CGagentHarness in your browser on macOS: start Ollama with a 32768-token
# window if it is not running, open the app if it is not running, find the
# loopback port the app's backend chose, and hand that exact origin to
# start-web.sh --desktop-url (the same path as the app's globe button).
#   scripts/start-macos.sh                       # app mode (default)
#   scripts/start-macos.sh --standalone [port]   # no app: start-web.sh runs `serve` itself
#   scripts/start-macos.sh --trust-cert          # app mode, and trust the home's certificate in your login keychain
#   scripts/start-macos.sh --no-ollama ...       # leave Ollama alone (chat uses LM Studio, another port or a remote model)
#   scripts/start-macos.sh --no-browser          # app mode, but only print the console origin (scripts/init.sh uses it)
# Env: CGAH_APP  app bundle (default /Applications, then ~/Applications)
# Run scripts/install-macos.sh first. Full guide: docs/INSTALL.md.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
loopback=127.0.0.1 # DevSkim: ignore DS162092 because the app and Ollama listen on loopback only.
ollama_api=http://$loopback:11434 # DevSkim: ignore DS137138 because Ollama's default endpoint is plain HTTP on loopback.
# Every ollama command talks to (or serves on) the loopback endpoint probed above,
# never an inherited OLLAMA_HOST such as 0.0.0.0.
export OLLAMA_HOST=$loopback:11434
sidecar='/Contents/MacOS/cgagentharness desktop$'

mode=app ollama=1 trust=0 browser=1 port_arg=''
# Options may come before or after the port; anything else is refused.
while [ "$#" -gt 0 ]; do
  case $1 in
    --standalone) mode=standalone ;;
    --no-ollama) ollama=0 ;;
    --trust-cert) trust=1 ;;
    --no-browser) browser=0 ;;
    -h | --help) sed -n '2,12p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "start-macos: unknown option: $1" >&2; exit 3 ;;
    *) [ -z "$port_arg" ] || { echo "start-macos: unexpected argument: $1" >&2; exit 3; }; port_arg=$1 ;;
  esac
  shift
done
[ "$mode" = standalone ] || [ -z "$port_arg" ] || { echo 'start-macos: a port is only valid with --standalone' >&2; exit 3; }
# Same port rules as start-web.sh, checked before anything starts.
if [ "$mode" = standalone ]; then
  p=${port_arg:-${PORT:-8790}}
  case $p in '' | *[!0-9]*) echo "start-macos: port must be a number: $p" >&2; exit 3 ;; esac
  [ "$p" -ge 1024 ] && [ "$p" -le 65535 ] || { echo "start-macos: port must be 1024-65535: $p" >&2; exit 3; }
fi
[ "$mode" = app ] || [ "$trust" = 0 ] || { echo 'start-macos: --trust-cert needs app mode (the server must be running)' >&2; exit 3; }
[ "$mode" = app ] || [ "$browser" = 1 ] || { echo 'start-macos: --no-browser needs app mode' >&2; exit 3; }
[ "$(uname -s)" = Darwin ] || { echo 'start-macos: macOS only; elsewhere run scripts/start-web.sh' >&2; exit 3; }

say() { printf '==> %s\n' "$*"; }
# The host can run it: the Mach-O has the host's slice (Apple silicon also runs
# x86_64 through Rosetta). `file` ships with macOS; lipo needs the Command Line Tools.
# A shell translated by Rosetta reports x86_64 from uname; sysctl.proc_translated=1 means Apple silicon.
host_arch() {
  if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then echo arm64; else uname -m; fi
}
runs_here() {
  local info
  info=$(file -b "$1" 2>/dev/null) || return 1
  case $(host_arch) in
    arm64) case $info in *arm64* | *x86_64*) return 0 ;; esac ;;
    *) case $info in *x86_64*) return 0 ;; esac ;;
  esac
  return 1
}
# Ollama.app reads launchctl's environment, not ours. Only an unset or default loopback OLLAMA_HOST there is safe to open.
launchd_ollama_host_ok() {
  local h
  h=$(launchctl getenv OLLAMA_HOST 2>/dev/null || true)
  case ${h#http://} in '' | 127.0.0.1 | 127.0.0.1:11434 | localhost | localhost:11434) return 0 ;; esac
  return 1
}
# --noproxy: an http_proxy/ALL_PROXY must never answer for a loopback service.
ollama_up() { curl --noproxy '*' --silent --fail --max-time 2 -o /dev/null "$ollama_api/api/version"; }
# PIDs of this user's app backends. A translocated or copied app runs from
# another path, so match the bundle layout, not one install location.
app_backends() { pgrep -u "$(id -u)" -f "$sidecar" || true; }
# Run a server in its own session, so the Ctrl-C that stops a later
# foreground command (start-web.sh) does not reach it. macOS has no setsid(1).
# shellcheck disable=SC2016 # perl expands its own variables.
detached() { nohup /usr/bin/perl -MPOSIX -e 'POSIX::setsid(); exec @ARGV or die "exec $ARGV[0]: $!\n"' -- "$@"; }
# A launchable Ollama.app: Info.plist names an executable that is present,
# and the bundle signature verifies. An interrupted copy fails this.
ollama_bundle_ok() {
  local exe
  exe=$(defaults read "$1/Contents/Info" CFBundleExecutable 2>/dev/null) && [ -n "$exe" ] \
    && [ -x "$1/Contents/MacOS/$exe" ] && runs_here "$1/Contents/MacOS/$exe" && codesign --verify "$1" >/dev/null 2>&1
}
# The Ollama CLI on PATH, else the one inside Ollama.app.
ollama_cli() {
  local c
  # Only a CLI that runs (not a stale link or a wrong-arch binary); --help needs no server.
  for c in "$(command -v ollama 2>/dev/null || true)" /Applications/Ollama.app/Contents/Resources/ollama "$HOME/Applications/Ollama.app/Contents/Resources/ollama"; do
    if [ -n "$c" ] && [ -x "$c" ] && "$c" --help >/dev/null 2>&1; then printf '%s\n' "$c"; return; fi
  done
}

if [ "$mode" = standalone ]; then
  # One server owns a home: its owner keeps <home>/.server.lock open. Ask who
  # holds the target home's lock instead of guessing from the environment, so
  # an app on a custom home neither blocks nor slips past this check. Checked
  # before Ollama is touched, so a refusal has no side effects.
  home=${CGAGENTHARNESS_HOME:-$HOME/.CGagentHarness}
  holder=$(lsof -t -- "$home/.server.lock" 2>/dev/null | head -n1 || true)
  if [ -n "$holder" ]; then
    echo "start-macos: $home is already owned by pid $holder ($(ps -o command= -p "$holder" 2>/dev/null || echo unknown))." >&2
    echo 'Use app mode (no --standalone) for the app, quit that owner, or set CGAGENTHARNESS_HOME to another absolute home.' >&2
    exit 3
  fi
fi

# App mode: pick and validate the bundle before Ollama is touched, so a launch
# that must be refused has no side effects. Empty when a backend already runs.
app=''
# More than one backend (one per home) is ambiguous; refuse before touching Ollama.
if [ "$mode" = app ] && [ "$(app_backends | grep -c .)" -gt 1 ]; then
  echo 'start-macos: more than one app backend is running (one per home); use the globe button in the app you want' >&2
  exit 3
fi
if [ "$mode" = app ] && [ -z "$(app_backends)" ]; then
  # The app's backend needs its home's lock. Another owner (a `serve` started by
  # hand, say) leaves the app on its recovery screen and this script waiting for
  # a backend that never starts, so refuse now. open gives the app launchd's
  # environment, not this shell's, so its home comes from launchctl.
  app_home=$(launchctl getenv CGAGENTHARNESS_HOME 2>/dev/null || true)
  app_home=${app_home:-$HOME/.CGagentHarness}
  holder=$(lsof -t -- "$app_home/.server.lock" 2>/dev/null | head -n1 || true)
  if [ -n "$holder" ]; then
    echo "start-macos: $app_home is owned by pid $holder ($(ps -o command= -p "$holder" 2>/dev/null || echo unknown)), so the app cannot start its backend." >&2
    echo 'Quit that process (or use the console it serves), then rerun.' >&2
    exit 3
  fi
  # A usable bundle: the same test as install-macos.sh's app_ok, including the
  # backend digest the shell was compiled with (checked without running either binary).
  sidecar_ok() {
    local d
    d=$(shasum -a 256 "$1/Contents/MacOS/cgagentharness" 2>/dev/null | awk '{print $1}') && [ ${#d} = 64 ] \
      && LC_ALL=C grep -q -a -F "$d" "$1/Contents/MacOS/cg-agent-harness-desktop"
  }
  app_ok() {
    [ -x "$1/Contents/MacOS/cg-agent-harness-desktop" ] && [ -x "$1/Contents/MacOS/cgagentharness" ] \
      && runs_here "$1/Contents/MacOS/cg-agent-harness-desktop" && runs_here "$1/Contents/MacOS/cgagentharness" \
      && [ "$(defaults read "$1/Contents/Info" CFBundleExecutable 2>/dev/null)" = cg-agent-harness-desktop ] \
      && sidecar_ok "$1" && codesign --verify --deep --strict "$1" >/dev/null 2>&1
  }
  app=${CGAH_APP:-}
  if [ -n "$app" ]; then
    app_ok "$app" || { echo "start-macos: CGAH_APP=$app is not a complete, signed app for this Mac with a matching backend; reinstall it or unset CGAH_APP" >&2; exit 3; }
  else
    # Skip a partial or damaged copy.
    for candidate in "/Applications/CG Agent Harness.app" "$HOME/Applications/CG Agent Harness.app"; do
      if app_ok "$candidate"; then
        if [ -z "$app" ]; then app=$candidate; else
          echo "start-macos: note: $candidate also exists; opening $app (set CGAH_APP to choose)" >&2
        fi
      fi
    done
  fi
  [ -n "$app" ] && [ -d "$app" ] || { echo 'start-macos: no usable app found (a partial, unsigned or wrong-architecture copy is skipped); run scripts/install-macos.sh or set CGAH_APP' >&2; exit 3; }
fi

if [ "$ollama" = 1 ]; then
  if ollama_up; then
    say "Ollama is running at $ollama_api (its context window was fixed when it started)"
  elif ollama_app=$(for a in /Applications/Ollama.app "$HOME/Applications/Ollama.app"; do
    if ollama_bundle_ok "$a"; then printf '%s\n' "$a"; break; fi
  done) && [ -n "$ollama_app" ]; then
    launchd_ollama_host_ok || { echo "start-macos: launchctl's OLLAMA_HOST is $(launchctl getenv OLLAMA_HOST), so Ollama.app would listen there, not only on loopback; run 'launchctl unsetenv OLLAMA_HOST', or start Ollama yourself (or pass --no-ollama)" >&2; exit 3; }
    # GUI apps read launchctl's environment. Keep a value the operator chose.
    if [ -z "$(launchctl getenv OLLAMA_CONTEXT_LENGTH 2>/dev/null || true)" ]; then
      launchctl setenv OLLAMA_CONTEXT_LENGTH 32768
    fi
    say "starting Ollama (OLLAMA_CONTEXT_LENGTH=$(launchctl getenv OLLAMA_CONTEXT_LENGTH))"
    # open waits while a first launch shows macOS's "downloaded from the
    # internet" prompt. Never block on it or let it hold our output open.
    # Open the bundle that passed validation, not whichever copy LaunchServices picks.
    open "$ollama_app" </dev/null >/dev/null 2>&1 &
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
    echo 'start-macos: Ollama is not installed (or its app is incomplete); run scripts/install-macos.sh or point chat at another model (docs/MODELS.md)' >&2
  fi
fi

if [ "$mode" = standalone ]; then
  exec bash "$root/scripts/start-web.sh" ${port_arg:+"$port_arg"}
fi

if [ -n "$app" ]; then
  say "opening $app"
  open "$app" </dev/null >/dev/null 2>&1 &
fi

# The backend binds port 0 and tells only the app which port it got, so read
# the listening sockets of the app's own backend process. It may also hold the
# private MCP gateway's port (mcp.server.enabled), so classify each listener
# with one plain-HTTP request to /api/status:
#   200  the console on an explicit legacy-HTTP home
#   000  no HTTP reply: a TLS listener, which only the console can be
#   else the MCP gateway (always plain HTTP; it refuses this path)
# Probing HTTPS instead would need a TLS 1.3 client, which the LibreSSL curl
# on macOS 12 is not.
port='' scheme='' pids=''
for _ in $(seq 1 120); do
  pids=$(app_backends)
  if [ "$(printf '%s\n' "$pids" | grep -c .)" -gt 1 ]; then
    echo 'start-macos: more than one app backend is running (one per home); use the globe button in the app you want' >&2
    exit 3
  fi
  if [ -n "$pids" ]; then
    # lsof exits 1 until the new backend listens; under pipefail that must not end the script.
    for p in $(lsof -nP -a -p "$pids" -iTCP@"$loopback" -sTCP:LISTEN -Fn 2>/dev/null | sed -n "s/^n$loopback:\([0-9]*\)\$/\1/p" || true); do
      code=$(curl --noproxy '*' --silent --max-time 3 -o /dev/null -w '%{http_code}' "http://$loopback:$p/api/status" || true) # DevSkim: ignore DS137138 because this only classifies loopback listeners.
      case $code in
        200) port=$p scheme=http; break ;;
        000) port=$p scheme=https; break ;;
      esac
    done
    [ -z "$port" ] || break
  fi
  sleep 0.5
done
[ -n "$port" ] || { echo 'start-macos: the app backend did not start listening; open Harness → Setup and recovery (Cmd and +) in the app' >&2; exit 2; }

if [ "$scheme" = http ]; then
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
    elif [ "$trust" = 1 ] && openssl x509 -in "$pem" -noout -text 2>/dev/null | grep -q 'CA:TRUE'; then
      # An operator-supplied tls.cert_file can be a CA; trusting it would cover other hosts.
      echo "start-macos: this home's certificate is a CA certificate (tls.cert_file); not trusting it from here (docs/SECURE_RESEARCH.md#https-and-native-trust)" >&2
    elif [ "$trust" = 1 ]; then
      say "trusting this home's certificate for TLS to $loopback in your login keychain (macOS asks for your password)"
      # -s limits the SSL trust setting to the loopback host the console is served on.
      security add-trusted-cert -r trustRoot -p ssl -s "$loopback" -k "$HOME/Library/Keychains/login.keychain-db" "$pem" \
        || echo "start-macos: the certificate was not trusted; the browser will warn" >&2
    else
      say 'browser trust: not trusted yet; your browser will warn. Rerun with --trust-cert, or see docs/SECURE_RESEARCH.md#https-and-native-trust'
    fi
  else
    echo "start-macos: could not export the certificate of $home; see docs/SECURE_RESEARCH.md" >&2
  fi
fi
if [ "$browser" = 0 ]; then
  echo "CGagentHarness console: $origin/"
  exit 0
fi
bash "$root/scripts/start-web.sh" --desktop-url "$origin"
