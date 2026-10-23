# src/server

The loopback HTTP/HTTPS console. [`guards.rs`](guards.rs) and
[`headers.rs`](headers.rs) are core paths. Agentic work goes out through
`crate::shim` as a child process, never in-process (I6). Routes:
[docs/API_ROUTES.md](../../docs/API_ROUTES.md); console behavior:
[docs/CONSOLE.md](../../docs/CONSOLE.md); contracts: [INVARIANTS.md](../../INVARIANTS.md).

- [`agent_jobs.rs`](agent_jobs.rs): `JobStore` for detached `/api/agent/jobs` runs; gated tasks retain memory-enqueue outcomes.
- [`agent_policy.rs`](agent_policy.rs): check-profile name to fixed-argv table; `RUN_ID_PATTERN` duplicate of `run_store`.
- [`agent_schedules.rs`](agent_schedules.rs): persisted recurring schedules, at-most-once per occurrence, through `prepare_run`.
- [`attachment_index.rs`](attachment_index.rs): query-ranked passage fence over owner-verified attachments.
- [`attachments.rs`](attachments.rs): chat upload `AttachmentStore`: cap-std jail, UUID blobs, multipart parse.
- [`chat_web.rs`](chat_web.rs): chat-model `web_search`/`web_fetch` tool loop with bounded reads and SSE.
- [`client.rs`](client.rs): `account`/`web` portal clients; `tls` inspects or renews home-owned certificates.
- [`compaction.rs`](compaction.rs): prompt-window estimation, reply reservation, web budgets, summary compaction.
- [`config_reload.rs`](config_reload.rs): hot reload of `RELOADABLE` (23 keys) into `RuntimeLimits`; SIGHUP installer.
- [`console.rs`](console.rs): `GET /` and `/static/{name}`; inserts response CSP nonces after `mod.rs` substitutes the CSRF token.
- [`desktop.rs`](desktop.rs): Unix-only parent/sidecar stdio protocol; carries ownership, no API authority.
- [`diag_reader.rs`](diag_reader.rs): allowlisted private log tails, bounded reads, redaction and injection scanning.
- [`docx.rs`](docx.rs): bounded DOCX main-document text extraction; no relationships, macros or disk.
- [`env_keys.rs`](env_keys.rs): `MANAGED_KEYS` allowlist for `/api/keys`: masked status, credential-store writes.
- [`errors.rs`](errors.rs): `ApiError` JSON `detail` envelope the console's `api()` helper reads.
- [`generation_gate.rs`](generation_gate.rs): single-holder local-LLM gate; busy claim returns 409; bounded preemption.
- [`guards.rs`](guards.rs): **core path.** Loopback, rate limit, same-origin, `account_gate` RBAC, `CSRF_HEADER`.
- [`headers.rs`](headers.rs): **core path.** Outermost security headers (`DEFAULT_CSP`, no-store), loopback `trusted_host`.
- [`live_state.rs`](live_state.rs): bounded in-memory status for local chat and prompt preview; no credential disclosure.
- [`mcp.rs`](mcp.rs): outbound MCP client `McpRuntime`: declared servers only, pinned DNS, stdio or SSE.
- [`mcp_keys.rs`](mcp_keys.rs): SQLite machine-key store for inbound MCP; the `mcp-key` CLI, local file authority.
- [`mcp_server.rs`](mcp_server.rs): default-off loopback inbound MCP memory gateway, own bearer auth and rates.
- [`mcp_server_config.rs`](mcp_server_config.rs): restart-only inbound MCP settings; fixed `TOOLS` list of three memory tools.
- [`memory_notes.rs`](memory_notes.rs): `/memory` pinned notes in `memory/notes.json`; injected only when on.
- [`mod.rs`](mod.rs): module tree, `build_app` test boundary, `serve_blocking` loopback bind and shutdown.
- [`model_limits.rs`](model_limits.rs): per-model budget proposals for `/model profile` and gated `auto_tune`.
- [`notes_corpus.rs`](notes_corpus.rs): owner-jailed `.md`/`.txt` corpus for local chat retrieval.
- [`notification_outbox.rs`](notification_outbox.rs): bounded outbox of delivery metadata persisted before any network attempt.
- [`notifications.rs`](notifications.rs): owner-bound completion webhooks, bearer tokens, pinned addresses, replay.
- [`passage_index.rs`](passage_index.rs): request-local BM25 `retrieve_passages` over already-authorized text.
- [`prompts.rs`](prompts.rs): system-prompt composition: soul, style, goal, web, memory, attachments, budgets.
- [`request_log.rs`](request_log.rs): one `tracing` line per request; no headers, bodies or query strings.
- [`retrieval.rs`](retrieval.rs): `RetrievalSurface` rules: which surfaces may inline attachment bytes.
- [`retrieval_ranking.rs`](retrieval_ranking.rs): private chunker, Tantivy query builder and diverse-selection ranking.
- [`schedule_time.rs`](schedule_time.rs): `ScheduleSpec` parsing and next-occurrence math; no dispatch.
- [`schemas.rs`](schemas.rs): request body structs with `validate()`, length caps, the 422 `ValidJson` extractor.
- [`self_manual.rs`](self_manual.rs): compiled manual chunks in a RAM index; local prompt context, never authority.
- [`session_export.rs`](session_export.rs): session Markdown export render/parse and `0o600` file write.
- [`session_search.rs`](session_search.rs): request-local Tantivy search over an owner's transcripts.
- [`sessions.rs`](sessions.rs): JSON `SessionStore`: owned sessions, messages, pins, token tallies, quarantine.
- [`slash.rs`](slash.rs): suggest-don't-guess slash parser; exact commands dispatch, fuzzy ones only suggest.
- [`state.rs`](state.rs): `AppState` shared by every handler, `OllamaControl`, concurrency tunables.
- [`structured_memory.rs`](structured_memory.rs): SQLite facts, proposals, episodes, consolidation receipts, restart outcomes, gates and recall.
- [`structured_memory_auto.rs`](structured_memory_auto.rs): idle auto-consolidation worker; proposals only.
- [`structured_memory_consolidate.rs`](structured_memory_consolidate.rs): manual consolidator: episodes to pending proposals.
- [`structured_memory_fts.rs`](structured_memory_fts.rs): safe FTS5 `MATCH` expression builder.
- [`structured_memory_suggest.rs`](structured_memory_suggest.rs): RAM-only completion queue; owner-scoped attempt metadata and durable run outcomes; proposals only.
- [`style.rs`](style.rs): output-style presets (`BUILTIN_IDS`) and overlays; never writes `soul.md`.
- [`tool_inventory.rs`](tool_inventory.rs): chat-callable tool names for prompts and grounding capability claims.
- [`transport.rs`](transport.rs): loopback TLS: PEM bundle generation, private material reads, listener scheme.
- [`views.rs`](views.rs): `HARNESS_SURFACES` registry, tools and skills views; the wired check behind `/api/tools`.
- [`web_command.rs`](web_command.rs): pure `/web` slash grammar parser; infers no URLs.
- [`web_google.rs`](web_google.rs): `WebTool::google_search` over the Google API or public listings.
- [`web_index.rs`](web_index.rs): rebuildable Tantivy passage index over the authorized web cache.
- [`web_intent.rs`](web_intent.rs): natural-language search intent parser (engine, count, quoted terms).
- [`web_policy.rs`](web_policy.rs): URL allow and deny rules, canonical URL and public-IP checks.
- [`web_research.rs`](web_research.rs): bounded research controller: planned queries, cited evidence, no authority.
- [`web_search.rs`](web_search.rs): `WebTool` fetch and search: policy reload, pinned DNS, bounded HTML extraction.
- [`routes/`](routes/README.md): router assembly and HTTP handlers grouped by surface.

Three stores share the word "memory": pinned notes (`memory_notes.rs`), the
uploaded notes corpus (`notes_corpus.rs`) and structured facts
(`structured_memory.rs`). Spend lives in [`src/llm/spend.rs`](../llm/spend.rs).
