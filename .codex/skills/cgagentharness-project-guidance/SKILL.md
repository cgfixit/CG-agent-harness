---
name: cgagentharness-project-guidance
description: Locate Harness rules, implementation owners and the task skill before repository work.
---

# Project guidance

Resolve paths from the target checkout, including from an installed mirror.
Read `AGENTS.md` for guide scope, `INVARIANTS.md` for contracts,
`src/README.md` for modules and `.codex/skills/README.md` for task skills.

## Trace the entrypoint

| Task | First source |
|---|---|
| HTTP authority and audit | `src/server/guards.rs`, `src/server/headers.rs`, `src/common/authz.rs`, `src/common/audit.rs` |
| Routes and console | `src/server/routes/mod.rs`, `src/server/slash.rs`, `src/server/views.rs`, `assets/static/harness.html` |
| Coding runs and approval | `src/server/routes/agent.rs`, `src/shim/mod.rs`, `src/agentic/commands.rs`, `src/agentic/writer.rs` |
| Command isolation and outbound MCP | `src/agentic/executor/sandbox.rs`, `src/common/process.rs`, `src/common/mcp.rs` |
| Shipped values and reload | `assets/config.default.yaml`, `src/common/config.rs`, `src/server/config_reload.rs` |
| Web provenance and pinned transport | `src/server/chat_web.rs`, `src/server/chat_web/evidence.rs`, `src/server/web_search.rs` |
| /browse holds and rediscovery | `src/server/routes/browser.rs`, `src/server/browser_jobs.rs`, `src/common/browser_host.rs`, `browser/sidecar.mjs`, `assets/static/harness.html` |
| Model locality and cloud authority | `src/llm/ollama.rs`, `src/llm/cloud_chat.rs` |
| Session and fact search | `src/server/session_search.rs`, `src/server/structured_memory.rs` |
| CI jobs and required checks | `.github/workflows/README.md`, `tests/ci_docs_only.rs` |

Use `cgagentharness-invariant-guard` for the test map, including PDF, memory,
schedules and netconnect.
