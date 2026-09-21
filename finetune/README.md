# CG-Agent MLX-vLM QLoRA Fine-Tuning (multimodal)

Vendored fine-tune toolchain for the `qwen3.8:27b-mlx` family on Apple Silicon, targeting
the CG-agent-harness repo itself. Trains a LoRA adapter that bakes CG-Agent's
architecture, security model, and module layout into the model — so chat **and** the
coding planner give repo-aware answers — while **preserving the model's vision capability**
(the vision encoder is frozen, not stripped).

**Target hardware:** MacBook Pro M5 Pro, 48 GB unified.
**Base model:** `mlx-community/Qwen3.8-27B-4bit` (Hugging Face 4-bit MLX, multimodal, mlx-vlm format).
**Trainer:** `mlx_vlm.lora` QLoRA — Apple Silicon / Metal, no CUDA. `--train-vision` off →
vision encoder frozen, LoRA adapts only the language model.

## Why mlx-vlm, not mlx-lm

The `qwen3.8:27b-mlx` you run is multimodal (Text+Image, `Qwen3_5ForConditionalGeneration`).
`mlx-lm` is text-only and would strip the vision encoder. `mlx-vlm` fine-tunes the VLM
directly with the vision tower frozen — so the fine-tuned model keeps vision. Our text-only
Q&A dataset trains the language model; the vision encoder is preserved and untouched.

## Why this exists

CG-Agent already speaks any OpenAI-compatible loopback model server (see `docs/FINETUNE.md`).
This directory provides everything **upstream** of the runtime: the dataset, the training
script, the Ollama Modelfile, and a mock server to verify the runtime path before you
train. No Rust changes are required to use a fine-tuned model — only config.

## Files

| File | Purpose |
|---|---|
| `curated_qa.py` | 20 hand-written Q&A — the reasoning core (architecture, gates, sanitize_handoff, loopback, structured memory, the two-config reality, the mlx-vlm path). |
| `build_dataset.py` | Emits `train.jsonl` / `valid.jsonl` (mlx-vlm `{"messages": [...]}`) from the curated Q&A + bounded code-reference pairs. Warns loudly on drifted `SIGNIFICANT_FILES`. |
| `train.sh` | `mlx_vlm.lora` CLI invocation (mlx-vlm uses args, not a YAML config). |
| `Modelfile.cgagent` | Ollama Modelfile — system-prompt path (stock model) **or** manual GGUF re-export path (fused model). |
| `mock_server.py` | Minimal OpenAI-compatible mock (`/v1/models`, `/v1/chat/completions`) for the smoke test. Stdlib only. |
| `smoke_test.sh` | Runbook: start mock, build, boot, send a chat, confirm the resolver + planner hit the mock. |
| `requirements.txt` | `mlx-vlm[train]` (training only; builder + mock are stdlib). |

## Quickstart

```bash
# 1. Build the dataset (from the repo root)
python3 finetune/build_dataset.py --repos . --dataset --dataset-dir finetune/data

# 2. Train (on the Mac, in a venv)
python3 -m venv .venv && source .venv/bin/activate
pip install -r finetune/requirements.txt
bash finetune/train.sh

# 3. Serve the fine-tuned model (adapter applied live — no fuse step)
mlx_vlm.server --model mlx-community/Qwen3.8-27B-4bit \
  --adapter-path ./adapters/cgagent-lora.safetensors --port 1234

# 4. Wire BOTH model configs at the server (MLX as PRIMARY, not fallback)
#    See docs/FINETUNE.md for the exact config block.

# (optional) Verify the runtime path before/after training:
bash finetune/smoke_test.sh
```

## The two things to get right

1. **MLX must be PRIMARY, not fallback.** `resolve_local_backend` only uses the
   fallback if the primary (Ollama) model is not installed. If Ollama is running
   `qwen3.8:27b-mlx`, a fine-tune configured as `models.local_llm.fallback` is never
   used. Set the MLX server as primary.

2. **Repoint BOTH configs.** `models.local_llm` controls chat; `agentic.deepagent_github`
   controls the coding planner. Repoint only the first and the coding planner stays on
   stock Ollama. See `docs/FINETUNE.md` for the full config block.

## Notes

- No fuse step — `mlx_vlm.server` applies the adapter live. The GGUF/Ollama path requires
  manual fusion (mlx-vlm has no fuse command).
- The 20 curated Q&A are CG-Agent-focused; the code-reference pairs are bounded to 28
  architecturally significant files (not every file). Expand `curated_qa.py` as the repo
  grows.
- The builder and mock server use only the Python standard library — no install needed
  for those; only training needs `mlx-vlm[train]`.
