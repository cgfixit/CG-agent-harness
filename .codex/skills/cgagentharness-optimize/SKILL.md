---
name: cgagentharness-optimize
description: Find and implement evidence-backed Harness performance or maintainability improvements.
---

# Optimize the Harness

Follow `AGENTS.md` for checkout isolation, remote refresh, overlap checks and
delivery. Read the affected invariant and production callers.

## Earn the change

- Agentic pipeline: compare exact-edit, reviewed-tree and lifecycle behavior
  along the same command path.
- Research: trace `src/server/chat_web/evidence.rs`, `src/common/pdf_text.rs`
  and `src/llm`. Separate cache hits, refreshes, worker runs and digest
  inference. Check retained passages and citations.
- Browser: compare `browser/sidecar.mjs` observation latency at equal field
  coverage; keep input classification fail closed.
- Desktop: trace ownership, sidecar identity and packaging provenance.
- Workflows: check platform coverage and trigger on the candidate SHA.
- Guidance: route readers to owning code. Check references and Markdown caps.

Record each finding's trigger, location, impact and verification. Measure
equivalent baseline and candidate workloads at the bottleneck before claiming
a speedup. Diagnose baseline failures and report a verified no-op when the
evidence supports it.

## Implement and deliver

Reuse existing helpers and exercise affected success and refusal paths. Use
`cgagentharness-verify` for applicable runtime surfaces. Keep each draft to one
concern. When a prerequisite lands, refresh `origin/main`, recheck overlap and
verify against the new base. Leave broad regression coverage to hosted CI.
Follow the exact head to terminal checks when requested.
