# .claude/skills

Claude Code skills, one `SKILL.md` per folder, invoked as `/<folder>`. Skills
marked **operator-only** carry `disable-model-invocation: true` and run only when
the operator types them; the rest may be model-loaded. Loading a skill grants no
push, merge or release authority. Start with `cgagentharness-project-guidance`;
the routing table lives in [AGENTS.md](../../AGENTS.md#project-skills). Most
mirror [.codex/skills/](../../.codex/skills/README.md); the Claude-only ones
are marked.

- [`cgagentharness-config-guard/`](cgagentharness-config-guard/SKILL.md): static
  fail-closed check of `assets/config.default.yaml` (gates, `confirm`, loopback).
- [`cgagentharness-gotchas/`](cgagentharness-gotchas/SKILL.md): session-tested
  traps: CI flakes, YAML `"true"`, Seatbelt noise, toolchain fights.
- [`cgagentharness-invariant-guard/`](cgagentharness-invariant-guard/SKILL.md):
  **operator-only.** "Do the invariants still hold?" gate for core-path diffs.
- [`cgagentharness-optimize/`](cgagentharness-optimize/SKILL.md): evidence-backed
  Rust, desktop, runtime and CI improvements as focused draft PRs.
- [`cgagentharness-otel-hardening/`](cgagentharness-otel-hardening/SKILL.md):
  **operator-only, Claude-only.** Telemetry-kill audit; ships `check_otel.py`
  and `verify.sh`.
- [`cgagentharness-project-guidance/`](cgagentharness-project-guidance/SKILL.md):
  read order and skill routing; load first.
- [`cgagentharness-runtime-invariant-check/`](cgagentharness-runtime-invariant-check/SKILL.md):
  Claude-only, report-only walkthrough of I6, guard chain, CSRF and write gates.
- [`cgagentharness-verify-deps/`](cgagentharness-verify-deps/SKILL.md):
  Claude-only, report-only check of locks, `cargo deny`, toolchain pins.
- [`cgagentharness-write-policy-redteam/`](cgagentharness-write-policy-redteam/SKILL.md):
  attack write gates, `confirm`+`reason`, clone jail and shim argv.
- [`dep-sync/`](dep-sync/SKILL.md): Claude-only. Fixes dependency, toolchain,
  deny and CI drift against `origin/main`.
- [`doc-sync/`](doc-sync/SKILL.md): **operator-only, Claude-only.** Rewrites
  stale doc sections to match the code; edit-only.
- [`fable-protocol/`](fable-protocol/SKILL.md): evidence-first reasoning before
  costly code, security or CI claims.
- [`run-cg-agent-harness/`](run-cg-agent-harness/SKILL.md): **operator-only,
  Claude-only.** Fake-model console smoke run; ships `driver.mjs`.
- [`verification-specialist/`](verification-specialist/SKILL.md): try to break a
  supplied change without modifying the tree.
