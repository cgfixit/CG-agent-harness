# CG-Agent MLX QLoRA Fine-Tuning

Vendored fine-tune toolchain for `qwen3.8:27b-mlx` on Apple Silicon, targeting the
CG-agent-harness repo itself. Trains a LoRA adapter that bakes CG-Agent's
architecture, security model, and module layout into the model — so chat **and** the
coding planner give repo-aware answers.

**Target hardware:** MacBook Pro M5 Pro, 48 GB unified.
**Base model:** `malekoo/Qwen3.8-27B-MLX-4bit` (Hugging Face 4-bit MLX).
**Runtime:** `mlx_lm.lora` QLoRA — Apple Silicon / Metal, no CUDA.

## Why this exists

CG-Agent already speaks any OpenAI-compatible loopback model server (see
`docs/FINETUNE.md`). This directory provides everything **upstream** of the runtime:
the dataset, the training config, the Ollama Modelfile, and a mock server to verify
the runtime path before you train. No Rust changes are required to use a fine-tuned
model — only config.

## Files

| File | Purpose |
|---|---|
| `curated_qa.py` | 20 hand-written Q&A — the reasoning core (architecture, gates, sanitize_handoff, loopback, structured memory, the two-config reality, MLX path). |
| `build_dataset.py` | Emits `train.jsonl` / `valid.jsonl` (MLX ChatML `{"text": ...}`) from the curated Q&A + bounded code-reference pairs. Warns loudly on drifted `SIGNIFICANT_FILES`. |
| `lora_config.yaml` | `mlx_lm.lora` QLoRA config (4-bit base, batch 1, grad_checkpoint, rank 16). |
| `Modelfile.cgagent` | Ollama Modelfile for a GGUF re-export of a fused model. No SYSTEM prompt — persona lives in the harness. |
| `mock_server.py` | Minimal OpenAI-compatible mock (`/v1/models`, `/v1/chat/completions`) for the smoke test. Stdlib only. |
| `smoke_test.sh` | Runbook: owned `CGAGENTHARNESS_HOME`, `serve --host/--port`, one chat; mock POST after boot is the proof. |
| `requirements.txt` | `mlx-lm` (training only; builder + mock are stdlib). |

## Quickstart

```bash
# 1. Build the dataset (from the repo root)
python3 finetune/build_dataset.py --repos . --dataset --dataset-dir finetune/data

# 2. Train (on the Mac, in a venv)
python3 -m venv .venv && source .venv/bin/activate
pip install -r finetune/requirements.txt
mlx_lm.lora --config finetune/lora_config.yaml

# 3. Fuse the adapter into the base
mlx_lm.fuse --model malekoo/Qwen3.8-27B-MLX-4bit \
  --adapter-path ./adapters --save-path ./cgagent-fused

# 4. Serve the fused model on loopback
mlx_lm.server --model ./cgagent-fused --port 1234

# 5. Wire BOTH model configs at the server (MLX as PRIMARY, not fallback)
#    See docs/FINETUNE.md for the exact config block.

# (optional) Verify the runtime path before/after training:
bash finetune/smoke_test.sh
```

## The two things to get right

1. **MLX must be PRIMARY, not fallback.** `resolve_local_backend` only uses the
   fallback if the primary (Ollama) model is not installed. If Ollama is running
   `qwen3.8:27b-mlx`, a fine-tune configured as `models.local_llm.fallback` is never
   used. Set the MLX server as primary, or import the fused model into Ollama (GGUF).

2. **Repoint BOTH configs.** `models.local_llm` controls chat; `agentic.deepagent_github`
   controls the coding planner. Repoint only the first and the coding planner stays on
   stock Ollama. See `docs/FINETUNE.md` for the full config block.

## Notes

- Ollama cannot hot-load an MLX adapter — always fuse first.
- The 20 curated Q&A are CG-Agent-focused; the code-reference pairs are bounded to 28
  architecturally significant files (not every file). Expand `curated_qa.py` as the
  repo grows.
- The builder and mock server use only the Python standard library — no install needed
  for those; only training needs `mlx-lm`.
