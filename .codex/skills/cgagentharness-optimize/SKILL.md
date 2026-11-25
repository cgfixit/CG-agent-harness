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
  and `src/llm`. Count reads, worker runs, tokens and retained evidence. For
  repeated reads, distinguish cache reuse, refresh and digest inference.
  Check retained passages and citations.
- Desktop: trace native ownership, sidecar identity and packaging provenance.
- Workflows and tests: inspect coverage, platform boundaries and the commit
  each gate checked.
- Guidance: compare claims with current code and move readers to the owner.
  Check references and lower trimmed Markdown caps.

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
