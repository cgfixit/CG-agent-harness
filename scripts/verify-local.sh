#!/usr/bin/env bash
# Full local verification: fmt, clippy, deny (if installed), tests, release
# build, then a live smoke of the built binary against the local model server.
#
# Usage: scripts/verify-local.sh            # everything
#        SKIP_LIVE=1 scripts/verify-local.sh # static + tests + build only
set -euo pipefail
cd "$(dirname "$0")/.."

CLIPPY="${CLIPPY:-cargo clippy}"

echo "== fmt"; cargo fmt --all -- --check
echo "== clippy"; $CLIPPY --all-targets --all-features -- -D warnings
if command -v cargo-deny >/dev/null 2>&1; then echo "== deny"; cargo deny check; else echo "== deny (skipped: cargo-deny not installed; CI runs it)"; fi
echo "== test"; GROK_API_KEY="" ANTHROPIC_API_KEY="" cargo test --all-targets
echo "== release build"; cargo build --release
if [[ "${SKIP_LIVE:-0}" == "1" ]]; then echo "== live smoke skipped"; exit 0; fi
exec scripts/smoke-ollama.sh
