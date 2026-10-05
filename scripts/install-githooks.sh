#!/usr/bin/env bash
# Wire this clone to the repo-managed hooks under .githooks/ (once per clone).
# Equivalent one-liner for an agent setup step: git config core.hooksPath .githooks
set -euo pipefail
cd "$(dirname "$0")/.."
chmod +x .githooks/pre-commit .githooks/pre-push
git config core.hooksPath .githooks
echo "core.hooksPath=$(git config core.hooksPath)"
echo "Installed git hooks (security gate: .githooks/_security.sh + security.conf; see docs/GITHOOKS.md):"
echo "  pre-commit  — secrets, blocked files, protected paths, cargo fmt"
echo "  pre-push    — main/force-push guard, secret scan of every pushed commit"
if ! command -v gitleaks >/dev/null 2>&1; then
  echo "NOTE: gitleaks is not installed; the gate falls back to provider-prefix patterns"
  echo "      only. Install it for entropy-based detection: brew install gitleaks"
fi
