# AGENTS.md — CGagentHarness operating manual

Every coding agent follows this file. Exceptions to "never" require explicit
operator approval. Read [INVARIANTS.md](INVARIANTS.md) before touching core paths:
`src/shim/`, `src/server/{guards,headers,routes/mod}.rs`, `src/common/authz.rs`,
`src/agentic/{writer,workspace}.rs`, `src/agentic/executor/sandbox.rs`,
`assets/config.default.yaml`.

Reading scope: never open `docs/learning/*` unless the operator asks. Open other
`docs/*` files only when a root file links the page and the task needs it.

## Where truth lives

1. Code. 2. `assets/config.default.yaml` (every tunable; read, never copied
elsewhere). 3. `INVARIANTS.md`. 4. This file. 5. `README.md`.
When prose contradicts code, fix the prose in the same PR; `lint-diff.py`
CG009 checks stated shipped values.

## The map

- `cgagentharness serve` is the loopback console (`src/server`); `account`,
  `web`, `tls` and `mcp-key` are its CLI faces. `agentic <action>`
  (`src/agentic`) runs only as a child the shim spawns, never in-process.
  `netconnect diag` alone dials, through the budgeted `Dialer`.
- Exit codes: `agentic`/`netconnect` `0` ok, `2` failed, `3` env/config, `4`
  refused (through the shim a non-zero child is HTTP 200 `ok=false`; shim
  failures 400/502/504, the disabled-layer banner 409). `serve`, `account`,
  `web`, `tls`, `mcp-key` exit `1`; hidden `desktop`/`mcp-stdio-worker` `3`;
  `pdf-text-worker` `0` text, `2` refused, `3` environment; `browser` and
  `browser-verify` `0` passed, `2` failed, `3` bad arguments or config.
- State lives in `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`); coding writes stay
  in pipeline clones under `data/agentic/workspaces`. Shipped defaults seed new
  homes only: an existing `config.yaml` is never rewritten.
- Read a feature's guide and owning `INVARIANTS.md` section first;
  [Source index](src/README.md) locates code.

| Changing | Read first / trap |
|----------|-------------------|
| Accounts, TLS, roles | [Secure portal](docs/SECURE_RESEARCH.md#accounts-and-roles), [native trust](docs/DESKTOP.md#ownership-ports-and-security-boundaries) |
| Chat search/fetch, providers | [Search](docs/SECURE_RESEARCH.md#google-search), [router](docs/WEB_DECISION_ROUTER.md): listings grant admin-turn exact-URL reads (`web.search.result_reads`; arXiv abstract → its PDF). A composed wildcard-rule URL needs Approve in a live admin turn (`web.fetch_provenance`). |
| PDF text | [PDF text](docs/CONSOLE.md#pdf-text): parsed only in the `pdf-text-worker` child; only textless, over-limit or malformed PDFs may escalate to Firecrawl. |
| Browser tasks (`/browse`) | [Browser use](docs/Browser-Use.md), [I6 edges](INVARIANTS.md#i6--process-isolation-between-the-console-and-the-agentic-pipeline): origins come only from the request; non-GET/HEAD requests are held; the host judges every URL against the block list. |
| Firecrawl, research, jobs | [Fetch and research](docs/SECURE_RESEARCH.md#fetch-discovery-and-research), [job contract](INVARIANTS.md#a-detached-run-cannot-outlive-its-gates): Firecrawl is opt-in, same-URL only. |
| Provider keys | [Credential storage](docs/INSTALL.md#supported-managed-keys): an inherited environment value (even empty) wins. |
| Memory, notes | [Memory contract](docs/STRUCTURED_MEMORY.md#two-memory-systems): pinned notes, notes corpus and structured facts are distinct. |
| Coding pipeline, jobs, `/loop` | [Pipeline](docs/CODING_PIPELINE.md), [jobs](docs/INSTALL.md#retained-jobs-and-runs), [loops](docs/CONSOLE.md#71-sessions-and-bounded-chat-continuation): retrieval and publication have separate gates. |
| MCP, subprocesses | [MCP](docs/MCP.md), [lifetimes](docs/PROCESS_LIFECYCLE.md): process groups are not strict containment; the inbound gateway is read-only and owner-bound. |
| Models, compaction | [Models](docs/MODELS.md): never send `num_ctx`. |
| Spend, webhooks | [Spend/notifications](docs/SPEND_AND_NOTIFICATIONS.md): estimates are not charges. |
| Config reload | [Reloadable limits](docs/INSTALL.md#reload-limits-without-a-restart): `config_reload.rs::RELOADABLE`; authority needs a restart. |
| netconnect, `/diag`, self-manual | [netconnect](docs/netconnect.md), [diagnostics](docs/TROUBLESHOOTING.md): untrusted context, no added authority. |

## Traps

- **Never** make `src/{server,shim,llm,common,netconnect}` reference
  `crate::agentic`, or `src/agentic` reference server or shim (I6);
  `tests/invariant_guard.rs` catches aliased `use` too. Cross only through
  `src/shim` and the CLI whitelist.
- Under operator review (2026-11-20): shim actions `test`, `context`,
  `propose-skill`, `apply-skill` lack a server caller; keep them.
- Config is strict by type. Gates read through `flag_is_true`: only literal
  YAML `true` is on; quoted `"true"` and a missing key are off, except the
  recorded `config::MISSING_MEANS_ON` keys. A missing key is not the shipped
  value. `AppConfig::load` rejects malformed shipped numbers (`FLOAT_KEYS` may
  be finite floats). Use literal-key accessors and load operator text through
  `load`; the guard enforces both.
- Unspecified malformed input is refused at parsing, naming the field, never
  its value; missing still means default. Leniency needs an operator decision.
- Write gates `agentic.enabled`, `deepagent_github.enabled` and
  `allow_git_write_tools` ship false. `confirm` is never defaulted on;
  `reason` is never optional. Repository retrieval has its own gate
  (`agentic.deepagent_github.retrieval.enabled`, ships true).
- The browser never supplies a command: fixed argv only; free text is one
  `--opt=value` element or a temp file.
- The console asset stays verbatim: `__CYCLAW_CSRF_TOKEN__`,
  `__CYCLAW_CSP_NONCE__` and the `X-CyClaw-CSRF` header are contractual.
- `GROK_API_KEY` and `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` are real on
  operator machines: tests never assert on the key, CI blanks it with
  `ANTHROPIC_API_KEY` and `DEEPAGENT_API_KEY`, and `cargo test` isolates the
  kill switch. Never flip `EXECUTION_ENABLED` or OR the switch to go green.
- Audit appends go to one bounded writer thread
  (`logging.audit_queue_lines`); overflow drops lines.
- Publish a live setting only after its save (`panels::commit_setting`);
  refuse before changing state.
- `[profile.dev.package."*"] opt-level=3` is load-bearing: scrypt is slow
  unoptimized.
- A weak-hash name used as a label ends its line with
  `DevSkim: ignore DS126858 because …`.
- Console replies check `sessionRevision` before rendering.
- Pinned egress reuses `common::dns::lookup_bounded` and
  `web_search::pinned_client_builder`; callers still authorize destinations.
  Fixture bypasses need `test-support`. reqwest 0.13 uses the OS trust store.
- `cgagentharness browser guards` and the other sidecar fixtures run Chrome
  with its sandbox on, so they refuse root: run them as an unprivileged user.

## Quality bar

- Once per clone: `bash scripts/ensure-githooks.sh`.
- CI gate: `cargo fmt --all -- --check`, `cargo clippy --all-targets
  --all-features -- -D warnings`, `cargo test --all-targets`,
  `cargo deny check`. Rust 1.89 (desktop 1.90). `verify-local.sh` skips a
  missing cargo-deny: not deny evidence.
- Locally: lint (fmt, clippy, `python3 scripts/lint-diff.py`), then run the
  changed code; focused tests when needed. Docs:
  `cargo test --test invariant_guard markdown_stays_on_the_docs_budget`.
  Workflows: `scripts/lint-workflows.sh`. Leave full suites to CI.
- A green PR is not Windows or desktop evidence (`desktop-check.yml` runs only
  when its path regex matches).
- Changing a shipped default, refusal order or exit code: `rg` the tests and
  run every match.
- New routes go in `routes/mod.rs::REGISTERED_PATHS` and `ROUTE_CAPS` (and
  `views.rs` if listed). New shim actions extend `shim::ACTIONS` (and
  `JSON_ACTIONS`), `agentic/commands.rs::dispatch` and the guard's whitelist.
- `/api/agent/run`, `/api/agent/jobs` and schedules all go through
  `prepare_run`. MCP tools never attach to `/loop`.
- Prefer a `#[cfg(test)]` unit test beside a pure parser over another
  integration test.
- PRs: draft, one concern, driver-prefixed branch, **based on `main`**, title
  `[prefix] - Sentence`, body from `.github/PULL_REQUEST_TEMPLATE.md` with
  `McLuhan tetrad` and `ELI5` (`scripts/check-pr-template.sh`). Core-path
  changes need an invariant statement.
- Before publishing, refresh `origin/main` and open PRs; trial-merge
  overlaps, including the generated manual and doc caps.
- Read the advisory `review gate` before merging. Never comment
  `@codex review` without operator approval.

## Rule enforcers

Enforce corrections highest-level; add rows.

| Rule | Enforcer |
|------|-------------|
| Duplicated constants (never deduplicate), `/loop` limits | `duplicated_constants_still_agree` |
| Async fns: no SQLite, file or hash work (`off_worker`) | `async_fns_leave_blocking_…` |
| Assert messages print no secret-bearing value | `lint-diff.py` CG011 |
| Recheck policy, authority after waits | `owner_permits` (copies fail); calling: prose |

## Docs policy

Before pushing feature or core changes, run `python3 scripts/docs-impact.py`.
Update owning guides for observable changes. For an unaffected guide, use
the printed `Docs-Impact` trailer with the source-backed reason.

Edit in place. Only folder README indexes may be new (150 words unless
budgeted). Link shared facts; evidence goes in PRs, screenshots in
`docs/screenshots/`. Check `wc -w` and `DOCS_BUDGET` (1 MiB/file).
Lower trimmed caps. New rows or raises need approval and `// why:`.
Remove deleted docs from budgets and references (CG008).

For broad syncs use `doc-sync` and `dep-sync` or verify code, pins, locks
and `deny.toml` against `origin/main`. Weekly, `cgfixit` reviews Markdown
diffs and [dependency watches](docs/DEPENDENCIES.md#retained-constraints).

After editing a `scripts/build-self-manual.py::INPUTS` source, this file
included, regenerate `assets/self/MANUAL.md` and commit it when it changes.

## Project skills

Repository guidance is separate from runtime `/api/skills`. Start with
`cgagentharness-project-guidance`, then the task skill; none expands
publication, merge or release authority. Indexes:
[.codex/skills](.codex/skills/README.md),
[.claude/skills](.claude/skills/README.md) (keep operator-only skills
operator-only), [.github/skills](.github/skills/README.md). Skill edits need
frontmatter, path/symbol, command-filter and budget checks; static checks
cannot prove runtime discovery.
