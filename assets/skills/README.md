# assets/skills

Runtime skills bundled into the binary (`seed_skills` in `src/common/home.rs`)
and copied once into a fresh home's `skills/` folder, never overwritten. These
are application `/api/skills` plugins, unlike the agent guidance under
[.claude/skills/](../../.claude/skills/README.md) and
[.codex/skills/](../../.codex/skills/README.md). Selecting one grants no authority.

- [`ponytail/`](ponytail/SKILL.md): the "ponytail" engineering discipline the
  code comments cite (bounded, measured, no speculative caches).
- [`karpathy-guidelines/`](karpathy-guidelines/SKILL.md): coding-agent guidelines
  for small, verified, reviewable changes.

Related: [docs/CONSOLE.md](../../docs/CONSOLE.md) (runtime skills in the UI),
[tests/runtime_skills.rs](../../tests/runtime_skills.rs).
