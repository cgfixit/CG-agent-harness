@AGENTS.md

# CLAUDE.md

The full manual is `AGENTS.md`, imported above and loaded every session; follow it
literally. Contracts: `INVARIANTS.md`. Truth order: code > `assets/config.default.yaml`
> `INVARIANTS.md` > `AGENTS.md` > `README.md`. This file holds only what is specific
to Claude Code.

## Critical Rules for this project:
- Ignore docs/learning/* completely unless explicitly requested by owner/operator.
- generally avoid reading anything under docs/* unless a root project file links to something relevant to a task - 
The idea is to avoid reading those .md files and .pdfs and .txt files under docs/ unless its for a defensible reason based on your task
- Going forward add screenshots under the screenshots/ folder but only for significant changes/new features/something warranting a screenshot being stored.

## Claude Code specifics
- `.claude/settings.json` pre-approves only `cargo` build/check/fmt/clippy/test/deny,
  `cargo run -- serve`, `scripts/verify-local.sh`, `scripts/check-pr-template.sh` and
  `python3 scripts/test-desktop-backend.py`. Anything else prompts; do not work
  around a prompt.
- A `SessionStart` hook runs `.claude/hooks/session-start.sh` (toolchain, libdbus,
  build cache). If a build fails right after start, read its output before debugging.
- `.claude/skills/<slug>/SKILL.md` runs as `/<slug>`. Five are operator-only
  (`disable-model-invocation: true`): `cgagentharness-invariant-guard`,
  `cgagentharness-parity`, `cgagentharness-otel-hardening`, `doc-sync`,
  `run-cg-agent-harness`. Ask the operator to run them; never self-load them.
- Branch prefix is `claude/`. Draft PR, one concern, based on `main`, body from
  `.github/PULL_REQUEST_TEMPLATE.md`.
- Never comment `@codex review` on a PR without the operator's approval.

## Local verification
Lint (fmt, clippy), then run the changed code. Never the full suite locally; CI
runs it with `GROK_API_KEY`, `ANTHROPIC_API_KEY` and `DEEPAGENT_API_KEY` blanked.
After any `.md` edit, `wc -w` it against its `DOCS_BUDGET` cap (`CLAUDE.md` 300,
`AGENTS.md` 2000).

