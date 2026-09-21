#!/usr/bin/env python3
"""Wrapper around mlx_vlm.lora that lets --dataset point at a LOCAL .jsonl file.

Why this exists: mlx_vlm.lora passes --dataset straight to
`datasets.load_dataset(path, config, split)`. That call does NOT auto-detect a local
.jsonl path (DatasetNotFoundError) and rejects save_to_disk dirs. So a local jsonl
dataset cannot be trained without either uploading to the HuggingFace Hub or this shim.

What it does: monkeypatch datasets.load_dataset so that when `path` ends in .jsonl, the
call is routed to the "json" builder with data_files=path — the known-reliable local
form. Everything else passes through unchanged, so Hub dataset ids still work.

Usage (same args as mlx_vlm.lora):
  python finetune/train.py --model-path mlx-community/Qwen3.8-27B-4bit \
      --dataset finetune/data/train.jsonl [other mlx_vlm.lora args...]

This is equivalent to `python -m mlx_vlm.lora` but with local .jsonl support.
"""
from __future__ import annotations

import datasets

_orig_load_dataset = datasets.load_dataset


def _patched_load_dataset(path, *args, **kwargs):
    """Route a local .jsonl path through the json builder; pass everything else through.

    mlx_vlm.lora calls load_dataset(args.dataset, args.dataset_config or None, split=...),
    so `args` may contain a leading None (the dataset config). Strip it for the json
    builder route so load_dataset("json", None, data_files=..., split=...) is clean.
    """
    if isinstance(path, str) and path.lower().endswith(".jsonl"):
        args = tuple(a for a in args if a is not None)
        kwargs.setdefault("data_files", path)
        return _orig_load_dataset("json", *args, **kwargs)
    return _orig_load_dataset(path, *args, **kwargs)


datasets.load_dataset = _patched_load_dataset

# Run mlx_vlm.lora as __main__ so its own CLI argparse parses sys.argv, with the
# patched load_dataset already in effect.
import runpy  # noqa: E402

runpy.run_module("mlx_vlm.lora", run_name="__main__", alter_sys=True)
