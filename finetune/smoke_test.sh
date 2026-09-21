#!/usr/bin/env bash
# CG-Agent MLX smoke test — confirm a fused-MLX-style OpenAI-compatible loopback
# endpoint works with NO Rust changes, before you train.
#
# What it checks: CG-Agent's resolver selects the mock as primary, state.chat calls
# it, and the coding planner (LocalProposerClient) hits /chat/completions with the
# configured model.
#
# Requires: Python 3 (stdlib only), cargo + a Rust toolchain, the harness built.
# Run from the repo root on the Mac:
#   bash finetune/smoke_test.sh
set -euo pipefail

PORT="${CGAGENT_MOCK_PORT:-1235}"
MODEL="${CGAGENT_MOCK_MODEL:-cgagent-fused}"
MOCK_LOG="$(mktemp)"
CONFIG_DIR="$(mktemp -d)"
trap 'kill "${MOCK_PID:-}" 2>/dev/null || true; rm -rf "$CONFIG_DIR" "$MOCK_LOG"' EXIT

echo "==> 1/4  start mock OpenAI server on 127.0.0.1:${PORT} (model=${MODEL})"  # DevSkim: ignore DS162092 because this smoke binds the mock to loopback only.
python3 finetune/mock_server.py --port "$PORT" --model "$MODEL" >"$MOCK_LOG" 2>&1 &
MOCK_PID=$!
sleep 1
curl -sf "http://127.0.0.1:${PORT}/v1/models" | head -c 200; echo  # DevSkim: ignore DS162092 because this smoke probes the loopback mock only.

echo "==> 2/4  write a config pointing BOTH model paths at the mock (MLX as primary)"
cat > "$CONFIG_DIR/config.yaml" <<YAML
server:
  bind: "127.0.0.1:8790"  # DevSkim: ignore DS162092 because this smoke config binds the harness to loopback only.
models:
  local_llm:
    provider: "lmstudio"
    base_url: "http://127.0.0.1:${PORT}/v1"  # DevSkim: ignore DS162092 because this smoke points chat at the loopback mock.
    model: "${MODEL}"
    reasoning_effort: "none"
  cloud_chat:
    enabled: false
agentic:
  deepagent_github:
    provider: "openai_compatible"
    base_url: "http://127.0.0.1:${PORT}/v1"  # DevSkim: ignore DS162092 because this smoke points the planner at the loopback mock.
    model: "${MODEL}"
    allow_cloud_providers: false
YAML

echo "==> 3/4  build the harness (cargo build --release)"
cargo build --release

echo "==> 4/4  boot the console against the mock and send one chat"
# Start the server in the background, send a chat, then shut it down.
BIN="./target/release/cgagentharness"
timeout 20 "$BIN" --config "$CONFIG_DIR/config.yaml" serve &
SRV_PID=$!
sleep 3
# Send a trivial chat turn to the console's chat endpoint (adjust route per build).
curl -sf -X POST "http://127.0.0.1:8790/api/chat" -H "Content-Type: application/json" -d '{"message":"ping"}' || echo "(chat route may differ by build — check server logs)"  # DevSkim: ignore DS162092 because this smoke chats with the loopback console only.
sleep 1
kill "$SRV_PID" 2>/dev/null || true

echo "==> mock server log (look for GET /v1/models + POST /v1/chat/completions):"
cat "$MOCK_LOG"
echo
echo "If the mock logged both a GET /v1/models (inventory probe) and a POST"
echo "/v1/chat/completions (chat + planner), the OpenAI-compatible loopback path"
echo "works with NO Rust changes. If the resolver or planner failed, that is the"
echo "exact gap to fix (most likely the proposer.rs provider label, cosmetic)."
