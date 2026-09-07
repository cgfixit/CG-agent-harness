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
  `grok/`, `kimi/`, `agent/`), with the body from `.github/PULL_REQUEST_TEMPLATE.md`
  (run `scripts/check-pr-template.sh` first). Touching a core path requires an
  explicit invariant statement in the body.
