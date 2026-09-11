---
name: cgagentharness-verify
description: Verify CG-agent-harness backend, native desktop and local model behavior using isolated fixtures and evidence tied to the tested source and binary. Use for smoke tests, regression checks or release acceptance.
---

# Verify the harness

Read `AGENTS.md`, `INVARIANTS.md`, current CI and nearest tests. Identify the real
checkout, source SHA, dirty state, binary path and host architecture. Preserve
operator homes and running services. Inspect fixture scripts before using them.

Choose checks by the changed contract:
- Backend quality: root-toolchain fmt, Clippy with warnings denied, all-target
  tests and cargo-deny, as documented in `AGENTS.md`.
- Policy/approval: `tests/write_policy.rs`, `tests/git_approval.rs`,
  `tests/invariant_guard.rs`, `tests/real_repo_loop.rs` and exact-edit tests.
- Native execution: `tests/macos_cargo.rs` and process lifecycle tests need real
  macOS Seatbelt; sandbox permission errors are not application regressions.
- Desktop: package before compiling the shell, then desktop-toolchain fmt,
  Clippy/tests and `scripts/test-desktop-backend.py` against the packaged backend.
- Model/runtime: inspect actual inventory and exact configured model tag; do not
  equate similarly named GGUF/MLX variants or turn on cloud fallback for a smoke test.

For live acceptance, use an owned temporary home and unique port. Start the known
binary with explicit environment, record its process/listener, issue one harmless
local chat, and verify clean stop. Fake model/provider tests prove protocol behavior,
not availability or live quality. Native app launch proves more than CLI startup but
less than full UI acceptance. Follow `docs/DESKTOP_ACCEPTANCE.md` for outstanding UI
checks and record only interactions actually observed.

Run a failing-case reproducer when correcting behavior. Never delete assertions or
weaken guards to make tests pass. Baseline failures must be diagnosed before further
implementation. Keep source changes out of runtime fixtures and close only processes
started by this verification. Report commands, exit statuses, tested SHA/architecture,
artifact hashes, skips and remaining limits without including credentials or home data.
