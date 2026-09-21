# Fine-Tuning CG-Agent with MLX-vLM QLoRA (multimodal)

How to train a LoRA adapter for the `qwen3.8:27b-mlx` family on Apple Silicon and serve the
fine-tuned model to CG-Agent — with **no Rust changes**.

This is the reference. For a step-by-step walkthrough, see
[`docs/guides/mlx-qlora-finetune.md`](guides/mlx-qlora-finetune.md). For the vendored
tooling, see [`finetune/`](../finetune/).

## TL;DR

CG-Agent already speaks any OpenAI-compatible loopback model server. So a fine-tuned model
is served by `mlx_vlm.server` on `127.0.0.1:1234` (with the adapter applied live) and wired
into the config — no new Rust model backend is required. The work is: build a dataset,
train, serve, and repoint two config blocks.

## Why mlx-vlm, not mlx-lm

The `qwen3.8:27b-mlx` you run is **multimodal** (Text+Image, `Qwen3_5ForConditionalGeneration`,
18 GB on the Ollama tag). `mlx-lm` is text-only — it cannot represent or train the vision
encoder, so it is the wrong tool for this target. `mlx-vlm` fine-tunes the VLM directly:
with `--train-vision` **off** (the default), the vision encoder is **frozen and preserved**,
and LoRA only adapts the language model. The result is a fine-tuned model that keeps vision
capability while the language model becomes repo-aware.

The training base is `mlx-community/Qwen3.8-27B-4bit` — the 4-bit MLX multimodal build of the
same Qwen3.8-27B (mlx-vlm format). The Ollama `qwen3.8:27b-mlx` tag is an Ollama registry
blob that `mlx-vlm` cannot read; train on the MLX build, serve the fine-tuned result.

## The runtime path (already supported — verified)

Two independent local-model configurations both accept an OpenAI-compatible loopback
endpoint, and the inventory probe already parses the standard `{"data":[{"id":...}]}` shape
that `mlx_vlm.server` emits on `/v1/models`:

- **`models.local_llm.*`** — chat, structured memory, compaction, web research. Provider
  accepts `ollama` | `lmstudio`. Resolved in `src/llm/backend.rs::resolve_local_backend`;
  the `ChatClient` is built from the resolved backend in `src/server/state.rs`.
- **`agentic.deepagent_github.*`** — the coding planner (`LocalProposerClient`). Provider
  accepts `ollama` | `openai_compatible` (`src/agentic/config.rs`,
  `VALID_DEEPAGENT_PROVIDERS`). `base_url` must be loopback (`is_loopback_url`).

Loopback-only enforcement (`is_loopback_url`) is preserved on both — the security
invariant holds. `docs/MODELS.md` already documents the `lmstudio` loopback example on
port 1234.

## The two things you must get right

### 1. MLX must be PRIMARY, not fallback

`resolve_local_backend` probes the primary first and only falls back if the primary
model is **not** installed. If Ollama is running and `qwen3.8:27b-mlx` is present (the
normal case), a fine-tuned model configured as `models.local_llm.fallback` is **never
used**. Fallback is a backup path, not a default-selection path.

To use the fine-tuned model by default, set it as primary (point both configs at the
`mlx_vlm.server` loopback URL), or fuse + GGUF-export and import into Ollama so it is the
primary tag.

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
    base_url: "http://127.0.0.1:1234/v1"              # mlx_vlm.server loopback
    model: "mlx-community/Qwen3.8-27B-4bit"           # exact id mlx_vlm.server reports
    reasoning_effort: "none"
agentic:
  deepagent_github:
    provider: "openai_compatible"                     # coding planner path
    base_url: "http://127.0.0.1:1234/v1"              # same server
    model: "mlx-community/Qwen3.8-27B-4bit"
    allow_cloud_providers: false
```

Verify the exact `model` id the server reports with `curl http://127.0.0.1:1234/v1/models`.
Both `base_url`s must be loopback. Restart the console to re-evaluate after changing
services.

## Workflow

1. **Build the dataset** — `python3 finetune/build_dataset.py --repos . --dataset
   --dataset-dir finetune/data` (20 curated Q&A + bounded code-reference pairs from 28
   significant files; mlx-vlm `{"messages": [...]}` format).
2. **Train** — `bash finetune/train.sh` (runs `mlx_vlm.lora` on the multimodal 4-bit base;
   `--train-vision` off → vision encoder frozen, LoRA adapts the language model).
3. **Serve** — `mlx_vlm.server --model mlx-community/Qwen3.8-27B-4bit
   --adapter-path ./adapters --port 1234`. There is **no fuse step** — the adapter is
   applied live at serve time.
4. **Wire** — paste the config block above into `config.yaml`, restart the console.

## GGUF / Ollama-native alternative

If you want one process (Ollama) serving both paths instead of running `mlx_vlm.server`:
mlx-vlm has no fuse command, so you must fuse the adapter into the base weights manually
(llama.cpp or a manual safetensors merge), convert the fused model to GGUF, then
`ollama create cgagent-fused -f finetune/Modelfile.cgagent` (after setting `FROM` to your
GGUF file), and set `models.local_llm.provider: "ollama"`,
`base_url: "http://127.0.0.1:11434/v1"`, `model: "cgagent-fused"`. Small quality loss vs
the live-MLX path; avoids a second process. Most users should prefer the `mlx_vlm.server`
loopback path.

## Verification (do this before training)

Run `bash finetune/smoke_test.sh` — it starts `finetune/mock_server.py` (an
OpenAI-compatible mock), points both configs at it, builds the harness, and sends a
chat. If the mock logs both a `GET /v1/models` (inventory probe) and a
`POST /v1/chat/completions` (chat + planner), the loopback path works with no Rust
changes. This confirms the integration before you spend time training.

## Notes & caveats

- **No fuse step** — `mlx_vlm.server` applies the adapter live (`--adapter-path`). The
  GGUF/Ollama path requires manual fusion because mlx-vlm has no fuse command.
- The `LocalProposerClient::provider()` audit label hardcodes `"ollama"` even when the
  planner is `openai_compatible` — cosmetic (the endpoint is correct); a small Rust
  cleanup is optional.
- On 48 GB unified, 27B multimodal QLoRA peak is ~28–34 GB (4-bit base ~16 GB + adapter/
  Adam/activations) — fits with OS headroom, not guaranteed under heavy memory pressure.
- The dataset is text-only Q&A (no images); that is correct for language-model LoRA on a
  VLM with the vision tower frozen. Do not add `--train-vision` unless you also supply
  image examples.
- The dataset is intentionally small (20 Q&A + 28 code-refs). Expand `curated_qa.py` as
  the repo grows; the builder warns on drifted `SIGNIFICANT_FILES`.
