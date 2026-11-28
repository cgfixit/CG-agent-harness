---
name: cgagentharness-invariant-guard
description: Assess Harness security contracts against a tree or diff, including core-path merge checks and requested invariant audits.
---

# Invariant guard

Read affected `INVARIANTS.md` sections. Select refusal checks by integration
target. For a full audit, blank provider keys and run
`cargo test --locked --test invariant_guard`.

| Contract | Evidence |
|---|---|
| I6, shim whitelist, route capabilities, console CSRF and storage | `invariant_guard`, `shim_and_agent_routes`, `src/server/routes/mod.rs`, `src/common/authz.rs` |
| Account revocation, TLS, Host/origin, CSRF, redaction | `auth_guards`, `secure_portal`, `security_headers`, `panels` |
| Required audit records and persisted gaps | `src/common/audit.rs`, `src/server/routes/auth.rs`, `diag_routes` |
| Mutation, approval, clone jail, rollback/quarantine | `cgagentharness-write-policy-redteam` |
| Sandbox selection, capture and cleanup | `macos_cargo`, `linux_bwrap`, `process_lifecycle`, `process_writer_outcome` |
| Run/step reauthorization, jobs and schedules | `secure_portal`, `shim_and_agent_routes`, `agent_schedules`, `owner_isolation` |
| Owner-scoped session search, export, adoption and memory recall | `owner_isolation`, `session_export`, `src/server/session_search.rs`, `structured_memory`, `structured_memory_phase7`, `structured_memory_suggestions` |
| Web policy, listing/arXiv grants and revocation | `web_research`, `web_phase2_redteam`, `chat_web`, `src/server/web_policy.rs` |
| Pinned web transport and bundled trust roots | `src/server/web_search.rs`, Linux `pinned_tls` probe |
| Bounded PDF worker and same-URL Firecrawl fallback | `src/common/pdf_text.rs`, `src/server/web_firecrawl.rs`, attachment/notes/web callers |
| /browse grants, one-shot body binding, policy/expiry recheck and job recovery | `auth_guards`, `secure_portal`, `browser takeover` fixture, `cgagentharness-verify` |
| Outbound MCP broker confirmation, containment and loop exclusion | `mcp_client`, `mcp_lifecycle`, `mcp_windows`, `loop_runner` |
| Inbound MCP machine key, owner-only reads and revocation | `mcp_server`, `src/server/mcp_keys.rs` |
| Atomic reload, preserved authority and rate history | `secure_portal`, `src/server/config_reload.rs::RELOADABLE`, `cgagentharness-config-guard` |
| Compaction, goal evidence, loop ownership and reservation | `chat_and_sessions`, `chat_web`, `loop_runner` |
| Usage versus prediction and private notification retries | `spend_ledger`, `notifications` |
| Ollama locality, cloud tags and aliases, no `num_ctx` | `cloud_model_selection`, `ollama_manage`, `src/llm/ollama.rs` |
| Cloud chat reauthorization before count and generation | `src/llm/cloud_chat.rs`, `secure_portal` |
| Provider-key storage and child scrubbing | `src/common/credential_store.rs`, `src/server/env_keys.rs`, `windows_private_credentials`, `child_env` |
| LAN scope, bounded dialing and passive status | `netconnect_flags`, `netconnect_scope`, `netconnect_cli`, `netconnect_syscall` |
| Release feature isolation | `invariant_guard::release_builds_leave_out_test_support` |
| Markdown limits | `invariant_guard::markdown_stays_on_the_docs_budget` |

Read `INVARIANTS.md#signals-weaker-than-their-name` before interpreting
CSRF, process or policy signals. Report PASS, FAIL or NOT RUN per contract,
with SHA, commands and platform gaps. A zero-test filter is NOT RUN.
