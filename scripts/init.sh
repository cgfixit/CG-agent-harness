#!/usr/bin/env bash
# One command from a fresh clone or download to a working CGagentHarness on an
# Apple silicon Mac: Command Line Tools, then scripts/install-macos.sh (Homebrew,
# pinned Rust, Ollama, the backend build, the checksummed release app), PATH
# lines for your login shell (zsh or bash), the admin password (replaced and
# kept in your login keychain), the web allowlist
# (scripts/web-allowlist-bootstrap.sh), then it opens what you choose.
# Rerunning is safe: finished steps are detected and skipped. Every line is
# logged to scripts/logs/install.log; passwords and setup codes never are.
#   scripts/init.sh                      # asks as it goes (works from zsh or bash)
#   scripts/init.sh --yes --choice 3     # asks nothing; a generated admin password goes to Keychain
# Options:
#   --choice 0|1|2|3  what to open at the end: 1 the app, 2 the console in your browser
#                     (no app; served from this terminal), 3 both, 0 nothing
#   --yes             never ask; a failed optional step is skipped, a failed required one stops
#   --model TAG       also pull an Ollama model (multi-GB)   --with-gh  install GitHub CLI
#   --no-ollama       leave Ollama alone                     --app-dir DIR  app location
#   --trust-cert      trust the console certificate for your browser (choices 2 and 3 with the app)
#   --no-profile      do not add Homebrew and Rust PATH lines to your shell profile
#   --no-allowlist    do not grant the web allowlist
# Full guide: docs/INSTALL.md ("One command: init.sh").

# Started from zsh or sh (`zsh scripts/init.sh`): rerun under bash. POSIX syntax until then.
if [ -z "${BASH_VERSION:-}" ]; then
  case ${ZSH_EVAL_CONTEXT:-} in *:file*) echo 'init.sh: run it (scripts/init.sh), do not source it' >&2; return 1 ;; esac
  exec /bin/bash "$0" "$@"
fi
# shellcheck disable=SC2317 # reached only when sourced from bash
(return 0 2>/dev/null) && { echo 'init.sh: run it (scripts/init.sh), do not source it' >&2; return 1; }
set -uo pipefail

[ "$(uname -s)" = Darwin ] || { echo 'init.sh: macOS only; elsewhere follow docs/INSTALL.md' >&2; exit 3; }
# A Terminal opened "using Rosetta" would build and install for Intel: rerun natively.
if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then exec arch -arm64 /bin/bash "$0" "$@"; fi
[ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ] || { echo 'init.sh: needs an Apple silicon Mac; on Intel follow docs/INSTALL.md' >&2; exit 3; }
[ "$(id -u)" != 0 ] || { echo 'init.sh: run as your user, not root (no sudo)' >&2; exit 3; }

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
choice='' interactive=1 trust=0 profile_opt=1 allowlist=1 ollama=1 app_dir=''
install_args=()
usage() { sed -n '2,22p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
while [ "$#" -gt 0 ]; do
  case $1 in
    --choice) case ${2:-} in 0 | 1 | 2 | 3) choice=$2; shift ;; *) echo 'init.sh: --choice takes 0, 1, 2 or 3' >&2; exit 3 ;; esac ;;
    --yes) interactive=0 ;;
    # A value never starts with "-": `--model --yes` must not swallow --yes.
    --model) [ "$#" -ge 2 ] && [ -n "$2" ] && [ "${2#-}" = "$2" ] || { echo 'init.sh: --model needs a tag' >&2; exit 3; }; install_args+=(--model "$2"); shift ;;
    --app-dir) [ "$#" -ge 2 ] && [ -n "$2" ] && [ "${2#-}" = "$2" ] || { echo 'init.sh: --app-dir needs a directory' >&2; exit 3; }; app_dir=$2; install_args+=(--app-dir "$2"); shift ;;
    --with-gh) install_args+=(--with-gh) ;;
    --no-ollama) ollama=0; install_args+=(--no-ollama) ;;
    --trust-cert) trust=1 ;;
    --no-profile) profile_opt=0 ;;
    --no-allowlist) allowlist=0 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "init.sh: unknown option: $1 (see --help)" >&2; exit 3 ;;
  esac
  shift
done
# No terminal to ask on (a pipe, CI): behave as --yes.
if [ "$interactive" = 1 ] && ! { : </dev/tty; } 2>/dev/null; then interactive=0; fi

# ------------------------------------------------------------------ logging
# Output reaches the terminal unchanged and scripts/logs/install.log line by
# line with a timestamp. A line that starts with \002 goes to the log only; a
# \001 ends a prompt's log line without a newline on screen. Setup codes and
# ANSI colours are stripped from the log; passwords are never printed at all.
log_dir=$root/scripts/logs
log=$log_dir/install.log
if ! mkdir -p "$log_dir" || ! chmod 700 "$log_dir"; then echo "init.sh: cannot create $log_dir" >&2; exit 3; fi
if ! (umask 077 && touch "$log") || ! chmod 600 "$log"; then echo "init.sh: cannot write $log" >&2; exit 3; fi
private=$(mktemp -d "${TMPDIR:-/tmp}/cgah-init.XXXXXX") || exit 3
mkfifo "$private/out" || exit 3
# shellcheck disable=SC2016 # perl expands its own variables.
/usr/bin/perl -MPOSIX=strftime -e '
  open(my $log, ">>", $ARGV[0]) or die "init.sh: cannot write $ARGV[0]: $!\n";
  sub logline {
    my $l = shift;
    $l =~ s/.*\r(?=.)//s;                    # progress bars: keep the last frame
    $l =~ s/\e\[[0-9;?]*[A-Za-z]//g;         # colours
    $l =~ s/(setup code\s+)[^\s;,.]+/$1<redacted>/gi;
    syswrite($log, strftime("%Y-%m-%d %H:%M:%S ", localtime) . $l . "\n");
  }
  my ($buf, $shown) = ("", 0);
  while (sysread(STDIN, my $chunk, 8192)) {
    $buf .= $chunk;
    while ($buf =~ s/^([^\n\001]*)([\n\001])//) {
      my ($line, $end) = ($1, $2);
      my $quiet = $line =~ s/^\002//;
      syswrite(STDOUT, substr($line, $shown) . ($end eq "\n" ? "\n" : "")) unless $quiet;
      $shown = 0;
      logline($line);
    }
    if (length($buf) > $shown && substr($buf, 0, 1) ne "\002") { syswrite(STDOUT, substr($buf, $shown)); $shown = length($buf); }
  }
  logline($buf) if length($buf);
' "$log" <"$private/out" &
logger=$!
exec 3>&1 4>&2 >"$private/out" 2>&1

server_pid='' signed_in=0 origin='' bin='' app_path='' guessed=0 failures=0
finish() {
  local rc=$?
  close_server
  # Back to the terminal; the logger sees end of input, writes its last line and exits.
  exec 1>&3 2>&4
  (sleep 5; kill "$logger" 2>/dev/null) &
  wait "$logger" 2>/dev/null
  rm -rf "$private"
  exit "$rc"
}
trap finish EXIT
trap 'exit 130' INT TERM

say() { printf '==> %s\n' "$*"; }
warn() { printf 'init: warning: %s\n' "$*"; }
note() { printf '\002%s\n' "$*"; }
# ask "question" default: the answer lands in REPLY (logged); --yes takes the default.
ask() {
  if [ "$interactive" = 0 ]; then REPLY=${2:-}; note "asked: $1 -> ${REPLY:-<empty>} (--yes)"; return; fi
  printf '%s \001' "$1"
  IFS= read -r REPLY </dev/tty || REPLY=''
  [ -n "$REPLY" ] || REPLY=${2:-}
  note "answer: ${REPLY:-<empty>}"
}
# Hidden input; never logged.
ask_secret() {
  printf '%s \001' "$1"
  IFS= read -r -s REPLY </dev/tty || REPLY=''
  printf '\n' >/dev/tty
  note 'answer: <hidden>'
}

# -------------------------------------------------------------- step runner
SKIP=99
step_names=() step_states=()
record() { step_names+=("$1"); step_states+=("$2"); }
# step NAME required|optional FUNCTION: run it; on failure retry, skip or stop.
step() {
  local name=$1 kind=$2 fn=$3 rc started
  while :; do
    printf '\n'
    say "[$name]"
    started=$SECONDS
    "$fn"
    rc=$?
    case $rc in
      0) say "[$name] done ($((SECONDS - started))s)"; record "$name" 'done'; return 0 ;;
      "$SKIP") record "$name" skipped; return 0 ;;
    esac
    warn "[$name] failed (exit $rc); the lines above say why, and $log keeps them"
    if [ "$interactive" = 1 ]; then
      ask "Retry, skip or quit? [r/s/q]" r
      case $REPLY in [Rr]*) continue ;; [Ss]*) record "$name" skipped; return 0 ;; *) record "$name" FAILED; exit "$rc" ;; esac
    fi
    record "$name" FAILED
    [ "$kind" = optional ] && return 0
    exit "$rc"
  done
}
summary() {
  local i
  printf '\n'
  say 'Summary'
  for i in "${!step_names[@]}"; do printf '    %-8s %s\n' "${step_states[$i]}" "${step_names[$i]}"; done
  say "log: $log"
}

# ------------------------------------------------------------------ helpers
clt_ok() { xcode-select -p >/dev/null 2>&1 && xcrun clang --version >/dev/null 2>&1; }
port_open() { (exec 5<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; } # DevSkim: ignore DS162092 because the harness listens on loopback only.
free_port() {
  local p=$1
  while [ "$p" -lt $(($1 + 100)) ]; do
    port_open "$p" || { echo "$p"; return 0; }
    p=$((p + 1))
  done
  return 1
}
app_backends() { pgrep -u "$(id -u)" -f '/Contents/MacOS/cgagentharness desktop$' || true; }
# The shell or its backend, the same test install-macos.sh uses before replacing the app.
app_running() { pgrep -u "$(id -u)" -f '/Contents/MacOS/(cgagentharness desktop$|cg-agent-harness-desktop( |$))' >/dev/null 2>&1; }
# What the public setup-status route says admin's first credential is: code,
# default, none, or unknown (an old curl without TLS 1.3, or no reply).
first_credential() {
  local status
  set --
  case $origin in
    https:*) cli tls certificate >"$private/cert.pem" 2>/dev/null && set -- --cacert "$private/cert.pem" ;;
  esac
  status=$(curl --noproxy '*' --silent --max-time 5 "$@" "$origin/api/auth/setup-status" 2>/dev/null) || status=''
  case $status in
    *'"setup_code":true'*) echo code ;;
    *'"default_password":true'*) echo default ;;
    *'"needs_password"'*) echo none ;;
    *) echo unknown ;;
  esac
}
find_app() {
  local d
  for d in ${app_dir:+"$app_dir"} /Applications "$HOME/Applications"; do
    if [ -x "$d/CG Agent Harness.app/Contents/MacOS/cg-agent-harness-desktop" ]; then app_path="$d/CG Agent Harness.app"; return 0; fi
  done
  app_path=''
  return 1
}
start_macos() { CGAH_APP=${app_dir:+$app_path} bash "$root/scripts/start-macos.sh" "$@"; }
cli() { CGAGENTHARNESS_HOME=$home "$bin" "$@"; }
# The reply (no secrets) says whether admin still has to replace its first password.
# Every failure counts toward the lockout cap below (five lock the account).
login_as_admin() {
  printf '%s\n' "$1" | cli account --url "$origin" login admin >"$private/login.json" 2>"$private/cli.err" && return 0
  failures=$((failures + 1))
  return 1
}
must_replace() { grep -q '"must_change_password": *true' "$private/login.json" 2>/dev/null; }
kc_get() { security find-generic-password -a admin -s "$kc_service" -w 2>/dev/null; }
# Store a password through stdin (never argv), then read it back: only an exact
# copy counts, and a wrong one is removed.
kc_put() {
  local escaped service
  escaped=$(printf '%s' "$1" | sed 's/[\\"]/\\&/g')
  service=$(printf '%s' "$kc_service" | sed 's/[\\"]/\\&/g')
  printf 'add-generic-password -U -a admin -s "%s" -l "CG Agent Harness admin password" -w "%s"\n' "$service" "$escaped" | security -i >/dev/null 2>&1
  [ "$(kc_get)" = "$1" ] && return 0
  security delete-generic-password -a admin -s "$kc_service" >/dev/null 2>&1
  return 1
}
# Finite input: with SIGPIPE ignored (CI runners, some launchers), BSD tr keeps
# reading an endless /dev/urandom after head exits, and this never returns.
generate_password() { head -c 1024 /dev/urandom | LC_ALL=C tr -dc 'A-Za-z0-9' 2>/dev/null | head -c 24 || true; }

# -------------------------------------------------------------------- steps
step_preflight() {
  local version login_shell
  version=$(sw_vers -productVersion)
  say "macOS $version on Apple silicon; clone $root"
  [ "${version%%.*}" -ge 14 ] || [ "$ollama" = 0 ] || warn "Ollama needs macOS 14 or newer; the install step will stop unless you rerun with --no-ollama"
  login_shell=$(dscl . -read "/Users/$(id -un)" UserShell 2>/dev/null | awk '{print $2}')
  login_shell=${login_shell:-${SHELL:-/bin/zsh}}
  case ${login_shell##*/} in
    zsh) profile=$HOME/.zprofile env_files="$HOME/.zshenv $HOME/.zprofile $HOME/.zshrc" ;;
    bash)
      # bash reads only the first of these that exists; never shadow ~/.profile.
      profile=$HOME/.bash_profile
      [ -f "$profile" ] || [ ! -f "$HOME/.profile" ] || profile=$HOME/.profile
      env_files="$profile $HOME/.bashrc $HOME/.profile" ;;
    *) profile='' env_files='' ;;
  esac
  say "login shell: $login_shell${profile:+ (profile $profile)}"
  curl --silent --head --max-time 10 -o /dev/null https://github.com || warn 'github.com did not answer; downloads will fail until the network works'
  local free_gb
  free_gb=$(df -g "$HOME" | awk 'NR==2 {print $4}')
  [ "${free_gb:-99}" -ge 15 ] || warn "only ${free_gb} GB free in your home volume; the build and app need about 10 GB, a model far more"
}

step_developer_tools() {
  local waited=0
  if clt_ok; then say "Command Line Tools: $(xcode-select -p)"; return "$SKIP"; fi
  if xcode-select -p >/dev/null 2>&1; then
    warn "developer tools at $(xcode-select -p) are selected but clang does not run; scripts/install-macos.sh prints the fix"
    return 1
  fi
  say 'installing the Command Line Tools: finish the macOS dialog (git and clang come with them)'
  xcode-select --install 2>&1 || true
  until clt_ok; do
    [ "$waited" -lt 2700 ] || { warn 'the Command Line Tools did not finish within 45 minutes'; return 1; }
    sleep 10
    waited=$((waited + 10))
    [ $((waited % 60)) != 0 ] || say "waiting for the Command Line Tools dialog ($((waited / 60)) min)"
  done
  say "Command Line Tools: $(xcode-select -p)"
}

step_checkout() {
  if [ ! -d "$root/.git" ]; then say 'downloaded copy (no git); nothing to set up'; return "$SKIP"; fi
  bash "$root/scripts/ensure-githooks.sh"
  say "checkout: $(git -C "$root" log -1 --format='%h %s' 2>/dev/null)"
}

step_install() {
  local rc
  bash "$root/scripts/install-macos.sh" --check ${install_args[@]+"${install_args[@]}"}
  rc=$?
  case $rc in
    0) say 'already installed; nothing to do'; return "$SKIP" ;;
    1) ;;
    *) return "$rc" ;;
  esac
  bash "$root/scripts/install-macos.sh" ${install_args[@]+"${install_args[@]}"}
  rc=$?
  if [ "$rc" != 0 ] && app_running; then
    # The installer never replaces a running app. Offer to quit it, then retry once.
    [ "$interactive" = 1 ] || { warn 'the app is running: quit it (Cmd-Q) and rerun; --yes never quits it for you'; return "$rc"; }
    ask 'The app is running and must quit before it is replaced. Quit it now? [Y/n]' y
    case $REPLY in [Nn]*) return "$rc" ;; esac
    osascript -e 'quit app "CG Agent Harness"' || return "$rc"
    # Its backend flushes and stops after the window closes; wait for both.
    for _ in $(seq 1 60); do app_running || break; sleep 0.5; done
    if app_running; then warn 'the app is still running; quit it and choose retry'; return "$rc"; fi
    bash "$root/scripts/install-macos.sh" ${install_args[@]+"${install_args[@]}"} || return $?
  elif [ "$rc" != 0 ]; then
    return "$rc"
  fi
  bash "$root/scripts/install-macos.sh" --check ${install_args[@]+"${install_args[@]}"}
}

# Add a line to a file once, under a marker comment.
add_line() {
  printf '\n# Added by CG-agent-harness scripts/init.sh\n%s\n' "$2" >>"$1" && say "added to $1: $2"
}
step_shell_profile() {
  local f added=0 has
  [ "$profile_opt" = 1 ] || { say 'skipped (--no-profile)'; return "$SKIP"; }
  [ -n "$profile" ] || { warn 'your login shell is not zsh or bash; add Homebrew and ~/.cargo/bin to its PATH yourself'; return "$SKIP"; }
  if [ -x /opt/homebrew/bin/brew ]; then
    has=0
    # shellcheck disable=SC2086 # env_files is a list of paths without spaces in $HOME's own name.
    for f in $env_files; do grep -qs 'brew shellenv' "$f" && has=1; done
    # shellcheck disable=SC2016 # the profile expands it, not this script.
    [ "$has" = 1 ] || { add_line "$profile" 'eval "$(/opt/homebrew/bin/brew shellenv)"'; added=1; }
  fi
  if [ -f "$HOME/.cargo/env" ]; then
    has=0
    # shellcheck disable=SC2086
    for f in $env_files; do grep -qs '\.cargo/env' "$f" && has=1; done
    # shellcheck disable=SC2016
    [ "$has" = 1 ] || { add_line "$profile" '. "$HOME/.cargo/env"'; added=1; }
  fi
  if [ "$added" = 0 ]; then say "Homebrew and Rust are already on your login shell's PATH"; return "$SKIP"; fi
  say 'open a new Terminal window to pick these up'
}

# The app's backend when it runs; otherwise a private server on a free port,
# stopped again when setup is done. Sets origin.
open_server() {
  local holder port code
  [ -z "$origin" ] || return 0
  if [ -n "$(app_backends)" ]; then
    origin=$(start_macos --no-ollama --no-browser 2>&1 | sed -n 's|^CGagentHarness console: \(.*\)/$|\1|p' | tail -n1)
    [ -n "$origin" ] || { warn 'the app is running, but its console address was not found'; return 1; }
    say "using the running app's backend at $origin"
    return 0
  fi
  holder=$(lsof -t -- "$home/.server.lock" 2>/dev/null | head -n1 || true)
  if [ -n "$holder" ]; then
    warn "$home is owned by pid $holder ($(ps -o command= -p "$holder" 2>/dev/null)); quit it, then retry"
    return 1
  fi
  port=$(free_port 8790) || { warn 'no free loopback port in 8790-8889'; return 1; }
  # Its output can name the setup code file; keep it out of the log.
  CGAGENTHARNESS_HOME=$home "$bin" serve --port "$port" >"$private/serve.out" 2>&1 &
  server_pid=$!
  for _ in $(seq 1 120); do
    kill -0 "$server_pid" 2>/dev/null || { server_pid=''; warn 'the backend stopped during startup:'; grep -v -i 'setup code' "$private/serve.out" | tail -n 5; return 1; }
    port_open "$port" && break
    sleep 0.5
  done
  port_open "$port" || { warn "the backend did not listen on $port"; return 1; }
  # Plain HTTP answers 200 only on a legacy-HTTP home; a TLS listener gives no HTTP reply.
  code=$(curl --noproxy '*' --silent --max-time 3 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/api/status" || true) # DevSkim: ignore DS137138,DS162092 because this only classifies a loopback listener.
  if [ "$code" = 200 ]; then origin="http://127.0.0.1:$port"; else origin="https://127.0.0.1:$port"; fi # DevSkim: ignore DS137138,DS162092 because a legacy-HTTP home serves plain HTTP on loopback.
  say "started a private setup server at $origin"
}
close_server() {
  if [ "$signed_in" = 1 ]; then cli account --url "$origin" logout >/dev/null 2>&1; signed_in=0; fi
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null
    wait "$server_pid" 2>/dev/null
    server_pid=''
    origin=''
  fi
}

step_admin_password() {
  local current='' fresh=0 stored='' new='' generated=0 code_file kind
  home=${CGAGENTHARNESS_HOME:-$HOME/.CGagentHarness}
  find_app
  # The app's own backend will own this home, so it seeds it; the clone build
  # (often newer than the release app) only when no app is installed.
  if [ -n "$app_path" ] && [ -x "$app_path/Contents/MacOS/cgagentharness" ]; then bin=$app_path/Contents/MacOS/cgagentharness
  elif [ -x "$root/target/release/cgagentharness" ]; then bin=$root/target/release/cgagentharness
  else warn 'no backend binary: rerun after the install step succeeds'; return 1; fi
  open_server || return 1
  mkdir -p "$home"
  kc_service="CGagentHarness admin ($(cd "$home" && pwd -P))"
  code_file=$home/setup-code
  stored=$(kc_get || true)
  kind=$(first_credential)
  note "first credential reported by setup-status: $kind"
  # Five failed sign-ins lock the account, so ask the server first and try
  # only the credential it names; automatic guesses run once per init.sh run.
  if [ "$kind" = code ] && [ -s "$code_file" ] && login_as_admin "$(cat "$code_file")"; then
    current=$(cat "$code_file")
  elif [ "$kind" = default ] && login_as_admin admin; then
    current='admin'
  elif [ "$kind" != code ] && [ "$kind" != default ] && [ -n "$stored" ] && [ "$guessed" = 0 ] && login_as_admin "$stored"; then
    current=$stored
    say 'signed in with the admin password from your login keychain'
  elif [ "$kind" = unknown ] && [ "$guessed" = 0 ] && [ -s "$code_file" ] && login_as_admin "$(cat "$code_file")"; then
    current=$(cat "$code_file")
  else
    [ "$kind" = code ] || [ "$kind" = default ] || [ -z "$stored" ] || [ "$guessed" = 1 ] || warn 'the admin password saved in your login keychain no longer works'
    guessed=1
    while [ "$interactive" = 1 ] && [ "$failures" -lt 4 ]; do
      ask_secret 'Current admin password (Enter to skip sign-in):'
      [ -n "$REPLY" ] || break
      if login_as_admin "$REPLY"; then current=$REPLY; break; fi
      warn "$(cat "$private/cli.err")"
      # A lock is a backoff (seconds, doubling to 15 minutes); more tries extend it.
      if grep -q AUTH_ACCOUNT_LOCKED "$private/cli.err"; then warn 'admin is locked for a while after failed sign-ins; wait a few minutes, then rerun'; break; fi
    done
    [ "$failures" -lt 4 ] || warn 'stopping before the account locks (five failed sign-ins); wait, then rerun'
    guessed=1
    [ -n "$current" ] || { warn 'admin is not signed in, so the allowlist step will be skipped'; return 1; }
    # A first password is replaced (and saved) below; offer only a real one.
    if [ "$current" != "$stored" ] && ! must_replace; then
      ask 'Save this admin password in your login keychain? [Y/n]' y
      case $REPLY in
        [Nn]*) ;;
        *) if kc_put "$current"; then say "saved in your login keychain ($kc_service)"; else warn 'Keychain did not accept it'; fi ;;
      esac
    fi
  fi
  signed_in=1
  must_replace && fresh=1
  if [ "$fresh" = 0 ]; then say 'admin already has its own password; nothing to replace'; return 0; fi

  say 'admin still has its first password; replacing it now (it allows nothing else until replaced)'
  if [ "$interactive" = 1 ]; then
    while :; do
      ask_secret 'New admin password (12+ characters; Enter to generate one and keep it in Keychain):'
      [ -n "$REPLY" ] || break
      [ "${#REPLY}" -ge 12 ] || { warn 'use at least 12 characters'; continue; }
      new=$REPLY
      ask_secret 'Type it again:'
      [ "$REPLY" = "$new" ] && break
      warn 'the two entries differ'
      new=''
    done
  fi
  if [ -z "$new" ]; then new=$(generate_password) generated=1; fi
  [ "${#new}" -ge 12 ] || { warn 'could not generate a password'; return 1; }
  # Keychain first: a generated password nobody has seen must never be lost.
  if ! kc_put "$new"; then
    if [ "$generated" = 1 ]; then
      warn 'Keychain refused the new password (is the login keychain locked? run: security unlock-keychain); admin was not changed'
      return 1
    fi
    warn 'Keychain refused the new password; it is still set, so keep it somewhere safe'
  fi
  if ! printf '%s\n%s\n' "$current" "$new" | cli account --url "$origin" password >/dev/null 2>"$private/cli.err"; then
    warn "$(cat "$private/cli.err")"
    # The CLI can fail after the server committed the change: ask the server
    # which password it accepts before touching the only saved copy.
    if login_as_admin "$new"; then
      signed_in=1
      say 'admin password replaced (the CLI reported an error afterwards) and saved in your login keychain'
      return 0
    fi
    security delete-generic-password -a admin -s "$kc_service" >/dev/null 2>&1
    [ -z "$stored" ] || kc_put "$stored" || true
    return 1
  fi
  # The replacement ends every session, this one included.
  signed_in=0
  login_as_admin "$new" || { warn "$(cat "$private/cli.err")"; return 1; }
  signed_in=1
  say "admin password replaced and saved in your login keychain as \"CG Agent Harness admin password\""
  say "read it back with: security find-generic-password -a admin -s '$kc_service' -w"
}

step_web_allowlist() {
  local rc
  [ "$allowlist" = 1 ] || { say 'skipped (--no-allowlist)'; return "$SKIP"; }
  [ "$signed_in" = 1 ] || { warn 'the CLI is not signed in as admin'; return 1; }
  CGAGENTHARNESS_HOME=$home CGAH_BIN=$bin CGAH_URL=$origin bash "$root/scripts/web-allowlist-bootstrap.sh"
  rc=$?
  [ "$rc" != 4 ] || warn 'raise web.max_rules in the home config.yaml, restart, and rerun'
  return "$rc"
}

launch() {
  local c=$choice ollama_flag=() trust_flag=() port
  [ "$ollama" = 1 ] || ollama_flag=(--no-ollama)
  find_app
  if [ -z "$c" ]; then
    if [ "$interactive" = 0 ]; then c=0; else
      printf '\n'
      say 'What should open now?'
      printf '    1) the CG Agent Harness app\n'
      printf '    2) the console in your default browser, without the app (served from this terminal)\n'
      printf '    3) both: the app, and its console in your browser\n'
      printf '    0) nothing for now\n'
      ask 'Choose 1, 2, 3 or 0 [3]:' 3
      c=$REPLY
    fi
  fi
  case $c in 1 | 3) [ -n "$app_path" ] || { warn 'no app is installed; opening the console in your browser instead'; c=2; } ;; esac
  if [ "$trust" = 0 ] && [ "$interactive" = 1 ] && { [ "$c" = 3 ] || { [ "$c" = 2 ] && [ -n "$(app_backends)" ]; }; }; then
    ask 'Trust the console certificate so your browser opens it without a warning? macOS asks for your password. [y/N]' n
    case $REPLY in [Yy]*) trust=1 ;; esac
  fi
  [ "$trust" = 0 ] || trust_flag=(--trust-cert)
  printf '\n'
  case $c in
    0) say 'nothing opened. Later: scripts/start-macos.sh (or --standalone for the browser only)' ;;
    1) say 'opening the app'; start_macos ${ollama_flag[@]+"${ollama_flag[@]}"} --no-browser ;;
    3) say 'opening the app and its console in your browser'; start_macos ${ollama_flag[@]+"${ollama_flag[@]}"} ${trust_flag[@]+"${trust_flag[@]}"} ;;
    2)
      if [ -n "$(app_backends)" ]; then
        say 'the app already owns the home; opening its console in your browser'
        start_macos ${ollama_flag[@]+"${ollama_flag[@]}"} ${trust_flag[@]+"${trust_flag[@]}"}
      else
        port=$(free_port 8790) || { warn 'no free loopback port in 8790-8889'; return 1; }
        say "serving the console from this terminal on port $port; press Ctrl-C to stop it"
        start_macos ${ollama_flag[@]+"${ollama_flag[@]}"} --standalone "$port"
      fi ;;
    *) warn "unknown choice: $c"; return 1 ;;
  esac
}

# --------------------------------------------------------------------- main
note "=== init.sh started $(date '+%Y-%m-%d %H:%M:%S %Z'); args: ${install_args[*]:-none}; choice ${choice:-ask}; interactive $interactive"
profile='' env_files='' home='' kc_service=''
step 'Check this Mac' required step_preflight
step 'Command Line Tools' required step_developer_tools
step 'Git hooks' optional step_checkout
step 'Install tools, backend and app (install-macos.sh)' required step_install
step 'Shell PATH for zsh or bash' optional step_shell_profile
step 'Admin password (replaced, kept in Keychain)' optional step_admin_password
step 'Web allowlist (web-allowlist-bootstrap.sh)' optional step_web_allowlist
close_server
summary
failed=0
for s in ${step_states[@]+"${step_states[@]}"}; do [ "$s" != FAILED ] || failed=1; done
launch || failed=1
[ "$failed" = 0 ] || exit 2
