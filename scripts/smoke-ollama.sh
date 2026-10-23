#!/usr/bin/env bash
# Live smoke of the release binary: bind guard, hardening headers, a real chat
# turn against the local model server (Ollama on 127.0.0.1:11434 by default),
# negative auth controls, the subprocess exit-code contract, and the
# /api/github/status route crossing the shim into a real child process.
# SMOKE_MODEL is any installed tag (default: the first /v1/models id), including
# a local derived tag such as qwen3.8:27b-mlx-cg. The first chat must answer
# exactly "pong". The server sends reasoning_effort none when the provider is
# ollama, for every tag. SMOKE_EXACT_REPLY=0 (live-ollama.yml's tiny fixture
# models) instead requires a non-empty reply from that model (fingerprint.py's
# chat check) plus completion tokens and no <think> text. FINGERPRINT_MODELS,
# when set, is the fingerprint set (scripts/live-ollama/), not that derived tag.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="${BIN:-target/release/cgagentharness}"
PORT="${PORT:-8791}"
MODEL_URL="${MODEL_URL:-http://127.0.0.1:11434}"
[ -x "$BIN" ] || { echo "build first: cargo build --release"; exit 1; }

CGAGENTHARNESS_HOME="$(mktemp -d)"
export CGAGENTHARNESS_HOME
trap 'rm -rf -- "$CGAGENTHARNESS_HOME"' EXIT
export CGAGENTHARNESS_API_KEY=""
export CGAGENTHARNESS_HARNESS_PORT="$PORT"
echo "== home $CGAGENTHARNESS_HOME"

echo "== bind guard"
# Watchdog, not a bare `if`: if the loopback guard ever regresses the server
# actually starts and this assertion would hang forever instead of failing.
"$BIN" serve --host 0.0.0.0 --port "$PORT" & probe=$!
( sleep 20; kill -9 "$probe" 2>/dev/null ) & watchdog=$!
bind_code=0; wait "$probe" || bind_code=$?
kill "$watchdog" 2>/dev/null || true
case "$bind_code" in
  0)   echo "non-loopback bind must be refused, but serve exited 0"; exit 1 ;;
  137) echo "non-loopback bind must be refused, but serve kept listening"; exit 1 ;;
esac

# The refused bind seeded the fresh configuration. Set the explicitly selected
# local endpoint before server startup, without requiring a YAML dependency.
# Temperature 0 makes the "pong" turn greedy: at the former 0.2 a 0.5B model
# sometimes sampled "péng!", so the check failed by chance, not by regression.
python3 - "$CGAGENTHARNESS_HOME/config.yaml" "$MODEL_URL" <<'PYMODEL'
import json,sys
from pathlib import Path
from urllib.parse import urlsplit
url=sys.argv[2].rstrip('/')
assert urlsplit(url).hostname in ('127.0.0.1','localhost','::1')  # DevSkim: ignore DS162092 because this refuses any non-loopback model URL.
p=Path(sys.argv[1]);text=p.read_text()
for old,new in (('base_url: "http://127.0.0.1:11434/v1"','base_url: '+json.dumps(url+'/v1')),('    temperature: 0.0\n','    temperature: 0\n')):  # DevSkim: ignore DS162092,DS137138 because these are the seeded loopback Ollama URL and the operator's loopback URL.
    # The first match is models.local_llm (agentic.deepagent_github repeats base_url later).
    assert old in text,'seeded config changed; update smoke-ollama.sh: '+old.strip()
    text=text.replace(old,new,1)
p.write_text(text)
PYMODEL
"$BIN" serve >"$CGAGENTHARNESS_HOME/serve.log" 2>&1 &
PID=$!
cleanup() {
  local status=$?
  kill "$PID" 2>/dev/null || true
  wait "$PID" 2>/dev/null || true
  if [[ "$status" -ne 0 && -f "$CGAGENTHARNESS_HOME/serve.log" ]]; then
    echo "== harness serve.log (last 200 lines)" >&2 || true
    tail -n 200 "$CGAGENTHARNESS_HOME/serve.log" >&2 || true
  fi
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
# Capture, then match. In `producer | grep -q` grep exits at its first match, and
# under pipefail the producer's SIGPIPE (curl 23, tee or echo 141) fails a passing
# check, or hides a failing one behind `&&`.
has() { grep -q -- "$1" <<<"$2"; }
for _ in $(seq 1 50); do curl_local -fsS "$BASE/api/status" >/dev/null 2>&1 && break; sleep 0.1; done

echo "== status"; STATUS=$(curl_local -fsS "$BASE/api/status"); printf '%s' "$STATUS" >&2; has '"version"' "$STATUS"; echo
echo "== headers"; HEADERS=$(curl_local -sSI "$BASE/api/status")
grep -qi 'x-content-type-options: nosniff' <<<"$HEADERS"
grep -qi "content-security-policy: default-src 'none'" <<<"$HEADERS"
PAGE=$(curl_local -fsS "$BASE/")
csrf_re='name="csrf-token" content="([^"]*)"'
CSRF=""; if [[ $PAGE =~ $csrf_re ]]; then CSRF=${BASH_REMATCH[1]}; fi
[ -n "$CSRF" ] || { echo "console did not embed a CSRF token"; exit 1; }
if has '__CYCLAW_CSP_NONCE__' "$PAGE"; then echo "nonce placeholder leaked"; exit 1; fi

echo "== negative controls"
test "$(curl_local -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/chat" -H 'Content-Type: application/json' -d '{"message":"x"}')" = 401
test "$(curl_local -s -o /dev/null -w '%{http_code}' "$BASE/api/status" -H 'Host: evil.example')" = 400
test "$(curl_local -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/soul" -H "X-CyClaw-CSRF: $CSRF" -H 'Origin: http://evil.example' -H 'Content-Type: application/json' -d '{"enabled":true}')" = 403

echo "== account login and forced password replacement"
# A fresh home has no shipped password: admin's first credential is its one-time setup code.
SETUP_CODE=$(cat "$CGAGENTHARNESS_HOME/setup-code")
printf '%s\n' "$SETUP_CODE" | "$BIN" account --url "$BASE" login admin >/dev/null
printf '%s\n' "$SETUP_CODE" smoke-fixture-password | "$BIN" account --url "$BASE" password >/dev/null
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
GITHUB_STATUS=$(curl_account -fsS "$BASE/api/github/status" -H "X-CyClaw-CSRF: $CSRF")
printf '%s' "$GITHUB_STATUS" >&2; has '"label":"ok"' "$GITHUB_STATUS"; echo
set +e
"$BIN" agentic --config /nonexistent.yaml status >/dev/null 2>&1; code=$?
set -e
[ "$code" -eq 3 ] || { echo "expected exit 3, got $code"; exit 1; }
AGENTIC_STATUS=$("$BIN" agentic --config "$CGAGENTHARNESS_HOME/config.yaml" status)
has "Agentic layer disabled" "$AGENTIC_STATUS"

echo "== open panels"
TOOLS=$(curl_account -fsS "$BASE/api/tools"); has '"wired":75[,}]' "$TOOLS"
SKILLS=$(curl_account -fsS "$BASE/api/skills"); has 'ponytail' "$SKILLS"
CHECKS=$(curl_account -fsS "$BASE/api/agent/checks"); has 'cargo-test' "$CHECKS"

echo "== live model turn ($MODEL_URL)"
if ! curl -fsS -m 3 "$MODEL_URL/v1/models" >/dev/null 2>&1; then
  if [[ "${SMOKE_REQUIRE_MODEL:-0}" == "1" ]]; then
    echo "model server not reachable at $MODEL_URL; every other stage passed, but the live chat turn is required (run scripts/smoke-ollama.sh without SMOKE_REQUIRE_MODEL to skip only this turn; SKIP_LIVE=1 scripts/verify-local.sh skips the whole smoke)" >&2
    exit 1
  fi
  echo "model server not reachable at $MODEL_URL; skipping the live chat turn"
  exit 0
fi
MODEL="${SMOKE_MODEL:-$(curl -fsS "$MODEL_URL/v1/models" | python3 -c 'import json,sys;print(json.load(sys.stdin)["data"][0]["id"])')}"
echo "== model $MODEL"
curl_account -fsS -X POST "$BASE/api/model" -H "X-CyClaw-CSRF: $CSRF" -H 'Content-Type: application/json' -d "{\"model\":\"$MODEL\"}" >/dev/null
# With auto_tune on, /model use starts a speed sample that holds the model;
# a chat meanwhile answers 409 CHAT_BUSY. Retry only that answer, bounded.
for attempt in $(seq 1 300); do
  REPLY=$(curl_account -sS -m 600 -X POST "$BASE/api/chat" -H "X-CyClaw-CSRF: $CSRF" -H 'Content-Type: application/json' -d '{"message":"Reply with the single word pong."}')
  has '"code":"CHAT_BUSY"' "$REPLY" || break
  [ "$attempt" -eq 1 ] && echo "model busy (auto_tune sample); retrying"
  sleep 2
done
printf '%s' "$REPLY" >&2; has '"reply"' "$REPLY"; echo
echo "$REPLY" | python3 -c '
import json,sys
d=json.load(sys.stdin);reply=d["reply"];model=sys.argv[1]
if sys.argv[2]=="0":
    # Greedy decoding still differs by backend: Metal says "péng!" where CPU says "pong".
    assert reply.strip() and "<think" not in reply.lower() and d["model"]==model and d["usage"]["completion_tokens"]>0,d
else:
    assert d["usage"]["completion_tokens"]>=0;assert reply.strip().lower().rstrip(".!")=="pong",reply
print("model:",d["model"],"tokens:",d["tally"]["total"])' "$MODEL" "${SMOKE_EXACT_REPLY:-1}"
if [[ -n "${FINGERPRINT_OUT:-}" ]]; then
  echo "== fingerprint (${FINGERPRINT_MODELS:-$MODEL})"
  python3 scripts/live-ollama/fingerprint.py --base "$BASE" --cacert "$CERT" \
    --session-file "$CGAGENTHARNESS_HOME/cli-session.json" --csrf="$CSRF" \
    --model-url "$MODEL_URL" --models "${FINGERPRINT_MODELS:-$MODEL}" --out "$FINGERPRINT_OUT"
fi
echo "== OK"
