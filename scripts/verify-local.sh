#!/usr/bin/env bash
# Full local verification: fmt, clippy, deny (if installed), tests, the CI
# contract scripts, release build, then a live smoke of the built binary
# against the local model server.
#
# Usage: scripts/verify-local.sh            # everything; an unreachable model server fails
#        SKIP_LIVE=1 scripts/verify-local.sh # static + tests + build only
set -euo pipefail
cd "$(dirname "$0")/.."

CLIPPY="${CLIPPY:-cargo clippy}"

echo "== fmt"; cargo fmt --all -- --check
echo "== clippy"; $CLIPPY --all-targets --all-features --locked -- -D warnings
if command -v cargo-deny >/dev/null 2>&1; then echo "== deny"; cargo deny check; else echo "== deny (skipped: cargo-deny not installed; CI runs it)"; fi
echo "== test"; GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" CGAGENTHARNESS_AGENTIC_WRITE_DISABLE="" cargo test --all-targets --locked
echo "== contract scripts"
python3 scripts/test-grok-acp-probe.py
python3 scripts/test-release-plan.py
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" CGAH_TEST_BINARY=target/debug/cgagentharness python3 scripts/test-desktop-backend.py
if command -v node >/dev/null 2>&1; then
  for t in scripts/test-*.mjs; do echo "== $t"; node "$t"; done
else
  echo "== console contract scripts (skipped: node not installed; CI runs them)"
fi
echo "== release build"; cargo build --release --locked
if [[ "${SKIP_LIVE:-0}" == "1" ]]; then echo "== live smoke skipped"; exit 0; fi
SMOKE_REQUIRE_MODEL=1 exec scripts/smoke-ollama.sh
