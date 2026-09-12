# Codex skills

Index of the eleven repository-guidance skills under `.codex/skills/`.
These are **not** runtime `/api/skills` plugins. Selecting a skill does
not authorize push, merge, or release.

Load [`cgagentharness-project-guidance`](skills/cgagentharness-project-guidance/SKILL.md)
at the start of substantive work.

| Skill | When to load |
|---|---|
| [`cgagentharness-config-guard`](skills/cgagentharness-config-guard/SKILL.md) | Before merging `assets/config.default.yaml` changes; when asked to check config |
| [`cgagentharness-gotchas`](skills/cgagentharness-gotchas/SKILL.md) | Install/verify, desktop packaging, Chrome CI, clippy fights, write-gate debugging, false greens |
| [`cgagentharness-invariant-guard`](skills/cgagentharness-invariant-guard/SKILL.md) | Before merging core-path security diffs; first gate of a harness security review |
| [`cgagentharness-optimize`](skills/cgagentharness-optimize/SKILL.md) | Focused improvement scans that may open a draft PR |
| [`cgagentharness-parity`](skills/cgagentharness-parity/SKILL.md) | Updating CyClaw↔harness parity docs without weakening harness invariants |
| [`cgagentharness-project-guidance`](skills/cgagentharness-project-guidance/SKILL.md) | Start of substantive repository work |
| [`cgagentharness-release`](skills/cgagentharness-release/SKILL.md) | Packaging, signing, checksum embed, release artifacts |
| [`cgagentharness-verify`](skills/cgagentharness-verify/SKILL.md) | Isolated backend, desktop, and local-model smoke/acceptance |
| [`cgagentharness-write-policy-redteam`](skills/cgagentharness-write-policy-redteam/SKILL.md) | Hardening writer, write gates, confirm+reason, clone jail, or shim argv |
| [`fable-protocol`](skills/fable-protocol/SKILL.md) | Evidence-first discipline before costly code/security/CI/GitHub claims |
| [`verification-specialist`](skills/verification-specialist/SKILL.md) | Independently try to break a *supplied* change; do not modify the tree |

Claude-depth twins and extra verification skills live under `.claude/skills/`.
The Codex `fable-protocol` / `cgagentharness-optimize` copies stay the short runtimes.
