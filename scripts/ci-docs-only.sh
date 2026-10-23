#!/usr/bin/env bash
# CI path classifier for the `changes` job in ci.yml.
#
# Reads `git diff --no-renames --name-only` output on stdin. Exits 0 only when
# every path is documentation that no build, test or package step reads, so a
# docs-only pull request may skip the compile jobs. Anything else exits 1 and
# CI runs everything: an unknown path, empty input, or a packaging script this
# classifier can no longer read.
#
#   docs-only: docs/** except the files scripts/package-desktop.sh bundles
#              and the files scripts/build-self-manual.py reads,
#              top-level *.md except README.md, AGENTS.md, CLAUDE.md and
#              INVARIANTS.md, *.md under .claude/ or .codex/, and LICENSE.
#   code:      everything else. assets/ and data/ hold Markdown compiled in
#              with include_str!; tests/invariant_guard.rs includes README.md
#              and the root CLAUDE.md, and asserts AGENTS.md exists and a
#              .claude/CLAUDE.md does not. tests/self_manual.rs regenerates
#              assets/self/MANUAL.md from the generator's INPUTS and fails
#              when the checked-in copy is stale, so editing an input without
#              regenerating must not skip the test job.
#
# tests/invariant_guard.rs also reads every Markdown file for the docs budget,
# so ci.yml runs its invariant-guard job whatever this script says.
set -euo pipefail

packaging="$(dirname "$0")/package-desktop.sh"
# Bundled docs are packaging inputs: moving one breaks the desktop bundle.
# A glob under docs/ would hide which files ship, so it counts as code.
bundled=$(grep -oE 'docs/[A-Za-z0-9_./-]+' "$packaging") || exit 1
if grep -qE 'docs/[^[:space:]"]*[][*?]' "$packaging"; then exit 1; fi
generator="$(dirname "$0")/build-self-manual.py"
# The INPUTS list of the self-manual generator, one path per line. An empty
# or unreadable list counts as code, like an unreadable packaging script.
inputs=$(awk '/^INPUTS = \[/ { reading = 1; next } reading && /^\]/ { exit } reading' "$generator" |
  grep -oE '"[^"]+"' | tr -d '"') || exit 1

count=0
while IFS= read -r path || [ -n "$path" ]; do
  [ -n "$path" ] || continue
  count=$((count + 1))
  if grep -qxF -- "$path" <<<"$inputs"; then exit 1; fi
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
