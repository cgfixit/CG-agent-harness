# .github/skills

Skills for the GitHub Copilot coding agent, one `SKILL.md` per folder. Copilot
also reads the root [AGENTS.md](../../AGENTS.md); these add task procedure, not
authority.

- [`repo-optimize/`](repo-optimize/SKILL.md): scans the default branch for code,
  CI, security and maintainability fixes and opens focused draft PRs only when a
  chunk earns it.

Related: [.claude/skills/](../../.claude/skills/README.md),
[.codex/skills/](../../.codex/skills/README.md),
[workflows/](../workflows/README.md) (`copilot-setup-steps.yml` prepares its runner).
