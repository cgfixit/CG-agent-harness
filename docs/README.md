# docs

Operator and contributor guides, one topic per file. The root
[README.md](../README.md#documentation) table is the short entry point; this
page describes every file, the feature it maps to, the code that owns it and
the tests that lock it. Agents open a file here only when a root file links it
and the task needs it; `learning/` is never read unless the operator asks.

## Start here

- **New operator, fresh machine:** [INSTALL.md](INSTALL.md), then
  [MODELS.md](MODELS.md) and [CONSOLE.md](CONSOLE.md).
  Keep [TROUBLESHOOTING.md](TROUBLESHOOTING.md) open.
- **Turning on an optional feature** (each ships off): [MEMORY_GUIDE.md](MEMORY_GUIDE.md),
  [CODING_PIPELINE.md](CODING_PIPELINE.md), [MCP_CLIENT.md](MCP_CLIENT.md),
  [MCP_SERVER.md](MCP_SERVER.md), [netconnect.md](netconnect.md). Read the
  owning [INVARIANTS.md](../INVARIANTS.md) section first; [AGENTS.md](../AGENTS.md#the-map)
  maps features to sections.
- **Contributor or agent:** [AGENTS.md](../AGENTS.md) and [INVARIANTS.md](../INVARIANTS.md)
  at the root, then [API_ROUTES.md](API_ROUTES.md), [PROCESS_LIFECYCLE.md](PROCESS_LIFECYCLE.md),
  [DEPENDENCIES.md](DEPENDENCIES.md), [GITHOOKS.md](GITHOOKS.md) and
  [RELEASING.md](RELEASING.md). Every source folder has a `README.md` indexing
  its files, starting at [src/](../src/README.md) and [tests/](../tests/README.md).

Some headings keep a number (`## 2. Prerequisites`, `## 7. Chat, soul, skills
and goals`): that is the order of the original single-file manual, kept so old
links resolve. Truth order for every claim is code, then
[config.default.yaml](../assets/config.default.yaml), then INVARIANTS.md.

## Setup, models and recovery

- [INSTALL.md](INSTALL.md): the automated macOS installer and launcher,
  prerequisites (hardware, Xcode tools, Rust, Ollama, optional GitHub CLI),
  getting the code, building the standalone backend or the macOS app, the first
  run and browser access to the app's server. Covers where the home and optional credentials
  live, which settings take effect where, supported managed keys, retained jobs
  and runs, and update, backup, rollback and uninstall. Use it to take a fresh
  machine to a verified console.
  Owns: `src/main.rs`, `src/common/{home,config,credential_store}.rs`,
  `src/server/env_keys.rs`, `scripts/verify-local.sh`, `scripts/{install,start}-macos.sh`,
  `.github/workflows/ci.yml`.
  Tests: `tests/common_layer.rs`, `tests/contract_locks.rs`, `tests/windows_private_credentials.rs`.
- [MODELS.md](MODELS.md): inventory the local Ollama install and select an
  installed tag for chat and web research. Explains the 32768-token window the
  seeded web budgets need, how to pin it with a derived local tag, and the
  exact-model diagnostics and optional fallback. Read it before the first chat.
  Owns: `src/llm/{ollama,inventory,profile,backend,cloud_chat}.rs`, `src/server/model_limits.rs`,
  `scripts/qwen3.8-27b-mlx-cg.Modelfile`, `scripts/smoke-ollama.sh`. Tests: `tests/ollama_manage.rs`,
  `tests/model_readiness.rs`, `tests/model_completion.rs`.
- [TROUBLESHOOTING.md](TROUBLESHOOTING.md): one symptom, likely cause and fix
  table. Rows cover missing spend or analytics, webhooks that never arrive,
  bounded MCP diagnostics, slash commands unknown to older builds, coding-loop
  file reads, session isolation and memory gates. Many fixes start by checking
  the installed build's `Resources/COMMIT`. Owns nothing; each row links the
  guide that does.

## Console and daily use

- [CONSOLE.md](CONSOLE.md): the main operator reference for the chat UI.
  Sessions and bounded continuation (`/goal`, `/loop`), transcript search and
  export, soul and persona editing, runtime skills, response styles, tool and
  connector visibility, text and DOCX attachments, streaming and cancellation,
  and local history compaction. Holds the slash-command quick reference and
  keyboard, zoom and screen-reader behavior.
  Owns: `assets/static/harness.html`, `src/server/{slash,sessions,prompts,
  compaction,attachments,style,routes/core}.rs`. Tests: `tests/chat_and_sessions.rs`,
  `tests/chat_stream.rs`, `tests/chat_attachments.rs`, `tests/persona_controls.rs`,
  `scripts/test-review-reset.mjs`, `scripts/test-header-buttons.mjs`, the other
  `scripts/test-*.mjs` contract tests.
- [SPEND_AND_NOTIFICATIONS.md](SPEND_AND_NOTIFICATIONS.md): the retained spend
  ledger, the **Analytics** view and `/analytics` (tokens and cost, sessions and
  coding runs by role, with API and compatibility notes), cloud-draft estimates
  with a per-call cap, and completion webhooks.
  Covers build requirements, reading partial ledgers, configuring a destination,
  the durable outbox and recovery, and inspecting or replaying deliveries without
  rerunning a job. Use it for cost control and job notifications.
  Owns: `src/llm/spend.rs`, `src/server/{notifications,notification_outbox}.rs`,
  `src/server/routes/notifications.rs`, `/api/analytics/summary` in
  `src/server/routes/core.rs`. Tests: `tests/spend_ledger.rs`, `tests/analytics.rs`,
  `tests/notifications.rs`, `scripts/test-analytics.mjs`,
  `scripts/test-spend-dashboard.mjs`, `scripts/test-delivery-review.mjs`.

## Memory

- [MEMORY_GUIDE.md](MEMORY_GUIDE.md): the how-to for both memory systems,
  shared pinned notes and the account-private structured store. Walks through
  gates, enabling an existing home, automatic suggestions and review, manual
  saves with a reason, episode consolidation, retrieval and turning automation
  off. Recalled text is untrusted background and authorizes nothing.
  Owns: `src/server/memory_notes.rs`, the `/memory` branch of `src/server/slash.rs`,
  and the structured-memory modules below from the operator's side.
  Tests: `tests/structured_memory.rs`, `tests/structured_memory_suggestions.rs`, `tests/panels.rs`.
- [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md): the contract behind that
  how-to. Defines what ships and what does not, gates, prompt budget, write-time
  injection checks, suggestion and episode and proposal contracts, FTS retrieval,
  consolidation, the at-rest threat model, HTTP surfaces, the evaluation corpus,
  enable order, rollback and verification. Read it before changing memory code.
  Owns: `src/server/structured_memory{,_auto,_consolidate,_fts,_suggest}.rs`,
  `src/server/routes/structured_memory.rs`, `src/common/private_sqlite.rs`.
  Tests: `tests/structured_memory.rs`, `tests/structured_memory_phase7.rs`,
  `tests/structured_memory_suggestions.rs`, `examples/structured_memory_eval.rs`.

## Coding pipeline

- [CODING_PIPELINE.md](CODING_PIPELINE.md): the optional, disarmed-by-default
  coding loop: clone, plan, patch, sandbox verify, human review, then commit,
  push and draft PR. Covers arming it (GitHub CLI, config, console, staging a
  session goal, the kill switch), bounded edits, repository retrieval, cloud
  planner egress, offline Cargo verification, git approval and the isolation
  check. Every write gate ships closed.
  Owns: `src/agentic/` (see its [README](../src/agentic/README.md)), `src/shim/mod.rs`,
  `src/server/{agent_jobs,agent_policy,agent_schedules,schemas}.rs`, `src/server/routes/agent.rs`.
  Tests: `tests/write_policy.rs`, `tests/git_approval.rs`, `tests/real_repo_loop.rs`,
  `tests/shim_and_agent_routes.rs`, `tests/agent_schedules.rs`, `tests/write_kill_switch.rs`.

## External tools and private access

- [MCP_CLIENT.md](MCP_CLIENT.md): the outbound MCP client. Declared servers,
  `mcp.enabled`, broker allowlist and literal `confirm: true`; migrating stdio
  declarations with capabilities, `env:NAME` references resolved at startup,
  lifecycle choices, the Windows trusted-server exception, and troubleshooting.
  `/loop` never gets MCP tools. Admission policy is `src/common/reference_loop.rs`; it does not spawn a connector.
  Owns: `src/server/mcp.rs`, `src/common/{mcp,mcp_policy,mcp_worker,sandbox_wrap,
  windows_job,tool_broker}.rs`. Tests: `tests/mcp_client.rs`, `tests/mcp_lifecycle.rs`,
  `tests/mcp_windows.rs`, fixture `scripts/mcp-fixture.py`.
- [MCP_SERVER.md](MCP_SERVER.md): the inbound, read-only memory gateway that
  runs in the harness process on its own listener. Lists the three
  `memory:read` tools, the deliberate activation steps (private `0700` home),
  keys, isolation and revocation, bounds and failures, and why remote exposure
  needs a separate reviewed change.
  Owns: `src/server/{mcp_server,mcp_server_config,mcp_keys}.rs`, the `mcp-key`
  command in `src/main.rs`. Tests: `tests/mcp_server.rs`.
- [netconnect.md](netconnect.md): fail-closed LAN observation. Every gate
  ships `false` and `allowed_cidrs` empty; explains `flag_is_true`,
  `tier_enabled` and `tier_may_run`, the `status`, `devices` and `diag` CLI,
  `/net` and its aliases, the panel, tools, trust boundaries, troubleshooting,
  the tiers in this build and the budgeted diagnostics dialer. Read it before
  arming anything.
  Owns: `src/netconnect/` (see its [README](../src/netconnect/README.md)),
  `src/server/routes/panels.rs`. Tests: `tests/netconnect_*.rs`,
  `.github/workflows/netconnect.yml`, `scripts/netconnect-syscall-proof.sh`.

## Accounts, trust and operations

- [SECURE_RESEARCH.md](SECURE_RESEARCH.md): accounts and roles (first login,
  password replacement, API Keys, account storage, session ownership), HTTPS and
  native trust, terminal access, web permissions, and search, fetch and research.
  Closes with the security boundaries and reproducible evidence. The reference
  for a home's trust posture and permitted web.
  Owns: `src/server/{guards,headers,transport,env_keys,web_policy,web_search,
  web_research,chat_web}.rs`, `src/common/{authn,auth_store,local_tls,credential_store}.rs`.
  Tests: `tests/auth_guards.rs`, `tests/secure_portal.rs`, `tests/security_headers.rs`,
  `tests/owner_isolation.rs`, `tests/chat_web.rs`, `tests/web_research.rs`,
  `tests/web_phase2_redteam.rs`.
- [DESKTOP.md](DESKTOP.md): the universal macOS app that owns its bundled
  backend in a WKWebView. Install and launch, notifications, home and
  credentials, ownership, ports and security boundaries, Finder setup, closing
  and recovery, rebuild and verify, and update or uninstall. Shipped inside the
  app's `Resources/`.
  Owns: `desktop/` (see its [README](../desktop/README.md)), `src/server/desktop.rs`,
  `scripts/package-desktop.sh`, `scripts/verify-desktop-bundle.sh`.
  Tests: `scripts/test-desktop-backend.py`, `.github/workflows/desktop.yml`.
- [PROCESS_LIFECYCLE.md](PROCESS_LIFECYCLE.md): how child processes are run,
  bounded and killed. The 4 MiB capture ceiling, cooperative cancellation,
  process-group signaling, best-effort macOS descendant cleanup, child
  environments, MCP stdio diagnostics and lifecycle policy, and desktop
  ownership and recovery.
  Owns: `src/common/process.rs`, `src/common/process/unix.rs`,
  `src/common/{child_env,mcp,mcp_worker,sandbox_wrap,windows_job}.rs`,
  `src/server/desktop.rs`, `desktop/src/backend.rs`.
  Tests: `tests/process_lifecycle.rs`, `tests/process_writer_outcome.rs`,
  `tests/child_env.rs`.
- [CONFIG_RELOAD.md](CONFIG_RELOAD.md): **Reload limits**,
  `POST /api/config/reload` and SIGHUP. Lists the 23 reloadable keys, what a
  reload preserves, why tightening a ceiling can return 429 at once, and refusal and
  recovery. Everything else, including MCP declarations, needs a restart.
  Owns: `RELOADABLE` in `src/server/config_reload.rs`. Tests: `tests/secure_portal.rs`.
- [GITHOOKS.md](GITHOOKS.md): the `pre-commit` and `pre-push` gate for
  secrets, private data and ask-first actions. Install once per clone, what runs
  when, operator-only overrides, privacy setup, what a hook cannot check, and
  reuse elsewhere. A speed bump; CI secret scanning and the `main` ruleset are
  the controls.
  Owns: [.githooks/](../.githooks/README.md), `scripts/ensure-githooks.sh`.
  Checked by: `.github/workflows/gitleaks.yml`, `.github/workflows/script-lint.yml`.

## Reference and maintenance

- [API_ROUTES.md](API_ROUTES.md): the HTTP route inventory grouped by feature,
  from console and accounts through chat, memory, web research, MCP, the coding
  agent and webhooks. Every route sits behind the guard chain. `REGISTERED_PATHS`
  in `src/server/routes/mod.rs` is authoritative.
  Owns: [src/server/routes/](../src/server/routes/README.md). Tests:
  `tests/smoke_wired_count.rs`, `tests/panels.rs`, the `routes/mod.rs` unit tests.
- [DEPENDENCIES.md](DEPENDENCIES.md): maintenance for the two Cargo crates,
  their toolchain pins (1.88 backend, 1.90 desktop) and `deny.toml` policies.
  A runtime crate table with purposes and features, the retained constraints
  (dependency watches), how to check and synchronize, and dated audits.
  Owns: `Cargo.toml`, `Cargo.lock`, `deny.toml`, `rust-toolchain.toml`, their
  `desktop/` twins, [.cargo/config.toml](../.cargo/README.md). Checked by:
  `.github/workflows/advisories.yml`, the `ci.yml` deny job, the `dep-sync` skill.
- [RELEASING.md](RELEASING.md): the scheduled `release` workflow (08:17 and
  20:17 UTC). How the tree is compared with the latest release, the Bundle
  gate, patch version selection, draft-then-Latest publication, changing the
  timing, running or previewing by hand, and verification limits.
  Owns: `.github/workflows/{release,bundle,desktop}.yml`, `scripts/release-plan.py`,
  `scripts/package-release.sh`. Tests: `scripts/test-release-plan.py`.

## Folders

- `bakeoff/`: a dated, measured local-model bake-off report that picks no
  winner. Owner-exempt from the group word cap; dated evidence otherwise
  belongs in PR bodies.
- [`learning/`](learning/README.md): operator reading material (Rust and AI
  design PDFs). Agents never open it unless the operator asks.
- [`screenshots/`](screenshots/README.md): flat captures indexed by series in
  their own README. Record hashes, platform and fixture-versus-live in the
  adding PR; add no Markdown there.

## How these docs are kept

Every file has a word cap in `tests/invariant_guard.rs` (`DOCS_BUDGET`); the
guard also caps each group and refuses a cap that leaves room to regrow. Edit
the section that owns a topic rather than adding a file; when a file goes,
delete every link to it in the same PR. Folder `README.md` files index their
folder and link here for the full doc. The `doc-sync` and `dep-sync` skills
(see [AGENTS.md](../AGENTS.md#docs-policy)) rewrite stale sections against the
code; a sync that only adds words is not done.
