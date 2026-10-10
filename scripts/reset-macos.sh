#!/usr/bin/env bash
# Return this Mac to a fresh-install state for the harness: delete the home
# directory and the Keychain entries the harness stored for that home.
#   scripts/reset-macos.sh            # dry run: prints what it would remove
#   scripts/reset-macos.sh --yes      # actually removes it
# Quit the app first. Removing the .app itself is separate: move it to Trash.
# The home holds accounts, sessions, memory, skills and logs; back it up first
# if you want any of it (cp -R ~/.CGagentHarness ~/CGagentHarness-backup).
set -euo pipefail

[ "$(uname -s)" = Darwin ] || { echo "reset-macos.sh: macOS only" >&2; exit 2; }
APPLY=0
case "${1:-}" in
  "") ;;
  --yes) APPLY=1 ;;
  *) echo "usage: scripts/reset-macos.sh [--yes]" >&2; exit 2 ;;
esac
[ "$#" -le 1 ] || { echo "usage: scripts/reset-macos.sh [--yes]" >&2; exit 2; }

HOME_DIR="${CGAGENTHARNESS_HOME:-$HOME/.CGagentHarness}"
HOME_DIR="${HOME_DIR%/}"
case "$HOME_DIR" in /*) ;; *) echo "refusing: home must be an absolute path: $HOME_DIR" >&2; exit 2 ;; esac
[ -d "$HOME_DIR" ] && [ ! -L "$HOME_DIR" ] || { echo "no home directory at $HOME_DIR (nothing to delete)"; HOME_DIR=""; }
if [ -n "$HOME_DIR" ]; then
  REAL="$(cd "$HOME_DIR" && pwd -P)"
  # Never delete $HOME, a top-level directory, or a directory that is not a harness home.
  case "$REAL" in "$HOME"|"$(cd "$HOME" && pwd -P)"|/|/Users|/Users/*/) echo "refusing: $REAL is not a harness home" >&2; exit 2 ;; esac
  [ "$(printf '%s' "$REAL" | tr -cd / | wc -c | tr -d ' ')" -ge 2 ] || { echo "refusing: $REAL is too shallow" >&2; exit 2; }
  [ -f "$REAL/config.yaml" ] || [ -f "$REAL/harness.json" ] || { echo "refusing: $REAL has no config.yaml or harness.json, so it does not look like a harness home" >&2; exit 2; }
  for f in "$REAL"/auth.sqlite3 "$REAL"/memory/structured.sqlite3; do
    if [ -e "$f" ] && command -v lsof >/dev/null && lsof -t -- "$f" >/dev/null 2>&1; then
      echo "refusing: $f is open; quit the harness first" >&2; exit 2
    fi
  done
  HOME_DIR="$REAL"
fi

# Keychain service name: "CGagentHarness (<home path>)", one entry per stored key.
SERVICE="CGagentHarness (${HOME_DIR:-${CGAGENTHARNESS_HOME:-$HOME/.CGagentHarness}})"
COUNT=0
while security find-generic-password -s "$SERVICE" >/dev/null 2>&1; do
  COUNT=$((COUNT + 1))
  if [ "$APPLY" -eq 1 ]; then
    security delete-generic-password -s "$SERVICE" >/dev/null 2>&1 || { echo "could not delete a Keychain entry for $SERVICE" >&2; exit 1; }
  else
    break
  fi
done

if [ "$APPLY" -eq 0 ]; then
  echo "dry run; nothing was changed. With --yes this would remove:"
  [ -n "$HOME_DIR" ] && echo "  directory: $HOME_DIR"
  if [ "$COUNT" -gt 0 ]; then echo "  Keychain entries with service: $SERVICE"; else echo "  (no Keychain entries found for: $SERVICE)"; fi
  echo "Quit the app first. Re-run with --yes to apply."
  exit 0
fi

[ -z "$HOME_DIR" ] || { rm -rf -- "$HOME_DIR"; echo "removed $HOME_DIR"; }
echo "removed $COUNT Keychain entr$([ "$COUNT" -eq 1 ] && echo y || echo ies) for $SERVICE"
