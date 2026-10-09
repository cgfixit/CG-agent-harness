# .claude

Claude Code project configuration. `settings.json` holds the pre-approved
commands and the `SessionStart` hook; `hooks/` holds the hook scripts;
`skills/` holds the `/<slug>` skills. The instructions Claude loads every
session are the root [CLAUDE.md](../CLAUDE.md), which imports
[AGENTS.md](../AGENTS.md). Every Markdown file here counts toward the
`DOCS_BUDGET` Agent group in `tests/invariant_guard.rs`.
