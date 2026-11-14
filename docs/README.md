# docs

Operator and contributor guides, indexed by feature with the code and checks
each one owns. Start with the root [README.md](../README.md#documentation).
Agents open a guide only when a root file links it and the task needs it;
`learning/` requires an explicit operator request.

## Start here

- **New operator, fresh machine:** [INSTALL.md](INSTALL.md), then
  [MODELS.md](MODELS.md) and [CONSOLE.md](CONSOLE.md), with
  [TROUBLESHOOTING.md](TROUBLESHOOTING.md) open.
- **Memory and optional features:** [MEMORY_GUIDE.md](MEMORY_GUIDE.md),
  [CODING_PIPELINE.md](CODING_PIPELINE.md), [MCP.md](MCP.md),
  [netconnect.md](netconnect.md). Read the owning
  [INVARIANTS.md](../INVARIANTS.md) section first;
  [AGENTS.md](../AGENTS.md#the-map) maps features to sections. Fresh homes
  enable the structured-memory gates; coding, MCP and netconnect stay off, and
  existing homes keep their settings.
- **Contributor or agent:** [AGENTS.md](../AGENTS.md) and
  [INVARIANTS.md](../INVARIANTS.md), then [API_ROUTES.md](API_ROUTES.md),
  [PROCESS_LIFECYCLE.md](PROCESS_LIFECYCLE.md),
  [DEPENDENCIES.md](DEPENDENCIES.md), [GITHOOKS.md](GITHOOKS.md) and
  [RELEASING.md](RELEASING.md). Every source folder has a `README.md` index,
  starting at [src/](../src/README.md) and [tests/](../tests/README.md).

Numbered headings (`## 2. Prerequisites`) keep the original single-file
manual's order so old links resolve. Truth order is code, then
[config.default.yaml](../assets/config.default.yaml), then INVARIANTS.md.

## Setup, models and recovery

- [INSTALL.md](INSTALL.md): from a fresh machine to a verified console. The
  macOS setup wizard, installer and launcher, prerequisites, building the
  backend or the app, first run, browser access, where the home and credentials
  live, managed keys, retained jobs, reloadable limits (`POST /api/config/reload`,
  SIGHUP), and update, backup, rollback and uninstall.
  Owns: `src/main.rs`, `src/common/{home,config,credential_store}.rs`,
  `src/server/{env_keys,config_reload}.rs`, `scripts/{init,install-macos,start-macos,verify-local}.sh`,
  `.github/workflows/ci.yml`.
  Tests: `tests/common_layer.rs`, `tests/contract_locks.rs`, `tests/windows_private_credentials.rs`.
- [MODELS.md](MODELS.md): local inventory and model selection, the 32768-token
  window seeded web budgets need, derived tags, warmup and tuning, consented
  cloud chat and the separate coding planner. Read it before the first chat.
  Owns: `src/llm/{ollama,inventory,profile,backend,cloud_chat}.rs`, `src/server/model_limits.rs`,
  `scripts/qwen3.8-27b-mlx-cg.Modelfile`, `scripts/smoke-ollama.sh`. Tests: `tests/ollama_manage.rs`,
  `tests/model_readiness.rs`, `tests/model_completion.rs`, `tests/cloud_model_selection.rs`.
- [TROUBLESHOOTING.md](TROUBLESHOOTING.md): one symptom, cause and fix table,
  plus read-only host inspection through declared MCP servers and bounded
  `/diag` logs.
  Owns: `src/server/diag_reader.rs`, `src/server/routes/diag.rs`.

## Console and daily use

- [CONSOLE.md](CONSOLE.md): the user guide for the chat UI. Sessions and
  bounded continuation (`/goal`, `/loop`), transcript search and export,
  persona, skills and styles, attachments, streaming, history compaction,
  prompt context, the slash-command reference and accessibility.
  Owns: `assets/static/harness.html`, `src/server/{slash,sessions,prompts,
  compaction,attachments,style,self_manual,live_state,loop_runner,routes/core}.rs`,
  `src/common/pdf_text.rs`.
  Tests: `tests/chat_and_sessions.rs`, `tests/self_manual.rs`,
  `tests/chat_stream.rs`, `tests/chat_attachments.rs`, `tests/persona_controls.rs`,
  `tests/loop_runner.rs`, the `scripts/test-*.mjs` contract tests.
- [SPEND_AND_NOTIFICATIONS.md](SPEND_AND_NOTIFICATIONS.md): cost control and
  job notifications. The retained spend ledger, the **Analytics** view and
  `/analytics`, cloud-draft estimates with a per-call cap, and completion
  webhooks with their durable outbox, inspection and replay.
  Owns: `src/llm/spend.rs`, `src/server/{notifications,notification_outbox}.rs`,
  `src/server/routes/notifications.rs`, `/api/analytics/summary` in
  `src/server/routes/core.rs`. Tests: `tests/spend_ledger.rs`, `tests/analytics.rs`,
  `tests/notifications.rs`, `scripts/test-analytics.mjs`,
  `scripts/test-spend-dashboard.mjs`, `scripts/test-delivery-review.mjs`.

## Memory

- [MEMORY_GUIDE.md](MEMORY_GUIDE.md): the how-to for shared pinned notes,
  uploaded notes and the account-private structured store: gates, suggestions
  and review, manual saves, consolidation, retrieval and turning automation
  off. Recalled text is untrusted background and authorizes nothing.
  Owns: `src/server/{memory_notes,notes_corpus}.rs`, `src/server/routes/notes_corpus.rs`,
  the `/memory` branch of `src/server/slash.rs`.
  Tests: `tests/structured_memory.rs`, `tests/structured_memory_suggestions.rs`, `tests/panels.rs`.
- [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md): the contract behind that
  how-to: what ships, gates, prompt budget, write-time injection checks,
  suggestion, episode and proposal contracts, FTS retrieval, the at-rest threat
  model, HTTP surfaces, enable order and rollback.
  Owns: `src/server/structured_memory{,_auto,_consolidate,_fts,_suggest}.rs`,
  `src/server/routes/structured_memory.rs`, `src/common/private_sqlite.rs`.
  Tests: `tests/structured_memory.rs`, `tests/structured_memory_phase7.rs`,
  `tests/structured_memory_suggestions.rs`, `examples/structured_memory_eval.rs`.

## Coding pipeline

- [CODING_PIPELINE.md](CODING_PIPELINE.md): the optional, disarmed-by-default
  coding loop: clone, plan, patch, sandbox verify, human review, then commit,
  push and draft PR. Arming it, bounded edits, repository retrieval, cloud
  planner egress, offline Cargo verification and Git approval.
  Owns: `src/agentic/` (see its [README](../src/agentic/README.md)), `src/shim/mod.rs`,
  `src/server/{agent_jobs,agent_policy,agent_schedules,schemas}.rs`, `src/server/routes/agent.rs`.
  Tests: `tests/write_policy.rs`, `tests/git_approval.rs`, `tests/real_repo_loop.rs`,
  `tests/shim_and_agent_routes.rs`, `tests/agent_schedules.rs`, `tests/write_kill_switch.rs`.

## External tools and private access

- [MCP.md](MCP.md): both MCP surfaces, with a topology diagram. The outbound
  client (declared servers, stdio capabilities, lifecycle choices, the Windows
  trusted-server exception) and the inbound read-only memory gateway on its own
  listener (three `memory:read` tools, machine keys, revocation, bounds).
  Owns: `src/server/{mcp,mcp_server,mcp_server_config,mcp_keys}.rs`,
  `src/common/{mcp,mcp_policy,mcp_worker,sandbox_wrap,windows_job,tool_broker}.rs`,
  the `mcp-key` command in `src/main.rs`. Tests: `tests/mcp_client.rs`,
  `tests/mcp_lifecycle.rs`, `tests/mcp_windows.rs`, `tests/mcp_server.rs`,
  fixture `scripts/mcp-fixture.py`.
- [netconnect.md](netconnect.md): fail-closed LAN observation. Every gate
  ships `false` and `allowed_cidrs` empty; covers the gate logic, the `status`,
  `devices` and `diag` CLI, `/net`, the panel and the budgeted diagnostics
  dialer. Read it before arming anything.
  Owns: `src/netconnect/` (see its [README](../src/netconnect/README.md)),
  `src/server/routes/panels.rs`. Tests: `tests/netconnect_*.rs`,
  `.github/workflows/netconnect.yml`, `scripts/netconnect-syscall-proof.sh`.

## Accounts, trust and operations

- [SECURE_RESEARCH.md](SECURE_RESEARCH.md): a home's trust posture and
  permitted web. Accounts and roles, API Keys, session ownership, HTTPS and
  native trust, terminal access, web permissions, and search, fetch and
  research.
  Owns: `src/server/{guards,headers,transport,env_keys,chat_web}.rs`, every
  `src/server/web_*.rs` (policy, provider, search, Google, Tavily, Firecrawl,
  research, ledger), `src/common/{authn,auth_store,local_tls,credential_store}.rs`.
  Tests: `tests/auth_guards.rs`, `tests/secure_portal.rs`, `tests/security_headers.rs`,
  `scripts/test-first-run-password.mjs`, `tests/owner_isolation.rs`, `tests/chat_web.rs`, `tests/web_research.rs`,
  `tests/web_phase2_redteam.rs`.
- [WEB_DECISION_ROUTER.md](WEB_DECISION_ROUTER.md): the design record behind
  goal and loop web turns: provider eligibility, the five routing stages, the
  32K starting profile, acceptance cases and measured runtimes. The README
  carries the one-page summary;
  [#772](https://github.com/cgfixit/CG-agent-harness/issues/772) tracks the
  open phases.
  Owns: `src/server/{chat_web,web_provider,web_google,web_tavily,web_index}.rs`,
  `src/server/chat_web/evidence.rs`, `src/server/loop_runner.rs`.
  Tests: `tests/chat_web.rs`, `tests/loop_runner.rs`, `tests/retrieval_ranking.rs`.
- [DESKTOP.md](DESKTOP.md): the universal macOS app that owns its bundled
  backend in a WKWebView: install and launch, browser opening, notifications,
  ownership and ports, Finder setup, recovery, rebuild and verify. Shipped
  inside the app's `Resources/`.
  Owns: `desktop/` (see its [README](../desktop/README.md)), `src/server/desktop.rs`,
  `scripts/package-desktop.sh`, `scripts/verify-desktop-bundle.sh`.
  Tests: `scripts/test-desktop-backend.py`, `.github/workflows/desktop.yml`.
- [PROCESS_LIFECYCLE.md](PROCESS_LIFECYCLE.md): how child processes are run,
  bounded and killed: the 4 MiB capture ceiling, cooperative cancellation,
  best-effort descendant cleanup, child environments, MCP stdio lifecycle
  policy, and desktop ownership and recovery.
  Owns: `src/common/process.rs`, `src/common/process/unix.rs`,
  `src/common/{child_env,mcp,mcp_worker,sandbox_wrap,windows_job}.rs`,
  `src/server/desktop.rs`, `desktop/src/backend.rs`.
  Tests: `tests/process_lifecycle.rs`, `tests/process_writer_outcome.rs`,
  `tests/child_env.rs`.
- [GITHOOKS.md](GITHOOKS.md): the `pre-commit` and `pre-push` gate for
  secrets, private data and ask-first actions. A speed bump; CI secret scanning
  and the `main` ruleset are the controls.
  Owns: [.githooks/](../.githooks/README.md), `scripts/ensure-githooks.sh`.
  Checked by: `.github/workflows/gitleaks.yml`, `.github/workflows/script-lint.yml`.

## Reference and maintenance

- [API_ROUTES.md](API_ROUTES.md): the HTTP route inventory grouped by feature.
  `REGISTERED_PATHS` in `src/server/routes/mod.rs` is authoritative.
  Owns: [src/server/routes/](../src/server/routes/README.md). Tests:
  `tests/smoke_wired_count.rs`, `tests/panels.rs`, the `routes/mod.rs` unit tests.
- [DEPENDENCIES.md](DEPENDENCIES.md): maintenance for the two Cargo crates,
  their toolchain pins (1.89 backend, 1.90 desktop) and `deny.toml` policies:
  runtime crates, retained constraints, how to check and synchronize, and the
  last dated sync.
  Owns: `Cargo.toml`, `Cargo.lock`, `deny.toml`, `rust-toolchain.toml`, their
  `desktop/` twins, [.cargo/config.toml](../.cargo/README.md). Checked by:
  `.github/workflows/dependency-advisories.yml`, the `ci.yml` deny job, the `dep-sync` skill.
- [RELEASING.md](RELEASING.md): the scheduled `release.yml` workflow (08:17 and
  20:17 UTC): tree comparison, the Bundle gate, patch versions, publication,
  manual runs and previews, and verification limits.
  Owns: `.github/workflows/{release,bundle,desktop}.yml`, `scripts/release-plan.py`,
  `scripts/package-release.sh`. Tests: `scripts/test-release-plan.py`.

## Folders

- `bakeoff/`: a dated, measured local-model bake-off report that picks no
  winner; dated evidence otherwise belongs in PR bodies.
- [`learning/`](learning/README.md): operator reading material. Agents never
  open it unless the operator asks.
- [`screenshots/`](screenshots/README.md): captures indexed by series, plus
  `demo/` recordings. Record hashes, platform and fixture-versus-live in the
  adding PR; add no Markdown there.

## How these docs are kept

`tests/invariant_guard.rs` enforces `DOCS_BUDGET`, group caps and the 150-word
folder-README default; the editing, sync and cap-lowering rules are in
[AGENTS.md](../AGENTS.md#docs-policy).
