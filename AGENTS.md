# AGENTS.md — CGagentHarness operating manual

Follow literally. Where a rule says "never", there is no exception without
explicit approval. Read `INVARIANTS.md` before touching `src/shim`,
`src/server/guards.rs`, `src/server/headers.rs`, `src/agentic/writer.rs`,
`src/agentic/executor/sandbox.rs`, `src/agentic/workspace.rs`, or
`assets/config.default.yaml`.

## Where truth lives

1. Code. 2. `assets/config.default.yaml` (every tunable; no hardcoded tunables
elsewhere). 3. `INVARIANTS.md`. 4. This file. 5. `README.md`.

## The map

- `cgagentharness serve` -> `src/server` (axum, 127.0.0.1 only).
- `cgagentharness agentic <action>` -> `src/agentic` (hidden; spawned by
  `src/shim`, never called in-process from the server).
- Exit codes are an API: `0` ok, `2` failed, `3` env/config, `4` write refused.
  A non-zero child exit is HTTP 200 with `ok=false`; only shim failures map to
  400/502/504 and the disabled-layer banner to 409.
- Home: `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`). Never write outside it
  except into a clone the pipeline itself made under `data/agentic/workspaces`.

## Traps

- **Never** make the server reference `crate::agentic`; `tests/invariant_guard.rs`
  fails the build-of-truth if you do. Add server-side behavior in `src/server`,
  cross the boundary only through `src/shim` and the CLI whitelist.
- Duplicated on purpose, kept in sync by tests: `RUN_ID_PATTERN` (server
  `agent_policy` vs agentic `run_store`), the planner/check timeout constants
  (shim vs agentic), the check-profile table. Do not "deduplicate" them across
  the boundary.
- Quoted YAML `"true"` is OFF for every gate (`flag_is_true`). Keep it that way.
- `confirm` is never defaulted on; `reason` is never optional on a write.
- The console asset stays verbatim: placeholders `__CYCLAW_CSRF_TOKEN__` /
  `__CYCLAW_CSP_NONCE__` and the `X-CyClaw-CSRF` header name are contractual.
- The env var `GROK_API_KEY` on a developer machine is real: tests must not
  assert on its presence and CI blanks it.
- The env var `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` on an operator machine is
  real: `cargo test` isolates it so later write gates are what fail. Do not
  flip `EXECUTION_ENABLED` to false (or OR the kill switch) to make tests green.
- scrypt at n=2^17 is slow unoptimized; `[profile.dev.package."*"] opt-level=3`
  is load-bearing for test time.
- `cargo clippy` may resolve to a rustup proxy older than Homebrew's toolchain
  on this machine; `CLIPPY=/opt/homebrew/bin/cargo-clippy scripts/verify-local.sh`.

## Quality bar

- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-targets` green; `cargo deny check` clean.
- New routes: add to `routes/mod.rs::REGISTERED_PATHS` (and `views.rs` if the
  console lists them) or `/api/tools` reports them unwired.
- New shim actions: extend `shim::ACTIONS`, the CLI dispatch, and the
  invariant guard's whitelist assertion together.
- `/api/agent/run` (sync) and `/api/agent/jobs` (detached) must stay in lockstep:
  both go through `agent::prepare_run` so the validation, budget check, tool
  broker, and both gates can never drift between the two paths.
- Prefer a small unit test with `#[cfg(test)] mod tests` beside the function
  over another integration test when the thing under test is a pure parser or
  matcher (`repo_paths`, `real_repo_loop`'s file-block parser, `guards`'
  same-origin check) — they run in milliseconds and don't need a server.
- PRs are draft, one concern, on a driver-prefixed branch (`claude/`, `codex/`,
  `grok/`, `kimi/`, `agent/`), **based on `main`** (never stacked on another
  feature branch; the `base branch is main` check fails otherwise), with the body
  from `.github/PULL_REQUEST_TEMPLATE.md` (run `scripts/check-pr-template.sh`
  first). Touching a core path requires an explicit invariant statement in the body.

## Project Codex skills

Read the relevant entrypoint under `.codex/skills` when its task applies:

- `cgagentharness-optimize/SKILL.md`: evidence-backed Rust/runtime/CI improvements;
  adapted from CyClaw's Claude workflow with this repository's contracts.
- `cgagentharness-release/SKILL.md`: universal macOS packaging, native acceptance,
  workflow provenance and release preparation.
- `cgagentharness-verify/SKILL.md`: isolated backend, desktop and local-model checks.
- `fable-protocol/SKILL.md`: evidence-first reasoning and verification discipline;
  load before costly code/security/CI/GitHub claims.
- `cgagentharness-invariant-guard/SKILL.md`: "do the invariants still hold?" gate;
  load before merging core-path security diffs.
- `cgagentharness-gotchas/SKILL.md`: session-tested traps (Chrome CI flake, YAML
  `"true"`, CSRF names, Seatbelt noise); load before install/verify/packaging.
- `cgagentharness-project-guidance/SKILL.md`: read order + skill routing; load at
  the start of substantive repository work.
- `cgagentharness-write-policy-redteam/SKILL.md`: adversarially exercise write
  gates, confirm+reason, clone jail, and shim argv boundaries.
- `verification-specialist/SKILL.md`: independently verify a *supplied* change by
  trying to break it, without modifying the tree.
- `cgagentharness-config-guard/SKILL.md`: statically assert `assets/config.default.yaml`
  still honors fail-closed contracts.
- `cgagentharness-parity/SKILL.md`: maintain CyClaw↔harness parity docs without
  weakening harness invariants.

## Project Claude skills

The eight Codex twins above that are not optimize/release/verify
(`fable-protocol`, `cgagentharness-invariant-guard`, `cgagentharness-gotchas`,
`cgagentharness-project-guidance`, `cgagentharness-write-policy-redteam`,
`verification-specialist`, `cgagentharness-config-guard`, `cgagentharness-parity`)
are mirrored under `.claude/skills/<slug>/SKILL.md`, plus a Claude-depth
`cgagentharness-optimize/SKILL.md` playbook (the `.codex` twin stays the short
runtime). Claude Code also has three verification skills that Codex does not:
`cgagentharness-doc-sync`, `cgagentharness-verify-deps`, and
`cgagentharness-runtime-invariant-check`. A separate `run-cg-agent-harness`
skill drives a local fake-model console smoke and has no slash command. Every
other Claude skill above is registered as `.claude/commands/<slug>.md`
(`/fable-protocol`, `/cgagentharness-doc-sync`, etc.) and loads the matching
`SKILL.md`.

These are repository guidance, not application `/api/skills` runtime plugins.
Existing user authorization governs publication; selecting a skill adds none.
