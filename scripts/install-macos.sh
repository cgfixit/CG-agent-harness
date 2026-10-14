#!/usr/bin/env bash
# Install everything CGagentHarness needs on macOS 12+, from a fresh clone.
# Idempotent: tools that already work are kept, never upgraded or replaced.
#   scripts/install-macos.sh               # install missing tools, build, fetch the app
#   scripts/install-macos.sh --check       # inventory only; exit 1 if something required is missing
#   scripts/install-macos.sh --dry-run     # print every change instead of making it
# Options:
#   --model TAG      also `ollama pull TAG` (a multi-GB download; nothing is pulled by default)
#   --with-gh        install GitHub CLI (the coding pipeline needs it; chat does not)
#   --no-ollama      skip Ollama            --no-build     skip the standalone backend build
#   --no-app         skip the release app   --replace-app  replace an installed app with the latest release
#   --app-dir DIR    app destination (default /Applications, or ~/Applications when not writable)
# Then run scripts/start-macos.sh. Full guide: docs/INSTALL.md.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
app_name='CG Agent Harness.app'
release=https://github.com/cgfixit/CG-agent-harness/releases/latest/download
ollama_api=http://127.0.0.1:11434 # DevSkim: ignore DS137138,DS162092 because Ollama's default endpoint is plain HTTP on loopback.

check=0 dry=0 model='' gh=0 ollama=1 build=1 app=1 replace=0 app_dir=''
usage() { sed -n '2,13p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
while [ "$#" -gt 0 ]; do
  case $1 in
    --check) check=1 ;;
    --dry-run) dry=1 ;;
    # A value never starts with "-": `--model --dry-run` must not swallow --dry-run.
    --model) [ "$#" -ge 2 ] && [ -n "$2" ] && [ "${2#-}" = "$2" ] || { echo 'install-macos: --model needs a tag' >&2; exit 3; }; model=$2; shift ;;
    --with-gh) gh=1 ;;
    --no-ollama) ollama=0 ;;
    --no-build) build=0 ;;
    --no-app) app=0 ;;
    --replace-app) replace=1 ;;
    --app-dir) [ "$#" -ge 2 ] && [ -n "$2" ] && [ "${2#-}" = "$2" ] || { echo 'install-macos: --app-dir needs a directory' >&2; exit 3; }; app_dir=$2; shift ;;
    -h | --help) usage; exit 0 ;;
    *) echo "install-macos: unknown option: $1 (see --help)" >&2; exit 3 ;;
  esac
  shift
done
[ "$ollama" = 1 ] || [ -z "$model" ] || { echo 'install-macos: --model needs Ollama; drop --no-ollama' >&2; exit 3; }
[ "$app" = 1 ] || [ "$replace" = 0 ] || { echo 'install-macos: --replace-app conflicts with --no-app' >&2; exit 3; }
case $model in *[!A-Za-z0-9._:/-]*) echo "install-macos: invalid model tag: $model" >&2; exit 3 ;; esac

say() { printf '==> %s\n' "$*"; }
warn() { printf 'install-macos: warning: %s\n' "$*" >&2; }
die() { printf 'install-macos: %s\n' "$1" >&2; exit "${2:-2}"; }
# Every change goes through run, so --dry-run prints exactly what a real run does.
run() {
  printf '+ %s\n' "$*"
  [ "$dry" = 1 ] || "$@"
}
have() { command -v "$1" >/dev/null 2>&1; }
# A GitHub CLI that runs (not a stale link or a wrong-arch binary).
gh_ok() { have gh && gh --version >/dev/null 2>&1; }
# rustup fails `run` for a component the toolchain lacks.
lints_ok() {
  rustup run "$toolchain" cargo-clippy --version >/dev/null 2>&1 && rustup run "$toolchain" rustfmt --version >/dev/null 2>&1
}
# --noproxy: an http_proxy/ALL_PROXY must never answer for a loopback service.
ollama_up() { curl --noproxy '*' --silent --fail --max-time 2 -o /dev/null "$ollama_api/api/version"; }
# A usable app: both executables present and the bundle signature verifies.
# A partial copy fails this, so a plain rerun repairs it.
app_ok() {
  [ -x "$1/Contents/MacOS/cg-agent-harness-desktop" ] && [ -x "$1/Contents/MacOS/cgagentharness" ] \
    && codesign --verify --deep --strict "$1" >/dev/null 2>&1
}
# A launchable Ollama.app: Info.plist names an executable that is present,
# and the bundle signature verifies. An interrupted copy fails this.
ollama_bundle_ok() {
  local exe
  exe=$(defaults read "$1/Contents/Info" CFBundleExecutable 2>/dev/null) && [ -n "$exe" ] \
    && [ -x "$1/Contents/MacOS/$exe" ] && codesign --verify "$1" >/dev/null 2>&1
}
# The usable Ollama app bundle, if any.
ollama_app() {
  for a in /Applications/Ollama.app "$HOME/Applications/Ollama.app"; do
    if [ -d "$a" ] && ollama_bundle_ok "$a"; then printf '%s\n' "$a"; return; fi
  done
}
# An Ollama.app directory that is not usable (a partial copy).
ollama_app_broken() {
  for a in /Applications/Ollama.app "$HOME/Applications/Ollama.app"; do
    if [ -d "$a" ] && ! ollama_bundle_ok "$a"; then printf '%s\n' "$a"; return; fi
  done
}
# Run a server in its own session, so the Ctrl-C that stops a later
# foreground command (start-web.sh) does not reach it. macOS has no setsid(1).
# shellcheck disable=SC2016 # perl expands its own variables.
detached() { nohup /usr/bin/perl -MPOSIX -e 'POSIX::setsid(); exec @ARGV or die "exec $ARGV[0]: $!\n"' -- "$@"; }
# An `ollama` on PATH that actually runs (not a stale link or a wrong-arch binary).
path_ollama_ok() { have ollama && ollama --help >/dev/null 2>&1; }
# An `ollama` on PATH that does not run.
path_ollama_broken() { have ollama && ! ollama --help >/dev/null 2>&1; }
# A runnable Ollama CLI: the one on PATH, else the one inside Ollama.app (the cask links that same file).
ollama_cli() {
  if path_ollama_ok; then command -v ollama; return; fi
  for c in /Applications/Ollama.app/Contents/Resources/ollama "$HOME/Applications/Ollama.app/Contents/Resources/ollama"; do
    if [ -x "$c" ] && "$c" --help >/dev/null 2>&1; then printf '%s\n' "$c"; return; fi
  done
}

[ "$(uname -s)" = Darwin ] || die 'macOS only; Linux builds follow docs/INSTALL.md by hand' 3
[ "$(id -u)" != 0 ] || die 'run as your user, not root; the installers ask for a password when they need one' 3
major=$(sw_vers -productVersion | cut -d. -f1)
[ "$major" -ge 12 ] || die "macOS 12 or newer is required (found $(sw_vers -productVersion))" 3
toolchain=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$root/rust-toolchain.toml")
[ -n "$toolchain" ] || die 'cannot read the pinned toolchain from rust-toolchain.toml'

# Prefer rustup's proxies and an existing Homebrew even when this shell has not
# loaded their PATH lines yet. Homebrew's own cargo would ignore rust-toolchain.toml.
# rustup installs its proxies under $CARGO_HOME/bin (default ~/.cargo/bin).
cargo_bin=${CARGO_HOME:-$HOME/.cargo}/bin
PATH=$cargo_bin:$PATH
for brew in /opt/homebrew/bin/brew /usr/local/bin/brew; do
  if ! have brew && [ -x "$brew" ]; then eval "$("$brew" shellenv)"; fi
done
export PATH

if [ -z "$app_dir" ]; then
  app_dir=/Applications
  [ -w "$app_dir" ] || app_dir=$HOME/Applications
fi
case $app_dir in /*) ;; *) die "--app-dir must be absolute: $app_dir" 3 ;; esac
app_path=$app_dir/$app_name

# ---------------------------------------------------------------- inventory
inventory() {
  local missing=0 v want tags
  say "macOS $(sw_vers -productVersion) on $(uname -m)"
  if xcode-select -p >/dev/null 2>&1; then say "Command Line Tools: $(xcode-select -p)"; else say 'Command Line Tools: MISSING'; missing=1; fi
  if have brew; then say "Homebrew: $(brew --version 2>/dev/null | head -n1)"; else say 'Homebrew: not installed (needed only to install Ollama or gh)'; fi
  if have rustup && ! rustup --version >/dev/null 2>&1; then
    say "rustup: $(command -v rustup) does not run (remove it or fix PATH, then rerun)"; missing=1
  elif have rustup; then
    say "rustup: $(command -v rustup)"
    if rustup run "$toolchain" rustc --version >/dev/null 2>&1; then
      say "Rust $toolchain: $(rustup run "$toolchain" rustc --version)"
      lints_ok || { say "Rust $toolchain: clippy or rustfmt MISSING (the lint gate needs both)"; missing=1; }
    else say "Rust $toolchain: MISSING"; missing=1; fi
  else
    say 'rustup: MISSING'; missing=1
  fi
  v=$(command -v cargo || true)
  case $v in "$cargo_bin/cargo" | '') ;; *) warn "cargo on PATH is $v, not rustup's; it ignores rust-toolchain.toml" ;; esac
  if [ "$build" = 1 ]; then
    if [ -x "$root/target/release/cgagentharness" ]; then say "backend: $root/target/release/cgagentharness"; else say 'backend: not built'; missing=1; fi
  fi
  if [ "$ollama" = 1 ]; then
    if path_ollama_ok; then
      say "Ollama: $(ollama --version 2>&1 | sed -n 's/.*version is //p' | tail -n1)$([ -n "$(ollama_app)" ] || echo ' (CLI only; started with ollama serve)')"
    elif [ -n "$(ollama_app)" ]; then say 'Ollama: app installed, CLI not on PATH'
    elif [ -n "$(ollama_app_broken)" ]; then say "Ollama: $(ollama_app_broken) is incomplete or fails codesign (a rerun reinstalls it)"; missing=1
    elif path_ollama_broken; then say "Ollama: $(command -v ollama) does not run (a rerun reinstalls Ollama)"; missing=1
    else say 'Ollama: MISSING'; missing=1; fi
    if ollama_up; then say "Ollama API: reachable at $ollama_api"; else say 'Ollama API: not running'; fi
    if [ -n "$model" ]; then
      # Ollama stores an untagged name as :latest.
      case $model in *:*) want=$model ;; *) want=$model:latest ;; esac
      if ! ollama_up; then say "model $want: unknown (Ollama is not running)"; missing=1
      # Read the whole reply first: grep -q in a pipe can end curl early, and pipefail would count that as a miss.
      elif tags=$(curl --noproxy '*' --silent --fail --max-time 5 "$ollama_api/api/tags") \
        && case $tags in *"\"name\":\"$want\""*) true ;; *) false ;; esac; then say "model $want: pulled"
      else say "model $want: not pulled"; missing=1; fi
    fi
    v=$(launchctl getenv OLLAMA_CONTEXT_LENGTH 2>/dev/null || true)
    say "OLLAMA_CONTEXT_LENGTH for GUI apps: ${v:-unset (scripts/start-macos.sh sets 32768)}"
  fi
  if gh_ok; then say "GitHub CLI: $(gh --version 2>/dev/null | head -n1)"
  elif have gh && [ "$gh" = 1 ]; then say "GitHub CLI: $(command -v gh) does not run (a rerun reinstalls it)"; missing=1
  elif [ "$gh" = 1 ]; then say 'GitHub CLI: MISSING'; missing=1; fi
  if [ "$app" = 1 ]; then
    if app_ok "$app_path"; then say "app: $app_path (commit $(cat "$app_path/Contents/Resources/COMMIT" 2>/dev/null || echo unknown))"; elif [ -d "$app_path" ]; then say "app: $app_path is incomplete or fails codesign (a rerun replaces it)"; missing=1; else say "app: not installed at $app_path"; missing=1; fi
  fi
  return "$missing"
}

if [ "$check" = 1 ]; then
  if inventory; then say 'everything required is present'; exit 0; fi
  say 'something required is missing; run scripts/install-macos.sh'
  exit 1
fi

# ------------------------------------------------------------- 1. Xcode CLT
if ! xcode-select -p >/dev/null 2>&1; then
  run xcode-select --install
  [ "$dry" = 1 ] || die 'finish the Command Line Tools dialog, then rerun this script' 3
fi

# ----------------------------------------------------------------- 2. Homebrew
need_brew=0
if [ "$ollama" = 1 ] && ! path_ollama_ok && [ -z "$(ollama_app)" ]; then need_brew=1; fi
if [ "$gh" = 1 ] && ! gh_ok; then need_brew=1; fi
if [ "$need_brew" = 1 ] && ! have brew; then
  say 'installing Homebrew (official installer; it asks for your password)'
  installer=$(mktemp)
  run curl --fail --location --silent --show-error -o "$installer" https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh
  run /bin/bash "$installer"
  rm -f "$installer"
  for brew in /opt/homebrew/bin/brew /usr/local/bin/brew; do
    if [ -x "$brew" ]; then eval "$("$brew" shellenv)"; break; fi
  done
  [ "$dry" = 1 ] || have brew || die 'Homebrew did not install; see https://brew.sh'
  say 'add Homebrew to new shells: follow the "Next steps" the installer printed'
fi

# --------------------------------------------------------------------- 3. Rust
# A rustup that is found but cannot run is not ours to replace: stop with its path.
if have rustup && ! rustup --version >/dev/null 2>&1; then
  [ "$dry" = 1 ] || die "rustup at $(command -v rustup) does not run (wrong architecture or a stale link); remove it or fix PATH, then rerun" 3
fi
if ! have rustup; then
  say "installing rustup (official installer; it adds $cargo_bin to your shell profile)"
  installer=$(mktemp)
  # rustup's documented flags: HTTPS only, TLS 1.2 or newer. A floor, not a pin.
  run curl --proto '=https' --tlsv1.2 --fail --silent --show-error -o "$installer" https://sh.rustup.rs # DevSkim: ignore DS440001,DS440016 because --tlsv1.2 sets a minimum TLS version.
  run sh "$installer" -y --profile minimal --default-toolchain none
  rm -f "$installer"
fi
if ! have rustup || ! rustup run "$toolchain" rustc --version >/dev/null 2>&1; then
  run rustup toolchain install "$toolchain" --profile minimal --component clippy --component rustfmt
elif ! lints_ok; then
  # An existing minimal toolchain can lack the components rust-toolchain.toml lists.
  run rustup component add --toolchain "$toolchain" clippy rustfmt
fi
case $(command -v cargo || true) in
  "$cargo_bin/cargo" | '') ;;
  *) warn "cargo on PATH is $(command -v cargo), not rustup's; this script builds through rustup" ;;
esac

# ------------------------------------------------------------------- 4. Ollama
if [ "$ollama" = 1 ]; then
  # A CLI-only Ollama (no app) is kept; it is started with `ollama serve` below.
  if ! path_ollama_ok && [ -z "$(ollama_app)" ]; then
    # --force replaces a partial Ollama.app or a dead `ollama` link that would block the cask.
    if [ -n "$(ollama_app_broken)" ] || path_ollama_broken; then run brew install --cask --force ollama; else run brew install --cask ollama; fi
  fi
  if [ -n "$model" ]; then
    if ! ollama_up; then
      ollama_app_path=$(ollama_app)
      cli=$(ollama_cli || true)
      if [ -z "$ollama_app_path" ] && [ -n "$cli" ]; then
        # CLI-only install: no app to open, so run its server directly.
        log=$HOME/Library/Logs/ollama-serve.log
        printf '+ OLLAMA_CONTEXT_LENGTH=%s %s serve (in the background; log %s)\n' "${OLLAMA_CONTEXT_LENGTH:-32768}" "$cli" "$log"
        if [ "$dry" = 0 ]; then OLLAMA_CONTEXT_LENGTH=${OLLAMA_CONTEXT_LENGTH:-32768} detached "$cli" serve </dev/null >>"$log" 2>&1 & fi
      else
        if [ -z "$(launchctl getenv OLLAMA_CONTEXT_LENGTH 2>/dev/null || true)" ]; then
          run launchctl setenv OLLAMA_CONTEXT_LENGTH 32768
        fi
        # open waits on a first-launch prompt; the bounded wait below decides instead.
        # Open the validated bundle, not whichever copy LaunchServices picks.
        [ -n "$ollama_app_path" ] || [ "$dry" = 1 ] || die 'no usable Ollama.app or CLI after install; reinstall Ollama, then rerun with --model'
        printf '+ open %s (in the background)\n' "${ollama_app_path:-Ollama.app}"
        if [ "$dry" = 0 ]; then open "$ollama_app_path" </dev/null >/dev/null 2>&1 & fi
      fi
      if [ "$dry" = 0 ]; then
        for _ in $(seq 1 60); do ollama_up && break; sleep 1; done
        ollama_up || die "Ollama did not answer at $ollama_api; open it once from Applications (or check ~/Library/Logs/ollama-serve.log), answer any prompt, and rerun"
      fi
    fi
    cli=$(ollama_cli || true)
    if [ -z "$cli" ] && [ "$dry" = 1 ]; then cli=ollama; fi
    [ -n "$cli" ] || die 'no Ollama CLI found (not on PATH, not inside Ollama.app); reinstall Ollama, then rerun with --model'
    run "$cli" pull "$model"
  fi
fi

# ----------------------------------------------------------------------- 5. gh
if [ "$gh" = 1 ] && ! gh_ok; then
  # reinstall repairs a broken Homebrew gh; install covers a missing one.
  if brew list --formula gh >/dev/null 2>&1; then run brew reinstall gh; else run brew install gh; fi
fi

# ---------------------------------------------------- 6. standalone backend
if [ "$build" = 1 ]; then
  say "building the backend with Rust $toolchain (first build compiles every dependency)"
  # Through rustup, wherever it lives (~/.cargo, Homebrew, a custom CARGO_HOME).
  (cd "$root" && run rustup run "$toolchain" cargo build --release --locked)
fi

# ---------------------------------------------------------------- 7. the app
if [ "$app" = 1 ]; then
  if [ "$replace" = 0 ] && app_ok "$app_path"; then
    say "keeping $app_path (commit $(cat "$app_path/Contents/Resources/COMMIT" 2>/dev/null || echo unknown)); --replace-app updates it"
  else
    [ ! -d "$app_path" ] || [ "$replace" = 1 ] || say "$app_path is incomplete or fails codesign; replacing it"
    # The shell can stay open on its recovery screen with no backend child.
    if pgrep -u "$(id -u)" -f '/Contents/MacOS/(cgagentharness desktop$|cg-agent-harness-desktop( |$))' >/dev/null 2>&1; then
      die 'the app is running; quit it with Cmd-Q (wait for work to stop), then rerun'
    fi
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    say "downloading the latest release app into $work"
    run curl --fail --location --silent --show-error -o "$work/app.zip" "$release/CG-Agent-Harness-macos-universal.zip"
    run curl --fail --location --silent --show-error -o "$work/SHA256SUMS" "$release/SHA256SUMS"
    if [ "$dry" = 0 ]; then
      # Integrity, not authorship: the checksum comes from the same release page.
      want=$(awk '$2 == "CG-Agent-Harness-macos-universal.zip" || $2 == "*CG-Agent-Harness-macos-universal.zip" {print $1; exit}' "$work/SHA256SUMS")
      got=$(shasum -a 256 "$work/app.zip" | awk '{print $1}')
      [ -n "$want" ] && [ "$want" = "$got" ] || die 'checksum mismatch: the download is incomplete or modified; nothing was installed'
      say "checksum ok ($got)"
    fi
    run ditto -x -k "$work/app.zip" "$work/unpacked"
    if [ "$dry" = 0 ]; then
      [ -d "$work/unpacked/$app_name" ] || die "the archive has no $app_name"
      codesign --verify --deep --strict "$work/unpacked/$app_name" || die 'the app signature does not verify; nothing was installed'
    fi
    run mkdir -p "$app_dir"
    # Copy next to the destination first, so a failed copy never touches the
    # working app; the swap below is two renames.
    staged=$app_dir/.$app_name.partial
    run rm -rf "$staged"
    run ditto "$work/unpacked/$app_name" "$staged" || die "copying the app failed; $app_path is unchanged (rerun to retry)"
    backup=$HOME/Library/Caches/CG-agent-harness-installer/previous
    if app_ok "$app_path"; then
      # Keep one previous working copy for rollback; the home directory is never touched.
      run rm -rf "$backup"
      run mkdir -p "$backup"
      run mv "$app_path" "$backup/"
      say "previous app kept at $backup/$app_name"
    elif [ -d "$app_path" ]; then
      # A broken copy is discarded, so it never displaces a working backup.
      run rm -rf "$app_path"
    fi
    run mv "$staged" "$app_path"
    [ "$dry" = 1 ] || say "installed $app_path (commit $(cat "$app_path/Contents/Resources/COMMIT" 2>/dev/null || echo unknown))"
  fi
fi

if [ "$app" = 0 ]; then
  say 'done. Next: scripts/start-macos.sh --standalone (no app: runs the console server from this clone).'
else
  case $app_dir in
    /Applications) say 'done. Next: scripts/start-macos.sh (opens the app and its console in your browser).' ;;
    "$HOME/Applications")
      # The launcher tries /Applications first; name this copy when one is there too.
      if [ -d "/Applications/$app_name" ]; then
        say "done. Next: CGAH_APP=$(printf %q "$app_path") scripts/start-macos.sh (/Applications also has a copy, which the launcher would pick first)"
      else
        say 'done. Next: scripts/start-macos.sh (opens the app and its console in your browser).'
      fi ;;
    *) say "done. Next: CGAH_APP=$(printf %q "$app_path") scripts/start-macos.sh (the launcher searches only /Applications and ~/Applications)" ;;
  esac
fi
say 'First sign-in is user admin; docs/INSTALL.md (First run) says where its first credential comes from. A new password is required next.'
