# .claude

Claude Code project configuration. The instructions Claude loads every session
are the root [CLAUDE.md](../CLAUDE.md), which imports [AGENTS.md](../AGENTS.md);
nothing here grants authority. Every Markdown file in this folder counts toward
the `DOCS_BUDGET` Agent group in [tests/invariant_guard.rs](../tests/invariant_guard.rs).

- [`settings.json`](settings.json): allows listed `cargo` and script commands,
  asks before `Agent`, denies `Workflow`, and declares the `SessionStart` hook.
- [`hooks/`](hooks/README.md): tracked-hook setup everywhere; toolchain, libdbus
  and build-cache preparation in Claude remote sessions.
- [`skills/`](skills/README.md): one `SKILL.md` per folder, invoked as `/<slug>`;
  four are operator-only.

Related: [.codex/](../.codex/README.md) (Codex counterpart),
[.github/skills/](../.github/skills/README.md) (Copilot counterpart).
