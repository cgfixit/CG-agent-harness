---
name: cgagentharness-optimize
description: Find and implement evidence-backed Harness performance or maintainability improvements, with focused drafts when publication is requested.
---

# Optimize the harness

Read `AGENTS.md` and the affected contract in `INVARIANTS.md`. Establish the
checkout, dirty state, default branch and refreshed remote base; preserve
unrelated work. Inspect open PR files and heads before choosing findings.

## Earn the change

Inspect the requested scope, tracing production callers before proposing edits:

- `src/agentic`: exact edits, reviewed-tree approval and process lifecycle.
- `src/server`, `src/shim`, `src/llm`: guards, cancellation and bounded model I/O.
- `desktop`, packaging scripts: native ownership, sidecar identity and provenance.
- Workflows/tests: meaningful coverage, platform boundaries and exact-head gates.
- Guidance: stale claims, broken references and duplicate owner sections.

For each finding, record the concrete trigger, location, impact and verification.
Large files or newer dependencies alone do not justify changes. Measure the
actual bottleneck on equivalent baseline/candidate workloads before claiming a
speedup; report output parity and measurement conditions. Diagnose relevant
baseline failures before relying on them. Zero findings is a valid result.

## Implement and verify

Group related changes to shared files into one concern. Independent drafts
start from current `origin/main` and target `main`; deliver dependent work after
its prerequisite lands, then refresh and check overlap again.

Use existing helpers and preserve affected security contracts. Follow
`AGENTS.md` for local verification: run changed behavior, use scoped static
checks, and select focused tests when direct execution cannot cover the risk.
Lint changed workflows and inspect their invoked scripts. Resolve backend and
desktop toolchains from their respective files. Native Seatbelt, live-model and
WKWebView claims require those actual environments.

For guidance changes, edit the owner section, verify references, remove stale
copies and lower the corresponding documentation caps. Keep evidence in the PR
body rather than adding parallel guides.

## Deliver within scope

When publication is requested, complete the commit, push and draft PR using the
repository template and validator. Otherwise deliver the local result. Verify
remote head and report current CI separately from review and mergeability;
monitor to completion when requested. Preserve the user's merge/release limits.
