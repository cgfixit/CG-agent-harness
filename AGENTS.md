# AGENTS.md — CGagentHarness operating manual

Every coding agent (Claude Code, Codex, Grok, Kimi, Copilot, others) follows this
file literally. Where a rule says "never", there is no exception without explicit
operator approval. Read `INVARIANTS.md` before touching a core path:
`src/shim/`, `src/server/{guards,headers}.rs`, `src/agentic/{writer,workspace}.rs`,
`src/agentic/executor/sandbox.rs`, `assets/config.default.yaml`.

Reading scope: never open `docs/learning/*` unless the operator asks. Open other
`docs/*` files only when a root file links the page and the task needs it.

## Where truth lives

1. Code. 2. `assets/config.default.yaml` (every tunable; no hardcoded tunables
elsewhere). 3. `INVARIANTS.md`. 4. This file. 5. `README.md`.
When prose contradicts code, fix the prose in the same PR.

## The map

- `cgagentharness serve` -> loopback HTTP/HTTPS console in `src/server`. Public
  `account`, `web` and `tls` CLI commands call the same protected service.
- `cgagentharness agentic <action>` -> `src/agentic` (hidden; spawned by
  `src/shim` as a child process, never called in-process).
- `cgagentharness netconnect status|devices` -> `src/netconnect`, passive and
  read-only. Every netconnect gate ships false; empty `allowed_cidrs` refuses
  armed tiers. `/net` (aliases `/netconnect`, `/lan`, `/scan`, `/ports`,
  `/speed`, each dropped if it collides with an existing slash name) dispatches
  `status`, `devices`, `ports`, `diag`, `watch`; the tier commands refuse unless
  `tier_may_run` is true and never connect. `device` always refuses.
- Exit codes are an API for the `agentic` and `netconnect` children: `0` ok,
  `2` failed, `3` env/config, `4` write refused. A non-zero child exit is HTTP
  200 with `ok=false`; shim failures map to 400/502/504 and the disabled-layer
  banner to 409. Other subcommands (`serve`, `account`, `web`, `tls`) exit `1`
  on error.
- Home: `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`). Never write outside it
  except into a clone the pipeline itself made under `data/agentic/workspaces`.
- Feature contracts live in `INVARIANTS.md`; read the owning section before
  changing a feature:

| Area | `INVARIANTS.md` section |
|------|-------------------------|
| Accounts, TLS, ownership, structured memory gates | Account, transport and request boundaries |
| Chat `web_search` / `web_fetch`, SerpAPI | Public web evidence requires current content permission |
| Provider keys (OS credential store; env wins) | Provider keys live in the OS credential store |
| Outbound MCP client (`mcp.enabled`, `mcp.servers`, stdio sandbox) | I6 — process isolation…; `docs/MCP_CLIENT.md` |
| Clone jail, read deny-list, repo retrieval | The clone jail |
| Detached jobs, schedules | A detached run cannot outlive its gates; Scheduled agentic runs… |
| Ollama inventory, pull, warmup | Native Ollama pull stays on loopback… |
| Session export and search | Session export and transcript search stay on the machine |
| Spend ledger and prediction | Inference spend keeps recorded usage separate… |
| Completion webhooks | Completion notifications do not grant… |
| Config reload | Reload changes limits, not authority |
| Inbound MCP memory server | Private MCP memory server is a separate authority boundary |
| netconnect | Netconnect is fail-closed and LAN-scoped |

## Traps

- **Never** make `src/{server,shim,llm,common,netconnect}` reference
  `crate::agentic`, and never make `src/agentic` reference server or shim (I6).
  `tests/invariant_guard.rs` fails if you do, including aliased `use` forms.
  Cross the boundary only through `src/shim` and the CLI whitelist.
- Duplicated on purpose, kept in sync by tests: `RUN_ID_PATTERN` (server
  `agent_policy` vs agentic `run_store`), the planner/check timeout constants
  (shim vs agentic), the check-profile table. Do not "deduplicate" them.
- Gates read through `flag_is_true`: only literal YAML `true` is on; quoted
  `"true"` and a missing key are off. Note a missing key is not the shipped
  value: several gates ship `true` (structured memory, Ollama warmup, auth, TLS).
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
- Server audit appends go to one writer thread (`logging.audit_queue_lines`,
  default 4096; 0 appends inline). SQLite work stays off async workers.
  Requests never wait on a file.
- scrypt at n=2^17 is slow unoptimized; `[profile.dev.package."*"] opt-level=3`
  is load-bearing for test time.
- `RELOADABLE` in `src/server/config_reload.rs` (23 keys) is the only
  hot-reload allowlist; everything else, including `web.concurrency`, is
  restart-only.

## Quality bar

- Once per clone, before committing: `bash scripts/ensure-githooks.sh`.
- CI merge gate: `cargo fmt --all -- --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-targets`, `cargo deny check`. Rust 1.88 is pinned for the
  backend; `desktop/` pins 1.90. `scripts/verify-local.sh` skips deny when
  cargo-deny is missing, so its green run is not deny evidence.
- Verifying a change locally: lint (fmt, clippy), then run the changed code
  (route, binary or function). If nothing can run directly, run one targeted
  test, such as `cargo test --test invariant_guard` after structural or doc
  edits; never the whole suite. Lint workflow edits with actionlint and zizmor.
- New routes: add to `src/server/routes/mod.rs::REGISTERED_PATHS` (and
  `views.rs` if the console lists them) or `/api/tools` reports them unwired.
- New shim actions: extend `shim::ACTIONS` (and `shim::JSON_ACTIONS` if it
  returns JSON), `agentic/commands.rs::dispatch`, and the invariant guard's
  whitelist assertion together.
- `/api/agent/run`, `/api/agent/jobs` and persisted schedules all go through
  `prepare_run`; never give one a path the others skip.
- MCP tools never attach to `/loop`; `/loop` stays tool-free.
- Prefer a `#[cfg(test)] mod tests` unit test beside a pure parser or matcher
  over another integration test.
- PRs are draft, one concern, on a driver-prefixed branch (`claude/`, `codex/`,
  `grok/`, `kimi/`, `agent/` when unknown), **based on `main`**, title
  `[prefix] - Sentence`, body from `.github/PULL_REQUEST_TEMPLATE.md` (run
  `scripts/check-pr-template.sh` first). Touching a core path requires an
  explicit invariant statement in the body.
- The advisory `review gate` check reports unresolved threads and running Codex
  reviews; it never fails CI. Read it before merging and re-run it after
  resolving the last thread. Never comment `@codex review` without the
  operator's approval.
- Loading a skill never authorizes push, merge or release.

## Docs policy

Edit the section that owns a topic; create no Markdown files except a folder
`README.md` (see below). Link instead of duplicating. Evidence belongs in
PRs/issues; screenshots go in `docs/screenshots/` (a root `screenshots/` fails
the guard), and only for significant changes. `tests/invariant_guard.rs`
enforces `DOCS_BUDGET`: every listed Markdown file has a word cap (this file
2000, `CLAUDE.md` 300), plus group caps, no new root screenshots or docs PDFs,
no files over 1 MiB. Check `wc -w` before committing any `.md` edit, including
web-UI edits. New rows or raised caps need operator approval and a `// why:`.

Every directory carries a `README.md` of at most `FOLDER_README_WORDS` (150)
words: what the folder holds, what reads it, and where the full doc lives. It
is an index, not a second copy of the topic. The guard caps these by name
pattern, so they need no `DOCS_BUDGET` row; a longer one fails.

Deleting a doc means deleting every reference to it in the same PR: its guard
row, links in other docs and skills, packaging scripts (`scripts/package-desktop.sh`
bundles some docs), and any `README.md` that indexed it. `grep -rn` the basename
before pushing.

Sync, then trim. On any PR that touches more than one module, or when the last
sync is older than a few weeks, run `doc-sync` (Claude: operator types
`/doc-sync`) and `dep-sync` (model-loadable), or do their work by hand in
another agent: re-read the code the docs describe, rewrite stale sections,
refresh pins, locks and `deny.toml` against `origin/main`. The same pass must
remove what no longer applies: duplicate rules, superseded notes, dead links,
retired dependencies and their watches. A sync that only adds words is not
done; lower the `DOCS_BUDGET` cap of every file that shrank so the cut stays
cut. Weekly, `cgfixit` reviews `git log --since=1.week --stat -- '*.md'` and
[dependency watches](docs/DEPENDENCIES.md#retained-constraints) the same way.

## Project skills

Three skill trees are repository guidance, not application `/api/skills` plugins:
`.codex/skills/` (Codex), `.claude/skills/` (Claude Code, run as `/<slug>`) and
`.github/skills/` (Copilot). Read the relevant `SKILL.md` when its task applies:

- `cgagentharness-project-guidance`: read order and skill routing; start here.
- `fable-protocol`: evidence-first reasoning before costly code/security/CI claims.
- `cgagentharness-invariant-guard`: "do the invariants still hold?" gate before
  merging core-path diffs.
- `cgagentharness-config-guard`: static fail-closed check of `config.default.yaml`.
- `cgagentharness-write-policy-redteam`: attack write gates, confirm+reason,
  clone jail and shim argv.
- `verification-specialist`: break a *supplied* change without modifying the tree.
- `cgagentharness-gotchas`: session-tested traps (Chrome CI flake, YAML `"true"`,
  CSRF names, Seatbelt noise, clippy toolchain fights).
- `cgagentharness-optimize`: evidence-backed Rust/runtime/CI improvements; never
  transplant CyClaw topology or defaults.
- Codex only: `cgagentharness-release` (macOS packaging, release prep),
  `cgagentharness-verify` (isolated backend, desktop and local-model checks).
- Claude only: `cgagentharness-verify-deps` and
  `cgagentharness-runtime-invariant-check` (report only), `dep-sync` (fixes
  dependency and CI drift against `origin/main`), `doc-sync` (rewrites stale
  docs), `run-cg-agent-harness` (fake-model console smoke),
  `cgagentharness-otel-hardening` (telemetry-kill contract).
