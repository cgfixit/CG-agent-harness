# AGENTS.md — CGagentHarness operating manual

Follow literally. Where a rule says "never", there is no exception without
explicit approval. Read `INVARIANTS.md` before touching `src/shim`,
`src/server/guards.rs`, `src/server/headers.rs`, `src/agentic/writer.rs`,
`src/agentic/executor/sandbox.rs`, `src/agentic/workspace.rs`, or
`assets/config.default.yaml`.

## Where truth lives

1. Code. 2. `assets/config.default.yaml` (every tunable; no hardcoded tunables
elsewhere). 3. `INVARIANTS.md`. 4. This file. 5. `README.md`.

## The map

- `cgagentharness serve` -> shared HTTP/HTTPS transport in `src/server`, loopback only.
- Public `account` and `web` CLI operations call the same protected service; `tls` exports/renews local certificate material. See `docs/SECURE_RESEARCH.md`.
- Fresh auth/TLS switches are true; existing explicit choices survive upgrades. SQLite accounts protect operational reads and writes. Harness API keys are optional metadata, never login authority.
- Fresh web settings start enabled with an empty URL allowlist; existing choices and absent/invalid legacy fields remain unchanged/off. Exact/wildcard content permission is distinct from account and provider authority. Research/web selection and structured memory (facts/proposals/episodes) are account scoped (`user_id`, documented `local` via `context_owner`, or labeled `user_*` fixture owners); sessions/jobs/pinned notes/persona are shared portal resources. Episode capture is a separate default-true gate and is not prompt injection. Explicit recall is a third default-true gate: selected facts enter `/prompt` only after operator selection and assembly-time owner/active/revision revalidation. Retrieval/FTS is a fourth default-true gate, independent of `/memory on` and `explicit_recall`: search is not inject. Per-request `retrieve` / `/memory retrieve` is the explicit pick for that prompt; `auto_retrieval` is a fifth default-true silent path. FTS indexes facts only. Manual consolidation is a sixth default-true gate: selected episodes become pending proposals only and never auto-apply facts. Automatic consolidation is a seventh default-true gate that also requires consolidation (AND): a bounded idle worker may reuse the same pending-proposal runner. Feature-off starts no worker; chat wins the generation gate. Two further default-true switches, `auto_suggest_chat` and `auto_suggest_coding`, require store + capture and may turn bounded current completion evidence into pending summaries/insights for the initiating owner. They never scan shared archives, fill human semantic summaries or auto-apply facts. See `docs/MEMORY_GUIDE.md`.
- Chat exposes only bounded `web_search` (Google listings) and `web_fetch` (permitted URL content) tools when web is enabled; `/loop` stays tool-free. `SERPAPI_API_KEY` selects the fixed Google-results API; no active key selects public Google, whose challenges are explicit failures. Listings never grant destination permissions.
- `cgagentharness agentic <action>` -> `src/agentic` (hidden; spawned by
  `src/shim`, never called in-process from the server).
- Exit codes are an API: `0` ok, `2` failed, `3` env/config, `4` write refused.
  A non-zero child exit is HTTP 200 with `ok=false`; only shim failures map to
  400/502/504 and the disabled-layer banner to 409.
- Home: `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`). Never write outside it
  except into a clone the pipeline itself made under `data/agentic/workspaces`.
- Local planner `=== READ ===` and operator `--read-file` refuse a small
  default basename deny-list (`agentic.deepagent_github.denied_read_basenames`)
  after clone-jail canonicalization. Deny-list ≠ secret scanner; jail ≠ secrets.

Saved `SERPAPI_API_KEY` changes activate immediately; explicit process environment
values take precedence. The fixed SerpAPI search endpoint needs no page URL grant.
Public Google fallback and destination page fetching retain URL permission checks.

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
- The env var `GROK_API_KEY` on a developer machine is real: tests must not
  assert on its presence and CI blanks it.
- The env var `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` on an operator machine is
  real: `cargo test` isolates it so later write gates are what fail. Do not
  flip `EXECUTION_ENABLED` to false (or OR the kill switch) to make tests green.
- scrypt at n=2^17 is slow unoptimized; `[profile.dev.package."*"] opt-level=3`
  is load-bearing for test time.
- `cargo clippy` may resolve to a rustup proxy older than Homebrew's toolchain
  on this machine; `CLIPPY=/opt/homebrew/bin/cargo-clippy scripts/verify-local.sh`.

## Quality bar

- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-targets` green; `cargo deny check` clean.
- New routes: add to `routes/mod.rs::REGISTERED_PATHS` (and `views.rs` if the
  console lists them) or `/api/tools` reports them unwired.
- New shim actions: extend `shim::ACTIONS`, the CLI dispatch, and the
  invariant guard's whitelist assertion together.
- `/api/agent/run` (sync) and `/api/agent/jobs` (detached) must stay in lockstep:
  both go through `agent::prepare_run` so the validation, budget check, tool
  broker, and both gates can never drift between the two paths.
- MCP is opt-in (`mcp.enabled` literal true, declared `mcp.servers` only). Calls
  go through `POST /api/mcp/call` with `confirm: true`, the tool-broker
  allowlist, and DNS-pinned SSE. Stdio children are wrapped with the same
  Seatbelt / bubblewrap helpers as agentic verification. Do not attach MCP
  tools to `/loop`. `GET /api/mcp` lists declared server/tool names to an
  authenticated session.
- Native Ollama management is loopback-only (`GET /api/ollama/inventory`,
  abortable `POST /api/ollama/pull`). Pull/warmup never send `num_ctx`. Warmup
  is `models.local_llm.warmup.enabled` (`flag_is_true`; missing in old homes is
  off) and a bounded background `keep_alive` generate; failure is a logged
  degrade, never a crash. Audit roles cannot pull; admin and operator can.
- Session Markdown export and transcript search stay on the machine.
  `GET /api/sessions/{session_id}/export` writes `{home}/exports/{id}.md` at
  `0o600`. `POST /api/sessions/search` uses a request-local Tantivy RAM index
  (same crate as `web_index`, never mixed with the web cache). `GET /api/sessions`
  still omits goal and bodies. Sessions remain shared portal resources.
- Inference spend is append-only JSONL (`logs/spend.jsonl`; home-relative
  `logging.spend_file`, with absolute/`..` falling back to that default).
  Dollars are read-time only; never persist `usd`, prompts, or keys. Local
  rows are unpriced. Pull/warmup/MCP dispatch are not ledger events. Guarded
  `GET /api/spend/summary` is the rollup. `usage_reported` is honest (both
  counts must be JSON numbers). Empty-text 2xx still records
  `failed_after_billing`.
- Prefer a small unit test with `#[cfg(test)] mod tests` beside the function
  over another integration test when the thing under test is a pure parser or
  matcher (`repo_paths`, `real_repo_loop`'s file-block parser, `guards`'
  same-origin check) — they run in milliseconds and don't need a server.
- PRs are draft, one concern, on a driver-prefixed branch (`claude/`, `codex/`,
  `grok/`, `kimi/`, `agent/`), **based on `main`** (never stacked on another
  feature branch; the `base branch is main` check fails otherwise), with the body
  from `.github/PULL_REQUEST_TEMPLATE.md` (run `scripts/check-pr-template.sh`
  first). Touching a core path requires an explicit invariant statement in the body.
- The `review gate` check (`.github/workflows/review-gate.yml`, published on
  the PR head via the Checks API so Codex's comment edits re-evaluate it) is
  advisory: it reports unresolved threads and an in-progress Codex review, and
  does not fail CI. Read it before merging. Resolving a thread fires no webhook,
  so re-run the job if the last thread was resolved without a push.

## Project Codex skills

Read the relevant entrypoint under `.codex/skills` when its task applies:

- `cgagentharness-optimize/SKILL.md`: evidence-backed Rust/runtime/CI improvements;
  adapted from CyClaw's Claude workflow with this repository's contracts.
- `cgagentharness-release/SKILL.md`: universal macOS packaging, native acceptance,
  workflow provenance and release preparation.
- `cgagentharness-verify/SKILL.md`: isolated backend, desktop and local-model checks.
- `fable-protocol/SKILL.md`: evidence-first reasoning and verification discipline;
  load before costly code/security/CI/GitHub claims.
- `cgagentharness-invariant-guard/SKILL.md`: "do the invariants still hold?" gate;
  load before merging core-path security diffs.
- `cgagentharness-gotchas/SKILL.md`: session-tested traps (Chrome CI flake, YAML
  `"true"`, CSRF names, Seatbelt noise); load before install/verify/packaging.
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

The eight Codex twins above that are not optimize/release/verify
(`fable-protocol`, `cgagentharness-invariant-guard`, `cgagentharness-gotchas`,
`cgagentharness-project-guidance`, `cgagentharness-write-policy-redteam`,
`verification-specialist`, `cgagentharness-config-guard`, `cgagentharness-parity`)
are mirrored under `.claude/skills/<slug>/SKILL.md`, plus a Claude-depth
`cgagentharness-optimize/SKILL.md` playbook (the `.codex` twin stays the short
runtime). Claude Code also has three read-only verification/report skills that
Codex does not: `cgagentharness-doc-sync`, `cgagentharness-verify-deps`, and
`cgagentharness-runtime-invariant-check`. Two further Claude-only skills
actively write fixes instead of only reporting drift: `doc-sync` (README,
AGENTS.md, setup-guide.md, `docs/*.md`) and `dep-sync` (Cargo manifests/locks,
`rust-toolchain.toml`, `deny.toml`, CI/release YAML), each diffing against
`origin/main` or the branch's upstream. A separate `run-cg-agent-harness`
skill drives a local fake-model console smoke and has no slash command. Every
other Claude skill above is registered as `.claude/commands/<slug>.md`
(`/fable-protocol`, `/cgagentharness-doc-sync`, `/doc-sync`, `/dep-sync`, etc.)
and loads the matching `SKILL.md`.

These are repository guidance, not application `/api/skills` runtime plugins.
Existing user authorization governs publication; selecting a skill adds none.
