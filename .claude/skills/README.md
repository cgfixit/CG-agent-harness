# .claude/skills

Claude Code skills, one `SKILL.md` per folder, invoked as `/<folder>`. Skills with
`disable-model-invocation: true` run only when the operator types them; the rest
may be model-loaded. Routing and read order: `cgagentharness-project-guidance`.
Most mirror `.codex/skills/`; `dep-sync`, `doc-sync`, `run-cg-agent-harness`,
`cgagentharness-verify-deps`, `cgagentharness-runtime-invariant-check` and
`cgagentharness-otel-hardening` are Claude-only. Loading a skill grants no push,
merge or release authority. The index lives in [AGENTS.md](../../AGENTS.md).
