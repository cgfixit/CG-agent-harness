# AGENTS.md — CGagentHarness operating manual

Every coding agent follows this file. Exceptions to "never" require explicit
operator approval. Read [INVARIANTS.md](INVARIANTS.md) before touching a core path:
`src/shim/`, `src/server/{guards,headers}.rs`, `src/agentic/{writer,workspace}.rs`,
`src/agentic/executor/sandbox.rs`, `assets/config.default.yaml`.

Reading scope: never open `docs/learning/*` unless the operator asks. Open other
`docs/*` files only when a root file links the page and the task needs it.

## Where truth lives

1. Code. 2. `assets/config.default.yaml` (every tunable; read, never copied
elsewhere). 3. `INVARIANTS.md`. 4. This file. 5. `README.md`.
When prose contradicts code, fix the prose in the same PR.

## The map

```text
browser/webview -> server guards -> chat / web / memory
desktop shell -> verified backend sidecar -> same server
server -> shim --child CLI--> agentic -> owned clone / sandbox
server -> MCP runtime -> declared stdio children / SSE servers
```

- `cgagentharness serve` -> loopback HTTP/HTTPS console in `src/server`. Public
  `account` and `web` call that service; `tls` manages local certificates and
  `mcp-key` manages machine keys by filesystem authority.
- `cgagentharness agentic <action>` -> `src/agentic` (hidden; spawned by
  `src/shim` as a child process, never called in-process).
- `cgagentharness netconnect status|devices|diag` and `/net` -> `src/netconnect`.
  Only `diag` dials, through the budgeted `Dialer` in `connect.rs`.
- `agentic`/`netconnect` exit codes: `0` ok, `2` failed, `3` env/config,
  `4` write refused. Through the shim, a non-zero child exit is HTTP
  200 with `ok=false`; shim failures map to 400/502/504 and the disabled-layer
  banner to 409. Other subcommands (`serve`, `account`, `web`, `tls`,
  `mcp-key`) exit `1` on error.
- Application state: `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`); coding writes
  stay in pipeline-created clones under `data/agentic/workspaces`.
- Before changing a feature, read its guide below and owning `INVARIANTS.md`
  section. [Source index](src/README.md) locates implementations.

| Changing | Read first / trap |
|----------|-------------------|
| Accounts, TLS, ownership, auditor role | [Secure portal](docs/SECURE_RESEARCH.md#accounts-and-roles), [native trust](docs/DESKTOP.md#ownership-ports-and-security-boundaries) |
| Chat search/fetch, providers | [Search permissions](docs/SECURE_RESEARCH.md#google-search): Tavily requires explicit selection; `auto` ignores its key. Listings grant no page access. |
| Exact fetch fallback | [Firecrawl](docs/SECURE_RESEARCH.md#fetch-discovery-and-research): opt-in, sends URLs off-machine; same-URL results only, never discovery/crawl/research. |
| Web research/jobs | [Research](docs/SECURE_RESEARCH.md#fetch-discovery-and-research), [job contract](INVARIANTS.md#a-detached-run-cannot-outlive-its-gates): local model, separate discovery/synthesis deadlines; owner-bound jobs survive tabs, not restart. |
| Provider keys | [Credential storage](docs/INSTALL.md#supported-managed-keys): inherited environment (even empty) wins; saved SerpAPI/Tavily/Firecrawl keys apply next search/read. |
| Memory, notes corpus | [Memory contract](docs/STRUCTURED_MEMORY.md#two-memory-systems), [source owners](src/server/README.md): pinned notes, uploaded corpus and structured facts are distinct. |
| Clone reads, edits, Git approval | [Coding pipeline](docs/CODING_PIPELINE.md): retrieval and publication have separate gates. |
| Coding jobs, schedules, `/loop` | [Retained jobs](docs/INSTALL.md#retained-jobs-and-runs), [loops](docs/CONSOLE.md#71-sessions-and-bounded-chat-continuation) |
| Outbound MCP / subprocess cleanup | [MCP client](docs/MCP_CLIENT.md), [lifetime limits](docs/PROCESS_LIFECYCLE.md): capabilities select confinement; process groups are not strict containment. |
| Inbound MCP memory gateway | [MCP server](docs/MCP_SERVER.md): separate machine credentials, owner-bound read-only facts. |
| Ollama, context, compaction | [Models](docs/MODELS.md), [compaction](docs/CONSOLE.md#local-history-compaction): inspect live allocation; never send `num_ctx`. |
| Session search/export | [Transcripts](docs/CONSOLE.md#search-and-export-session-transcripts): owner-scoped, local only. |
| Spend, estimates, webhooks | [Spend/notifications](docs/SPEND_AND_NOTIFICATIONS.md): estimates are not charges; notifications grant no job authority. |
| Config reload | [Reload](docs/CONFIG_RELOAD.md): `src/server/config_reload.rs::RELOADABLE` lists reloadable limits; authority and `web.concurrency` require restart. |
| netconnect | [Gates/scope](docs/netconnect.md): tools inventory is not chat dispatch. |
| Self-manual, live state, `/diag` | [Prompt context](docs/CONSOLE.md#72-default-soul-effective-prompt-and-persona-editing), [diagnostics](docs/TROUBLESHOOTING.md): untrusted local context, no additional authority. |

## Traps

- **Never** make `src/{server,shim,llm,common,netconnect}` reference
  `crate::agentic`, and never make `src/agentic` reference server or shim (I6).
  `tests/invariant_guard.rs` fails if you do, including aliased `use` forms.
  Cross the boundary only through `src/shim` and the CLI whitelist.
- Duplicated on purpose, kept in sync by tests: `RUN_ID_PATTERN` (server
  `agent_policy` vs agentic `run_store`), the planner/check timeout and
  setup-budget constants (shim vs agentic), the iteration default. Never
  deduplicate them.
- Config is strict by type. Gates read through `flag_is_true`: only literal
  YAML `true` is on; quoted `"true"` and a missing key are off. A missing key
  is not the shipped value: several gates ship `true` (structured memory,
  Ollama warmup, auth, TLS). `AppConfig::load` rejects malformed shipped
  numeric values; `FLOAT_KEYS` permits finite numbers, otherwise unsigned integers.
  Use literal-key `AppConfig` accessors and load operator text through `load`;
  the invariant guard enforces both.
- Unspecified malformed-input handling defaults to strict: refuse at parsing,
  name the field, never its value, preserve missing-means-default, and record
  the choice in the PR. Lenient handling needs a recorded operator decision.
- Write gates `agentic.enabled`, `deepagent_github.enabled` and
  `deepagent_github.allow_git_write_tools` ship false. `confirm` is never
  defaulted on; `reason` is never optional on a write.
- Repository retrieval is gated by `agentic.deepagent_github.retrieval.enabled`
  (ships false), not the parent key.
- The browser never supplies a command: fixed argv only; free text is one
  `--opt=value` element or a temp file.
- The console asset stays verbatim: `__CYCLAW_CSRF_TOKEN__`,
  `__CYCLAW_CSP_NONCE__` and the `X-CyClaw-CSRF` header name are contractual.
- `GROK_API_KEY` on a developer machine is real: tests never assert on its
  presence, and CI blanks it with `ANTHROPIC_API_KEY` and `DEEPAGENT_API_KEY`.
- `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` on an operator machine is real:
  `cargo test` isolates it so later write gates are what fail. Never flip
  `EXECUTION_ENABLED` to false (or OR the kill switch) to make tests green.
- Server audit appends go to one bounded writer thread
  (`logging.audit_queue_lines`; 0 appends inline); SQLite work stays off async
  workers. Audit queue overflow drops lines; explicit flushes can wait.
- scrypt at n=2^17 is slow unoptimized; `[profile.dev.package."*"] opt-level=3`
  is load-bearing for test time.
- Reuse `common::dns::lookup_bounded` and `web_search::pinned_client_builder`
  for pinned egress; callers still authorize destinations. Fixture bypasses
  require `test-support`, absent from default release builds.

## Quality bar

- Once per clone, before committing: `bash scripts/ensure-githooks.sh`.
- CI merge gate: `cargo fmt --all -- --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-targets`, `cargo deny check`. Rust 1.88 is pinned for the
  backend; `desktop/` pins 1.90. `scripts/verify-local.sh` skips deny when
  cargo-deny is missing, so its green run is not deny evidence.
- Verifying a change locally: lint (fmt, clippy), then run the changed code
  (route, binary or function). Use focused tests when needed; for docs, run
  `cargo test --test invariant_guard markdown_stays_on_the_docs_budget`.
  Leave full suites to CI. Lint workflow edits with actionlint and zizmor.
- New routes: add to `src/server/routes/mod.rs::REGISTERED_PATHS` (and
  `views.rs` if the console lists them) or `/api/tools` reports them unwired.
- New shim actions: extend `shim::ACTIONS` (and `shim::JSON_ACTIONS` if it
  returns JSON), `agentic/commands.rs::dispatch`, and the invariant guard's
  whitelist assertion together.
- `/api/agent/run`, `/api/agent/jobs` and persisted schedules all go through
  `prepare_run`; never give one a path the others skip.
- MCP tools never attach to `/loop`; permitted read-only web tools can.
- Prefer a `#[cfg(test)] mod tests` unit test beside a pure parser or matcher
  over another integration test.
- PRs are draft, one concern, on a driver-prefixed branch (`claude/`, `codex/`,
  `grok/`, `kimi/`, `agent/` when unknown), **based on `main`**, title
  `[prefix] - Sentence`, body from `.github/PULL_REQUEST_TEMPLATE.md` (run
  `scripts/check-pr-template.sh` first). Touching a core path requires an
  explicit invariant statement in the body.
- Refresh `origin/main` and open PR heads/files/checks before publication;
  trial-merge overlapping changes, including generated manuals and doc caps.
- The advisory `review gate` check reports unresolved threads and running Codex
  reviews; it never fails CI. Read it before merging and re-run it after
  resolving the last thread. Never comment `@codex review` without the
  operator's approval.

## Docs policy

Edit the owning section; create no Markdown except folder `README.md` indexes.
Link instead of duplicating. Evidence belongs in PRs/issues; significant-change
screenshots in `docs/screenshots/`. `tests/invariant_guard.rs` enforces
`DOCS_BUDGET`: per-file/group word caps, no new root screenshots or docs PDFs,
1 MiB/file (8 MiB for screenshot videos). Check `wc -w` before Markdown commits,
including web edits. New rows or raised caps need operator approval and `// why:`.

A folder `README.md` without its own budget row has `FOLDER_README_WORDS`
(150) words: contents, consumers and links to full docs.

Deleting a doc requires removing its budget row and references in docs, skills,
indexes and packaging scripts; search its basename with `rg` first.

Sync, then trim. For multi-module PRs or a sync older than weeks, use `doc-sync`
(operator-typed in Claude) and `dep-sync`, or perform their work manually:
recheck described code, pins, locks and `deny.toml` against `origin/main`;
remove stale/duplicate guidance, dead links and retired dependency watches.
Lower each trimmed file's `DOCS_BUDGET` cap. Weekly, `cgfixit` reviews
`git log --since=1.week --stat -- '*.md'` and
[dependency watches](docs/DEPENDENCIES.md#retained-constraints).

After editing a `scripts/build-self-manual.py::INPUTS` source (including this
map), regenerate `assets/self/MANUAL.md`; commit it when the bytes change.

## Project skills

Repository guidance is separate from runtime `/api/skills`. Start with
`cgagentharness-project-guidance`, then the applicable task skill; neither
expands publication, merge or release authority.

- Codex: [.codex/skills/README.md](.codex/skills/README.md) indexes contract/config
  guards, write-policy probes, verification, optimization and macOS release work.
- Claude: [.claude/skills/README.md](.claude/skills/README.md) includes its separate
  invocation policies and Claude-only procedures. Preserve operator-only skills;
  `doc-sync` remains operator-typed in Claude.
- Copilot: [.github/skills/README.md](.github/skills/README.md) owns its inventory.

Keep operating rules here, contracts in `INVARIANTS.md`, mechanics in skills.
Verify each inventory and invocation policy independently. Skill edits require
frontmatter, path/symbol, command-filter and Markdown-budget checks; static
checks cannot prove runtime discovery or instruction reload in existing chats.
