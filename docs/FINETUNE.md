# Fine-Tuning CG-Agent with MLX QLoRA

How to train a LoRA adapter for `qwen3.8:27b-mlx` on Apple Silicon and serve the
fused model to CG-Agent — with **no Rust changes**.

This is the reference. For a step-by-step walkthrough, see
[`docs/guides/mlx-qlora-finetune.md`](guides/mlx-qlora-finetune.md). For the vendored
tooling, see [`finetune/`](../finetune/).

## TL;DR

CG-Agent already speaks any OpenAI-compatible loopback model server. So a fine-tuned
model is served by `mlx_lm.server` on `127.0.0.1:1234` and wired into the config — no
new Rust model backend is required. The work is: build a dataset, train, fuse, serve,
and repoint two config blocks.

The checked-in recipe targets `malekoo/Qwen3.8-27B-MLX-4bit`; an installed
Ollama tag does not establish that these are the same weights. Base-model
matching and a successful adapter training/fusion run remain unverified (see
the current [tooling warning](../finetune/README.md)). The mock smoke below
proves chat protocol compatibility only.

## The runtime path (already supported — verified)

Two independent local-model configurations both accept an OpenAI-compatible loopback
endpoint, and the inventory probe already parses the standard `{"data":[{"id":...}]}`
shape that `mlx_lm.server` emits:

- **`models.local_llm.*`** — chat, structured memory, compaction, web research. Provider
  accepts `ollama` | `lmstudio`. Resolved in `src/llm/backend.rs::resolve_local_backend`;
  the `ChatClient` is built from the resolved backend in `src/server/mod.rs`.
- **`agentic.deepagent_github.*`** — the coding planner (`LocalProposerClient`). Provider
  accepts `ollama` | `openai_compatible` (`src/agentic/config.rs`,
  `VALID_DEEPAGENT_PROVIDERS`). `base_url` must be loopback (`is_loopback_url`).

Loopback-only enforcement (`is_loopback_url`) is preserved on both — the security
invariant holds. `docs/MODELS.md` already documents the `lmstudio` fallback example on
port 1234 (`mlx_lm.server`'s default).

## The two things you must get right

### 1. MLX must be PRIMARY, not fallback

`resolve_local_backend` probes the primary first and only falls back if the primary
model is **not** installed. If Ollama is running and `qwen3.8:27b-mlx` is present (the
normal case), a fine-tuned model configured as `models.local_llm.fallback` is **never
used**. Fallback is a backup path, not a default-selection path.

To use the fine-tuned model by default, set it as primary, or import the fused model
into Ollama via GGUF (so it is the primary tag).

### 2. Repoint BOTH configs

`models.local_llm` controls chat; `agentic.deepagent_github` controls the coding
planner. They are independent. Repoint only the first and the **coding planner stays on
stock Ollama** — defeating the main value (repo-aware code suggestions). Cloud chat
(`models.cloud_chat.grok` / `.claude`) is a third, separate path and is not part of
local fine-tuning.

## The full config block (MLX as primary, both paths)

```yaml
models:
  local_llm:
    provider: "lmstudio"                              # chat path
    base_url: "http://127.0.0.1:1234/v1"              # mlx_lm.server loopback
    model: "cgagent-fused"                            # exact id mlx_lm.server reports
    reasoning_effort: "none"
agentic:
  deepagent_github:
    provider: "openai_compatible"                     # coding planner path
    base_url: "http://127.0.0.1:1234/v1"              # same server
    model: "cgagent-fused"
    allow_cloud_providers: false
    providers:
      grok:
        enabled: false
      claude:
        enabled: false
```

`load_agentic_config` rejects parent-off plus any enabled cloud-provider child.
When setting `allow_cloud_providers: false`, also disable every enabled
`providers.*` child (today `grok` and `claude`) or the home will fail to load.

Both `base_url`s must be loopback. Restart the console to re-evaluate after changing
services.

The compatible-backend chat path reserves twice its output ceiling for prompt
safety, even with `reasoning_effort: "none"` in YAML (that field is sent only
to Ollama). Keep chat and `/loop` ceilings at most 12952; the defaults 4096 and
2048 already fit. Summary tuning and usage calibration apply here too; see
[local history compaction](CONSOLE.md#local-history-compaction).

## Workflow

1. **Build the dataset** — `python3 finetune/build_dataset.py --repos . --dataset
   --dataset-dir finetune/data` (20 curated Q&A + bounded code-reference pairs from 28
   significant files; MLX ChatML `{"text": ...}`).
2. **Train** — `mlx_lm.lora --config finetune/lora_config.yaml` (QLoRA auto-detected on
   the 4-bit MLX base; base frozen, adapter trained full-precision).
3. **Fuse** — `mlx_lm.fuse --model malekoo/Qwen3.8-27B-MLX-4bit --adapter-path ./adapters
   --save-path ./cgagent-fused`.
4. **Serve** — `mlx_lm.server --model ./cgagent-fused --port 1234`.
5. **Wire** — paste the config block above into `config.yaml`, restart the console.

## GGUF / Ollama-native alternative

If you want one process (Ollama) serving both paths instead of running `mlx_lm.server`:
fuse, convert the fused model to GGUF (llama.cpp), then
`ollama create cgagent-fused -f finetune/Modelfile.cgagent` (after setting `FROM` to your
GGUF file), and set `models.local_llm.provider: "ollama"`,
`base_url: "http://127.0.0.1:11434/v1"`, `model: "cgagent-fused"`. Small quality loss vs
the live-MLX path; avoids a second process.

## Verification (do this before training)

Run `bash finetune/smoke_test.sh` — it starts `finetune/mock_server.py` (an
OpenAI-compatible mock), exports `CGAGENTHARNESS_HOME` to an owned temp home
(`serve` accepts only `--host` / `--port`; there is no global `--config`), points
both configs at the mock, builds the harness, and sends one authenticated chat.
The smoke's own `GET /v1/models` is mock liveness only, not resolver proof. If
chat succeeds, the mock must log a real `POST /v1/chat/completions` after harness
boot; if chat fails, the script prints that clearly. Planner `/api/agent/run`
coverage is out of scope for this smoke.

## Notes & caveats

- **Ollama cannot hot-load an MLX adapter** — always fuse first.
- The `LocalProposerClient::provider()` audit label hardcodes `"ollama"` even when the
  planner is `openai_compatible` — cosmetic (the endpoint is correct); a small Rust
  cleanup is optional.
- On 48 GB unified, 27B QLoRA peak is ~24–28 GB (4-bit base ~16–18 GB + adapter/Adam/
  activations) — fits with OS headroom, not guaranteed under heavy memory pressure.
- The dataset is intentionally small (20 Q&A + 28 code-refs). Expand `curated_qa.py` as
  the repo grows; the builder warns on drifted `SIGNIFICANT_FILES`.
