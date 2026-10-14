#!/usr/bin/env bash
# Usage: run.sh <label> <plan.mjs>  — fresh home, fake model, real server, headless console run.
set -euo pipefail
RIG="$(cd "$(dirname "$0")" && pwd)"
REPO=/home/user/CG-agent-harness
LABEL="$1"; PLAN="$2"
OUT="$RIG/out/$LABEL"; rm -rf "$OUT"; mkdir -p "$OUT"
MPORT=${MPORT:-11555}; HPORT=${HPORT:-8799}
python3 "$RIG/make_home.py" "$REPO" "$OUT/home" "$MPORT" >/dev/null
python3 "$RIG/fake_ollama.py" "$MPORT" "$OUT/model-requests.jsonl" & MPID=$!
env -u GROK_API_KEY -u ANTHROPIC_API_KEY -u DEEPAGENT_API_KEY -u SERPAPI_API_KEY \
  CGAGENTHARNESS_HOME="$OUT/home" "$REPO/target/debug/cgagentharness" serve --port "$HPORT" >"$OUT/server.log" 2>&1 & SPID=$!
trap 'kill $SPID $MPID 2>/dev/null || true' EXIT
for i in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:$HPORT/" -o /dev/null 2>/dev/null && [ -f "$OUT/home/setup-code" ]; then break; fi
  sleep 0.5
done
MODEL_LOG="$OUT/model-requests.jsonl" node "$RIG/driver.mjs" "http://127.0.0.1:$HPORT" "$OUT/home" "$PLAN" "$OUT/results.json" 2>&1 | tee "$OUT/driver.log"
