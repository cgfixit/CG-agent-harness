# assets/skills

Runtime skills bundled into the binary (`seed_skills` in `src/common/home.rs`)
and copied into the home's `skills/` folder when missing, never overwritten. These
are application `/api/skills` plugins, unlike the agent guidance under
[.claude/skills/](../../.claude/skills/README.md) and
[.codex/skills/](../../.codex/skills/README.md). Selecting one grants no authority.

- [`ponytail/`](ponytail/SKILL.md): optional coding and review context: small,
  complete changes, claims grounded in supplied evidence, no implied execution.
- [`karpathy-guidelines/`](karpathy-guidelines/SKILL.md): coding-agent guidelines
  for small, verified, reviewable changes.

Related: [docs/CONSOLE.md](../../docs/CONSOLE.md) (runtime skills in the UI),
[tests/runtime_skills.rs](../../tests/runtime_skills.rs).
