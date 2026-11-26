@AGENTS.md

# CLAUDE.md

The full manual is `AGENTS.md`, imported above and loaded every session; follow it
literally. Contracts: `INVARIANTS.md`. Truth order: code > `assets/config.default.yaml`
> `INVARIANTS.md` > `AGENTS.md` > `README.md`. This file holds only what is specific
to Claude Code.

## Claude Code specifics
- `.claude/settings.json` pre-approves only `cargo` build/check/fmt/clippy/test/deny,
  `cargo run -- serve`, `scripts/verify-local.sh`, `scripts/check-pr-template.sh` and
  `python3 scripts/test-desktop-backend.py`. Anything else prompts; do not work
  around a prompt.
- Agent fan-out is capped (operator decision, 2026-10-19): settings deny the
  `Workflow` tool and gate every `Agent` spawn. Ask before using more than 2
  subagents on one task; never route around the cap.
- A `SessionStart` hook runs `.claude/hooks/session-start.sh` (git hooks; in cloud
  sessions also toolchain, libdbus, build cache). If a build fails right after
  start, read `${TMPDIR:-/tmp}/cgah-session-start.log` first.
- `.claude/skills/<slug>/SKILL.md` runs as `/<slug>`. Four are operator-only
  (`disable-model-invocation: true`): `cgagentharness-invariant-guard`,
  `cgagentharness-otel-hardening`, `cg-doc-sync`, `run-cg-agent-harness`. Ask the
  operator to run them; never self-load them.
- Branch prefix is `claude/`. Never comment `@codex review` without the
  operator's approval.
- Watching a PR (`subscribe_pr_activity`): when the operator comments
  `@codex review`, first refresh the PR body (head SHA, commands with exit codes,
  merge order, `Last updated` stamp) and pass `scripts/check-pr-template.sh`,
  then handle findings. The `review gate` stays neutral until the body is newer
  than that comment.
- Answer the pre-commit security gate's questions in the PR body.
- After a dependency, network-crate or `Command::new(` change, ask the operator
  to run `/cgagentharness-otel-hardening`: CI runs only its static checker
  (`telemetry-contract.yml`).

## Cloud sessions (Linux container)
- No `gh`: use the GitHub MCP tools. `cargo-deny` may be absent; say so, never
  count it as a pass.
- `desktop/` does not compile here (macOS-only bindings); its gate is
  `desktop-check.yml`.
- Keep extra build output outside the repository (`CARGO_TARGET_DIR`); an
  untracked directory blocks the stop hook.

## Local verification
Lint (fmt, clippy), then run the changed code. Never the full suite locally; CI
runs it with `GROK_API_KEY`, `ANTHROPIC_API_KEY` and `DEEPAGENT_API_KEY` blanked.
After any `.md` edit, `wc -w` it against its `DOCS_BUDGET` cap (`CLAUDE.md` 340,
`AGENTS.md` 1250).
