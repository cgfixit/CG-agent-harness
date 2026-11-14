#!/usr/bin/env bash
# Undo scripts/init.sh on this Mac: the harness home and its Keychain entries
# (through scripts/reset-macos.sh), the trusted console certificate, the app and
# its macOS data, the installer's leftovers, and then this clone. Shared tools
# (Rust, Ollama, models, gh) go only when you say so; Homebrew and the Command
# Line Tools are never removed, only named with the command that removes them.
# It prints the plan first and changes nothing until you type UNINSTALL.
#   scripts/uninstall-macos.sh                     # asks as it goes
#   scripts/uninstall-macos.sh --dry-run           # prints the plan only
#   scripts/uninstall-macos.sh --yes --remove-ollama --remove-models
# Options:
#   --dry-run        print what would be removed, change nothing
#   --yes            never ask; removes harness items, and shared tools only by flag
#   --keep-clone     keep this clone (its target/ and scripts/logs/ are still removed)
#   --remove-toolchain  the pinned Rust toolchain (rust-toolchain.toml); rustup stays
#   --remove-rustup  rustup, ~/.cargo and ~/.rustup (every Rust toolchain on this Mac)
#   --remove-ollama  Ollama (Homebrew cask) and its serve log
#   --remove-models  ~/.ollama: every downloaded model
#   --remove-gh      GitHub CLI (Homebrew formula)
#   --app-dir DIR    also look for the app in DIR (init.sh --app-dir)
# Exit codes: 0 done, 2 a removal failed, 3 wrong environment, 4 refused or declined.
# Full guide: docs/INSTALL.md ("Update, backup, rollback and uninstall").
set -uo pipefail

[ "$(uname -s)" = Darwin ] || { echo 'uninstall-macos: macOS only' >&2; exit 3; }
[ "$(id -u)" != 0 ] || { echo 'uninstall-macos: run as your user, not root (no sudo)' >&2; exit 3; }

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
bundle_id=com.cgfixit.agent-harness
app_name='CG Agent Harness.app'
cert_cn='CG Agent Harness local portal'
login_kc=$HOME/Library/Keychains/login.keychain-db
marker='# Added by CG-agent-harness scripts/init.sh'
dry=0 interactive=1 keep_clone=0 toolchain_rm=0 rustup_rm=0 ollama_rm=0 models_rm=0 gh_rm=0 extra_app_dir=''
[ -t 0 ] || interactive=0

usage() { sed -n '2,21p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
while [ "$#" -gt 0 ]; do
  case $1 in
    --dry-run) dry=1 ;;
    --yes) interactive=0 ;;
    --keep-clone) keep_clone=1 ;;
    --remove-toolchain) toolchain_rm=1 ;;
    --remove-rustup) rustup_rm=1 ;;
    --remove-ollama) ollama_rm=1 ;;
    --remove-models) models_rm=1 ;;
    --remove-gh) gh_rm=1 ;;
    --app-dir)
      [ "$#" -ge 2 ] && [ -n "$2" ] && [ "${2#-}" = "$2" ] || { echo 'uninstall-macos: --app-dir needs a directory' >&2; exit 3; }
      extra_app_dir=$2; shift ;;
    -h | --help) usage; exit 0 ;;
    *) echo "uninstall-macos: unknown option: $1 (see --help)" >&2; exit 3 ;;
  esac
  shift
done

say() { printf '==> %s\n' "$*"; }
warn() { printf 'uninstall-macos: warning: %s\n' "$*" >&2; }
have() { command -v "$1" >/dev/null 2>&1; }
failed=0
# run CMD...: print it; under --dry-run do nothing else; a failure is counted, not fatal.
run() {
  printf '+ %s\n' "$*"
  [ "$dry" = 1 ] && return 0
  "$@" || { warn "failed (exit $?): $*"; failed=1; return 1; }
}
# yes_no QUESTION DEFAULT(y|n): interactive answer, else the default.
yes_no() {
  local reply
  [ "$interactive" = 1 ] || { [ "$2" = y ]; return; }
  printf '%s [%s] ' "$1" "$([ "$2" = y ] && echo Y/n || echo y/N)"
  IFS= read -r reply </dev/tty || reply=''
  case ${reply:-$2} in [Yy]*) return 0 ;; *) return 1 ;; esac
}
app_running() { pgrep -u "$(id -u)" -f '/Contents/MacOS/(cgagentharness desktop$|cg-agent-harness-desktop( |$))' >/dev/null 2>&1; }
# The app bundles this harness installed: ours by bundle identifier, never by name alone.
our_apps() {
  local d
  for d in /Applications "$HOME/Applications" ${extra_app_dir:+"$extra_app_dir"}; do
    [ -d "$d/$app_name" ] || continue
    [ "$(defaults read "$d/$app_name/Contents/Info" CFBundleIdentifier 2>/dev/null)" = "$bundle_id" ] && printf '%s\n' "$d/$app_name"
  done | awk '!seen[$0]++'
}
# Homes to reset: the shell's (or the default) and the one GUI apps get from launchctl.
homes() {
  local launchd
  printf '%s\n' "${CGAGENTHARNESS_HOME:-$HOME/.CGagentHarness}"
  launchd=$(launchctl getenv CGAGENTHARNESS_HOME 2>/dev/null || true)
  [ -z "$launchd" ] || [ "$launchd" = "${CGAGENTHARNESS_HOME:-$HOME/.CGagentHarness}" ] || printf '%s\n' "$launchd"
}
cert_hashes() { security find-certificate -a -c "$cert_cn" -Z "$login_kc" 2>/dev/null | awk '/^SHA-1 hash:/ {print $3}'; }
brew_has() { have brew && brew list "$1" "$2" >/dev/null 2>&1; }
# Homebrew has shipped the Ollama app as both of these cask names.
ollama_cask() { local c; for c in ollama ollama-app; do brew_has --cask "$c" && { echo "$c"; return 0; }; done; return 1; }
toolchain=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$root/rust-toolchain.toml" 2>/dev/null)
# This clone, recognized by its own files, never $HOME or a shallow path.
clone_ok() {
  local h
  h=$(cd "$HOME" && pwd -P) || return 1
  [ -f "$root/init.command" ] && [ -f "$root/scripts/uninstall-macos.sh" ] && grep -q '^name = "cgagentharness"' "$root/Cargo.toml" 2>/dev/null || return 1
  # Never $HOME or a folder that contains it; at least /Users/<you>/<folder> deep.
  case "$h/" in "$root"/*) return 1 ;; esac
  [ "$(printf '%s' "$root" | tr -cd / | wc -c | tr -d ' ')" -ge 3 ]
}
# Local work a deleted clone would lose: uncommitted changes, or commits on no remote.
clone_unsaved() {
  [ -e "$root/.git" ] && have git || return 1
  [ -n "$(git -C "$root" status --porcelain 2>/dev/null)" ] && return 0
  [ -n "$(git -C "$root" log --branches --not --remotes --oneline 2>/dev/null | head -n1)" ]
}

# ------------------------------------------------------------------ choices
if [ "$interactive" = 1 ]; then
  say 'Shared tools: other software may use them, so each is kept unless you say yes.'
  [ "$toolchain_rm" = 1 ] || ! have rustup || ! rustup run "$toolchain" rustc --version >/dev/null 2>&1 ||
    { yes_no "Remove the Rust $toolchain toolchain the build used (rustup stays)?" n && toolchain_rm=1; }
  [ "$rustup_rm" = 1 ] || ! have rustup || { yes_no 'Remove rustup entirely (~/.cargo, ~/.rustup, every Rust toolchain)?' n && rustup_rm=1; }
  [ "$ollama_rm" = 1 ] || ! ollama_cask >/dev/null || { yes_no 'Remove Ollama?' n && ollama_rm=1; }
  [ "$models_rm" = 1 ] || [ ! -d "$HOME/.ollama" ] || { yes_no "Remove every Ollama model in ~/.ollama ($(du -sh "$HOME/.ollama" 2>/dev/null | awk '{print $1}'))?" n && models_rm=1; }
  [ "$gh_rm" = 1 ] || ! brew_has --formula gh || { yes_no 'Remove GitHub CLI (gh)?' n && gh_rm=1; }
  [ "$keep_clone" = 1 ] || yes_no "Delete this clone ($root) at the end?" y || keep_clone=1
fi
[ "$rustup_rm" = 0 ] || toolchain_rm=0
if [ "$keep_clone" = 0 ] && ! clone_ok; then warn "$root does not look like a CG-agent-harness clone; keeping it"; keep_clone=1; fi
if [ "$keep_clone" = 0 ] && clone_unsaved; then
  warn "$root has uncommitted changes or commits no remote has"
  if [ "$interactive" = 0 ] || ! yes_no 'Delete it anyway, losing that work?' n; then say 'keeping the clone'; keep_clone=1; fi
fi

# --------------------------------------------------------------------- plan
# bash 3.2 (macOS /bin/bash) has no mapfile; the app name has spaces.
apps=()
while IFS= read -r a; do apps+=("$a"); done < <(our_apps)
say 'Plan:'
while IFS= read -r h; do echo "    harness home and Keychain entries for $h (scripts/reset-macos.sh)"; done < <(homes)
[ -z "$(launchctl getenv CGAGENTHARNESS_HOME 2>/dev/null || true)" ] || echo '    launchctl CGAGENTHARNESS_HOME'
n=$(cert_hashes | grep -c . || true)
[ "$n" = 0 ] || echo "    $n trusted \"$cert_cn\" certificate(s) in the login keychain"
for a in ${apps[@]+"${apps[@]}"}; do echo "    app: $a"; done
echo "    app data under ~/Library for $bundle_id; ~/Library/Caches/CG-agent-harness-installer"
echo "    init.sh lines in your shell profile whose tool is gone"
[ "$toolchain_rm" = 0 ] || echo "    Rust $toolchain toolchain"
[ "$rustup_rm" = 0 ] || echo '    rustup, ~/.cargo, ~/.rustup'
[ "$ollama_rm" = 0 ] || echo '    Ollama, ~/Library/Logs/ollama-serve.log, launchctl OLLAMA_CONTEXT_LENGTH'
[ "$models_rm" = 0 ] || echo '    ~/.ollama (models)'
[ "$gh_rm" = 0 ] || echo '    GitHub CLI'
if [ "$keep_clone" = 0 ]; then echo "    this clone: $root"; else echo "    $root/target and $root/scripts/logs (clone kept)"; fi
say 'Kept: Homebrew and the Command Line Tools (see the end), other homes, gh sign-ins.'
if [ "$dry" = 1 ]; then say 'dry run: nothing changes; the commands are printed below.'
elif [ "$interactive" = 1 ]; then
  printf 'Type UNINSTALL to remove all of the above: '
  IFS= read -r reply </dev/tty || reply=''
  [ "$reply" = UNINSTALL ] || { say 'nothing was removed'; exit 4; }
fi

# ---------------------------------------------------------------- 1. stop
if app_running; then
  if [ "$dry" = 1 ]; then warn 'the app is running; quit it before the real run'
  elif [ "$interactive" = 1 ] && yes_no 'The app is running. Quit it now?' y; then
    osascript -e 'quit app "CG Agent Harness"' || true
    for _ in $(seq 1 60); do app_running || break; sleep 0.5; done
  fi
  [ "$dry" = 1 ] || ! app_running || { echo 'uninstall-macos: quit the app (Cmd-Q) and rerun; nothing was removed' >&2; exit 4; }
fi
if [ "$ollama_rm" = 1 ] && [ "$dry" = 0 ] && pgrep -xq Ollama; then osascript -e 'quit app "Ollama"' >/dev/null 2>&1 || true; fi

# --------------------------------------------- 2. home, Keychain, certificate
# reset-macos.sh refuses before deleting anything when a server still holds the
# home; a failure stops here, keeping the clone that holds the script to rerun.
while IFS= read -r h; do
  say "resetting $h"
  if [ "$dry" = 1 ]; then CGAGENTHARNESS_HOME=$h bash "$root/scripts/reset-macos.sh"
  elif ! CGAGENTHARNESS_HOME=$h bash "$root/scripts/reset-macos.sh" --yes; then
    echo "uninstall-macos: resetting $h failed (above); nothing after it was removed. Fix that, then rerun." >&2
    exit 2
  fi
done < <(homes)
[ -z "$(launchctl getenv CGAGENTHARNESS_HOME 2>/dev/null || true)" ] || run launchctl unsetenv CGAGENTHARNESS_HOME
# A trusted certificate outlives the home whose key it names; -t drops its trust setting too.
for h in $(cert_hashes); do run security delete-certificate -t -Z "$h" "$login_kc"; done

# ---------------------------------------------------------- 3. app, its data
for a in ${apps[@]+"${apps[@]}"}; do run rm -rf -- "$a"; done
for p in "Library/Caches/$bundle_id" "Library/WebKit/$bundle_id" "Library/HTTPStorages/$bundle_id" \
  "Library/Saved Application State/$bundle_id.savedState" "Library/Preferences/$bundle_id.plist" \
  Library/Caches/CG-agent-harness-installer; do
  [ ! -e "$HOME/$p" ] || run rm -rf -- "$HOME/$p"
done

# ------------------------------------------------------------ 4. shared tools
[ "$toolchain_rm" = 0 ] || run rustup toolchain uninstall "$toolchain"
[ "$rustup_rm" = 0 ] || run rustup self uninstall -y
if [ "$ollama_rm" = 1 ]; then
  if c=$(ollama_cask); then run brew uninstall --cask "$c"; fi
  [ ! -e "$HOME/Library/Logs/ollama-serve.log" ] || run rm -f -- "$HOME/Library/Logs/ollama-serve.log"
  [ -z "$(launchctl getenv OLLAMA_CONTEXT_LENGTH 2>/dev/null || true)" ] || run launchctl unsetenv OLLAMA_CONTEXT_LENGTH
fi
[ "$models_rm" = 0 ] || [ ! -d "$HOME/.ollama" ] || run rm -rf -- "$HOME/.ollama"
[ "$gh_rm" = 0 ] || ! brew_has --formula gh || run brew uninstall gh

# ----------------------------------------------------------- 5. shell profile
# init.sh writes a marker line, then one PATH line. Drop the pair once its tool is gone.
for f in "$HOME/.zprofile" "$HOME/.bash_profile" "$HOME/.profile"; do
  [ -f "$f" ] || continue
  grep -qxF "$marker" "$f" || continue
  cargo_gone=0 brew_gone=0
  { [ "$rustup_rm" = 1 ] || [ ! -f "$HOME/.cargo/env" ]; } && cargo_gone=1
  [ -x /opt/homebrew/bin/brew ] || brew_gone=1
  new=$(awk -v m="$marker" -v c="$cargo_gone" -v b="$brew_gone" '
    { line[NR] = $0 }
    END {
      for (i = 1; i < NR; i++)
        if (line[i] == m && ((line[i + 1] ~ /\.cargo\/env/ && c) || (line[i + 1] ~ /brew shellenv/ && b))) {
          skip[i] = skip[i + 1] = drop = 1
          if (i > 1 && line[i - 1] == "") skip[i - 1] = 1   # the blank line init.sh wrote before the marker
        }
      for (i = 1; i <= NR; i++) if (!skip[i]) print line[i]
      exit !drop
    }' "$f") || continue
  printf '+ remove init.sh PATH lines from %s\n' "$f"
  [ "$dry" = 1 ] || { cp -p -- "$f" "$f.cgah-uninstall.bak" && printf '%s\n' "$new" >"$f" && say "kept a copy at $f.cgah-uninstall.bak"; } || failed=1
done

# Parsed whole before it runs: step 6 deletes this file.
{
# ------------------------------------------------------------------ 6. clone
if [ "$keep_clone" = 0 ]; then
  cd / && run rm -rf -- "$root"
else
  for p in target scripts/logs; do [ ! -e "$root/$p" ] || run rm -rf -- "$root/$p"; done
fi

say 'Left in place (remove yourself only if nothing else needs them):'
echo '    Homebrew: see https://github.com/Homebrew/install#uninstall-homebrew'
echo '    Command Line Tools: sudo rm -rf /Library/Developer/CommandLineTools'
[ "$gh_rm" = 1 ] || ! have gh || echo '    gh sign-in: gh auth logout'
if [ "$failed" = 1 ]; then say 'finished with failures; the lines above say which'; exit 2; fi
say "$([ "$dry" = 1 ] && echo 'dry run finished' || echo 'uninstalled')"
exit 0
}
