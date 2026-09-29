# AGENTS.md — CGagentHarness operating manual

Follow literally. Where a rule says "never", there is no exception without
explicit approval. Read `INVARIANTS.md` before touching `src/shim`,
`src/server/guards.rs`, `src/server/headers.rs`, `src/agentic/writer.rs`,
`src/agentic/executor/sandbox.rs`, `src/agentic/workspace.rs`, or
`assets/config.default.yaml`. `CLAUDE.md` (repository root) is the per-session
summary of this file; when they disagree, this file wins and the summary is fixed.

## Where truth lives

1. Code. 2. `assets/config.default.yaml` (every tunable; no hardcoded tunables
elsewhere). 3. `INVARIANTS.md`. 4. This file. 5. `README.md`.

## The map

- `cgagentharness serve` -> shared HTTP/HTTPS transport in `src/server`, loopback only.
  Public `account` and `web` CLI operations call the same protected service; `tls`
  exports/renews local certificate material. See `docs/SECURE_RESEARCH.md`.
- Fresh auth/TLS switches are true; existing explicit choices survive upgrades.
  SQLite accounts protect operational reads and writes. Harness API keys are
  optional metadata, never login authority.
- Fresh web settings start enabled with an empty URL allowlist; existing choices
  and absent/invalid legacy fields stay unchanged/off. Exact/wildcard content
  permission is distinct from account and provider authority. Chat exposes only
  bounded `web_search` (Google listings) and `web_fetch` (permitted URL content)
  when web is enabled; `/loop` stays tool-free. `SERPAPI_API_KEY` selects the
  fixed Google-results API and needs no page URL grant; without a key, public
  Google is used and its challenges are explicit failures. Listings never grant
  destination permissions. Saved key changes apply immediately; process
  environment values win.
- Ownership: research/web selection and structured memory (facts, proposals,
  episodes) are account scoped (`user_id`, documented `local` via `context_owner`,
  or labeled `user_*` fixture owners). Sessions, detached jobs and schedules
  require the initiating owner; unassigned legacy sessions need explicit admin
  adoption (clearing prior coding approval). Pinned notes/persona, model
  selection, spend and agentic run records are shared portal resources.
- Memory is a set of default-true gates behind the store, each fail-closed:
  episode capture, `explicit_recall` (selected facts enter `/prompt` only after
  operator selection and assembly-time revalidation), `retrieval` (FTS over
  facts only; search is not inject), manual `consolidation` (selected episodes
  become pending proposals, never applied facts). Dependent gates AND their
  prerequisite: `auto_retrieval` needs `retrieval`; automatic consolidation
  needs `consolidation`; `auto_suggest_chat` / `auto_suggest_coding` need
  capture (bounded completion evidence into pending summaries for the
  initiating owner; never scans shared archives). Feature-off starts no
  worker; chat wins the generation gate and preempts a running suggestion (it
  parks as `preempted` and requeues) instead of answering `CHAT_BUSY`. Contract:
  `docs/STRUCTURED_MEMORY.md`; operator view: `docs/MEMORY_GUIDE.md`.
- `cgagentharness agentic <action>` -> `src/agentic` (hidden; spawned by
  `src/shim`, never called in-process from the server).
- `cgagentharness netconnect status|devices` is passive and read-only.
  `netconnect` gates ship false; empty `allowed_cidrs` refuses armed tiers.
  Scope is operator IPv4 CIDRs inside RFC1918 or 127/8 at prefix /16 or
  longer, never local interfaces. Invalid scope exits 3; a closed master
  gate exits 4. `/net` (exact aliases `/netconnect`, `/lan`, `/scan`,
  `/ports`, `/speed`) runs only exact `status` and `devices`. Tier tools
  register only when `tier_may_run` is true. The console LAN panel is
  `GET /api/netconnect`. See the netconnect section of `INVARIANTS.md`.
- Exit codes are an API: `0` ok, `2` failed, `3` env/config, `4` write refused.
  A non-zero child exit is HTTP 200 with `ok=false`; only shim failures map to
  400/502/504 and the disabled-layer banner to 409.
- Home: `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`). Never write outside it
  except into a clone the pipeline itself made under `data/agentic/workspaces`.
- Local planner `=== READ ===` and operator `--read-file` refuse the default
  basename deny-list (`agentic.deepagent_github.denied_read_basenames`) after
  clone-jail canonicalization. Deny-list ≠ secret scanner; jail ≠ secrets.
- Repository retrieval (`agentic.deepagent_github.retrieval`, literal true only)
  runs inside the agentic child for local proposers: a bounded per-step Tantivy
  RAM index over the clone capability, minus denied basenames, binary/oversized
  files and scanner hits. Re-verify the full hash before injection; persist only
  retrieval metadata. No new read authority for cloud proposers and no change to
  confirm/reason or write gates.
- Non-secret runtime limits reload through one validated snapshot:
  `POST /api/config/reload` (non-bootstrap admin + CSRF) and SIGHUP on Unix
  `serve` and native sidecars. The allowlist is
  `src/server/config_reload.rs::RELOADABLE` (22 keys); everything else is
  restart-only, including `web.concurrency`. Invalid or mixed candidates keep the
  whole old snapshot and audit a refusal. See `docs/CONFIG_RELOAD.md`.

## Traps

- **Never** make the server reference `crate::agentic`; `tests/invariant_guard.rs`
  fails the build-of-truth if you do. Add server-side behavior in `src/server`,
  cross the boundary only through `src/shim` and the CLI whitelist.
- Duplicated on purpose, kept in sync by tests: `RUN_ID_PATTERN` (server
  `agent_policy` vs agentic `run_store`), the planner/check timeout constants
  (shim vs agentic), the check-profile table. Do not "deduplicate" them across
  the boundary.
- Quoted YAML `"true"` is OFF for every gate (`flag_is_true`). Keep it that way.
- `confirm` is never defaulted on; `reason` is never optional on a write.
- The console asset stays verbatim: placeholders `__CYCLAW_CSRF_TOKEN__` /
  `__CYCLAW_CSP_NONCE__` and the `X-CyClaw-CSRF` header name are contractual.
- `GROK_API_KEY` on a developer machine is real: tests must not assert on its
  presence and CI blanks it (with `ANTHROPIC_API_KEY` and `DEEPAGENT_API_KEY`).
- `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` on an operator machine is real:
  `cargo test` isolates it so later write gates are what fail. Do not flip
  `EXECUTION_ENABLED` to false (or OR the kill switch) to make tests green.
- Server audit appends go to one writer thread (`logging.audit_queue_lines`,
  default 4096; 0 appends inline), flushed before each agentic child and at
  shutdown; a full queue drops lines with a warning. SQLite store work stays off
  async workers. Keep new I/O that way: requests never wait on the file.
- scrypt at n=2^17 is slow unoptimized; `[profile.dev.package."*"] opt-level=3`
  is load-bearing for test time.

## Quality bar

- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-targets` green; `cargo deny check` clean. Rust 1.88 is
  pinned. `scripts/verify-local.sh` skips deny when cargo-deny is missing, so
  its green run is not deny evidence.
- New routes: add to `routes/mod.rs::REGISTERED_PATHS` (and `views.rs` if the
  console lists them) or `/api/tools` reports them unwired.
- New shim actions: extend `shim::ACTIONS`, `agentic/commands.rs::dispatch`, and
  the invariant guard's whitelist assertion together.
- `/api/agent/run` (sync) and `/api/agent/jobs` (detached) must stay in lockstep:
  both go through `agent::prepare_run` so validation, budget check, tool broker
  and both gates never drift. Persisted schedules (`POST /api/agent/schedules`)
  fire that same jobs path; unbound or unreviewed goals fail closed; occurrences
  are at-most-once across restart.
- MCP is opt-in (`mcp.enabled` literal true, declared `mcp.servers` only). Calls
  go through `POST /api/mcp/call` with `confirm: true`, the tool-broker allowlist
  and DNS-pinned SSE. Stdio children use the same Seatbelt/bubblewrap helpers as
  agentic verification and require explicit versioned capabilities; no unconfined
  or network-weaker fallback. Linux strict containment needs an externally owned
  systemd/cgroup service; other platforms refuse strict mode. Windows
  `job_object` is a trusted-server exception (process ownership, not data
  isolation). Never attach MCP tools to `/loop`. See `docs/MCP_CLIENT.md`.
- Inbound MCP memory is a separate default-off loopback listener (`mcp.server`)
  with explicit tool grants and dedicated `mcp_keys.sqlite3` machine credentials
  over the store's owner-filtered read methods. Never accept console cookies
  there or machine keys as console authority; no writes or agent tools; outside
  config reload. See `docs/MCP_SERVER.md`.
- Native Ollama management is loopback-only (`GET /api/ollama/inventory`,
  abortable `POST /api/ollama/pull`). Pull/warmup never send `num_ctx`. Warmup
  (`models.local_llm.warmup.enabled`, `flag_is_true`; missing is off) is a
  bounded background `keep_alive` generate whose failure is a logged degrade.
  Audit roles cannot pull; admin and operator can.
- Session Markdown export and transcript search stay on the machine:
  `GET /api/sessions/{session_id}/export` writes `{home}/exports/{id}.md` at
  `0o600`; `POST /api/sessions/search` uses a request-local Tantivy RAM index
  never mixed with the web cache; `GET /api/sessions` omits goal and bodies.
  All three require the initiating owner.
- Inference spend is append-only JSONL (`logs/spend.jsonl`; home-relative
  `logging.spend_file`, absolute/`..` falls back). Dollars are read-time only;
  never persist `usd`, prompts or keys. Local rows are unpriced; pull/warmup/MCP
  dispatch are not ledger events. `GET /api/spend/summary` is the read-only
  rollup (Analytics → Tokens and cost). Empty or truncated cloud 2xx records
  `failed_after_billing` and is never shown or saved. `POST /api/spend/predict`
  counts only the supplied cloud draft (Claude vendor count with bounded
  fallback, Grok bytes/4), creates no ledger row, reserves full output without
  cache credit; heuristic gating needs literal `budget_on_heuristic: true`.
  Unknown rates stay unpriced. See `docs/SPEND_AND_NOTIFICATIONS.md`.
- Completion webhooks are default-off, restart-only and metadata-only: at most
  three retries per bounded replay cycle, exact owned destination grants, DNS
  pinning, no redirects, stable deduplication IDs, owner-scoped status/replay
  that rechecks authority. Delivery failure never changes a job outcome; the
  console's Job webhooks button shows only while notifications are enabled.
- Prefer a `#[cfg(test)] mod tests` unit test beside a pure parser or matcher
  (`repo_paths`, `real_repo_loop`'s file-block parser, `guards`' same-origin
  check) over another integration test.
- PRs are draft, one concern, on a driver-prefixed branch (`claude/`, `codex/`,
  `grok/`, `kimi/`, `agent/`), **based on `main`** (the `base branch is main`
  check fails stacked PRs), title `[prefix] - Sentence`, body from
  `.github/PULL_REQUEST_TEMPLATE.md` (run `scripts/check-pr-template.sh` first).
  Touching a core path requires an explicit invariant statement in the body.
- The `review gate` check (`.github/workflows/review-gate.yml`) is advisory: it
  reports unresolved threads and an in-progress Codex review and does not fail
  CI. Read it before merging; resolving a thread fires no webhook, so re-run the
  job if the last thread was resolved without a push.

## Docs policy

Do not create Markdown files. Edit the section that owns the topic, and link to
it instead of restating it. Evidence (acceptance runs, closeouts, verification
notes) goes in the PR body or an issue comment; screenshots go in
`docs/screenshots/`. `DOCS_BUDGET` in `tests/invariant_guard.rs` fails on any
`.md` without a row (tracked or not), on a file or group over its word cap, on
anything new under a root `screenshots/` or a PDF under `docs/`, and on a new
file over 1 MiB. A new row or a raised cap needs the operator's approval and a
`// why:` comment in the same PR.

## Project Codex skills

Read the relevant entrypoint under `.codex/skills` when its task applies:

- `cgagentharness-optimize/SKILL.md`: evidence-backed Rust/runtime/CI improvements
  under this repository's contracts; never transplant CyClaw topology or defaults.
- `cgagentharness-release/SKILL.md`: universal macOS packaging, native acceptance,
  workflow provenance and release preparation.
- `cgagentharness-verify/SKILL.md`: isolated backend, desktop and local-model checks.
- `fable-protocol/SKILL.md`: evidence-first reasoning and verification discipline;
  load before costly code/security/CI/GitHub claims.
- `cgagentharness-invariant-guard/SKILL.md`: "do the invariants still hold?" gate;
  load before merging core-path security diffs.
- `cgagentharness-gotchas/SKILL.md`: session-tested traps (Chrome CI flake, YAML
  `"true"`, CSRF names, Seatbelt noise, clippy toolchain fights).
- `cgagentharness-project-guidance/SKILL.md`: read order + skill routing; load at
  the start of substantive repository work.
- `cgagentharness-write-policy-redteam/SKILL.md`: adversarially exercise write
  gates, confirm+reason, clone jail, and shim argv boundaries.
- `verification-specialist/SKILL.md`: independently verify a *supplied* change by
  trying to break it, without modifying the tree.
- `cgagentharness-config-guard/SKILL.md`: statically assert `assets/config.default.yaml`
  still honors fail-closed contracts.
- `cgagentharness-parity/SKILL.md`: maintain CyClaw↔harness parity docs without
  weakening harness invariants.

## Project Claude skills

Claude Code loads the root `CLAUDE.md` every session and runs each
`.claude/skills/<slug>/SKILL.md` as `/<slug>`. `.claude/settings.json`
pre-approves only `cargo` build/check/fmt/clippy/test/deny, `cargo run -- serve`,
`scripts/verify-local.sh`, `scripts/check-pr-template.sh` and
`scripts/test-desktop-backend.py`. All skills but three are
`disable-model-invocation: true` and run only when the operator types them;
`cgagentharness-verify-deps`, `cgagentharness-runtime-invariant-check` (both
report only) and `verification-specialist` may be model-loaded. The Codex skills
above, except release and verify, are mirrored there; `fable-protocol` and
`cgagentharness-optimize` are long playbooks behind the short Codex versions.
Claude-only: the two report-only skills, `dep-sync` (fixes Cargo, toolchain,
`deny.toml` and CI/release drift against `origin/main`), `doc-sync` (rewrites
existing docs to match the code) and `run-cg-agent-harness` (fake-model console
smoke; its `driver.mjs` is historical, not acceptance evidence).

Both skill trees are repository guidance, not application `/api/skills` runtime
plugins. Existing user authorization governs publication; selecting a skill adds none.
