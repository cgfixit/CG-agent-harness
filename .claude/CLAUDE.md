# CLAUDE.md

Loaded every session: only what most tasks need. Full manual (follow it literally): `AGENTS.md`.
Contracts: `INVARIANTS.md`. This file lives in `.claude/` because a root `CLAUDE.md` fails
`tests/invariant_guard.rs`.

**Truth order:** code > `assets/config.default.yaml` > `INVARIANTS.md` > `AGENTS.md` > `README.md`.
When prose and code disagree, fix the prose in the same PR.

## Before pushing
```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" cargo test --all-targets
cargo deny check
```
Rust 1.88 is pinned. Always blank those three keys; never assert on a real key.
After structural changes, `cargo test --test invariant_guard` is the fast check.

## Rules CI catches late, or not at all
- **I6:** `src/{server,shim,llm,common}` never import `crate::agentic` under any alias;
  `src/agentic` never imports server or shim. Cross only via `shim::ACTIONS`; a new action
  changes `ACTIONS`, `agentic/commands.rs::dispatch` and the guard's whitelist together.
- **Core paths:** `src/shim/`, `src/server/{guards,headers}.rs`, `src/agentic/{writer,workspace}.rs`,
  `src/agentic/executor/sandbox.rs`, `assets/config.default.yaml`. Read `INVARIANTS.md` first,
  put an invariant statement in the PR, and ask the operator to run the manual-only
  `/cgagentharness-invariant-guard` before merge.
- **Write gates:** `agentic.enabled`, `deepagent_github.enabled`, `deepagent_github.allow_git_write_tools`
  ship `false`. Quoted `"true"` is off. `confirm` is never defaulted; `reason` is never optional.
- **The browser never supplies a command:** check-profile names map to fixed argv; free text
  crosses as one `--opt=value` element or a temp file.
- New route → `REGISTERED_PATHS`. `/api/agent/run` and `/api/agent/jobs` both go through `prepare_run`.
- Never rename `__CYCLAW_CSRF_TOKEN__`, `__CYCLAW_CSP_NONCE__` or `X-CyClaw-CSRF`. Never "dedupe"
  `RUN_ID_PATTERN`, the planner/check timeouts or the check-profile table across the boundary.
- The harness writes only inside `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`) or a pipeline-owned
  clone; keep new code that way.

## Docs, tests, PRs
- Edit the section that owns a topic; don't create `.md` files. Evidence and closeouts go in
  the PR body; screenshots go in `docs/screenshots/`.
- Prefer a `#[cfg(test)]` test beside a pure function over a new `tests/` file.
- Branch `claude/<kebab-topic>`; title `[prefix] - Sentence`, where prefix is one of
  invariant, security, infra, fix, docs, harness, agentic, test, feat.
  Draft, one concern, body from `.github/PULL_REQUEST_TEMPLATE.md`, checked with `scripts/check-pr-template.sh`.
- Loading a skill never authorizes push, merge or release.
