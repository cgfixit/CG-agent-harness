# tests

Integration tests by surface or contract. [`ci.yml`](../.github/workflows/ci.yml)
blanks `GROK_API_KEY`, `ANTHROPIC_API_KEY` and `DEEPAGENT_API_KEY`. Use focused
local checks; CI owns broad coverage. Capability-dependent tests may skip unless
their `CGAH_REQUIRE_*` gate is set; required platform jobs enforce those gates.
Docs-only classification can skip compile jobs. CI expectations: [docs/INSTALL.md](../docs/INSTALL.md#tests-and-cicd).

## Structure and contracts

- [`invariant_guard.rs`](invariant_guard.rs): I6 source boundary, shim whitelist, duplicated constants, shipped defaults, console asset, `DOCS_BUDGET`; includes the focused Markdown-budget check.
- [`shipped_config.rs`](shipped_config.rs): validates actual seeded model, web, workspace and tuning settings without protocol-fixture overrides.
- [`contract_locks.rs`](contract_locks.rs): binary exit codes, confirm and reason flags, kill switch, shim redaction, raw listener authority.
- [`common_layer.rs`](common_layer.rs): `src/common` primitives (not this folder's `common/`): atomic write, rate limit, auth records, redaction, `flag_is_true`.
- [`child_env.rs`](child_env.rs): `gh` and shim children never inherit provider keys; credentials are action-scoped.
- [`ci_docs_only.rs`](ci_docs_only.rs), [`pr_template_check.rs`](pr_template_check.rs), [`start_web_script.rs`](start_web_script.rs), [`smoke_wired_count.rs`](smoke_wired_count.rs): lock `scripts/` behavior (docs-only classifier, PR-body gate, `start-web.sh`, the smoke wired count against `HARNESS_SURFACES`).

## Server, accounts and console

- [`auth_guards.rs`](auth_guards.rs): rate, Host/origin, CSRF and account-session refusals; the optional key grants no authority.
- [`security_headers.rs`](security_headers.rs): hardening headers, no-store on `/api`, per-response CSP nonce.
- [`secure_portal.rs`](secure_portal.rs): real HTTPS/HTTP account boundary, config reload (admin, CSRF, SIGHUP), shutdown.
- [`owner_isolation.rs`](owner_isolation.rs): two accounts cannot read, mutate, export or schedule each other's sessions.
- [`panels.rs`](panels.rs): `/api/keys`, `/web`, `/memory`, views, `/api/auth/*` and `/net` panel routes.
- [`diag_routes.rs`](diag_routes.rs): log allowlist, roles, CSRF, private reads and session-bound staging; restricted `portal_auditor` chat.
- [`persona_controls.rs`](persona_controls.rs): `/api/soul` document, proposals and prompt preview; confirm, backups, interrupted apply.
- [`style_presets.rs`](style_presets.rs), [`runtime_skills.rs`](runtime_skills.rs): `/api/style` presets and per-session skill selection; neither executes anything.
- [`analytics.rs`](analytics.rs), [`spend_ledger.rs`](spend_ledger.rs): analytics summary composition and the spend ledger on the chat path; shim failure reads unavailable, never empty.
- [`notifications.rs`](notifications.rs): completion webhooks: owner-bound delivery, crash recovery, retries, replay, revocation and bearer permissions.

## Chat, sessions and web

- [`chat_and_sessions.rs`](chat_and_sessions.rs): `/api/chat` turns, compaction and reply budgets, cancel, busy gate, `/loop`, sessions, 422 envelope.
- [`chat_stream.rs`](chat_stream.rs): streaming chat shares guards, tools, session commit and the generation gate with JSON chat.
- [`chat_attachments.rs`](chat_attachments.rs): text and DOCX attachments and the notes corpus: limits, magic, prompt fence.
- [`chat_web.rs`](chat_web.rs): chat `web_fetch`/`web_search` rounds: URL permission, token budgets, cancel, revoked sources.
- [`web_commands.rs`](web_commands.rs), [`web_research.rs`](web_research.rs), [`web_phase2_redteam.rs`](web_phase2_redteam.rs): `/web` grammar, the deterministic research controller, and the non-empty allowlist red-team.
- [`session_export.rs`](session_export.rs), [`retrieval_ranking.rs`](retrieval_ranking.rs): export is CSRF-guarded and 0600; golden BM25 ranking order.
- [`self_manual.rs`](self_manual.rs): generator drift, scanner cleanliness and local-only manual/live-state prompt context.
- [`cloud_model_selection.rs`](cloud_model_selection.rs): guarded cloud switching, profiles, budgets, cancellation and restart reset with synthetic loopback providers.
- [`model_completion.rs`](model_completion.rs), [`model_readiness.rs`](model_readiness.rs), [`ollama_manage.rs`](ollama_manage.rs): truncated replies never accepted; readiness probing; native Ollama inventory, pull and warmup on loopback.

## Memory

- [`structured_memory.rs`](structured_memory.rs): gates, facts, proposals, episodes, explicit recall, FTS retrieval, consolidation workers.
- [`structured_memory_phase7.rs`](structured_memory_phase7.rs): fixture-model evaluation meets documented baselines (`fixtures/structured_memory/phase7`).
- [`structured_memory_suggestions.rs`](structured_memory_suggestions.rs): pending-only automatic drafts; owner-scoped queue outcomes, expiry, cancellation, clear, preemption; detached coding results.
- [`structured_memory_model.rs`](structured_memory_model.rs): selected local models, captured turn models, cloud fallback, and failed-attempt provenance.

## MCP

- [`mcp_client.rs`](mcp_client.rs): outbound client: `mcp.enabled` off, broker confirm, stdio and SSE fixtures, SSRF (`CGAH_REQUIRE_LINUX_MCP`).
- [`mcp_server.rs`](mcp_server.rs): inbound memory server: Streamable HTTP, key namespace, key store, transport refusals.
- [`mcp_lifecycle.rs`](mcp_lifecycle.rs): Linux systemd and cgroup containment (`CGAH_REQUIRE_LINUX_LIFECYCLE`).
- [`mcp_windows.rs`](mcp_windows.rs): Windows Job Object containment (`CGAH_REQUIRE_WINDOWS_MCP`).
- [`windows_private_credentials.rs`](windows_private_credentials.rs): Windows credential file ACLs (`CGAH_REQUIRE_WINDOWS_CREDENTIALS`).

## Coding pipeline and processes

- [`shim_and_agent_routes.rs`](shim_and_agent_routes.rs): shim argv builder, `/api/agent/run` and `/api/agent/jobs` against the real child, the 409 banner.
- [`agent_schedules.rs`](agent_schedules.rs), [`goal_workflow.rs`](goal_workflow.rs): unreviewed schedules fail closed, at-most-once firing; goal staging never executes.
- [`agentic_foundations.rs`](agentic_foundations.rs): agentic config, run store, clone jail, sandbox, manifest, registry lock, `gh` argv.
- [`write_policy.rs`](write_policy.rs), [`write_kill_switch.rs`](write_kill_switch.rs), [`git_approval.rs`](git_approval.rs): real git write gates reload per mutation; `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` still disables; approval integrity.
- [`exact_edits.rs`](exact_edits.rs), [`real_repo_loop.rs`](real_repo_loop.rs), [`exhausted_run.rs`](exhausted_run.rs): exact-edit proposals (Unix), the loop end to end (`CGAH_REQUIRE_CHECK_SANDBOX`), exhausted-run records.
- [`linux_bwrap.rs`](linux_bwrap.rs), [`macos_cargo.rs`](macos_cargo.rs): bubblewrap acceptance (`CGAH_REQUIRE_LINUX_BWRAP`); the macOS Seatbelt cargo build never skips.
- [`process_lifecycle.rs`](process_lifecycle.rs), [`process_writer_outcome.rs`](process_writer_outcome.rs): Unix process ownership, deadlines, grandchild kill; output overflow leaves publication indeterminate.

## Netconnect

- [`netconnect_cli.rs`](netconnect_cli.rs), [`netconnect_flags.rs`](netconnect_flags.rs), [`netconnect_scope.rs`](netconnect_scope.rs): exit codes 0/2/3/4; table-driven tier gates; invalid CIDRs fail at load with exit 3.
- [`netconnect_collectors.rs`](netconnect_collectors.rs), [`netconnect_untrusted.rs`](netconnect_untrusted.rs): passive collectors on `/proc` and BSD fixtures; device names capped and labeled untrusted.
- [`netconnect_syscall.rs`](netconnect_syscall.rs), [`netconnect_workflow.rs`](netconnect_workflow.rs): the strace proof script's self-check; `netconnect.yml` shape (empty permissions, pinned SHAs).

## Support folders

- [`common/`](common/README.md): shared in-process server, mock model, fake `gh`, bare remote, kill-switch isolation.
- [`fixtures/`](fixtures/README.md): deterministic data: sandbox crate, structured-memory cases, phase 7 and web-research corpora.
- [`netconnect_support/`](netconnect_support/README.md): the master, tier and scope combination table.
