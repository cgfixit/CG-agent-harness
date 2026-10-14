#!/usr/bin/env bash
# Smoother demo: real serve + stand-in model, headed Chromium on Xvfb, ffmpeg x11grab at 30 fps.
set -euo pipefail
RIG="$(cd "$(dirname "$0")" && pwd)"; REPO=/home/user/CG-agent-harness
OUT="$RIG/out"; rm -rf "$OUT"; mkdir -p "$OUT"
MPORT=11557; HPORT=8802; DISP=:99; W=1440; H=900
python3 "$RIG/make_home.py" "$REPO" "$OUT/home" "$MPORT" >/dev/null
python3 "$RIG/fake_ollama.py" "$MPORT" "$OUT/model-requests.jsonl" & MPID=$!
env -u GROK_API_KEY -u ANTHROPIC_API_KEY -u DEEPAGENT_API_KEY -u SERPAPI_API_KEY \
  CGAGENTHARNESS_HOME="$OUT/home" "$REPO/target/debug/cgagentharness" serve --port "$HPORT" >"$OUT/server.log" 2>&1 & SPID=$!
Xvfb "$DISP" -screen 0 "${W}x${H}x24" -nolisten tcp >"$OUT/xvfb.log" 2>&1 & XPID=$!
FPID=""
trap 'kill $SPID $MPID $XPID 2>/dev/null || true; [ -n "$FPID" ] && kill -INT $FPID 2>/dev/null || true' EXIT
for i in $(seq 1 120); do curl -fsS --noproxy '*' "http://127.0.0.1:$HPORT/" -o /dev/null 2>/dev/null && [ -f "$OUT/home/setup-code" ] && break; sleep 0.5; done
test ! -e "$OUT/home/tools/web_allowlist.json" && echo "allowlist: none (fresh home)"
READY="$OUT/ready"
DISPLAY="$DISP" node "$RIG/demo2.mjs" "http://127.0.0.1:$HPORT" "$OUT/home" "$OUT/demo-log.json" "$READY" & NPID=$!
while [ "$(cat "$READY" 2>/dev/null)" != go ]; do kill -0 $NPID 2>/dev/null || { echo "driver exited early" >&2; exit 1; }; sleep 0.1; done
ffmpeg -hide_banner -loglevel error -y -f x11grab -framerate 30 -video_size "${W}x${H}" -draw_mouse 0 -i "$DISP" \
  -c:v libx264 -preset veryfast -crf 21 -pix_fmt yuv420p -movflags +faststart "$OUT/raw.mp4" </dev/null & FPID=$!
sleep 0.5; rm -f "$READY"
while [ "$(cat "$READY" 2>/dev/null)" != stop ]; do kill -0 $NPID 2>/dev/null || break; sleep 0.1; done
kill -INT $FPID; wait $FPID || true; FPID=""
wait $NPID
echo "recorded $OUT/raw.mp4"
