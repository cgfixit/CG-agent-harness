"""
Curated CG-Agent Q&A dataset for MLX LoRA/QLoRA fine-tuning.

CG-agent-harness (github.com/CGFixIT/CG-agent-harness, Rust, "CG-Agent") is a
loopback-only agentic coding console — the Rust successor to CyClaw's Python
harness/. These Q&A teach a model to REASON about CG-Agent's architecture,
security model, and local-MLX fine-tuning path — not to memorize files.

Sourced from the live repo (main, 2026-09-20): src/agentic/, src/llm/,
src/server/, src/common/, setup-guide.md, docs/MODELS.md, docs/CODING_PIPELINE.md,
INVARIANTS.md. Each claim is verifiable against the cited module/function.

Target: MacBook Pro M5 Pro, 48 GB unified, qwen3.8:27b-mlx base, mlx_lm.lora QLoRA.
Schema: each entry is {"instruction": str, "input": str, "output": str} -> MLX ChatML.
"""

from __future__ import annotations

CURATED_QA: list[dict[str, str]] = [

# ═══ 1. ARCHITECTURE & MODULE LAYOUT ═════════════════════════════════════════

{
    "instruction": "Describe the CG-agent-harness Rust module layout and what each top-level crate does.",
    "input": "",
    "output": (
        "CG-Agent is one Rust crate (cgagentharness, edition 2021, MSRV 1.88) with a library + binary. "
        "src/agentic/ is the coding pipeline: real_repo_loop.rs (plan->edit->check), cloud_proposer.rs "
        "(CloudProposerClient + sanitize_handoff), proposer.rs (LocalProposerClient), commands.rs, "
        "governance.rs, edits.rs, workspace.rs, run_store.rs, unslop.rs. src/llm/ is the model layer: "
        "backend.rs (resolve_local_backend), ollama.rs, openai_chat.rs (ChatClient), openai_stream.rs, "
        "inventory.rs (model_readiness), cloud_chat.rs (Grok/Claude), spend.rs. src/server/ is the axum "
        "console: mod.rs, chat_web.rs, sessions.rs, session_export.rs, session_search.rs, "
        "structured_memory_*.rs, web_research.rs, agent_jobs.rs, agent_schedules.rs. src/common/ is "
        "shared primitives: config.rs (AppConfig), injection.rs, auth_*.rs, audit.rs, local_tls.rs, "
        "sandbox_wrap.rs, ratelimit.rs. src/shim/ bridges platform specifics."
    ),
},

{
    "instruction": "What is the relationship between CG-agent-harness and CyClaw's harness/ package?",
    "input": "",
    "output": (
        "CG-agent-harness is the Rust port and successor to CyClaw's Python harness/ package. The Cargo "
        "description states it is 'a loopback-only agentic coding harness console (Rust port of CyClaw's "
        "harness/ + agentic/ real-repo pipeline)'. CyClaw removed harness/ in PR #1367; the coding-console "
        "role now lives in this separate Rust repo. CyClaw remains the RAG/security-gateway agent; CG-Agent "
        "is the bounded coding loop. They are siblings under the CGFixIT org, not a single program."
    ),
},

# ═══ 2. THE CODING LOOP (real-repo) ══════════════════════════════════════════

{
    "instruction": "Explain the real-repo coding loop and its stages.",
    "input": "",
    "output": (
        "The real-repo loop (src/agentic/real_repo_loop.rs) is a bounded plan->edit->check->feedback cycle. "
        "A planner proposes an approach; the loop applies edits to the real working tree (edits.rs, "
        "BoundedEdits), runs checks (build/test/lint), and feeds results back for the next iteration. It "
        "is deliberately bounded — the model's output is a proposal, not authority: the harness validates, "
        "scopes, and gates every mutation. The loop never bypasses retrieval, audit, or approval gates."
    ),
},

{
    "instruction": "What does LocalProposerClient do, and how is its model endpoint configured?",
    "input": "",
    "output": (
        "LocalProposerClient (src/agentic/proposer.rs) is the local planner model client. It POSTs to "
        "{base_url}/chat/completions using base_url/model/api_key from the agentic.deepagent_github.* "
        "config. Its provider() returns the string \"ollama\" but that is ONLY an audit label and the "
        "is_cloud() discriminator — the actual endpoint comes from config, so it can target any "
        "OpenAI-compatible loopback server (including mlx_lm.server). The payload includes model, "
        "messages, max_tokens, temperature, and optional reasoning_effort."
    ),
},

# ═══ 3. CLOUD-CODING GATES & HANDOFF ═════════════════════════════════════════

{
    "instruction": "What are the cloud-coding gates and when is egress permitted?",
    "input": "",
    "output": (
        "Cloud coding (sending a coding task to Grok or Claude) is opt-in behind six gates, including the "
        "master cloud gate (agentic.deepagent_github.allow_cloud_providers) and a per-run --confirm-online. "
        "A cloud turn sends only the handoff payload, never local history, skills, memory, or web context. "
        "Both the master gate and the specific provider flag (grok/claude) must be true for that provider "
        "to be usable; a provider with the master gate off is treated as not configured."
    ),
},

{
    "instruction": "What is sanitize_handoff and why does it exist?",
    "input": "",
    "output": (
        "sanitize_handoff (src/agentic/cloud_proposer.rs, pub fn) scrubs the handoff payload before any "
        "egress to a cloud provider. The handoff contains only: the instruction, an approved plan, prior "
        "feedback, bounded source excerpts, and fetched PR/issue context. Sanitization strips anything that "
        "could leak identity, secrets, or excessive local context — the soul/identity preamble is "
        "intentionally omitted so identity never leaves the box. This is the confused-deputy defense: even "
        "with cloud credentials, the agent cannot exfiltrate the user's local environment."
    ),
},

# ═══ 4. LOOPBACK-ONLY CHAT & SECURITY INVARIANTS ═════════════════════════════

{
    "instruction": "What is the loopback-only chat invariant and how is it enforced?",
    "input": "",
    "output": (
        "CG-Agent talks to local model servers only on loopback (127.0.0.1 / localhost / ::1). "  # DevSkim: ignore DS162092 because this Q&A documents the loopback-only invariant.
        "is_loopback_url (src/llm/backend.rs) enforces this for models.local_llm.base_url and "
        "agentic.deepagent_github.base_url; local_endpoint (src/llm/inventory.rs) additionally rejects "
        "URLs with credentials, query, or fragment. The inventory probe never probes external endpoints "
        "and never downloads models. This keeps all model traffic on-machine — a cloud provider is only "
        "contacted for an explicit, gated cloud-coding turn, never for local chat."
    ),
},

{
    "instruction": "How does CG-Agent defend against prompt injection in tool/skill output?",
    "input": "",
    "output": (
        "src/common/injection.rs normalizes (NFKC) and screens tool/skill output for injection markers "
        "before it re-enters the model context. The prompt-context path is governed: prompt_context "
        "requires explicit /skill use, and the governed catalog has no execution adapter — tool output is "
        "treated as untrusted data, not instructions. The six cloud-coding gates plus sanitize_handoff "
        "extend this defense to any egress path. This is defense-in-depth: retrieval + injection filtering "
        "+ bounded egress."
    ),
},

# ═══ 5. SESSION EXPORT & SEARCH ══════════════════════════════════════════════

{
    "instruction": "What did PR #157 add to session handling?",
    "input": "",
    "output": (
        "PR #157 (src/server/session_export.rs + session_search.rs) added exporting sessions as Markdown "
        "and searching transcripts locally. A session's conversation can be rendered to a .md file, and "
        "the transcript store is full-text searchable so you can find prior turns without re-asking the "
        "model. This keeps session knowledge local and retrievable — consistent with the offline-first, "
        "loopback-only posture."
    ),
},

# ═══ 6. STRUCTURED MEMORY ════════════════════════════════════════════════════

{
    "instruction": "Describe the structured memory subsystem and its parts.",
    "input": "",
    "output": (
        "Structured memory (src/server/structured_memory*.rs) is the durable, queryable memory layer with "
        "five cooperating modules: structured_memory.rs (core), _fts.rs (full-text search over notes), "
        "_auto.rs (automatic note extraction from turns), _consolidate.rs (merge/dedupe notes), and "
        "_suggest.rs (surface relevant notes into context). It sits on SQLite (rusqlite bundled) and keeps "
        "memory local — no cloud sync. Memory is separate from identity and from control flow, mirroring "
        "CyClaw's identity != memory != topology invariant."
    ),
},

# ═══ 7. WEB RESEARCH ═════════════════════════════════════════════════════════

{
    "instruction": "How does CG-Agent's web research pipeline work and what constrains it?",
    "input": "",
    "output": (
        "Web research (src/server/web_*.rs) is a policy-gated pipeline: web_intent classifies whether a "
        "turn needs the web, web_policy enforces what is permitted, web_search/web_google fetch results, "
        "web_index/index organize them, and web_research synthesizes. It is loopback-aware and bounded — "
        "fetched content is treated as untrusted (injection-filtered) and never auto-executed. The pipeline "
        "is opt-in per the secure-research rules (docs/SECURE_RESEARCH.md), so the agent can ground answers "
        "in live sources without opening an unbounded egress path."
    ),
},

# ═══ 8. AGENT JOBS & SCHEDULES ═══════════════════════════════════════════════

{
    "instruction": "What are agent jobs and schedules used for?",
    "input": "",
    "output": (
        "src/server/agent_jobs.rs and agent_schedules.rs let the console run bounded, repeatable agent "
        "tasks — a job is a single bounded run of the coding loop; a schedule recurs it (e.g. a nightly "
        "triage). They reuse the same plan->edit->check loop and the same gates, so a scheduled job cannot "
        "escalate beyond what an interactive turn could do. Jobs/schedules persist locally and report "
        "through the console, keeping automation on the same loopback-only, audited footing as manual use."
    ),
},

# ═══ 9. MODEL BACKEND: TWO-CONFIG REALITY ════════════════════════════════════

{
    "instruction": "CG-Agent has two local-model configurations. What does each control, and why must both be set for a fine-tuned model to reach the coding planner?",
    "input": "",
    "output": (
        "models.local_llm.* controls the chat backend (console chat, structured memory, compaction, web "
        "research) via the resolved ChatClient in state.rs. agentic.deepagent_github.* controls the coding "
        "planner (LocalProposerClient). They are independent: repointing only models.local_llm gives the "
        "fine-tune to chat but leaves the coding planner on stock Ollama. To get repo-aware coding "
        "suggestions from a fine-tuned model, BOTH configs must point at the same fused-model server. "
        "Cloud chat (models.cloud_chat.grok/claude) is a third, separate path and is not part of local "
        "fine-tuning."
    ),
},

{
    "instruction": "Why won't a fine-tuned model configured as models.local_llm.fallback be used by default?",
    "input": "",
    "output": (
        "resolve_local_backend (src/llm/backend.rs) probes the primary first and only falls back if the "
        "primary model is NOT installed. If Ollama is running and qwen3.8:27b-mlx is present (the normal "
        "case), the fallback is never reached. To use a fine-tuned model by default, set it as PRIMARY "
        "(models.local_llm.provider + base_url + model pointing at the fused-model server), or import the "
        "fused model into Ollama via GGUF so it is the primary tag. Fallback is a backup path, not a "
        "default-selection path."
    ),
},

{
    "instruction": "How does the inventory readiness probe work, and does it require an Ollama-specific response?",
    "input": "",
    "output": (
        "model_readiness (src/llm/inventory.rs) does GET {endpoint}/models and checks the standard "
        "OpenAI shape: value[\"data\"] is an array and some row[\"id\"] equals the configured model. It "
        "does NOT assume an Ollama-specific response — any OpenAI-compatible /v1/models endpoint works, "
        "including mlx_lm.server. The probe is bounded (timeout + max bytes), never probes external "
        "endpoints, and never downloads models. A model is 'installed' only if its exact id appears in "
        "that list."
    ),
},

# ═══ 10. MLX QLORA FINE-TUNING (MAC) ═════════════════════════════════════════

{
    "instruction": "Walk through the MLX QLoRA fine-tuning workflow for CG-Agent on Apple Silicon.",
    "input": "",
    "output": (
        "1) Build the dataset: python finetune/build_dataset.py --repos . --dataset --dataset-dir finetune/data "
        "(emits train.jsonl/valid.jsonl in MLX ChatML {\"text\": ...} format). 2) Train: mlx_lm.lora --config "
        "finetune/lora_config.yaml (QLoRA auto-detected on the 4-bit MLX base; base frozen, adapter trained "
        "full-precision). 3) Fuse: mlx_lm.fuse --model malekoo/Qwen3.8-27B-MLX-4bit --adapter-path ./adapters "
        "--save-path ./cgagent-fused. 4) Serve: mlx_lm.server --model ./cgagent-fused --port 1234. 5) Wire BOTH "
        "configs at the server (MLX as primary): models.local_llm.provider=lmstudio, base_url=http://127.0.0.1:1234/v1, "  # DevSkim: ignore DS162092 because this Q&A documents the loopback MLX bind.
        "and agentic.deepagent_github.provider=openai_compatible, same base_url. Always fuse before serving — "
        "Ollama cannot hot-load an MLX adapter."
    ),
},

{
    "instruction": "Is LoRA fine-tuning CUDA/NVIDIA-only, or does it work on Mac?",
    "input": "",
    "output": (
        "LoRA the technique is not CUDA-specific. Apple's MLX framework implements LoRA and QLoRA natively on "
        "the Metal GPU with no CUDA dependency — mlx_lm.lora trains LoRA/DoRA/full, and QLoRA is automatic when "
        "the base model is quantized (point --model at a 4-bit MLX base). The CUDA association comes from the "
        "old ecosystem (Unsloth, bitsandbytes, PyTorch+PEFT on NVIDIA). On an M5 Pro 48 GB, a 27B QLoRA fits: "
        "4-bit base ~16-18 GB plus adapter/Adam/activations ~24-28 GB peak, with OS headroom. Ollama still "
        "cannot hot-load an MLX adapter — fuse first, then serve via mlx_lm.server or re-export to GGUF."
    ),
},

{
    "instruction": "What is the GGUF/Ollama-native alternative to running mlx_lm.server, and when would you use it?",
    "input": "",
    "output": (
        "Instead of keeping mlx_lm.server running as a separate loopback process, you can re-export the fused "
        "model to GGUF and register it in Ollama: ollama create cgagent-fused -f finetune/Modelfile.cgagent "
        "(after GGUF conversion via llama.cpp). Then set models.local_llm.provider=ollama, "
        "base_url=http://127.0.0.1:11434/v1, model=cgagent-fused. Use this when you want one process (Ollama) "  # DevSkim: ignore DS162092 because this Q&A documents the loopback Ollama bind.
        "serving both chat and the planner, or when mlx_lm.server's per-request latency is a concern. The "
        "loopback mlx_lm.server path is the alternative — it preserves MLX optimizations and avoids GGUF "
        "conversion's small quality loss."
    ),
},

# ═══ 11. VERIFYING THE INTEGRATION (SMOKE TEST) ══════════════════════════════

{
    "instruction": "How do you smoke-test that CG-Agent can use a fused MLX model without Rust changes?",
    "input": "",
    "output": (
        "Run finetune/mock_server.py on 127.0.0.1:1235 — it answers GET /v1/models with the configured id "  # DevSkim: ignore DS162092 because this Q&A documents the loopback smoke mock.
        "and POST /v1/chat/completions with finish_reason=stop. Then point both configs at it as primary "
        "(models.local_llm.provider=lmstudio, agentic.deepagent_github.provider=openai_compatible, same "
        "loopback base_url), cargo build --release, boot the console, send a chat, and run a real-repo "
        "loop. If the resolver selects the mock, state.chat calls it, and the planner hits /chat/completions "
        "with the configured model, no Rust changes are needed. The mock confirms the OpenAI-compatible "
        "loopback path end-to-end before you spend time training."
    ),
},

# ═══ 12. DESIGN PHILOSOPHY ═══════════════════════════════════════════════════

{
    "instruction": "What are CG-Agent's core design invariants?",
    "input": "",
    "output": (
        "Three invariants, mirroring CyClaw: (1) loopback-only — all local model traffic stays on-machine, "
        "enforced by is_loopback_url; cloud egress is explicit and gated. (2) Model output is a proposal, "
        "not authority — the harness bounds, validates, and gates every mutation (bounded edits, the "
        "plan->edit->check loop, approval gates). (3) Defense-in-depth — retrieval + NFKC injection "
        "filtering + sanitize_handoff + the six cloud gates layer so no single bypass compromises the "
        "system. Identity, memory, and control flow remain separate concerns."
    ),
},

]
