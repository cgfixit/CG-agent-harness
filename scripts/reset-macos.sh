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
[ ! -L "$HOME_DIR" ] || { echo "refusing: $HOME_DIR is a symlink; point CGAGENTHARNESS_HOME at the real directory" >&2; exit 2; }
[ -d "$HOME_DIR" ] || { echo "no home directory at $HOME_DIR (nothing to delete)"; HOME_DIR=""; }
if [ -n "$HOME_DIR" ]; then
  REAL="$(cd "$HOME_DIR" && pwd -P)"
  # Never delete $HOME, a top-level directory, or a directory that is not a harness home.
  case "$REAL" in "$HOME"|"$(cd "$HOME" && pwd -P)"|/|/Users) echo "refusing: $REAL is not a harness home" >&2; exit 2 ;; esac
  case "$REAL" in /Users/*/*) ;; /Users/*) echo "refusing: $REAL is a direct child of /Users" >&2; exit 2 ;; esac
  [ "$(printf '%s' "$REAL" | tr -cd / | wc -c | tr -d ' ')" -ge 2 ] || { echo "refusing: $REAL is too shallow" >&2; exit 2; }
  [ -f "$REAL/config.yaml" ] || [ -f "$REAL/harness.json" ] || { echo "refusing: $REAL has no config.yaml or harness.json, so it does not look like a harness home" >&2; exit 2; }
  command -v lsof >/dev/null || { echo "refusing: lsof not found, so open databases cannot be checked" >&2; exit 2; }
  # Every running owner (serve, the app's backend) keeps .server.lock open.
  for f in "$REAL"/.server.lock "$REAL"/auth.sqlite3 "$REAL"/memory/structured.sqlite3; do
    [ -e "$f" ] || continue
    if PIDS="$(lsof -t -- "$f" 2>/dev/null)"; then RC=0; else RC=$?; fi
    if [ -n "$PIDS" ] || [ "$RC" -gt 1 ]; then
      echo "refusing: $f is open or could not be checked; quit the harness first" >&2; exit 2
    fi
  done
  HOME_DIR="$REAL"
fi

# Keychain services: "CGagentHarness (<home>)" for managed keys, and
# "CGagentHarness admin (<home>)" for the admin password scripts/init.sh saves.
# The harness canonicalizes the home (credential_store.rs for_home) and falls
# back to the path as given, so check both.
command -v security >/dev/null || { echo "refusing: security tool not found" >&2; exit 2; }
GIVEN="${CGAGENTHARNESS_HOME:-$HOME/.CGagentHarness}"
SERVICES="CGagentHarness (${HOME_DIR:-$GIVEN})
CGagentHarness admin (${HOME_DIR:-$GIVEN})"
[ -z "$HOME_DIR" ] || [ "$GIVEN" = "$HOME_DIR" ] || SERVICES="$SERVICES
CGagentHarness ($GIVEN)
CGagentHarness admin ($GIVEN)"
# A missing home cannot be canonicalized, but the harness and init.sh stored the
# resolved path while it existed (/tmp/h is /private/tmp/h), so resolve its parent.
if [ -z "$HOME_DIR" ] && PARENT="$(cd "$(dirname -- "$GIVEN")" 2>/dev/null && pwd -P)"; then
  RESOLVED="${PARENT%/}/$(basename -- "$GIVEN")"
  [ "$RESOLVED" = "$GIVEN" ] || SERVICES="$SERVICES
CGagentHarness ($RESOLVED)
CGagentHarness admin ($RESOLVED)"
fi
COUNT=0
FOUND=""
while IFS= read -r SERVICE; do
  N=0
  while :; do
    # 44 is errSecItemNotFound; any other failure (locked, denied) aborts before rm.
    if security find-generic-password -s "$SERVICE" </dev/null >/dev/null 2>&1; then RC=0; else RC=$?; fi
    [ "$RC" -eq 44 ] && break
    [ "$RC" -eq 0 ] || { echo "refusing: Keychain lookup for $SERVICE failed (status $RC)" >&2; exit 1; }
    FOUND="$FOUND  Keychain entries with service: $SERVICE
"
    [ "$APPLY" -eq 1 ] || break
    N=$((N + 1))
    [ "$N" -le 100 ] || { echo "refusing: more than 100 Keychain entries for $SERVICE; stopping" >&2; exit 1; }
    security delete-generic-password -s "$SERVICE" </dev/null >/dev/null 2>&1 || { echo "could not delete a Keychain entry for $SERVICE" >&2; exit 1; }
    COUNT=$((COUNT + 1))
  done
done <<EOF_SERVICES
$SERVICES
EOF_SERVICES

if [ "$APPLY" -eq 0 ]; then
  echo "dry run; nothing was changed. With --yes this would remove:"
  [ -n "$HOME_DIR" ] && echo "  directory: $HOME_DIR"
  if [ -n "$FOUND" ]; then printf '%s' "$FOUND"; else echo "  (no Keychain entries found)"; fi
  echo "Quit the app first. Re-run with --yes to apply."
  exit 0
fi

[ -z "$HOME_DIR" ] || { rm -rf -- "$HOME_DIR"; echo "removed $HOME_DIR"; }
echo "removed $COUNT Keychain entr$([ "$COUNT" -eq 1 ] && echo y || echo ies)"
