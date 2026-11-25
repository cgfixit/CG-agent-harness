---
name: cgagentharness-project-guidance
description: Locate Harness rules, implementation owners and the task skill before repository work.
---

# Project guidance

Resolve paths from the target checkout, including from an installed mirror.
Read root `AGENTS.md` and the affected `INVARIANTS.md` contract.
Use `src/README.md` for module navigation and `.codex/skills/README.md` to
select the task skill.

## Trace the entrypoint

| Task | First source |
|---|---|
| HTTP authority, TLS and CSRF | `src/server/guards.rs`, `src/server/headers.rs`, `src/common/authz.rs` |
| Routes, slash handling, console | `src/server/routes/mod.rs`, `src/server/slash.rs`, `src/server/views.rs`, `assets/static/harness.html` |
| Child dispatch and clone writes | `src/shim/mod.rs`, `src/agentic/commands.rs`, `src/agentic/writer.rs`, `src/agentic/workspace.rs` |
| Command isolation and outbound MCP | `src/agentic/executor/sandbox.rs`, `src/common/process.rs`, `src/common/mcp.rs` |
| Shipped values and reload | `assets/config.default.yaml`, `src/common/config.rs`, `src/server/config_reload.rs` |
| Web discovery and goal evidence | `src/server/web_search.rs`, `src/server/web_research.rs`, `src/server/chat_web/evidence.rs` |
| /browse and held requests | `src/server/routes/browser.rs`, `src/common/browser_host.rs`, `browser/sidecar.mjs` |
| PDF extraction and model context | `src/common/pdf_text.rs`, `src/server/compaction.rs`, `src/llm`. Trace each caller's eligibility and fallback. |

Use `cgagentharness-invariant-guard` for the test map, including ownership,
memory, schedules and netconnect. Use `cgagentharness-release` for packaging.
The owning guide links and reading scope live in `AGENTS.md`.
