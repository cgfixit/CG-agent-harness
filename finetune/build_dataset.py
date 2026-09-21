#!/usr/bin/env python3
"""Build the CG-Agent MLX fine-tuning dataset + training script + Modelfile.

Emits train.jsonl / valid.jsonl in mlx-vlm LoRA format (one {"messages": [...]}
line per example) from two tiers:

  Tier 1 — curated reasoning Q&A (curated_qa.CURATED_QA). The high-quality core that
           teaches the model to REASON about CG-Agent's architecture/security/patterns.
  Tier 2 — bounded code-reference pairs from architecturally significant source files,
           framed as "following CG-Agent's established patterns" (NOT raw file dumps).

mlx-vlm (not mlx-lm) is the trainer: it fine-tunes the multimodal Qwen3.8-27B (vision
encoder frozen by default, --train-vision off), so the fine-tuned model keeps vision
capability while the language model becomes repo-aware. mlx-vlm's lora.py does NOT
require an images column — a {"messages": [...]} dataset trains the language model
on text-only Q&A with the vision tower left intact and frozen.

The old v1 anti-pattern (dumping every file) is gone. SIGNIFICANT_FILES is curated to the
files that define the architecture; if a listed file drifts (path changes), the builder
warns loudly instead of silently skipping.

Usage:
  python finetune/build_dataset.py --repos . --dataset --train-script --modelfile --dataset-dir finetune/data

Requires: Python 3.10+, git, the repo cloned (or pass a path). No third-party deps for
the builder itself; mlx-vlm[train] is only needed to actually train (see train.sh).
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

CGAGENT_SYSTEM_PROMPT = """You are CG-Agent's internal assistant with deep knowledge of the CG-agent-harness Rust codebase.

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

def _example(user: str, assistant: str) -> dict:
    """One mlx-vlm training example: a messages list with a user + assistant turn."""
    return {
        "messages": [
            {"role": "user", "content": user},
            {"role": "assistant", "content": assistant},
        ]
    }


def build_dataset(repo_path: Path, out_dir: Path, train_ratio: float = 0.9) -> int:
    """Emit train.jsonl / valid.jsonl in mlx-vlm LoRA {"messages": [...]} format."""
    out_dir.mkdir(parents=True, exist_ok=True)
    examples: list[dict] = []

    # Tier 1 — curated reasoning Q&A
    for qa in CURATED_QA:
        examples.append(_example(qa["instruction"], qa["output"]))

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
        assistant = f"Reference ({REPO_LANG}): {label}\n```\n{content}\n```"
        examples.append(_example(instruction, assistant))

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
        "\n".join(json.dumps(ex) for ex in train) + "\n", encoding="utf-8"
    )
    if valid:
        (out_dir / "valid.jsonl").write_text(
            "\n".join(json.dumps(ex) for ex in valid) + "\n", encoding="utf-8"
        )

    print(f"[dataset] {n} examples ({n_train} train / {len(valid)} valid) -> {out_dir}")
    print(f"[dataset] curated Q&A: {len(CURATED_QA)} | code-ref pairs: {n - len(CURATED_QA)}")
    if missing:
        print(f"[dataset] WARNING: {len(missing)} significant files missing (see stderr)")
    return n


# ─── train.sh + Modelfile ─────────────────────────────────────────────────────

# mlx-vlm fine-tunes the MULTIMODAL Qwen3.8-27B (Qwen3_5ForConditionalGeneration).
# mlx-community/Qwen3.8-27B-4bit is the 4-bit MLX multimodal build (mlx-vlm format);
# the Ollama qwen3.8:27b-mlx tag is the same family (18 GB, Text+Image) but is an
# Ollama registry blob mlx-vlm cannot read — train on the MLX build, serve the fused
# result. --train-vision is OFF by default: the vision encoder is frozen and preserved,
# LoRA only adapts the language model. Our text-only Q&A dataset is fine for this.
MLX_VLM_BASE_MODEL = "mlx-community/Qwen3.8-27B-4bit"


def write_train_script(out_path: Path, base_model: str = MLX_VLM_BASE_MODEL,
                       dataset_dir: str = "finetune/data") -> None:
    """Write train.sh: the mlx_vlm.lora CLI invocation (mlx-vlm uses args, not YAML).

    Memory: 4-bit multimodal base ~16 GB + LoRA/Adam/activations ~28-34 GB peak.
    batch_size 1 + grad_checkpoint keep peak down; fits 48 GB with OS headroom, not
    guaranteed under heavy memory pressure (reduce --max-seq-length if OOM).
    """
    script = (
        "#!/usr/bin/env bash\n"
        "# CG-Agent mlx-vlm QLoRA training (Apple Silicon / Metal, no CUDA).\n"
        "# mlx-vlm fine-tunes the MULTIMODAL Qwen3.8-27B; --train-vision is off so the\n"
        "# vision encoder is frozen and preserved — LoRA only adapts the language model.\n"
        "# No YAML config: mlx_vlm.lora takes CLI args.\n"
        "#\n"
        "# Setup:  python3 -m venv .venv && source .venv/bin/activate\n"
        "#         pip install -r finetune/requirements.txt\n"
        "# Build:  python3 finetune/build_dataset.py --repos . --dataset --dataset-dir finetune/data\n"
        "set -euo pipefail\n"
        "cd \"$(dirname \"$0\")/..\"\n\n"
        f"python finetune/train.py \\\n"
        f"  --model-path {base_model} \\\n"
        f"  --dataset {dataset_dir}/train.jsonl \\\n"
        "  --iters 600 \\\n"
        "  --batch-size 1 \\\n"
        "  --learning-rate 1e-4 \\\n"
        "  --max-seq-length 2048 \\\n"
        "  --grad-checkpoint \\\n"
        "  --lora-rank 16 \\\n"
        "  --lora-alpha 32 \\\n"
        "  --steps-per-report 10 \\\n"
        "  --steps-per-eval 100 \\\n"
        "  --steps-per-save 100 \\\n"
        "  --val-batches 5 \\\n"
        "  --output-path ./adapters/cgagent-lora.safetensors\n"
    )
    out_path.write_text(script, encoding="utf-8")
    out_path.chmod(0o755)
    print(f"[train] script -> {out_path}  (base: {base_model})")


def build_modelfile(out_path: Path, model_tag: str = "qwen3.8:27b-mlx",
                    num_ctx: int = 32768, temperature: float = 0.2) -> None:
    """Ollama Modelfile — system-prompt path (stock multimodal model + CG-Agent prompt).

    NOTE: the fine-tuned LoRA is NOT served via Ollama here. mlx-vlm has no fuse command;
    the live fine-tuned model is served by `mlx_vlm.server --model <base> --adapter-path
    <adapter>` (loopback, OpenAI-compatible) and wired into BOTH CG-Agent model configs
    (see docs/FINETUNE.md). This Modelfile is only the no-training system-prompt path, or
    the manual GGUF re-export path (fuse via llama.cpp -> ollama create) for users who
    specifically want Ollama as the runtime.
    """
    lines = [
        f"FROM {model_tag}",
        f'SYSTEM """{CGAGENT_SYSTEM_PROMPT}"""',
        f"PARAMETER temperature {temperature}",
        f"PARAMETER num_ctx {num_ctx}",
        "PARAMETER top_p 0.9",
    ]
    out_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"[modelfile] -> {out_path}  (FROM {model_tag}, num_ctx {num_ctx})")


# ─── CLI ──────────────────────────────────────────────────────────────────────

def main() -> None:
    ap = argparse.ArgumentParser(description="CG-Agent mlx-vlm fine-tuning dataset builder")
    ap.add_argument("--repos", default=".",
                    help="Path to the CG-agent-harness repo (default: current dir)")
    ap.add_argument("--dataset", action="store_true")
    ap.add_argument("--train-script", action="store_true")
    ap.add_argument("--modelfile", action="store_true")
    ap.add_argument("--dataset-dir", default="finetune/data")
    ap.add_argument("--train-script-output", default="finetune/train.sh")
    ap.add_argument("--modelfile-output", default="finetune/Modelfile.cgagent")
    ap.add_argument("--base-model", default=MLX_VLM_BASE_MODEL)
    ap.add_argument("--model-tag", default="qwen3.8:27b-mlx")
    ap.add_argument("--num-ctx", type=int, default=32768)
    ap.add_argument("--temperature", type=float, default=0.2)
    args = ap.parse_args()

    repo_path = Path(args.repos).resolve()
    if not repo_path.exists():
        print(f"[!] repo path not found: {repo_path}")
        return

    if not any([args.dataset, args.train_script, args.modelfile]):
        args.dataset = True

    if args.dataset:
        build_dataset(repo_path, Path(args.dataset_dir))
    if args.train_script:
        write_train_script(Path(args.train_script_output), args.base_model, args.dataset_dir)
    if args.modelfile:
        build_modelfile(Path(args.modelfile_output), args.model_tag,
                        args.num_ctx, args.temperature)


if __name__ == "__main__":
    main()
