# AGENTS.md — CGagentHarness operating manual

Every coding agent follows this file. Exceptions to "never" need explicit
operator approval. Read [INVARIANTS.md](INVARIANTS.md) before touching core
paths: `src/shim/`, `src/server/{guards,headers,routes/mod}.rs`,
`src/common/authz.rs`, `src/agentic/{writer,workspace}.rs`,
`src/agentic/executor/sandbox.rs`, `assets/config.default.yaml`.

Never open `docs/learning/*` unless asked; open other `docs/*` pages only
when a root file links one the task needs.

## Where truth lives

1. Code. 2. `assets/config.default.yaml` (every tunable; read, never copied).
3. `INVARIANTS.md`. 4. This file. 5. `README.md`.
When prose contradicts code, fix the prose in the same PR; `lint-diff.py`
CG009 checks stated shipped values.

## The map

- `cgagentharness serve` is the loopback console (`src/server`); `account`,
  `web`, `tls` and `mcp-key` are its CLI faces. `agentic <action>`
  (`src/agentic`) runs only as a child the shim spawns, never in-process.
  `netconnect diag` alone dials, through the budgeted `Dialer`.
- Exit codes: `agentic`/`netconnect` `0` ok, `2` failed, `3` env/config, `4`
  refused (via the shim a non-zero child is HTTP 200 `ok=false`; shim
  failures 400/502/504, disabled-layer banner 409). `serve`, `account`,
  `web`, `tls`, `mcp-key` `1`; hidden `desktop`/`mcp-stdio-worker` `3`;
  `pdf-text-worker` `0` text/`2` refused/`3` environment; `browser`,
  `browser-verify` `0` passed/`2` failed/`3` bad args or config.
- State: `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`); coding writes stay in
  pipeline clones under `data/agentic/workspaces`. Shipped defaults seed new
  homes only; an existing `config.yaml` is never rewritten.

Read the feature's guide and `INVARIANTS.md` section first
([source index](src/README.md)):

| Changing | Read first / trap |
|----------|-------------------|
| Accounts, TLS, roles | [Secure portal](docs/SECURE_RESEARCH.md#accounts-and-roles), [native trust](docs/DESKTOP.md#ownership-ports-and-security-boundaries) |
| Search, fetch, providers | [Search](docs/SECURE_RESEARCH.md#google-search), [router](docs/WEB_DECISION_ROUTER.md): listings grant admin turns exact-URL reads (`web.search.result_reads`); composed wildcard URLs need a live Approve (`web.fetch_provenance`). |
| PDF text | [PDF text](docs/CONSOLE.md#pdf-text): parsed only in `pdf-text-worker`; only textless, over-limit or malformed PDFs reach Firecrawl. |
| `/browse` | [Browser use](docs/Browser-Use.md), [I6 edges](INVARIANTS.md#i6--process-isolation-between-the-console-and-the-agentic-pipeline): request-only origins; non-GET/HEAD held; every URL checked against the block list; takeover sends one held request (id, hash, `confirm`, `reason`). |
| Firecrawl, research, jobs | [Fetch and research](docs/SECURE_RESEARCH.md#fetch-discovery-and-research), [job contract](INVARIANTS.md#a-detached-run-cannot-outlive-its-gates): Firecrawl is opt-in, same-URL. |
| Provider keys | [Credential storage](docs/INSTALL.md#supported-managed-keys): an inherited environment value (even empty) wins. |
| Memory, notes | [Memory contract](docs/STRUCTURED_MEMORY.md#two-memory-systems): pinned notes, notes corpus and structured facts are distinct. |
| Coding pipeline, jobs, `/loop` | [Pipeline](docs/CODING_PIPELINE.md), [jobs](docs/INSTALL.md#retained-jobs-and-runs), [loops](docs/CONSOLE.md#71-sessions-and-bounded-chat-continuation): retrieval and publication have separate gates. |
| MCP, subprocesses | [MCP](docs/MCP.md), [lifetimes](docs/PROCESS_LIFECYCLE.md): process groups are not containment; the inbound gateway is read-only, owner-bound. |
| Models, compaction | [Models](docs/MODELS.md): never send `num_ctx`. |
| Spend, webhooks | [Spend/notifications](docs/SPEND_AND_NOTIFICATIONS.md): estimates are not charges. |
| Config reload | [Reloadable limits](docs/INSTALL.md#reload-limits-without-a-restart): `config_reload.rs::RELOADABLE`; authority needs a restart. |
| netconnect, `/diag`, self-manual | [netconnect](docs/netconnect.md), [diagnostics](docs/TROUBLESHOOTING.md): untrusted context, no added authority. |

## Traps

- **Never** make `src/{server,shim,llm,common,netconnect}` reference
  `crate::agentic`, or `src/agentic` reference server or shim (I6);
  `tests/invariant_guard.rs` catches aliased `use` too. Cross only through
  `src/shim` and the CLI whitelist.
- Keep shim actions `test`, `context`, `propose-skill`, `apply-skill`: no
  server caller, under operator review (2026-11-20).
- Config is strict by type. `flag_is_true`: only literal YAML `true` is on;
  quoted `"true"` and a missing key are off (except
  `config::MISSING_MEANS_ON`): a missing key is not the shipped value.
  `AppConfig::load` refuses malformed numbers (`FLOAT_KEYS` may be floats).
  The guard enforces literal-key accessors and `load` for operator text.
- Malformed input is refused at parsing, naming the field, never its value;
  missing means default. Leniency needs an operator decision.
- Write gates `agentic.enabled`, `deepagent_github.enabled` and
  `allow_git_write_tools` ship false. `confirm` is never defaulted; `reason`
  is never optional. Retrieval has its own gate
  (`agentic.deepagent_github.retrieval.enabled`, ships true).
- The browser never supplies a command: fixed argv only; free text is one
  `--opt=value` element or a temp file.
- Console asset contracts: `__CYCLAW_CSRF_TOKEN__`, `__CYCLAW_CSP_NONCE__`
  and the `X-CyClaw-CSRF` header stay verbatim; replies check
  `sessionRevision` before rendering.
- Operator machines set `GROK_API_KEY` and
  `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE`: tests never assert on the key (CI
  blanks it, `ANTHROPIC_API_KEY` and `DEEPAGENT_API_KEY`) and isolate the kill
  switch. Never flip `EXECUTION_ENABLED` or OR the switch to go green.
- Publish a live setting only after its save (`panels::commit_setting`);
  refuse before changing state.
- `[profile.dev.package."*"] opt-level=3` is load-bearing (scrypt).
- A weak-hash name used as a label ends its line with
  `DevSkim: ignore DS126858 because …`.
- Pinned egress reuses `common::dns::lookup_bounded` and
  `web_search::pinned_client_builder`; callers still authorize destinations;
  fixture bypasses need `test-support`. reqwest 0.13 trusts the OS store.
- Sidecar fixtures (`cgagentharness browser guards`, …) keep Chrome's
  sandbox on and refuse root: run them unprivileged.

## Quality bar

- Once per clone: `bash scripts/ensure-githooks.sh`.
- CI gate: `cargo fmt --all -- --check`, `cargo clippy --all-targets
  --all-features -- -D warnings`, `cargo test --all-targets`,
  `cargo deny check`. Rust 1.89 (desktop 1.90). `verify-local.sh` skips a
  missing cargo-deny: not deny evidence.
- Locally: lint (fmt, clippy, `python3 scripts/lint-diff.py`), then run the
  changed code and focused tests; full suites are CI's. Docs:
  `cargo test --test invariant_guard markdown_stays_on_the_docs_budget`;
  workflows: `scripts/lint-workflows.sh`.
- A green PR is not Windows or desktop evidence; `desktop-check.yml` runs
  only on a path-regex match.
- Changed a shipped default, refusal order or exit code? `rg` the tests;
  run every match.
- New routes: `routes/mod.rs::REGISTERED_PATHS`, `ROUTE_CAPS` (and
  `views.rs` if listed). New shim actions: `shim::ACTIONS` (and
  `JSON_ACTIONS`), `agentic/commands.rs::dispatch`, the guard's whitelist.
- `/api/agent/run`, `/api/agent/jobs` and schedules share `prepare_run`.
  MCP tools never attach to `/loop`.
- Prefer a `#[cfg(test)]` unit test beside a pure parser to a new
  integration test.
- PRs: draft, one concern, driver-prefixed branch **based on `main`**, title
  `[prefix] - Sentence`, template body (`scripts/check-pr-template.sh`);
  core-path changes need an invariant statement. Before publishing, refresh
  `origin/main` and open PRs; trial-merge overlaps, manual and doc caps too.
- Read the advisory `review gate` before merging. Never comment
  `@codex review` without operator approval.

## Rule enforcers

Enforce corrections at the highest level; add rows.

| Rule | Enforcer |
|------|-------------|
| Duplicated constants (never deduplicate), `/loop` limits | `duplicated_constants_still_agree` |
| Async fns: no SQLite, file or hash work (`off_worker`) | `async_fns_leave_blocking_…` |
| Assert messages print no secret-bearing value | `lint-diff.py` CG011 |
| Recheck policy, authority after waits | `owner_permits` (copies fail); calling: prose |

## Docs policy

Before pushing feature or core changes, run `python3 scripts/docs-impact.py`:
update the owning guides, or use its `Docs-Impact` trailer with a
source-backed reason.

Edit in place; only folder README indexes may be new (150 words unless
budgeted). Link shared facts; evidence goes in PRs, screenshots in
`docs/screenshots/`. Check `wc -w` against `DOCS_BUDGET` (1 MiB/file). Lower
trimmed caps; new rows or raises need approval and `// why:`. Remove deleted
docs from budgets and references (CG008).

Broad syncs: `doc-sync` and `dep-sync`. `cgfixit` reviews Markdown diffs and
[dependency watches](docs/DEPENDENCIES.md#retained-constraints) weekly.

Editing a `scripts/build-self-manual.py::INPUTS` source (this file too)?
Regenerate `assets/self/MANUAL.md`; commit it if it changed.

## Project skills

Start with `cgagentharness-project-guidance`; pick by task from the
[Codex](.codex/skills/README.md), [Claude](.claude/skills/README.md) or
[Copilot](.github/skills/README.md) index; each keeps its own inventory. Keep
operator-only skills and authority limits; check frontmatter, references,
budgets and runtime discovery. Releases: [release controls](docs/RELEASING.md).
