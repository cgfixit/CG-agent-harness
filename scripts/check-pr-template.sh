#!/usr/bin/env bash
# Local PR-body gate against .github/PULL_REQUEST_TEMPLATE.md minimums.
#
# Usage:
#   scripts/check-pr-template.sh PATH/TO/body.md
#   gh pr view --json body -q .body | scripts/check-pr-template.sh -
#   CGAGENTHARNESS_PR_BODY_FILE=body.md scripts/check-pr-template.sh
#   CYCLAW_PR_BODY_FILE=body.md scripts/check-pr-template.sh   # fallback alias
#
# Exit 0 = ok; exit 1 = missing required sections.
# Git hooks cannot intercept GitHub API / gh pr create bodies — agents and
# humans should run this before opening a PR. CI runs the same headers as a
# blocking check (.github/workflows/pr-template-check.yml).
#
# The core-path rule is mirrored from that workflow too: when the change
# touches src/shim/, the guard or header layers, writer, sandbox, workspace,
# or the shipped config, the body must mention an invariant. The changed-file
# list comes from CGAGENTHARNESS_PR_FILES (newline-separated) when set,
# otherwise from the working tree against the merge base with
# CGAGENTHARNESS_PR_BASE (default origin/main). With neither source the rule
# is reported as skipped rather than silently passed.
set -euo pipefail

input="${1:-${CGAGENTHARNESS_PR_BODY_FILE:-${CYCLAW_PR_BODY_FILE:-}}}"
if [[ -z "$input" ]]; then
  printf '%s\n' \
    "usage: scripts/check-pr-template.sh <body.md|->" \
    "   or: CGAGENTHARNESS_PR_BODY_FILE=body.md scripts/check-pr-template.sh" \
    "   or: CYCLAW_PR_BODY_FILE=body.md scripts/check-pr-template.sh  # fallback alias" \
    >&2
  exit 2
fi

if [[ "$input" == "-" ]]; then
  body="$(cat)"
elif [[ -f "$input" ]]; then
  body="$(cat "$input")"
else
  printf 'check-pr-template: file not found: %s\n' "$input" >&2
  exit 2
fi

fail=0
missing=()

require_header() {
  local label="$1"
  local pattern="$2"
  if ! printf '%s' "$body" | grep -Eiq "$pattern"; then
    missing+=("$label")
    fail=1
  fi
}

# Align with template + advisory CI loose matching, but require the
# CGagentHarness-named sections that Grok Build / agents must fill.
require_header "Proposed changes (or Why/Benefits/Summary)" \
  '^#{1,4}[[:space:]]*(proposed changes|benefits|why|summary|what)\b'
require_header "Types of changes" \
  '^#{1,4}[[:space:]]*types of changes\b'
require_header "Benefits / why" \
  '^#{1,4}[[:space:]]*(benefits|why)\b'
require_header "Risks to monitor" \
  '^#{1,4}[[:space:]]*risks?([[:space:]]*(to[[:space:]]*monitor|impact))?\b'
require_header "Checklist" \
  '^#{1,4}[[:space:]]*checklist\b'

# CI measures the trimmed body; a whitespace-padded stub must not pass here
# and fail there.
trimmed="${body#"${body%%[![:space:]]*}"}"
trimmed="${trimmed%"${trimmed##*[![:space:]]}"}"
if [[ "${#trimmed}" -lt 40 ]]; then
  missing+=("Body too short (< 40 chars)")
  fail=1
fi

# Core-path rule (same file set as pr-template-check.yml).
core_pattern='^(src/shim/|src/server/guards\.rs$|src/server/headers\.rs$|src/agentic/writer\.rs$|src/agentic/executor/sandbox\.rs$|src/agentic/workspace\.rs$|assets/config\.default\.yaml$)'
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
base="${CGAGENTHARNESS_PR_BASE:-origin/main}"
changed=""
files_source=""
if [[ -n "${CGAGENTHARNESS_PR_FILES:-}" ]]; then
  changed="$CGAGENTHARNESS_PR_FILES"
  files_source="CGAGENTHARNESS_PR_FILES"
elif merge_base="$(git -C "$repo_root" merge-base "$base" HEAD 2>/dev/null)"; then
  # --no-renames: a core file moved elsewhere must still surface its source
  # path, not only the destination Git's rename detection would report.
  changed="$(git -C "$repo_root" diff --name-only --no-renames "$merge_base" 2>/dev/null || true)"
  files_source="git diff against $base merge base"
fi
if [[ -z "$files_source" ]]; then
  printf 'check-pr-template: core-path rule skipped (set CGAGENTHARNESS_PR_FILES or fetch %s)\n' "$base" >&2
elif printf '%s\n' "$changed" | grep -Eq "$core_pattern"; then
  # The template itself says "invariant" in its headings and checklist, so
  # only contributor-written lines (those not copied verbatim from the
  # template) can satisfy the statement requirement.
  # Ticking a template checkbox is not a statement either, so `- [x]` is
  # folded back to `- [ ]` on both sides before the comparison.
  template="$repo_root/.github/PULL_REQUEST_TEMPLATE.md"
  fold_boxes() { sed -E 's/^([[:space:]]*- \[)[xX](\])/\1 \2/'; }
  contributed="$body"
  if [[ -f "$template" ]]; then
    contributed="$(printf '%s\n' "$body" | fold_boxes | grep -Fxv -f <(fold_boxes < "$template") || true)"
  fi
  if ! printf '%s' "$contributed" | grep -Eiq 'invariant'; then
    missing+=("Invariant / Governance Impact statement (a core path changed; say which invariant and why it holds)")
    fail=1
  fi
fi

if [[ "$fail" -ne 0 ]]; then
  printf '%s\n' \
    "check-pr-template: PR body is missing required sections from" \
    "  .github/PULL_REQUEST_TEMPLATE.md" \
    "" \
    "Missing:" \
    >&2
  for m in "${missing[@]}"; do
    printf '  - %s\n' "$m" >&2
  done
  printf '%s\n' \
    "" \
    "Fill the full template before: gh pr create / GitHub connector create_pull_request" \
    "Grok Build branch prefix: grok/<feature>" \
    "Title format: [prefix] - Short descriptive sentence" \
    >&2
  exit 1
fi

printf 'check-pr-template: OK — required template sections present (%s)\n' "${files_source:-core-path rule skipped}"
exit 0
