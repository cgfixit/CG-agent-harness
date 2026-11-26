---
name: cgagentharness-invariant-guard
description: Assess Harness security contracts against a tree or diff, including core-path merge checks and requested invariant audits.
---

# Invariant guard

Read the affected `INVARIANTS.md` sections and trace entrypoints through their
callers. Names below are integration test targets unless a path is given.
Select affected checks, including refusals, or run the entire invariant target
for a requested full audit:

```sh
GROK_API_KEY='' ANTHROPIC_API_KEY='' DEEPAGENT_API_KEY='' \
  cargo test --locked --test invariant_guard -- --nocapture
```

| Contract | Evidence |
|---|---|
| I6, shim whitelist, route capabilities, console CSRF and storage | `invariant_guard`, `shim_and_agent_routes`, `src/server/routes/mod.rs`, `src/common/authz.rs` |
| Accounts, role revocation, TLS, Host/origin, CSRF, redaction | `auth_guards`, `secure_portal`, `security_headers`, `panels`, `src/common/auth_store.rs` |
| Mutation, approval, clone jail, rollback/quarantine | Use `cgagentharness-write-policy-redteam` for the owning test map. |
| Sandbox selection, capture and cleanup | `macos_cargo`, `linux_bwrap`, `process_lifecycle`, `process_writer_outcome` |
| Shared preparation, job ownership, reviewed goals and schedule occurrences | `shim_and_agent_routes`, `agent_schedules`, `owner_isolation` |
| Session/export/adoption, memory proposals and recall | `owner_isolation`, `session_export`, `structured_memory`, `structured_memory_phase7`, `structured_memory_suggestions` |
| Web policy, DNS, listing/arXiv grants, revocation and detached delivery | `web_research`, `web_phase2_redteam`, `chat_web`, `src/server/web_policy.rs` |
| Bounded PDF worker and same-URL Firecrawl fallback | `src/common/pdf_text.rs`, `src/server/web_firecrawl.rs`, attachment/notes/web callers |
| /browse grants, one-shot body binding, policy/expiry recheck and job recovery | `auth_guards`, `secure_portal`, `browser takeover` fixture, `cgagentharness-verify` |
| Outbound MCP broker confirmation, containment and loop exclusion | `mcp_client`, `mcp_lifecycle`, `mcp_windows`, `loop_runner` |
| Inbound MCP machine key, owner-only reads and revocation | `mcp_server`, `src/server/mcp_keys.rs` |
| Atomic reload, preserved authority and rate history | `secure_portal`, `src/server/config_reload.rs::RELOADABLE`. Use `cgagentharness-config-guard` for shipped config. |
| Compaction, goal evidence and stopped turns | `chat_and_sessions`, `chat_web`, `loop_runner` |
| Usage versus prediction and private notification retries | `spend_ledger`, `notifications` |
| Loopback Ollama, no num_ctx, local cloud-tag refusal, native sidecar identity | `ollama_manage`, `cloud_model_selection`, `scripts/test-desktop-backend.py` |
| Provider-key storage and child scrubbing | `src/common/credential_store.rs`, `src/server/env_keys.rs`, `windows_private_credentials`, `child_env` |
| LAN scope, bounded dialing and passive status | `netconnect_flags`, `netconnect_scope`, `netconnect_cli`, `netconnect_syscall` |
| Markdown limits | `invariant_guard::markdown_stays_on_the_docs_budget` |

Check `INVARIANTS.md#signals-weaker-than-their-name` before drawing conclusions
from CSRF, process groups, private storage or policy rechecks. Report PASS, FAIL
or NOT RUN per contract with SHA, commands, results and platform gaps. A
zero-test filter is NOT RUN.
