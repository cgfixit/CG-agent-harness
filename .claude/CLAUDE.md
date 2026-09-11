# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Why this file lives in `.claude/`

`tests/invariant_guard.rs::readme_does_not_link_to_missing_claude_md` fails the build if a
root-level `CLAUDE.md` exists (the root file was renamed to `AGENTS.md`). Do not create one.
`AGENTS.md` is the tool-neutral operating manual and must be followed literally; read
`INVARIANTS.md` before touching `src/shim`, `src/server/guards.rs`, `src/server/headers.rs`,
`src/agentic/writer.rs`, `src/agentic/executor/sandbox.rs`, `src/agentic/workspace.rs`, or
`assets/config.default.yaml`. Truth precedence: code > `assets/config.default.yaml` >
`INVARIANTS.md` > `AGENTS.md` > `README.md`.

## Commands

Rust 1.88 (pinned in `rust-toolchain.toml`), edition 2021, single crate `cgagentharness`.

```bash
cargo build --all-targets --locked
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo deny check                                   # advisories/licenses/bans (deny.toml)
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" cargo test --all-targets
cargo test --test shim_and_agent_routes            # one integration test file
cargo test --test real_repo_loop real_repo_run_smoke_end_to_end -- --nocapture   # one test
cargo test --test invariant_guard                  # fast source-scan guard; run after any structural change
SKIP_LIVE=1 scripts/verify-local.sh                # fmt + clippy + deny (if installed) + tests + release build
CGAH_TEST_BINARY=target/debug/cgagentharness python3 scripts/test-desktop-backend.py  # stdlib-only backend contract suite
cargo run -- serve                                 # console at http://127.0.0.1:8790/
scripts/check-pr-template.sh                       # validate PR body before opening a PR
```

- Always blank `GROK_API_KEY`, `ANTHROPIC_API_KEY`, `DEEPAGENT_API_KEY` when running tests; a real
  `GROK_API_KEY` exists on the maintainer's machine and tests must not assert on it.
- Tests drive a real `git`; a global `user.name`/`user.email` must be configured.
- `[profile.dev.package."*"] opt-level=3` in `Cargo.toml` is load-bearing (scrypt n=2^17); do not remove.
- `tests/macos_cargo.rs` is macOS-only and needs
  `cargo fetch --locked --manifest-path tests/fixtures/cargo-sandbox/Cargo.toml` first.
- `desktop/` is a separate Tauri crate (own `Cargo.lock`, Rust 1.90, not a workspace member). Build it
  with `cd desktop && cargo ...`; packaging is `scripts/package-desktop.sh` (macOS only, see `docs/DESKTOP.md`).
- If `cargo clippy` resolves to an old rustup proxy: `CLIPPY=/opt/homebrew/bin/cargo-clippy scripts/verify-local.sh`.

## Architecture

One self-reexecuting binary with three subcommands (`src/main.rs`): `serve` (public loopback console),
`agentic` (hidden, spawned only by the server), `desktop` (hidden, unix sidecar over inherited pipes).

### I6: process isolation (the rule everything else hangs on)

```
src/server  src/shim  src/llm  src/common     <- console side; NEVER references `agentic::`
        |
   src/shim/mod.rs: 12-action whitelist (`shim::ACTIONS`) -> argv -> spawns
   `current_exe() agentic <action>` as a CHILD PROCESS with a hard timeout
        |
src/agentic                                   <- pipeline side; NEVER references server or shim
```

- `tests/invariant_guard.rs` scans source in both directions and fails the build on a violation.
  Add server behavior in `src/server`; cross the boundary only via the shim and CLI whitelist.
- Exit codes are the interface: `0` ok, `2` failed, `3` env/config, `4` write refused. A non-zero child
  exit is HTTP 200 with `ok=false`; only shim failures map to 400/502/504, disabled-layer banner to 409.
- New shim action = extend `shim::ACTIONS`, the CLI dispatch in `src/agentic/cli.rs`, and the
  invariant guard's whitelist assertion, together.
- Constants duplicated on purpose across the boundary and kept in sync by tests (do not "dedupe"):
  `RUN_ID_PATTERN` (server `agent_policy` vs agentic `run_store`), planner/check timeouts (shim vs
  agentic), the check-profile table.

### Server (`src/server`)

- Guard chain on every operator route, order is load-bearing:
  `rate limit -> same-origin -> API key (constant-time; loopback bypass only when
  security.api_key_optional) -> CSRF (per-process token)`. Lives in `guards.rs`/`headers.rs`;
  locked by `tests/auth_guards.rs` and `tests/security_headers.rs`.
- `routes/mod.rs::REGISTERED_PATHS` feeds `/api/tools`'s "wired" report. A new route must be added
  there (and `views.rs` if the console lists it) or it shows as unwired.
- The browser never supplies a command: `POST /api/agent/run` carries check-profile NAMES and
  `agent_policy.rs` maps them to fixed argv. Free text crosses the shim as `--opt=value` single
  elements or temp files, never separate argv tokens.
- `/api/agent/run` (sync) and `/api/agent/jobs` (detached, `agent_jobs.rs`) must both go through
  `agent::prepare_run`; the guard test checks this.
- The console UI is one asset, `assets/static/harness.html`, served verbatim. Placeholders
  `__CYCLAW_CSRF_TOKEN__` / `__CYCLAW_CSP_NONCE__` and the `X-CyClaw-CSRF` header are contractual.
- `src/llm` resolves an OpenAI-compatible local backend (default Ollama at `127.0.0.1:11434/v1`).

### Agentic pipeline (`src/agentic`)

`real_repo_loop.rs` drives: clone into `<home>/data/agentic/workspaces` (`workspace.rs`) ->
plan (`proposer.rs` / `cloud_proposer.rs`) -> bounded edits (`edits.rs`, injection + scope + budget
checks in `writer.rs`, clone jail via `cap_std`) -> sandboxed checks (`executor/sandbox.rs`:
Seatbelt / `unshare --net` / Job Object; no backend = exit 3) -> feedback into the next attempt.
Runs are retained by `run_store.rs`; `decide` (local commit), `push`, and `publish` (draft PR via
`gh_client.rs`) are separate actions, each requiring `--reason=<why> --confirm`. Combined
`decide --push/--publish` is refused. `governance.rs` reloads `config.yaml` at every mutation
boundary; a changed repo/workspace/scope/budget refuses rather than continuing on a stale snapshot.

### Config and home

- `assets/config.default.yaml` is embedded and holds every tunable; no hardcoded tunables elsewhere.
- Home is `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`, must be absolute). Never write outside it
  except into a clone the pipeline made.
- All write gates ship closed (`agentic.enabled`, `mode`, `writes_enabled`, `deepagent_github.enabled`,
  `allow_git_write_tools`); `tests/invariant_guard.rs::shipped_config_keeps_every_gate_closed` enforces it.
  Quoted YAML `"true"` is OFF for every gate (`flag_is_true`). `confirm` is never defaulted on;
  `reason` is never optional on a write. `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE=1` is AND-ed with
  `EXECUTION_ENABLED` and can only disable.

### Tests

Integration tests live in `tests/` and spin up the axum app via `tower`; the key files are
`invariant_guard.rs`, `shim_and_agent_routes.rs` (hostile-argv matrix), `write_policy.rs`,
`real_repo_loop.rs`, `exact_edits.rs`, `agentic_foundations.rs` (clone jail, sandbox),
`auth_guards.rs`. For a pure parser or matcher, prefer a `#[cfg(test)] mod tests` beside the function
over a new integration test.

## PR conventions

- Branch `claude/<feature>` (kebab-case; allowlist is `TEMPLATE_BRANCH_PREFIXES` in `src/common/identity.rs`).
- Title `[prefix] - Short sentence`, prefixes: `[invariant] [security] [infra] [fix] [docs] [harness] [agentic] [test] [feat]`.
- Draft PR, one concern, body from `.github/PULL_REQUEST_TEMPLATE.md`, validated with
  `scripts/check-pr-template.sh`. Touching a core path requires an explicit invariant statement in the body.
- Quality bar before pushing: fmt, clippy `-D warnings`, `cargo test --all-targets`, `cargo deny check` all green.
