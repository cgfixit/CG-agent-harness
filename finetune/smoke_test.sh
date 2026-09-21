#!/usr/bin/env bash
# CG-Agent MLX smoke test — confirm an OpenAI-compatible loopback model endpoint
# works with NO Rust changes, before you train.
#
# What it checks: the harness boots against the stdlib mock, the inventory probe
# (GET /v1/models) and a chat turn (POST /v1/chat/completions) both reach the mock
# with the configured model id. Verified empirically (cargo build + boot + chat).
#
# The harness config is loaded from $CGAGENTHARNESS_HOME/config.yaml (the console's
# home dir), and POST /api/chat is CSRF-guarded — the token is embedded in the
# console HTML at <meta name="csrf-token">. This script fetches it automatically.
#
# Requires: Python 3 (stdlib only for the mock), cargo + a Rust toolchain.
# Run from the repo root:
#   bash finetune/smoke_test.sh
set -uo pipefail
cd "$(dirname "$0")/.."

PORT="${CGAGENT_PORT:-8790}"
MOCK_PORT="${CGAGENT_MOCK_PORT:-1235}"
MODEL="${CGAGENT_MOCK_MODEL:-cgagent-fused}"
HOME_DIR="$(mktemp -d)"
export CGAGENTHARNESS_HOME="$HOME_DIR"
MOCK_LOG="$(mktemp)"; SERVE_LOG="$(mktemp)"; COOKIES="$(mktemp)"
trap 'kill ${SRV_PID:-} ${MOCK_PID:-} 2>/dev/null || true; rm -rf "$HOME_DIR" "$MOCK_LOG" "$SERVE_LOG" "$COOKIES"' EXIT

echo "==> 1/5  start mock OpenAI server on 127.0.0.1:${MOCK_PORT} (model=${MODEL})"
python3 finetune/mock_server.py --port "$MOCK_PORT" --model "$MODEL" >"$MOCK_LOG" 2>&1 &
MOCK_PID=$!
sleep 1
curl -sf "http://127.0.0.1:${MOCK_PORT}/v1/models"; echo

echo "==> 2/5  write config (both model paths at the mock, MLX as primary)"
cat > "$HOME_DIR/config.yaml" <<YAML
server:
  bind: "127.0.0.1:${PORT}"
models:
  local_llm:
    provider: "lmstudio"
    base_url: "http://127.0.0.1:${MOCK_PORT}/v1"
    model: "${MODEL}"
    reasoning_effort: "none"
  cloud_chat:
    enabled: false
agentic:
  deepagent_github:
    provider: "openai_compatible"
    base_url: "http://127.0.0.1:${MOCK_PORT}/v1"
    model: "${MODEL}"
    allow_cloud_providers: false
YAML

echo "==> 3/5  build the harness (cargo build --release)"
cargo build --release

echo "==> 4/5  boot the console + fetch the CSRF token from the console page"
./target/release/cgagentharness serve --port "$PORT" >"$SERVE_LOG" 2>&1 &
SRV_PID=$!
sleep 4
TOKEN=$(curl -s "http://127.0.0.1:${PORT}/" \
  | grep -oE '<meta name="csrf-token" content="[^"]+"' \
  | sed -E 's/.*content="([^"]+)".*/\1/')
if [ -z "$TOKEN" ]; then echo "!! could not extract CSRF token; serve log:"; tail -20 "$SERVE_LOG"; exit 1; fi

echo "==> 5/5  send one chat turn (POST /api/chat with the CSRF token)"
HTTP=$(curl -s -o /tmp/cgagent_chat.json -w "%{http_code}" -X POST "http://127.0.0.1:${PORT}/api/chat" \
  -H 'Content-Type: application/json' -H "x-cyclaw-csrf: $TOKEN" \
  -d '{"message":"ping"}')
echo "chat HTTP ${HTTP}"
head -c 400 /tmp/cgagent_chat.json; echo

echo
echo "==> mock server log (expect GET /v1/models + POST /v1/chat/completions):"
cat "$MOCK_LOG"
echo
echo "If the mock logged GET /v1/models (inventory probe) and POST /v1/chat/completions"
echo "(chat turn) with HTTP 200, the OpenAI-compatible loopback path works with NO"
echo "Rust changes. The mock is a stand-in for mlx_vlm.server — swap it for the real"
echo "server (same base_url/model) once trained."
