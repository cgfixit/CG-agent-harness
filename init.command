#!/bin/bash
# Double-click in Finder to run scripts/init.sh in a Terminal window: the same
# one-command setup as docs/INSTALL.md ("One command: init.sh"), asking as it
# goes. Terminal starts it in your home folder, so it finds the clone from here.
# Options still work from a shell: ./init.command --yes --choice 3
cd "$(dirname "$0")" || exit 3
bash scripts/init.sh "$@"
rc=$?
# Keep the summary on screen whatever Terminal does when a shell exits.
if { : </dev/tty; } 2>/dev/null; then
  printf '\ninit.sh finished (exit %s). Press Return to close. ' "$rc" >/dev/tty
  read -r _ </dev/tty || true
fi
exit "$rc"
