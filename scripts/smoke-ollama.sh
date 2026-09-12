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
export CGAGENTHARNESS_API_KEY=""
export CGAGENTHARNESS_HARNESS_PORT="$PORT"
echo "== home $CGAGENTHARNESS_HOME"

echo "== bind guard"
if "$BIN" serve --host 0.0.0.0 --port "$PORT"; then echo "non-loopback bind must be refused"; exit 1; fi

# The refused bind seeded the fresh configuration. Set the explicitly selected
# local endpoint before server startup, without requiring a YAML dependency.
python3 - "$CGAGENTHARNESS_HOME/config.yaml" "$MODEL_URL" <<'PYMODEL'
import json,sys
from pathlib import Path
from urllib.parse import urlsplit
url=sys.argv[2].rstrip('/')
assert urlsplit(url).hostname in ('127.0.0.1','localhost','::1')
p=Path(sys.argv[1]);p.write_text(p.read_text().replace('base_url: "http://127.0.0.1:11434/v1"', 'base_url: '+json.dumps(url+'/v1'), 1))
PYMODEL
"$BIN" serve >"$CGAGENTHARNESS_HOME/serve.log" 2>&1 &
PID=$!
cleanup() {
  kill "$PID" 2>/dev/null || true
  wait "$PID" 2>/dev/null || true
  rm -rf -- "$CGAGENTHARNESS_HOME"
}
trap cleanup EXIT
BASE="https://127.0.0.1:$PORT"
CERT="$CGAGENTHARNESS_HOME/public-certificate.pem"
for _ in $(seq 1 50); do
  if "$BIN" tls certificate >"$CERT" 2>/dev/null; then break; fi
  sleep 0.1
 done
curl_local() { curl --noproxy '*' --cacert "$CERT" "$@"; }
for _ in $(seq 1 50); do curl_local -fsS "$BASE/api/status" >/dev/null 2>&1 && break; sleep 0.1; done

echo "== status"; curl_local -fsS "$BASE/api/status" | tee /dev/stderr | grep -q '"version"'; echo
echo "== headers"; curl_local -sSI "$BASE/api/status" | grep -qi 'x-content-type-options: nosniff'
curl_local -sSI "$BASE/api/status" | grep -qi "content-security-policy: default-src 'none'"
CSRF=$(curl_local -fsS "$BASE/" | sed -n 's/.*name="csrf-token" content="\([^"]*\)".*/\1/p' | head -1)
[ -n "$CSRF" ] || { echo "console did not embed a CSRF token"; exit 1; }
curl_local -fsS "$BASE/" | grep -q '__CYCLAW_CSP_NONCE__' && { echo "nonce placeholder leaked"; exit 1; }

echo "== negative controls"
test "$(curl_local -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/chat" -H 'Content-Type: application/json' -d '{"message":"x"}')" = 401
test "$(curl_local -s -o /dev/null -w '%{http_code}' "$BASE/api/status" -H 'Host: evil.example')" = 400
test "$(curl_local -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/soul" -H "X-CyClaw-CSRF: $CSRF" -H 'Origin: http://evil.example' -H 'Content-Type: application/json' -d '{"enabled":true}')" = 403

echo "== account login and forced password replacement"
printf '%s\n' admin | "$BIN" account --url "$BASE" login admin >/dev/null
printf '%s\n' admin smoke-fixture-password | "$BIN" account --url "$BASE" password >/dev/null
COOKIE_FILE="$CGAGENTHARNESS_HOME/smoke.cookies"
python3 - "$CGAGENTHARNESS_HOME/cli-session.json" "$COOKIE_FILE" <<'PYCOOKIE'
import json,os,sys
session=json.load(open(sys.argv[1]))['cookie'].split('=',1)[1]
with open(sys.argv[2],'w',opener=lambda path,flags:os.open(path,flags,0o600)) as output:
    output.write('# Netscape HTTP Cookie File\n127.0.0.1\tFALSE\t/\tTRUE\t0\tcgagentharness_session\t'+session+'\n')
PYCOOKIE
curl_account() { curl_local --cookie "$COOKIE_FILE" "$@"; }
test "$(curl_account -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/chat" -H 'Content-Type: application/json' -d '{"message":"x"}')" = 403

echo "== subprocess contract (real child)"
curl_account -fsS "$BASE/api/github/status" -H "X-CyClaw-CSRF: $CSRF" | tee /dev/stderr | grep -q '"label":"ok"'; echo
set +e
"$BIN" agentic --config /nonexistent.yaml status >/dev/null 2>&1; code=$?
set -e
[ "$code" -eq 3 ] || { echo "expected exit 3, got $code"; exit 1; }
"$BIN" agentic --config "$CGAGENTHARNESS_HOME/config.yaml" status | grep -q "Agentic layer disabled"

echo "== open panels"
curl_account -fsS "$BASE/api/tools" | grep -q '"wired":38'
curl_account -fsS "$BASE/api/skills" | grep -q 'ponytail'
curl_account -fsS "$BASE/api/agent/checks" | grep -q 'cargo-test'

echo "== live model turn ($MODEL_URL)"
if ! curl -fsS -m 3 "$MODEL_URL/v1/models" >/dev/null 2>&1; then
  echo "model server not reachable at $MODEL_URL; skipping the live chat turn"
  exit 0
fi
MODEL="${SMOKE_MODEL:-$(curl -fsS "$MODEL_URL/v1/models" | python3 -c 'import json,sys;print(json.load(sys.stdin)["data"][0]["id"])')}"
echo "== model $MODEL"
curl_account -fsS -X POST "$BASE/api/model" -H "X-CyClaw-CSRF: $CSRF" -H 'Content-Type: application/json' -d "{\"model\":\"$MODEL\"}" >/dev/null
REPLY=$(curl_account -fsS -m 600 -X POST "$BASE/api/chat" -H "X-CyClaw-CSRF: $CSRF" -H 'Content-Type: application/json' -d '{"message":"Reply with the single word pong."}')
echo "$REPLY" | tee /dev/stderr | grep -q '"reply"'; echo
echo "$REPLY" | python3 -c 'import json,sys;d=json.load(sys.stdin);assert d["usage"]["completion_tokens"]>=0;assert d["reply"].strip().lower().rstrip(".!")=="pong",d["reply"];print("model:",d["model"],"tokens:",d["tally"]["total"])'
echo "== OK"
