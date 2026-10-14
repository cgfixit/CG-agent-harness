#!/usr/bin/env bash
set -euo pipefail
RIG="$(cd "$(dirname "$0")" && pwd)"; REPO=/home/user/CG-agent-harness
OUT="$RIG/out/demo"; rm -rf "$OUT"; mkdir -p "$OUT/video"
MPORT=11556; HPORT=8801
python3 "$RIG/make_home.py" "$REPO" "$OUT/home" "$MPORT" >/dev/null
python3 "$RIG/fake_ollama.py" "$MPORT" "$OUT/model-requests.jsonl" & MPID=$!
env -u GROK_API_KEY -u ANTHROPIC_API_KEY -u DEEPAGENT_API_KEY -u SERPAPI_API_KEY \
  CGAGENTHARNESS_HOME="$OUT/home" "$REPO/target/debug/cgagentharness" serve --port "$HPORT" >"$OUT/server.log" 2>&1 & SPID=$!
trap 'kill $SPID $MPID 2>/dev/null || true' EXIT
for i in $(seq 1 120); do curl -fsS "http://127.0.0.1:$HPORT/" -o /dev/null 2>/dev/null && [ -f "$OUT/home/setup-code" ] && break; sleep 0.5; done
test ! -e "$OUT/home/tools/web_allowlist.json" && echo "allowlist: none (fresh home)"
node "$RIG/demo.mjs" "http://127.0.0.1:$HPORT" "$OUT/home" "$OUT/video" "$OUT/demo-log.json"
