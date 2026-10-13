# .claude

Claude Code project configuration. The instructions Claude loads every session
are the root [CLAUDE.md](../CLAUDE.md), which imports [AGENTS.md](../AGENTS.md);
nothing here grants authority. Every Markdown file in this folder counts toward
the `DOCS_BUDGET` Agent group in [tests/invariant_guard.rs](../tests/invariant_guard.rs).

- [`settings.json`](settings.json): the pre-approved `cargo` and script commands
  and the `SessionStart` hook declaration; anything else prompts.
- [`hooks/`](hooks/README.md): scripts those hooks run (toolchain, libdbus,
  build cache at session start).
- [`skills/`](skills/README.md): one `SKILL.md` per folder, invoked as `/<slug>`;
  four are operator-only.

Related: [.codex/](../.codex/README.md) (Codex counterpart),
[.github/skills/](../.github/skills/README.md) (Copilot counterpart).
