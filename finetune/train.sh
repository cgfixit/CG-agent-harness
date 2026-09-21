#!/usr/bin/env bash
# CG-Agent mlx-vlm QLoRA training (Apple Silicon / Metal, no CUDA).
#
# mlx-vlm fine-tunes the MULTIMODAL Qwen3.8-27B (Qwen3_5ForConditionalGeneration).
# --train-vision is OFF by default: the vision encoder is frozen and preserved, and LoRA
# only adapts the language model. Our text-only Q&A dataset trains the language model;
# the fine-tuned model keeps vision capability.
#
# No YAML config: mlx_vlm.lora takes CLI args (unlike mlx-lm).
# Uses finetune/train.py (a thin shim) so --dataset can point at a LOCAL .jsonl —
# mlx_vlm.lora passes --dataset to datasets.load_dataset, which does not auto-detect
# local jsonl; the shim routes .jsonl paths through the json builder. Hub ids still work.
#
# Setup:  python3 -m venv .venv && source .venv/bin/activate
#         pip install -r finetune/requirements.txt
# Build:  python3 finetune/build_dataset.py --repos . --dataset --dataset-dir finetune/data
set -euo pipefail
cd "$(dirname "$0")/.."

python finetune/train.py \
  --model-path mlx-community/Qwen3.8-27B-4bit \
  --dataset finetune/data/train.jsonl \
  --iters 600 \
  --batch-size 1 \
  --learning-rate 1e-4 \
  --max-seq-length 2048 \
  --grad-checkpoint \
  --lora-rank 16 \
  --lora-alpha 32 \
  --steps-per-report 10 \
  --steps-per-eval 100 \
  --steps-per-save 100 \
  --val-batches 5 \
  --output-path ./adapters/cgagent-lora.safetensors
