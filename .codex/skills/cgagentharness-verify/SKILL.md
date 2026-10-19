---
name: cgagentharness-verify
description: Verify changed Harness backend, desktop or local-model behavior with scoped checks and isolated fixtures.
---

# Verify the harness

Read `AGENTS.md`, `INVARIANTS.md`, current CI and nearest tests. Identify the real
checkout, source SHA, dirty state, binary and architecture. Preserve operator
homes/services. Inspect fixtures and current test assertions.

Choose checks by the changed contract:
- Backend: run the changed command, route or function and inspect its effects;
  use the scoped local checks in `AGENTS.md`. Broad regression tests belong in CI.
- Contracts: use `cgagentharness-invariant-guard` for current test mapping,
  including ownership, memory, MCP, web, reload, schedules and notifications.
  Documentation edits select `invariant_guard::markdown_stays_on_the_docs_budget`.
- Native execution: `tests/macos_cargo.rs` and process lifecycle tests need real
  macOS Seatbelt; sandbox permission errors are not application regressions.
- Desktop: use `cgagentharness-release` for packaging order and provenance;
  select desktop checks and `scripts/test-desktop-backend.py` cases that exercise
  the changed boundary against the packaged backend.
- Model/runtime: inspect actual inventory and exact configured model tag; do not
  equate similarly named GGUF/MLX variants or turn on cloud fallback for a smoke test.

For live acceptance, use an owned temporary home and unique port. Record the
known binary, environment and listener, issue harmless local chat, then stop it.
Fake model/provider tests prove protocol behavior,
not availability or live quality. App launch alone is not UI acceptance.
Record only native UI interactions actually observed, in the PR body.

Keep a failing-case reproducer for corrections; diagnose baseline failures before
attributing them to the candidate. Report commands, exit statuses, tested
SHA/architecture, artifact hashes and remaining limits. A zero-test selection is
NOT RUN. Follow `AGENTS.md` for preserving assertions and operator data/services.
