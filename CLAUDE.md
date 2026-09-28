# CLAUDE.md

Loaded every session: only what most tasks need. Full manual (follow it literally): `AGENTS.md`;
contracts: `INVARIANTS.md`. Where they disagree, `AGENTS.md` wins.

**Truth order:** code > `assets/config.default.yaml` > `INVARIANTS.md` > `AGENTS.md` > `README.md`.
Fix prose that contradicts code in the same PR.

## Before pushing
```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" cargo test --all-targets
cargo deny check
```
Rust 1.88 is pinned. Always blank those three keys; never assert on a real key.
`cargo test --test invariant_guard` is the fast check after structural changes.

## Rules CI catches late, or not at all
- **I6:** `src/{server,shim,llm,common}` never import `crate::agentic`; `src/agentic` never
  imports server or shim. A new action changes `shim::ACTIONS`, `agentic/commands.rs::dispatch`
  and the guard's whitelist together.
- **Core paths:** `src/shim/`, `src/server/{guards,headers}.rs`, `src/agentic/{writer,workspace}.rs`,
  `src/agentic/executor/sandbox.rs`, `assets/config.default.yaml`. Read `INVARIANTS.md` first;
  put an invariant statement in the PR body.
- **Write gates:** `agentic.enabled`, `deepagent_github.enabled`, `deepagent_github.allow_git_write_tools`
  ship `false`. Quoted `"true"` is off. `confirm` is never defaulted; `reason` is never optional.
- **The browser never supplies a command:** profile names map to fixed argv; free text crosses
  as one `--opt=value` element or a temp file.
- New route → `REGISTERED_PATHS`. `/api/agent/run` and `/api/agent/jobs` both go through `prepare_run`.
- Never rename `__CYCLAW_CSRF_TOKEN__`, `__CYCLAW_CSP_NONCE__` or `X-CyClaw-CSRF`. Never "dedupe"
  `RUN_ID_PATTERN`, the planner/check timeouts or the check-profile table across the boundary.
- Write only inside `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`) or a pipeline clone
  under `data/agentic/workspaces`.

## Docs and PRs
- Edit the section that owns a topic; never create `.md` files (`DOCS_BUDGET` in
  `tests/invariant_guard.rs` fails on one). Evidence goes in the PR body.
- Branch `claude/<kebab-topic>` off `main`; title `[prefix] - Sentence` (invariant, security, infra,
  fix, docs, harness, agentic, test, feat); draft, one concern, body from
  `.github/PULL_REQUEST_TEMPLATE.md` via `scripts/check-pr-template.sh`.
- Loading a skill never authorizes push, merge or release.
