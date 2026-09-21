#!/usr/bin/env python3
"""Build the CG-Agent MLX fine-tuning dataset + optional RAG-less code-reference corpus.

Emits train.jsonl / valid.jsonl in mlx_lm.lora format (one {"text": ...} ChatML line per
example) from two tiers:

  Tier 1 — curated reasoning Q&A (curated_qa.CURATED_QA). The high-quality core that
           teaches the model to REASON about CG-Agent's architecture/security/patterns.
  Tier 2 — bounded code-reference pairs from architecturally significant source files,
           framed as "following CG-Agent's established patterns" (NOT raw file dumps).

The old v1 anti-pattern (dumping every file) is gone. SIGNIFICANT_FILES is curated to the
files that define the architecture; if a listed file drifts (path changes), the builder
warns loudly instead of silently skipping.

Usage:
  python finetune/build_dataset.py --repos . --dataset --dataset-dir finetune/data
  python finetune/build_dataset.py --repos . --dataset --modelfile --lora-config

Requires: Python 3.10+, git, the repo cloned (or pass a path). No third-party deps for
the builder itself; mlx-lm is only needed to actually train (see lora_config.yaml).
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

try:
    from curated_qa import CURATED_QA  # type: ignore[import]
except Exception:  # standalone fallback if curated_qa.py is absent
    CURATED_QA = []

# ─── Repo identity ────────────────────────────────────────────────────────────

REPO_NAME = "CG-agent-harness"
REPO_LANG = "rust"

# Architecturally significant files — the ones that define CG-Agent's design.
# Kept deliberately small; expanded beyond the original 9 to cover the modules that
# grew in #157+ (session export/search, structured memory, web research, jobs).
# If any path drifts, build_dataset warns on stderr instead of silently skipping.
SIGNIFICANT_FILES: list[str] = [
    "src/main.rs",
    "src/lib.rs",
    "src/agentic/mod.rs",
    "src/agentic/real_repo_loop.rs",
    "src/agentic/cloud_proposer.rs",
    "src/agentic/proposer.rs",
    "src/agentic/commands.rs",
    "src/agentic/governance.rs",
    "src/agentic/config.rs",
    "src/llm/backend.rs",
    "src/llm/inventory.rs",
    "src/llm/openai_chat.rs",
    "src/llm/ollama.rs",
    "src/llm/cloud_chat.rs",
    "src/common/config.rs",
    "src/common/injection.rs",
    "src/server/mod.rs",
    "src/server/state.rs",
    "src/server/session_export.rs",
    "src/server/session_search.rs",
    "src/server/structured_memory.rs",
    "src/server/web_research.rs",
    "src/server/agent_jobs.rs",
    "src/server/chat_web.rs",
    "Cargo.toml",
    "INVARIANTS.md",
    "AGENTS.md",
    "setup-guide.md",
]

# CG-Agent has no RAG indexer (that is CyClaw's job), so there is no corpus build
# here. This list exists only so a future multi-repo builder can reuse it.

CYCLAW_SYSTEM_PROMPT = """You are CG-Agent's internal assistant with deep knowledge of the CG-agent-harness Rust codebase.

CG-Agent (github.com/CGFixIT/CG-agent-harness) is a loopback-only agentic coding console — the Rust
successor to CyClaw's Python harness/. Three invariants:
1. Loopback-only — all local model traffic stays on-machine (is_loopback_url); cloud egress is explicit and gated.
2. Model output is a proposal, not authority — the harness bounds, validates, and gates every mutation.
3. Defense-in-depth — retrieval + NFKC injection filtering + sanitize_handoff + the six cloud gates.

Module map:
- src/agentic/ — the coding pipeline: real_repo_loop (plan->edit->check), cloud_proposer (CloudProposerClient + sanitize_handoff), proposer (LocalProposerClient), commands, governance, edits, workspace, run_store.
- src/llm/ — model layer: backend (resolve_local_backend), ollama, openai_chat (ChatClient), openai_stream, inventory (model_readiness), cloud_chat (Grok/Claude), spend.
- src/server/ — axum console: mod, chat_web, sessions, session_export, session_search, structured_memory_*, web_*, agent_jobs, agent_schedules, console, compaction.
- src/common/ — config (AppConfig), injection, auth_*, audit, local_tls, sandbox_wrap, ratelimit.

Two local-model configs: models.local_llm.* (chat/structured-memory/compaction/web) and
agentic.deepagent_github.* (the coding planner, LocalProposerClient). Both accept an OpenAI-compatible
loopback endpoint; repoint BOTH to use a fine-tuned model for chat AND coding. Cloud chat (grok/claude)
is separate and gated.

When answering, reference specific files/functions. Follow the three invariants. Never propose changes
that bypass loopback enforcement, the bounded-edit gates, sanitize_handoff, or the approval flow.
"""


# ─── Dataset ──────────────────────────────────────────────────────────────────

def build_dataset(repo_path: Path, out_dir: Path, train_ratio: float = 0.9) -> int:
    """Emit train.jsonl / valid.jsonl in mlx_lm.lora ChatML format."""
    out_dir.mkdir(parents=True, exist_ok=True)
    examples: list[str] = []

    # Tier 1 — curated reasoning Q&A
    for qa in CURATED_QA:
        examples.append(
            f"<|im_start|>user\n{qa['instruction']}<|im_end|>\n"
            f"<|im_start|>assistant\n{qa['output']}<|im_end|>"
        )

    # Tier 2 — bounded code-reference pairs from significant files
    missing: list[str] = []
    for rel in SIGNIFICANT_FILES:
        f = repo_path / rel
        if not f.exists():
            missing.append(rel)
            continue
        content = f.read_text(encoding="utf-8", errors="ignore")[:3500]
        label = f"{REPO_NAME}/{rel}"
        instruction = (
            f"Following the established patterns of {REPO_NAME}, here is the reference "
            f"implementation of {rel}. Use it to answer questions about {REPO_NAME}'s "
            f"architecture, not to reproduce the file verbatim."
        )
        examples.append(
            f"<|im_start|>user\n{instruction}<|im_end|>\n"
            f"<|im_start|>assistant\nReference ({REPO_LANG}): {label}\n```\n{content}\n```<|im_end|>"
        )

    if missing:
        print(
            f"[!] {len(missing)} SIGNIFICANT_FILES not found under {repo_path} — paths may have drifted:\n    "
            + "\n    ".join(missing),
            file=sys.stderr,
        )

    import random
    random.seed(3407)
    random.shuffle(examples)
    n = len(examples)
    n_train = int(n * train_ratio)
    train, valid = examples[:n_train], examples[n_train:]

    (out_dir / "train.jsonl").write_text(
        "\n".join(json.dumps({"text": t}) for t in train) + "\n", encoding="utf-8"
    )
    if valid:
        (out_dir / "valid.jsonl").write_text(
            "\n".join(json.dumps({"text": t}) for t in valid) + "\n", encoding="utf-8"
        )

    print(f"[dataset] {n} examples ({n_train} train / {len(valid)} valid) -> {out_dir}")
    print(f"[dataset] curated Q&A: {len(CURATED_QA)} | code-ref pairs: {n - len(CURATED_QA)}")
    if missing:
        print(f"[dataset] WARNING: {len(missing)} significant files missing (see stderr)")
    return n


# ─── lora_config + Modelfile ─────────────────────────────────────────────────

def write_lora_config(out_path: Path, base_model: str = "malekoo/Qwen3.8-27B-MLX-4bit") -> None:
    """Write lora_config.yaml for 48 GB Apple Silicon QLoRA of a 27B model.

    Memory: 4-bit base ~16-18 GB + LoRA/Adam/activations ~24-28 GB peak. batch_size 1 +
    grad_checkpoint keeps peak down; fits 48 GB with OS headroom, not guaranteed under
    heavy memory pressure.
    """
    cfg = [
        f"model: {base_model}",
        "train: true",
        "data: ./data",
        "adapter_path: ./adapters",
        "iters: 600",
        "batch_size: 1",
        "num_layers: 16",
        "learning_rate: 1.0e-4",
        "max_seq_length: 2048",
        "grad_checkpoint: true",
        "steps_per_report: 10",
        "steps_per_eval: 100",
        "save_every: 100",
        "lora_parameters:",
        "  rank: 16",
        "  alpha: 32",
        "  dropout: 0.0",
        "  scale: 10.0",
    ]
    out_path.write_text("\n".join(cfg) + "\n", encoding="utf-8")
    print(f"[lora] config -> {out_path}")


def build_modelfile(out_path: Path, model_tag: str = "qwen3.8:27b-mlx",
                    num_ctx: int = 32768, temperature: float = 0.2) -> None:
    """Ollama Modelfile. NOTE: a fine-tuned MLX adapter is NOT attached here — Ollama
    cannot hot-load an MLX adapter. This is the system-prompt path (stock model + prompt)
    OR the GGUF re-export path (fuse -> GGUF -> ollama create with this Modelfile)."""
    lines = [
        f"FROM {model_tag}",
        f'SYSTEM """{CYCLAW_SYSTEM_PROMPT}"""',
        f"PARAMETER temperature {temperature}",
        f"PARAMETER num_ctx {num_ctx}",
        "PARAMETER top_p 0.9",
    ]
    out_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"[modelfile] -> {out_path}  (FROM {model_tag}, num_ctx {num_ctx})")


# ─── CLI ──────────────────────────────────────────────────────────────────────

def main() -> None:
    ap = argparse.ArgumentParser(description="CG-Agent MLX fine-tuning dataset builder")
    ap.add_argument("--repos", default=".",
                    help="Path to the CG-agent-harness repo (default: current dir)")
    ap.add_argument("--dataset", action="store_true")
    ap.add_argument("--modelfile", action="store_true")
    ap.add_argument("--lora-config", action="store_true")
    ap.add_argument("--dataset-dir", default="data")
    ap.add_argument("--modelfile-output", default="Modelfile.cgagent")
    ap.add_argument("--lora-config-output", default="lora_config.yaml")
    ap.add_argument("--model-tag", default="qwen3.8:27b-mlx")
    ap.add_argument("--num-ctx", type=int, default=32768)
    ap.add_argument("--temperature", type=float, default=0.2)
    args = ap.parse_args()

    repo_path = Path(args.repos).resolve()
    if not repo_path.exists():
        print(f"[!] repo path not found: {repo_path}")
        return

    if not any([args.dataset, args.modelfile, args.lora_config]):
        args.dataset = True

    if args.dataset:
        build_dataset(repo_path, Path(args.dataset_dir))
    if args.lora_config:
        write_lora_config(Path(args.lora_config_output))
    if args.modelfile:
        build_modelfile(Path(args.modelfile_output), args.model_tag,
                        args.num_ctx, args.temperature)


if __name__ == "__main__":
    main()
