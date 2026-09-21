#!/usr/bin/env bash
# CG-Agent MLX smoke test — confirm a fused-MLX-style OpenAI-compatible loopback
# endpoint works with NO Rust changes, before you train.
#
# What it checks: after an owned-home boot, a console chat can POST
# /v1/chat/completions to the mock. It does NOT treat this script's own mock
# liveness GET as a resolver probe, and it does not exercise /api/agent/run
# (planner coverage is a separate follow-up).
#
# Public CLI: `cgagentharness serve` accepts only --host / --port. Home is
# CGAGENTHARNESS_HOME. There is no global --config on Serve.
#
# Requires: Python 3 (stdlib only), cargo + a Rust toolchain, the harness built.
# Run from the repo root on the Mac:
#   bash finetune/smoke_test.sh
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="${BIN:-target/release/cgagentharness}"
MOCK_PORT="${CGAGENT_MOCK_PORT:-1235}"
HARNESS_PORT="${CGAGENT_HARNESS_PORT:-8790}"
MODEL="${CGAGENT_MOCK_MODEL:-cgagent-fused}"
MOCK_LOG="$(mktemp)"
export CGAGENTHARNESS_HOME="$(mktemp -d)"
export CGAGENTHARNESS_API_KEY=""
export CGAGENTHARNESS_HARNESS_PORT="$HARNESS_PORT"
MOCK_PID=""
SRV_PID=""

cleanup() {
  if [[ -n "${SRV_PID}" ]]; then
    kill "$SRV_PID" 2>/dev/null || true
    wait "$SRV_PID" 2>/dev/null || true
  fi
  if [[ -n "${MOCK_PID}" ]]; then
    kill "$MOCK_PID" 2>/dev/null || true
    wait "$MOCK_PID" 2>/dev/null || true
  fi
  rm -rf -- "$CGAGENTHARNESS_HOME" "$MOCK_LOG"
}
trap cleanup EXIT

echo "== home $CGAGENTHARNESS_HOME"

echo "==> 1/3  start mock OpenAI server on 127.0.0.1:${MOCK_PORT} (model=${MODEL})"  # DevSkim: ignore DS162092 because this smoke binds the mock to loopback only.
python3 finetune/mock_server.py --port "$MOCK_PORT" --model "$MODEL" >"$MOCK_LOG" 2>&1 &
MOCK_PID=$!
sleep 1
echo "==> mock liveness GET (this script's check — not harness resolver evidence)"
curl -sf "http://127.0.0.1:${MOCK_PORT}/v1/models" | head -c 200; echo  # DevSkim: ignore DS162092 because this smoke probes the loopback mock only.

echo "==> 2/3  build the harness if needed, seed a default home, point BOTH model paths at the mock"
if [[ ! -x "$BIN" ]]; then
  cargo build --release
fi
[[ -x "$BIN" ]] || { echo "build failed: $BIN is not executable" >&2; exit 1; }
# Refused non-loopback bind still runs ensure_layout and writes the shipped
# defaults (structured_memory and write gates stay as tip). Then overlay only
# the mock endpoints and disable cloud-provider children with the parent.
set +e
"$BIN" serve --host 0.0.0.0 --port "$HARNESS_PORT" >/dev/null 2>&1
seed_code=$?
set -e
if [[ ! -f "$CGAGENTHARNESS_HOME/config.yaml" ]]; then
  echo "owned home was not seeded (serve seed exit ${seed_code})" >&2
  exit 1
fi
python3 - "$CGAGENTHARNESS_HOME/config.yaml" "http://127.0.0.1:${MOCK_PORT}" "$MODEL" <<'PY'
import json, sys
from pathlib import Path
from urllib.parse import urlsplit

path = Path(sys.argv[1])
url = sys.argv[2].rstrip("/")
model = sys.argv[3]
assert urlsplit(url).hostname in ("127.0.0.1", "localhost", "::1")
text = path.read_text()
text = text.replace(
    '  local_llm:\n    provider: "ollama"',
    '  local_llm:\n    provider: "lmstudio"',
    1,
)
text = text.replace(
    '    provider: "ollama"\n    base_url: "http://127.0.0.1:11434/v1"',
    "    provider: \"openai_compatible\"\n    base_url: " + json.dumps(url + "/v1"),
    1,
)
text = text.replace(
    'base_url: "http://127.0.0.1:11434/v1"',
    "base_url: " + json.dumps(url + "/v1"),
    1,
)
text = text.replace('model: "qwen3.8:27b-mlx"', "model: " + json.dumps(model), 2)
text = text.replace("    allow_cloud_providers: true", "    allow_cloud_providers: false", 1)
text = text.replace(
    """    providers:
      grok:
        enabled: true
        model: "grok-4.5"
      claude:
        enabled: true
        model: "claude-sonnet-5"
""",
    """    providers:
      grok:
        enabled: false
        model: "grok-4.5"
      claude:
        enabled: false
        model: "claude-sonnet-5"
""",
    1,
)
path.write_text(text)
if "allow_cloud_providers: false" not in text:
    raise SystemExit("patch did not disable allow_cloud_providers")
if text.count("provider: \"lmstudio\"") < 1 or text.count("provider: \"openai_compatible\"") < 1:
    raise SystemExit("patch did not repoint both model providers")
PY

echo "==> 3/3  boot the console against the mock and send one chat"
"$BIN" serve --host 127.0.0.1 --port "$HARNESS_PORT" >"$CGAGENTHARNESS_HOME/serve.log" 2>&1 &  # DevSkim: ignore DS162092 because this smoke binds the harness to loopback only.
SRV_PID=$!
BASE="https://127.0.0.1:${HARNESS_PORT}"  # DevSkim: ignore DS162092 because this smoke talks to the loopback console only.
CERT="$CGAGENTHARNESS_HOME/public-certificate.pem"
for _ in $(seq 1 200); do
  if "$BIN" tls certificate >"$CERT" 2>/dev/null; then
    break
  fi
  sleep 0.15
done
curl_local() { curl --noproxy '*' --cacert "$CERT" "$@"; }
booted=0
for _ in $(seq 1 200); do
  if curl_local -fsS "$BASE/api/status" >/dev/null 2>&1; then
    booted=1
    break
  fi
  sleep 0.1
done
if [[ "$booted" -ne 1 ]]; then
  echo "FAIL: harness did not become ready on ${BASE}" >&2
  echo "---- serve.log ----" >&2
  cat "$CGAGENTHARNESS_HOME/serve.log" >&2 || true
  exit 1
fi

CHAT_OK=0
CSRF=$(curl_local -fsS "$BASE/" | sed -n 's/.*name="csrf-token" content="\([^"]*\)".*/\1/p' | head -1)
if [[ -z "$CSRF" ]]; then
  echo "FAIL: chat not attempted — console did not embed a CSRF token"
else
  printf '%s\n' admin | "$BIN" account --url "$BASE" login admin >/dev/null
  printf '%s\n' admin smoke-fixture-password | "$BIN" account --url "$BASE" password >/dev/null
  COOKIE_FILE="$CGAGENTHARNESS_HOME/smoke.cookies"
  python3 - "$CGAGENTHARNESS_HOME/cli-session.json" "$COOKIE_FILE" <<'PYCOOKIE'
import json, os, sys
session = json.load(open(sys.argv[1]))["cookie"].split("=", 1)[1]
with open(sys.argv[2], "w", opener=lambda path, flags: os.open(path, flags, 0o600)) as output:
    output.write(
        "# Netscape HTTP Cookie File\n127.0.0.1\tFALSE\t/\tTRUE\t0\tcgagentharness_session\t"
        + session
        + "\n"
    )
PYCOOKIE
  if REPLY=$(curl_local --cookie "$COOKIE_FILE" -fsS -m 60 -X POST "$BASE/api/chat" \
    -H "X-CyClaw-CSRF: $CSRF" -H "Content-Type: application/json" \
    -d '{"message":"ping"}'); then  # DevSkim: ignore DS162092 because this smoke chats with the loopback console only.
    echo "$REPLY" | python3 -c 'import json,sys; d=json.load(sys.stdin); assert "reply" in d, d; print("chat ok; model=", d.get("model"))'
    CHAT_OK=1
  else
    echo "FAIL: authenticated chat POST to ${BASE}/api/chat did not succeed"
  fi
fi

echo "==> mock server log (pre-boot GET is this script's liveness check only):"
cat "$MOCK_LOG"
echo
POSTS=$(python3 - "$MOCK_LOG" <<'PY'
import sys
from pathlib import Path
text = Path(sys.argv[1]).read_text()
posts = sum(1 for line in text.splitlines() if "POST" in line and "chat/completions" in line)
print(posts)
PY
)

if [[ "$CHAT_OK" -eq 1 ]]; then
  if [[ "$POSTS" -gt 0 ]]; then
    echo "OK: chat succeeded and the mock logged a real POST /v1/chat/completions after harness boot."
    echo "That is the chat-path proof. A GET /v1/models from this script is not resolver evidence"
    echo "(fallback is off, so resolve_local_backend returns primary without probing)."
    echo "This smoke does not cover /api/agent/run / LocalProposerClient."
  else
    echo "FAIL: chat succeeded but the mock logged no POST /v1/chat/completions — the resolver/chat client did not hit the mock." >&2
    exit 1
  fi
else
  echo "FAIL: chat did not succeed. Do not treat the mock log as proof the harness used the mock."
  echo "A GET /v1/models from this script is only mock liveness. A POST /v1/chat/completions"
  echo "after harness boot is required if chat succeeds. Planner /api/agent/run is out of scope."
  exit 1
fi
