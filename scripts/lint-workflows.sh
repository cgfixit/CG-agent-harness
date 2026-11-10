#!/usr/bin/env bash
# Runs workflow-lint.yml's two linters locally, so a workflow edit is linted
# before it is pushed instead of being reported as "not installed here".
# Each tool comes from PATH, else a pinned fallback that needs no root:
#   actionlint  the container image pinned in workflow-lint.yml, else `go run`
#   zizmor      `uvx`, else `pipx run`
# zizmor's online audits need a token and run only in CI; a green run here is
# evidence for the offline audits alone.
#
# Usage: scripts/lint-workflows.sh
# Exit: 0 clean, 1 findings, 3 a linter could not be obtained.
set -euo pipefail
cd "$(dirname "$0")/.."

# One source for the pin: the image reference CI runs.
image="$(grep -oE 'rhysd/actionlint:[0-9.]+@sha256:[0-9a-f]{64}' .github/workflows/workflow-lint.yml | head -n 1)"
[[ -n "$image" ]] || { echo "lint-workflows: no actionlint pin in workflow-lint.yml" >&2; exit 3; }
actionlint_version="${image#rhysd/actionlint:}"; actionlint_version="${actionlint_version%%@*}"
# The release zizmorcore/zizmor-action hash-locks; move it with that pin.
zizmor_version="${ZIZMOR_VERSION:-1.31.1}"

rc=0
echo "== actionlint ${actionlint_version}"
if command -v actionlint >/dev/null 2>&1; then
  actionlint -color || rc=1
elif command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  docker run --rm -v "${PWD}:/repo:ro" -w /repo "$image" -color || rc=1
elif command -v go >/dev/null 2>&1; then
  go run "github.com/rhysd/actionlint/cmd/actionlint@v${actionlint_version}" -color || rc=1
else
  echo "lint-workflows: actionlint, a running docker and go are all missing" >&2; exit 3
fi

echo "== zizmor ${zizmor_version} (offline audits)"
if command -v zizmor >/dev/null 2>&1; then
  zizmor --offline . || rc=1
elif command -v uvx >/dev/null 2>&1; then
  uvx "zizmor==${zizmor_version}" --offline . || rc=1
elif command -v pipx >/dev/null 2>&1; then
  pipx run "zizmor==${zizmor_version}" --offline . || rc=1
else
  echo "lint-workflows: zizmor, uvx and pipx are all missing" >&2; exit 3
fi
exit "$rc"
