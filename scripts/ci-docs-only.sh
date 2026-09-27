#!/usr/bin/env bash
# CI path classifier for the `changes` job in ci.yml and bundle.yml.
#
# Reads `git diff --no-renames --name-only` output on stdin. Exits 0 only when
# every path is documentation that no build, test or package step reads, so a
# docs-only pull request may skip the compile jobs. Anything else exits 1 and
# CI runs everything: an unknown path, empty input, or a packaging script this
# classifier can no longer read.
#
#   docs-only: docs/** except the files scripts/package-desktop.sh bundles,
#              top-level *.md except README.md, AGENTS.md and CLAUDE.md,
#              *.md under .claude/ or .codex/, and LICENSE.
#   code:      everything else. assets/ and data/ hold Markdown compiled in
#              with include_str!; tests/invariant_guard.rs includes README.md
#              and asserts AGENTS.md exists and a root CLAUDE.md does not.
#
# tests/invariant_guard.rs also reads every Markdown file for the docs budget,
# so ci.yml runs its invariant-guard job whatever this script says.
set -euo pipefail

packaging="$(dirname "$0")/package-desktop.sh"
# Bundled docs are packaging inputs: moving one breaks the desktop bundle.
# A glob under docs/ would hide which files ship, so it counts as code.
bundled=$(grep -oE 'docs/[A-Za-z0-9_./-]+' "$packaging") || exit 1
if grep -qE 'docs/[^[:space:]"]*[][*?]' "$packaging"; then exit 1; fi

count=0
while IFS= read -r path || [ -n "$path" ]; do
  [ -n "$path" ] || continue
  count=$((count + 1))
  case $path in
    README.md | AGENTS.md | CLAUDE.md) exit 1 ;;
    docs/*) if grep -qxF -- "$path" <<<"$bundled"; then exit 1; fi ;;
    .claude/*.md | .codex/*.md) ;;
    */*) exit 1 ;;
    *.md | LICENSE) ;;
    *) exit 1 ;;
  esac
done
[ "$count" -gt 0 ]
