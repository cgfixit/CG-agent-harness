---
name: cgagentharness-project-guidance
description: Locate current Harness operating rules, implementation sources and task-specific skills before repository work.
---

# Project guidance

Start with root `AGENTS.md`: it owns the architecture map, truth order, core-path
list, reading restrictions, local verification policy and PR requirements.
Use `INVARIANTS.md` for the affected contract, then trace its implementation,
configuration and callers. Paths below are relative to the repository root.
These files guide coding agents; they are not the harness runtime `/api/skills`.

## Select the task's sources

| Task | Start here |
|---|---|
| HTTP authority, roles, TLS, CSRF | `src/server/guards.rs`, `src/server/headers.rs`, `tests/auth_guards.rs`, `tests/secure_portal.rs` |
| Shim and child dispatch | `src/shim/mod.rs`, `src/agentic/commands.rs`, `tests/shim_and_agent_routes.rs` |
| Writes, approval, clone jail | `src/agentic/writer.rs`, `src/agentic/workspace.rs`, `tests/write_policy.rs`, `tests/git_approval.rs` |
| Process containment | `src/agentic/executor/sandbox.rs`, `src/common/process.rs`, `src/common/mcp.rs` |
| Defaults and reload | `assets/config.default.yaml`, `src/common/config.rs`, `src/server/config_reload.rs` |
| Routes and slash commands | `src/server/routes/mod.rs`, `src/server/slash.rs`, `src/server/views.rs`, `assets/static/harness.html` |
| Model budgets and compaction | `src/llm`, `src/server/compaction.rs`, `tests/chat_and_sessions.rs` |
| Ownership, memory, MCP, web, schedules, notifications, netconnect | Contract-to-test map in `cgagentharness-invariant-guard`; select the affected rows |
| Install and packaging | Root README's installation links, `scripts/package-desktop.sh`, `.github/workflows/desktop.yml` |

## Select a skill

The [skills index](../README.md) lists every repository Codex skill. Load the
one that answers the current question, rather than the entire collection:

- Use `fable-protocol` when the premise or evidence needs investigation.
- Use `cgagentharness-invariant-guard` for contract review,
  `cgagentharness-config-guard` for shipped defaults, and
  `cgagentharness-write-policy-redteam` for hostile mutation probes.
- Use `cgagentharness-verify` for smoke/acceptance work or
  `verification-specialist` for an independent supplied-patch assessment.
- Use `cgagentharness-gotchas` to diagnose misleading failures or false passes.
- Use `cgagentharness-release` for artifact provenance and release acceptance;
  `cgagentharness-optimize` for evidence-backed improvements.

## Keep guidance current

Check paths, symbols and command filters against the current checkout. Keep
shared operating rules in `AGENTS.md`, security contracts in `INVARIANTS.md`,
and task-specific mechanics in their skill. Update the owning section and
lower its `DOCS_BUDGET` cap when trimming it; preserve useful refusal coverage.
Loading a skill does not expand the user's publication or mutation authority.
