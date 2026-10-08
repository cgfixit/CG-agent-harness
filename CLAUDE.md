# CLAUDE.md

Full manual (follow it literally): `AGENTS.md`; contracts: `INVARIANTS.md`.
Where this summary and `AGENTS.md` disagree, `AGENTS.md` wins.

**Truth order:** code > `assets/config.default.yaml` > `INVARIANTS.md` > `AGENTS.md` > `README.md`.
Fix prose that contradicts code in the same PR.

## CI merge gate
```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" cargo test --all-targets
cargo deny check
```
Blank those three keys; never assert on a real key.
Locally: fmt, clippy, then run the changed code; never the full suite (CI runs it).

## Rules CI catches late, or not at all
- **I6:** `src/{server,shim,llm,common}` never import `crate::agentic`; `src/agentic` never
  imports server or shim.
- **Core paths:** `src/shim/`, `src/server/{guards,headers}.rs`, `src/agentic/{writer,workspace}.rs`,
  `src/agentic/executor/sandbox.rs`, `assets/config.default.yaml`. Read `INVARIANTS.md` first.
- **Write gates:** `agentic.enabled`, `deepagent_github.enabled`, `deepagent_github.allow_git_write_tools`
  ship `false`. Quoted `"true"` is off. `confirm` is never defaulted; `reason` is never optional.
- **The browser never supplies a command:** fixed argv only; free text is one `--opt=value`
  element or a temp file.
- Never rename `__CYCLAW_CSRF_TOKEN__`, `__CYCLAW_CSP_NONCE__` or `X-CyClaw-CSRF`. Never "dedupe"
  `RUN_ID_PATTERN`, the planner/check timeouts or the check-profile table across the boundary.
- Write only inside `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`) or a pipeline clone
  under `data/agentic/workspaces`.

## Docs and PRs
- Ignore `docs/learning/*` during agent work unless explicitly requested.
- Edit the section that owns a topic; never create `.md` files. `DOCS_BUDGET` caps words
  per file and group.
- Draft PR off `main`, one concern; branch, title and body follow
  `.github/PULL_REQUEST_TEMPLATE.md`, checked by `scripts/check-pr-template.sh`.
- Loading a skill never authorizes push, merge or release.
- Never comment `@codex review` on a PR without the operator's approval.
