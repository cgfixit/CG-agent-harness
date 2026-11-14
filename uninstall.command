#!/bin/bash
# Double-click in Finder to run scripts/uninstall-macos.sh in a Terminal window:
# it shows what it would remove, asks about shared tools (Rust, Ollama, gh), and
# removes nothing until you type UNINSTALL. It undoes init.command, this clone included.
# Options still work from a shell: ./uninstall.command --dry-run
cd "$(dirname "$0")" || exit 3
bash scripts/uninstall-macos.sh "$@"
rc=$?
# Keep the summary on screen whatever Terminal does when a shell exits.
if { : </dev/tty; } 2>/dev/null; then
  printf '\nuninstall finished (exit %s). Press Return to close. ' "$rc" >/dev/tty
  read -r _ </dev/tty || true
fi
exit "$rc"
