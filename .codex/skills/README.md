# .codex/skills

Codex skills, one `SKILL.md` per folder, read when the task applies. These are
the short versions; [.claude/skills/](../../.claude/skills/README.md) carries
longer playbooks for most of them plus Claude-only skills. The index and when to
load each lives in [AGENTS.md](../../AGENTS.md#project-skills). Loading a skill
grants no push, merge or release authority.

- [`cgagentharness-config-guard/`](cgagentharness-config-guard/SKILL.md): static
  fail-closed check of `assets/config.default.yaml`.
- [`cgagentharness-gotchas/`](cgagentharness-gotchas/SKILL.md): session-tested
  traps for install, packaging, CI and write-gate debugging.
- [`cgagentharness-invariant-guard/`](cgagentharness-invariant-guard/SKILL.md):
  invariant gate before merging core-path diffs.
- [`cgagentharness-optimize/`](cgagentharness-optimize/SKILL.md): evidence-backed
  Rust, desktop, runtime and CI improvements.
- [`cgagentharness-project-guidance/`](cgagentharness-project-guidance/SKILL.md):
  read order and skill routing; load first.
- [`cgagentharness-release/`](cgagentharness-release/SKILL.md): **Codex-only.**
  macOS universal packaging, Bundle provenance, release prep.
- [`cgagentharness-verify/`](cgagentharness-verify/SKILL.md): **Codex-only.**
  Isolated backend, desktop and local-model smoke and acceptance checks.
- [`cgagentharness-write-policy-redteam/`](cgagentharness-write-policy-redteam/SKILL.md):
  attack write gates, `confirm`+`reason`, clone jail and shim argv.
- [`fable-protocol/`](fable-protocol/SKILL.md): evidence-first reasoning before
  costly code, security or CI claims.
- [`verification-specialist/`](verification-specialist/SKILL.md): try to break a
  supplied change without modifying the tree.
