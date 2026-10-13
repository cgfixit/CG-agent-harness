# docs

Operator and contributor guides, one topic per file. The root
[README.md](../README.md#documentation) table is the short entry point; this
page describes every file and the feature it maps to. Agents open a file here
only when a root file links it and the task needs it; `learning/` is never read
unless the operator asks. Every file has a word cap in `tests/invariant_guard.rs`
(`DOCS_BUDGET`); edit the owning section rather than adding files, and delete
every link when a file goes.

## Setup, models and recovery

- [INSTALL.md](INSTALL.md): prerequisites (hardware, Xcode tools, Rust, Ollama,
  optional GitHub CLI), getting the code, building the standalone backend or the
  macOS app, and the first run. Covers where the home and optional credentials
  live, which settings take effect where, supported managed keys, retained jobs
  and runs, and update, backup, rollback and uninstall. Use it to take a fresh
  machine to a verified console.
- [MODELS.md](MODELS.md): inventory the local Ollama install and select an
  installed tag for chat and web research. Explains the 32768-token window the
  seeded web budgets need, how to pin it with a derived local tag, and the
  exact-model diagnostics and optional fallback. Read it before the first chat.
- [TROUBLESHOOTING.md](TROUBLESHOOTING.md): one symptom, likely cause and fix
  table. Rows cover missing spend or analytics, webhooks that never arrive,
  bounded MCP diagnostics, slash commands unknown to older builds, coding-loop
  file reads, session isolation and memory gates. Many fixes start by checking
  the installed build's `Resources/COMMIT`.

## Console and daily use

- [CONSOLE.md](CONSOLE.md): the main operator reference for the chat UI.
  Sessions and bounded continuation (`/goal`, `/loop`), transcript search and
  export, soul and persona editing, runtime skills, response styles, tool and
  connector visibility, text and DOCX attachments, streaming and cancellation,
  and local history compaction. Holds the slash-command quick reference.
- [USER_MANUAL.md](USER_MANUAL.md): short orientation for operators. Explains
  which staged reviews logout and session changes discard, keyboard, zoom and
  screen-reader behavior, and where spend, notifications, limit reload, memory
  and analytics live. Links the deeper guides rather than repeating them.
- [ANALYTICS.md](ANALYTICS.md): the **Analytics** view and `/analytics`.
  Describes the tokens-and-cost, sessions and coding-run sections, who may read
  them by role, owner filtering, and the API and compatibility notes. Use it to
  audit usage without mistaking missing data for zero.
- [SPEND_AND_NOTIFICATIONS.md](SPEND_AND_NOTIFICATIONS.md): the retained spend
  ledger, cloud-draft estimates with a per-call cap, and completion webhooks.
  Covers build requirements, reading partial ledgers, configuring a destination,
  the durable outbox and recovery, and inspecting or replaying deliveries without
  rerunning a job. Use it for cost control and job notifications.

## Memory

- [MEMORY_GUIDE.md](MEMORY_GUIDE.md): the how-to for both memory systems,
  shared pinned notes and the account-private structured store. Walks through
  gates, enabling an existing home, automatic suggestions and review, manual
  saves with a reason, episode consolidation, retrieval and turning automation
  off. Recalled text is untrusted background and authorizes nothing.
- [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md): the contract behind that
  how-to. Defines what ships and what does not, gates, prompt budget, write-time
  injection checks, suggestion and episode and proposal contracts, FTS retrieval,
  consolidation, the at-rest threat model, HTTP surfaces, the evaluation corpus,
  enable order, rollback and verification. Read it before changing memory code.

## Coding pipeline

- [CODING_PIPELINE.md](CODING_PIPELINE.md): the optional, disarmed-by-default
  coding loop: clone, plan, patch, sandbox verify, human review, then commit,
  push and draft PR. Covers arming it (GitHub CLI, config, console, staging a
  session goal, the kill switch), bounded edits, repository retrieval, cloud
  planner egress, offline Cargo verification, git approval and the isolation
  check. Every write gate ships closed.

## External tools and private access

- [MCP_CLIENT.md](MCP_CLIENT.md): the outbound MCP client. Declared servers,
  `mcp.enabled`, broker allowlist and literal `confirm: true`; migrating stdio
  declarations with capabilities, `env:NAME` references resolved at startup,
  lifecycle choices, the Windows trusted-server exception, and troubleshooting.
  `/loop` never gets MCP tools.
- [MCP_SERVER.md](MCP_SERVER.md): the inbound, read-only memory gateway that
  runs in the harness process on its own listener. Lists the three
  `memory:read` tools, the deliberate activation steps (private `0700` home),
  keys, isolation and revocation, bounds and failures, and why remote exposure
  needs a separate reviewed change.
- [netconnect.md](netconnect.md): fail-closed LAN observation. Every gate
  ships `false` and `allowed_cidrs` empty; explains `flag_is_true`,
  `tier_enabled` and `tier_may_run`, the `status` and `devices` CLI, `/net`
  and its aliases, the panel, tools, trust boundaries, troubleshooting and the
  tiers in this build. Read it before arming anything.

## Accounts, trust and operations

- [SECURE_RESEARCH.md](SECURE_RESEARCH.md): accounts and roles (first login,
  password replacement, API Keys, account storage, session ownership), HTTPS and
  native trust, terminal access, web permissions, and search, fetch and research.
  Closes with the security boundaries and reproducible evidence. The reference
  for a home's trust posture and permitted web.
- [DESKTOP.md](DESKTOP.md): the universal macOS app that owns its bundled
  backend in a WKWebView. Install and launch, notifications, home and
  credentials, ownership, ports and security boundaries, Finder setup, closing
  and recovery, rebuild and verify, and update or uninstall. Shipped inside the
  app's `Resources/`.
- [PROCESS_LIFECYCLE.md](PROCESS_LIFECYCLE.md): how child processes are run,
  bounded and killed. The 4 MiB capture ceiling, cooperative cancellation,
  process-group signaling, best-effort macOS descendant cleanup, child
  environments, MCP stdio diagnostics and lifecycle policy, and desktop
  ownership and recovery.
- [CONFIG_RELOAD.md](CONFIG_RELOAD.md): **Reload limits**,
  `POST /api/config/reload` and SIGHUP. Lists the 23 reloadable keys, what a
  reload preserves, why tightening a ceiling can return 429 at once, and refusal and
  recovery. Everything else, including MCP declarations, needs a restart.
- [GITHOOKS.md](GITHOOKS.md): the `pre-commit` and `pre-push` gate for
  secrets, private data and ask-first actions. Install once per clone, what runs
  when, operator-only overrides, privacy setup, what a hook cannot check, and
  reuse elsewhere. A speed bump; CI secret scanning and the `main` ruleset are
  the controls.

## Reference and maintenance

- [API_ROUTES.md](API_ROUTES.md): the HTTP route inventory grouped by feature,
  from console and accounts through chat, memory, web research, MCP, the coding
  agent and webhooks. Every route sits behind the guard chain. `REGISTERED_PATHS`
  in `src/server/routes/mod.rs` is authoritative.
- [DEPENDENCIES.md](DEPENDENCIES.md): maintenance for the two Cargo crates,
  their toolchain pins (1.88 backend, 1.90 desktop) and `deny.toml` policies.
  A runtime crate table with purposes and features, the retained constraints
  (dependency watches), how to check and synchronize, and dated audits.
- [RELEASING.md](RELEASING.md): the scheduled `release` workflow (08:17 and
  20:17 UTC). How the tree is compared with the latest release, the Bundle
  gate, patch version selection, draft-then-Latest publication, changing the
  timing, running or previewing by hand, and verification limits.

## Folders

- `bakeoff/`: a dated, measured local-model bake-off report that picks no
  winner. Owner-exempt from the group word cap; dated evidence otherwise
  belongs in PR bodies.
- `learning/`: operator reading material (Rust and AI design PDFs). Agents
  never open it unless the operator asks.
- `screenshots/`: flat captures indexed by their own README. Record hashes,
  platform and fixture-versus-live in the adding PR; add no Markdown there.
