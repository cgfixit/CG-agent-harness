#!/usr/bin/env bash
# Live smoke of the release binary: bind guard, hardening headers, a real chat
# turn against the local model server (Ollama on 127.0.0.1:11434 by default),
# negative auth controls, the subprocess exit-code contract, and the
# /api/github/status route crossing the shim into a real child process.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="${BIN:-target/release/cgagentharness}"
PORT="${PORT:-8791}"
MODEL_URL="${MODEL_URL:-http://127.0.0.1:11434}"
[ -x "$BIN" ] || { echo "build first: cargo build --release"; exit 1; }

export CGAGENTHARNESS_HOME="$(mktemp -d)"
export CGAGENTHARNESS_API_KEY="$(openssl rand -hex 16)"
export CGAGENTHARNESS_HARNESS_PORT="$PORT"
echo "== home $CGAGENTHARNESS_HOME"

echo "== bind guard"
if "$BIN" serve --host 0.0.0.0 --port "$PORT"; then echo "non-loopback bind must be refused"; exit 1; fi

"$BIN" serve >"$CGAGENTHARNESS_HOME/serve.log" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
for _ in $(seq 1 50); do curl -fsS "http://127.0.0.1:$PORT/api/status" >/dev/null 2>&1 && break; sleep 0.1; done
BASE="http://127.0.0.1:$PORT"

echo "== status"; curl -fsS "$BASE/api/status" | tee /dev/stderr | grep -q '"version"'; echo
echo "== headers"; curl -sSI "$BASE/api/status" | grep -qi 'x-content-type-options: nosniff'
curl -sSI "$BASE/api/status" | grep -qi "content-security-policy: default-src 'none'"
CSRF=$(curl -fsS "$BASE/" | sed -n 's/.*name="csrf-token" content="\([^"]*\)".*/\1/p' | head -1)
[ -n "$CSRF" ] || { echo "console did not embed a CSRF token"; exit 1; }
curl -fsS "$BASE/" | grep -q '__CYCLAW_CSP_NONCE__' && { echo "nonce placeholder leaked"; exit 1; }

echo "== negative controls"
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/chat" -H 'Content-Type: application/json' -d '{"message":"x"}')" = 401
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/chat" -H "Authorization: Bearer $CGAGENTHARNESS_API_KEY" -H 'Content-Type: application/json' -d '{"message":"x"}')" = 403
test "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/status" -H 'Host: evil.example')" = 400
test "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/soul" -H "Authorization: Bearer $CGAGENTHARNESS_API_KEY" -H "X-CyClaw-CSRF: $CSRF" -H 'Origin: http://evil.example' -H 'Content-Type: application/json' -d '{"enabled":true}')" = 403

echo "== subprocess contract (real child)"
curl -fsS "$BASE/api/github/status" -H "Authorization: Bearer $CGAGENTHARNESS_API_KEY" -H "X-CyClaw-CSRF: $CSRF" | tee /dev/stderr | grep -q '"label":"ok"'; echo
set +e
"$BIN" agentic --config /nonexistent.yaml status >/dev/null 2>&1; code=$?
set -e
[ "$code" -eq 3 ] || { echo "expected exit 3, got $code"; exit 1; }
"$BIN" agentic --config "$CGAGENTHARNESS_HOME/config.yaml" status | grep -q "Agentic layer disabled"

echo "== open panels"
curl -fsS "$BASE/api/tools" | grep -q '"wired":29'
curl -fsS "$BASE/api/skills" | grep -q 'ponytail'
curl -fsS "$BASE/api/agent/checks" | grep -q 'cargo-test'

echo "== live model turn ($MODEL_URL)"
if ! curl -fsS -m 3 "$MODEL_URL/v1/models" >/dev/null 2>&1; then
  echo "model server not reachable at $MODEL_URL; skipping the live chat turn"
  exit 0
fi
MODEL="${SMOKE_MODEL:-$(curl -fsS "$MODEL_URL/v1/models" | python3 -c 'import json,sys;print(json.load(sys.stdin)["data"][0]["id"])')}"
echo "== model $MODEL"
curl -fsS -X POST "$BASE/api/model" -H "Authorization: Bearer $CGAGENTHARNESS_API_KEY" -H "X-CyClaw-CSRF: $CSRF" -H 'Content-Type: application/json' -d "{\"model\":\"$MODEL\"}" >/dev/null
REPLY=$(curl -fsS -m 600 -X POST "$BASE/api/chat" -H "Authorization: Bearer $CGAGENTHARNESS_API_KEY" -H "X-CyClaw-CSRF: $CSRF" -H 'Content-Type: application/json' -d '{"message":"Reply with the single word pong."}')
echo "$REPLY" | tee /dev/stderr | grep -q '"reply"'; echo
echo "$REPLY" | python3 -c 'import json,sys;d=json.load(sys.stdin);assert d["usage"]["completion_tokens"]>=0;print("model:",d["model"],"tokens:",d["tally"]["total"])'
echo "== OK"
