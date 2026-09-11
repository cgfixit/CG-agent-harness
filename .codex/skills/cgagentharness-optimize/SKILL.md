---
name: cgagentharness-optimize
description: Find and implement evidence-backed Rust, desktop, runtime, and CI improvements in CG-agent-harness, grouped into focused draft PRs when publication is requested. Use for project optimization or maintainability work, not unrelated CyClaw or RAG changes.
---

# Optimize CG-agent-harness

Adapted for Codex from [CyClaw's Claude Optimize skill](https://github.com/cgfixit/CyClaw/blob/a414ba86ebf5f3c8bb901466c16b4f015bbc79c9/.claude/skills/CyClaw-Optimize/SKILL.md).
Retain evidence-first selection, PR deduplication, shared-file planning, and
honest verification. Use this repository's Rust contracts and tools.

Read root `AGENTS.md`, `INVARIANTS.md`, the current workflows and PR template.
Find the actual checkout, dirty state, default branch, and exact remote base.
Preserve work; create an isolated `codex/optimize-<topic>` branch when needed.
Check clean-base tests before edits. If they fail, report the failing command
and cause before building further work on that baseline.

Inspect the requested scope before choosing findings. Useful areas are:
- `src/agentic`: exact edits, reviewed-tree approval, sandbox and process lifecycle.
- `src/server`, `src/shim`, `src/llm`: guard ordering, cancellation, bounded model I/O.
- `desktop`, `scripts/package-desktop.sh`: native ownership, sidecar identity,
  architecture slices, dependency policy and reproducible verification.
- `.github/workflows`, `tests`: meaningful coverage, exact-head gates, artifact provenance.

For each retained finding, show a concrete trigger, code location, observed
impact and a discriminating check. Large files, newer dependencies or speculative
speedups do not alone justify changes. Check open PRs before selecting work;
zero findings is valid. Never manufacture a quota of findings or PRs.

Map files to proposed chunks. Consolidate related edits to shared files or stack
dependent branches explicitly. Independent branches start at current default
branch; stacked children start at their parent and name it as PR base. Use a
throwaway worktree for trial merges where overlapping changes create risk.

Implement only the authorized scope. Preserve I6: server/common/LLM/shim never
import agentic; only the shim dispatches whitelisted child actions. Keep shipped
write gates closed, explicit confirmation/reason, fresh policy checks, clone jail,
reviewed commit/origin binding, loopback guards and bounded subprocess capture.
Do not transplant CyClaw's Python/RAG topology, model defaults, hooks or credentials.

Run the relevant Rust tests plus the required quality checks from `AGENTS.md`;
use `desktop/rust-toolchain.toml` for desktop checks and root toolchain for backend.
Native Seatbelt, real model, and WKWebView claims need corresponding native evidence.
Record skipped/unavailable checks and performance measurements with conditions.

If the user requested publication, inspect the diff, run
`scripts/check-pr-template.sh <body-file>`, push the scoped branch and open a draft
PR using the actual template. Otherwise deliver the local change or assessment.
Skill selection is not authorization to push, publish a release, merge or change
host settings. Verify remote head and exact-head CI after publication. Report
concrete changes, tests, remaining risk and parent-first merge order if stacked.
