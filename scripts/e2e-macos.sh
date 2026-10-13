#!/usr/bin/env bash
# Staged end-to-end run for Apple Silicon: the same stages the manual e2e.yml
# workflow runs, so a green local run and a green workflow mean the same thing.
#
# Usage: scripts/e2e-macos.sh [stage ...]       # default: all
#   preflight  host, memory, tools, Chrome, free ports, model server
#   build      cargo build --release --locked
#   contracts  python and node contract scripts, then chat browser acceptance
#   smoke      scripts/smoke-ollama.sh (the live chat turn needs E2E_LIVE=1)
#   browser    browser-fixture.py + browser-acceptance.mjs in real Chrome
#              (clone, plan, patch, verify, approve, push, mock draft PR)
#   desktop    package the .app, verify it, test its sidecar
# Env:
#   E2E_LIVE=1             1 runs smoke with a required model and the browser
#                          stage; 0 skips the model-backed work (hosted runners)
#   E2E_DESKTOP=1          1 includes the desktop stage in `all`
#   E2E_MODEL              installed Ollama tag (default qwen3.8:27b-mlx-cg)
#   E2E_CREATE=1           ollama create E2E_MODEL from scripts/qwen3.8-27b-mlx-cg.Modelfile
#   E2E_PULL=1             ollama pull the Modelfile's FROM tag first (tens of GB)
#   E2E_MIN_MEM_GB=40      unified-memory floor for a live run (0 disables)
#   MODEL_URL              loopback Ollama (default http://127.0.0.1:11434)
#   CHROME_BIN             Chrome binary (default: the /Applications install)
#   E2E_OUT                log directory (default: $RUNNER_TEMP/e2e-out, else a new temp dir)
# Exit: 0 ok, 2 a stage failed, 3 environment or usage.
# Homes are disposable. The real ~/.CGagentHarness is never read or written.
set -euo pipefail
cd "$(dirname "$0")/.."

E2E_LIVE="${E2E_LIVE:-1}"
E2E_DESKTOP="${E2E_DESKTOP:-1}"
E2E_MODEL="${E2E_MODEL:-qwen3.8:27b-mlx-cg}"
E2E_MIN_MEM_GB="${E2E_MIN_MEM_GB:-40}"
MODEL_URL="${MODEL_URL:-http://127.0.0.1:11434}" # DevSkim: ignore DS162092 because this e2e reaches loopback only.
SMOKE_PORT="${SMOKE_PORT:-8791}"
FIXTURE_PORT="${FIXTURE_PORT:-8792}"
E2E_OUT="${E2E_OUT:-${RUNNER_TEMP:+$RUNNER_TEMP/e2e-out}}"
E2E_OUT="${E2E_OUT:-$(mktemp -d)}"
CHROME_BIN="${CHROME_BIN:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
BASE_TAG="$(sed -n 's/^FROM //p' scripts/qwen3.8-27b-mlx-cg.Modelfile | head -1)"
BIN="target/release/cgagentharness"

# Ambient state must not leak in: stages build their own homes and keys.
unset CGAGENTHARNESS_HOME CGAGENTHARNESS_API_KEY CGAGENTHARNESS_HARNESS_PORT
mkdir -p "$E2E_OUT"

die() { echo "e2e: $1" >&2; exit "${2:-2}"; }
env_die() { die "$1" 3; }
port_open() { (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; } # DevSkim: ignore DS162092 because this e2e reaches loopback only.
blank_keys() { env GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" CGAGENTHARNESS_AGENTIC_WRITE_DISABLE="" "$@"; }

ensure_model() {
  curl -fsS -m 5 "$MODEL_URL/api/tags" >"$E2E_OUT/tags.json" 2>/dev/null \
    || env_die "no model server at $MODEL_URL; start Ollama (loopback only)"
  has_tag() { python3 -c 'import json,sys;sys.exit(0 if sys.argv[1] in {m["name"] for m in json.load(open(sys.argv[2]))["models"]} else 1)' "$1" "$E2E_OUT/tags.json"; }
  has_tag "$E2E_MODEL" && return 0
  if [[ "$E2E_MODEL" == qwen3.8:27b-mlx-cg && "${E2E_CREATE:-0}" == 1 ]]; then
    if ! has_tag "$BASE_TAG"; then
      [[ "${E2E_PULL:-0}" == 1 ]] || env_die "$BASE_TAG is not installed; run: ollama pull $BASE_TAG (or set E2E_PULL=1)"
      ollama pull "$BASE_TAG"
    fi
    ollama create "$E2E_MODEL" -f scripts/qwen3.8-27b-mlx-cg.Modelfile
    curl -fsS -m 5 "$MODEL_URL/api/tags" >"$E2E_OUT/tags.json"
    has_tag "$E2E_MODEL" && return 0
  fi
  env_die "model $E2E_MODEL is not installed; run: ollama pull $BASE_TAG && ollama create $E2E_MODEL -f scripts/qwen3.8-27b-mlx-cg.Modelfile (or set E2E_CREATE=1)"
}

stage_preflight() {
  [[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || env_die "requires an Apple Silicon Mac (got $(uname -s) $(uname -m))"
  local tool
  for tool in cargo rustc git curl python3 node; do
    command -v "$tool" >/dev/null 2>&1 || env_die "missing tool: $tool"
  done
  node --experimental-websocket -e 'if (typeof WebSocket !== "function") process.exit(1)' \
    || env_die "node $(node --version) has no WebSocket; Chrome acceptance needs Node 20.10 or newer"
  [[ -x "$CHROME_BIN" ]] || env_die "Chrome not found at $CHROME_BIN (set CHROME_BIN)"
  echo "host: $(sysctl -n machdep.cpu.brand_string), $(( $(sysctl -n hw.memsize) / 1073741824 )) GB unified memory, $(df -g . | awk 'NR==2 {print $4}') GB free"
  echo "chrome: $("$CHROME_BIN" --version)"
  local port
  for port in "$SMOKE_PORT" "$FIXTURE_PORT"; do
    ! port_open "$port" || env_die "127.0.0.1:$port is in use; set SMOKE_PORT/FIXTURE_PORT" # DevSkim: ignore DS162092 because this e2e reaches loopback only.
  done
  if [[ "$E2E_LIVE" == 1 ]]; then
    local gb=$(( $(sysctl -n hw.memsize) / 1073741824 ))
    (( gb >= E2E_MIN_MEM_GB )) || env_die "$gb GB unified memory is below E2E_MIN_MEM_GB=$E2E_MIN_MEM_GB"
    case "$MODEL_URL" in http://127.0.0.1:* | http://localhost:*) ;; *) env_die "MODEL_URL must be loopback" ;; esac # DevSkim: ignore DS162092 because this e2e reaches loopback only.
    ensure_model
    echo "model: $E2E_MODEL at $MODEL_URL"
  fi
  if [[ "$E2E_DESKTOP" == 1 ]]; then
    local toolchain
    for toolchain in 1.88 1.90; do
      rustup toolchain list | grep -q "^$toolchain" || env_die "rust $toolchain missing for the desktop stage: rustup toolchain install $toolchain"
    done
  fi
  echo "preflight ok; logs in $E2E_OUT"
}

stage_build() { cargo build --release --locked; }

stage_contracts() {
  [[ -x "$BIN" ]] || env_die "build first: $0 build"
  python3 scripts/test-grok-acp-probe.py
  python3 scripts/test-prepare-cargo.py
  python3 scripts/test-release-plan.py
  blank_keys env CGAH_TEST_BINARY="$BIN" python3 scripts/test-desktop-backend.py
  local t
  for t in scripts/test-*.mjs; do echo "== $t"; node "$t"; done
  CHROME_BIN="$CHROME_BIN" node --experimental-websocket scripts/chat-browser-acceptance.mjs
}

stage_smoke() {
  [[ -x "$BIN" ]] || env_die "build first: $0 build"
  if [[ "$E2E_LIVE" == 1 ]]; then
    blank_keys env PORT="$SMOKE_PORT" MODEL_URL="$MODEL_URL" SMOKE_REQUIRE_MODEL=1 SMOKE_MODEL="$E2E_MODEL" scripts/smoke-ollama.sh
  else
    # Without a model server the script stops after every non-model check.
    blank_keys env PORT="$SMOKE_PORT" MODEL_URL="http://127.0.0.1:9" scripts/smoke-ollama.sh # DevSkim: ignore DS162092 because this e2e reaches loopback only.
  fi
}

stage_browser() {
  [[ "$E2E_LIVE" == 1 ]] || env_die "the browser stage plans with a real model; set E2E_LIVE=1"
  [[ -x "$BIN" ]] || env_die "build first: $0 build"
  set -m # own process group, so cleanup also reaches the serve child of the fixture
  local fixture="$E2E_OUT/fixture" info="$E2E_OUT/fixture.json" pid status=0
  python3 scripts/browser-fixture.py "$fixture" --model "$E2E_MODEL" --endpoint "$MODEL_URL/v1" \
    --port "$FIXTURE_PORT" --http >"$info" 2>"$E2E_OUT/fixture.err" &
  pid=$!
  # shellcheck disable=SC2064
  trap "kill -- -$pid 2>/dev/null || true; wait $pid 2>/dev/null || true; rm -rf -- '$fixture'" EXIT
  local _
  for _ in $(seq 1 600); do
    port_open "$FIXTURE_PORT" && break
    kill -0 "$pid" 2>/dev/null || { cat "$E2E_OUT/fixture.err" >&2; die "fixture exited before serving"; }
    sleep 0.2
  done
  port_open "$FIXTURE_PORT" || die "fixture did not open port $FIXTURE_PORT"
  local home
  home="$(python3 -c 'import json,sys;print(json.loads(open(sys.argv[1]).readline())["home"])' "$info")"
  local fixture_url="http://127.0.0.1:$FIXTURE_PORT" # DevSkim: ignore DS162092 because this e2e reaches loopback only.
  CGAH_TEST_BASE_URL="$fixture_url" CGAH_TEST_HOME="$home" CGAH_TEST_FIXTURE=disposable-arithmetic \
    CHROME_BIN="$CHROME_BIN" node --experimental-websocket scripts/browser-acceptance.mjs || status=$?
  return "$status"
}

stage_desktop() {
  [[ "$(uname -s)" == Darwin ]] || env_die "the desktop stage packages a macOS .app"
  scripts/package-desktop.sh
  local app="dist/CG Agent Harness.app" check
  check="$(mktemp -d "$E2E_OUT/desktop-verify.XXXXXX")"
  (cd dist && shasum -a 256 -c SHA256SUMS)
  ditto -x -k dist/CG-Agent-Harness-macos-arm64.zip "$check"
  scripts/verify-desktop-bundle.sh "$check/CG Agent Harness.app" arm64
  (cd desktop && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked)
  blank_keys env CGAH_TEST_BINARY="$app/Contents/MacOS/cgagentharness" python3 scripts/test-desktop-backend.py
}

# Each stage runs in its own subshell with errexit on. Testing a function's
# status (`f || ...`) would switch errexit off inside it, so a failed command
# mid-stage would be ignored; capture the status around a plain call instead.
run_stage() {
  echo; echo "== stage: $1"
  local started=$SECONDS rc=0
  set +e
  ( set -e; "stage_$1" )
  rc=$?
  set -e
  if [[ "$rc" -ne 0 ]]; then
    [[ "$rc" -eq 3 ]] || rc=2 # keep 3 (environment); any other failure is 2
    die "stage $1 failed (exit $rc) after $((SECONDS - started))s" "$rc"
  fi
  echo "== stage $1 ok ($((SECONDS - started))s)"
}

stages=("$@")
if [[ ${#stages[@]} -eq 0 || "${stages[0]}" == all ]]; then
  stages=(preflight build contracts smoke)
  if [[ "$E2E_LIVE" == 1 ]]; then stages+=(browser); fi
  if [[ "$E2E_DESKTOP" == 1 ]]; then stages+=(desktop); fi
fi
for s in "${stages[@]}"; do
  case "$s" in preflight | build | contracts | smoke | browser | desktop) ;; *) env_die "unknown stage: $s" ;; esac
done
for s in "${stages[@]}"; do run_stage "$s"; done
echo; echo "e2e: ${stages[*]} passed"
