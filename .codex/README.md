# .codex

Codex project configuration. Codex reads the root [AGENTS.md](../AGENTS.md)
directly; nothing here grants authority. Markdown here counts toward the
`DOCS_BUDGET` Agent group in [tests/invariant_guard.rs](../tests/invariant_guard.rs).

- [`skills/`](skills/README.md): ten repository guidance entrypoints, one
  `SKILL.md` per folder, with Codex-specific procedures and separate Claude
  counterparts. These are distinct from harness runtime `/api/skills` plugins.
- [`rules/`](rules/README.md): reserved for Codex rule files; empty today.

Related: [.claude/](../.claude/README.md) (Claude Code counterpart),
[.github/skills/](../.github/skills/README.md) (Copilot counterpart).
