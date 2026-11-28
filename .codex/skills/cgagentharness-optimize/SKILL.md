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
- Research: separate policy parses from URL checks, and cache hits from
  fetches, PDF workers and digest inference. Compare retained citations.
- Session search: measure cold build and warm reconciliation in
  `src/server/session_search.rs`. Exercise edits, deletes and owner isolation.
- Browser: compare `browser/sidecar.mjs` observation latency at equal field
  coverage; keep input classification fail closed.
- Desktop: trace ownership, sidecar identity and packaging provenance.
- Workflows: check platform coverage and trigger on the candidate SHA.
- Guidance: route readers to owning code. Check references and Markdown caps.

Measure equivalent workloads at the bottleneck. Record trigger, impact
and evidence. Diagnose any baseline failure before claiming improvement.

## Implement and deliver

Exercise affected success and refusal paths with `cgagentharness-verify`.
When a prerequisite lands, refresh `origin/main`, recheck overlap and verify
against the new base. Follow the exact head to terminal checks when requested.
