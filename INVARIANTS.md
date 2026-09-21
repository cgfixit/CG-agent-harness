# CGagentHarness invariants

These are the guarantees the codebase enforces by structure, and where each one
lives. When code and this file disagree, code wins; fix this file.

## I6 — process isolation between the console and the agentic pipeline

The HTTP console (`src/server`, `src/shim`, `src/llm`, `src/common`) never links
or calls the agentic pipeline (`src/agentic`). The only edge is
`src/shim/mod.rs`, which builds an argv list from a 12-action whitelist and
spawns `current_exe() agentic <action>` as a CHILD PROCESS with a hard timeout
(shared bounded Unix runner with cancellation cleanup; `kill_on_drop` elsewhere). The agentic side never references
the server or the shim.

- Locked by: `tests/invariant_guard.rs` (source scan, both directions; only the
  shim may spawn a process on the server side).
- Why a process, not a module: a SIGKILL'd child can leak a clone, but it cannot
  corrupt the server's memory or bypass its guard chain; the exit-code contract
  (0 ok / 2 failed / 3 env_config / 4 write_refused) is the whole interface.

MCP stdio children are spawned from `src/common/mcp.rs` with a constructed
environment (secret and linker-hijack names stripped). Required per-server
versioned capabilities select explicit read/write roots, network denial or an
unrestricted grant, and strict containment or a process-group exception. Missing
or invalid capability declarations fail closed; authority changes require restart.
Filesystem confinement is the default. Linux probe failures never silently remove
network/filesystem protection; Windows confined stdio is refused. `linux-bwrap-fs` only
exists after an explicit unrestricted network grant. macOS uses `darwin-seatbelt`.

Strict Linux calls use `linux-systemd-bwrap`: a private lifeline connects the
harness to a transient systemd service, whose dedicated cgroup owns supervisor
and descendants before tool execution. The worker validates actual membership
and process/memory limits; non-delegated control files and bubblewrap mounts
prevent child migration. Service-main exit and its external runtime bound kill
the cgroup, including detached descendants. Strict mode on other hosts refuses.
An explicit `process_group` exception keeps filesystem/network confinement but
only kills the owned Unix group/direct child; runner death or detached children
can escape cleanup. `/tools mcp` displays this limitation.

Windows supports a separate, explicit trusted-server exception: `job_object`
requires `filesystem: unrestricted`, `network: unrestricted`, no root grants and
process/memory limits. The unnamed non-inheritable Job Object is assigned during
CreateProcess through JOB_LIST before any child code runs. Kill-on-close applies
to normal descendants, including detached processes; neither breakaway flag is
set. Only the three child pipe handles are inherited. This exception provides no
filesystem/home secrecy or network isolation and cannot confine work delegated
to an outside OS service. It is never an automatic fallback.

`src/server` still contains no `Command::new`. The harness home and its `.env`
are unreachable from confined MCP stdio children: command paths, cwd, and read/write
roots overlapping that home are refused (`MCP_HOME_REFUSED`). Actual backend,
probe result and declared capabilities are audited on `mcp_stdio_spawn`;
refusals include the policy and error code. See `docs/MCP_CLIENT.md` and
`docs/PROCESS_LIFECYCLE.md` for migration and precise residual limits.
Servers are operator-declared in `mcp.servers`; unknown names fail closed.
SSE URLs reuse DNS-pinned SSRF checks; loopback SSE is `mcp.sse_allow_loopback`
and ships false. Namespaced tools (`mcp:<server>:<tool>`) pass `tool_broker`
and require `confirm: true`. MCP tools are not attached to `/loop`. `GET /api/mcp`
discloses declared server and tool names to any authenticated session; that is
acceptable for a single-operator console and must be revisited before persistent
multi-user accounts. Calls and refusals are audited (`mcp_stdio_spawn`,
`mcp_sse_call`, `mcp_refused`) without arguments or payloads.

## Account, transport and request boundaries

Fresh configuration requires `auth.enabled: true` and `tls.enabled: true`.
Existing explicit configuration is preserved. Invalid auth/TLS switch types fail
closed. The optional harness key grants no account, provider or coding authority;
legacy key-required mode is deprecated and ignored. Forwarded requests are refused.

Every API request passes rate limiting, exact same-origin checks and direct
loopback validation. Operational reads and writes additionally require an enabled
account and its server-side role permissions. Mutations retain the existing CSRF
header/token; only minimal status/static/login/setup surfaces are public. A fresh
admin/admin account can only replace its password, inspect its identity or log out.
The replacement revokes old sessions. The last enabled administrator is protected.

SQLite is authoritative after transactional legacy migration; corrupt, empty or
missing initialized storage never recreates default credentials. Failed commits
never publish an in-memory account mutation. Private account identity scopes web
selection and structured memory (facts, governed proposals, and optional
episodes). Shared portal
sessions/jobs/pinned notes/persona remain explicitly shared. Structured memory
uses authenticated `user_id`, the documented `local` namespace from
`context_owner` when accounts are disabled, or labeled `user_*` fixture owners. Canonical facts change only through
an explicit human confirm+reason path; proposals may suggest but never apply
themselves. Episode capture never writes facts. `structured_memory.enabled`,
`structured_memory.episode_capture`, `structured_memory.explicit_recall`,
`structured_memory.retrieval`, `structured_memory.auto_retrieval`,
`structured_memory.consolidation`, `structured_memory.auto_consolidation`,
`structured_memory.auto_suggest_chat`, and `structured_memory.auto_suggest_coding` are
independent literal-boolean gates (`flag_is_true`); slash overrides use
versioned explicit booleans and are administrator-only when accounts are enabled.
An explicit override `false` wins over config `true`; legacy override `false`
keeps its prior unset meaning. The store-opening gate remains config + restart only.
All nine ship true in fresh configuration; existing config files and explicit off
overrides remain authoritative. Fresh pinned-note inclusion comes from
`memory.enabled: true`; existing harness.json values remain unchanged. Capture off means no episode writes; recall off
means no selected-fact prompt injection; retrieval off means no FTS search or
force-include. Search returns candidates only — prompt injection still requires
an explicit pick (`selected_facts`, `/memory retrieve`, or the per-request
`retrieve` flag) plus assembly-time owner/active/revision recheck, unless the
separately gated `auto_retrieval` silent path is on. FTS indexes facts only,
never episode summaries. Manual consolidation off means no summarizer call and
no proposal writes from selected episodes; when on, the local model may create
pending proposals only. Automatic consolidation is a separate default-true
gate that also requires consolidation (AND); when on, a bounded idle worker
may enqueue the same pending-proposal runner. Feature-off starts no worker.
Disabling stops new claims without corrupting in-flight work. Interactive
chat wins generation-gate contention. Independently gated completion suggestions
require store + capture, use bounded current completion evidence for the initiating
owner, and produce pending proposals only. They never read shared archives or
write human semantic summaries. `/memory save <text> :: <reason>` is an explicit
human confirmed fact write through the existing API, not model authority.
`/memory on` remains pinned-note prompt inclusion
only and does not open capture, recall, retrieval, or consolidation. Recalled text is
untrusted background context and cannot authorize tools, coding, or network.
The structured SQLite file uses owner-private mode as OS access
control — that is not encryption at rest. Pinned `/memory` notes stay on
`memory/notes.json` and are not migrated. Session clear keeps derived episodes
unless the operator confirms `delete_derived_episodes`; that cascade still
preserves facts, proposals, and episodes referenced by pending proposals.

The HTTP/1 Host and HTTP/2 authority must be unambiguous loopback names. HTTPS
scheme comes from the actual listener, never forwarding headers. TLS generation
is atomic and private; certificates are reused and invalid/expired material fails
startup. Native readiness and webview trust bind to the exact certificate supplied
by the owned sidecar's private handshake. No global TLS bypass or root installation.
Cookies are Secure over HTTPS, HttpOnly and SameSite=Strict, including clearing.

Locked by `tests/auth_guards.rs`, `tests/secure_portal.rs`,
`tests/security_headers.rs`, transport/account units and native trust checks.
See [migration and operation](docs/SECURE_RESEARCH.md).

## Public web evidence requires current content permission

Fresh web settings are enabled with an empty URL allowlist, so no content can be
fetched until an administrator grants URL permission. Existing true/false choices
are preserved; absent or invalid legacy `web_enabled` values remain off.
`tools/web_allowlist.json` is a bounded,
versioned document; malformed, missing, unreadable or partially invalid policy
refuses access. Exact HTTP(S) URLs retain scheme, port, path and query identity.
Only explicit `*.host` and `/path/*` rules broaden scope. Path wildcards include
query strings; exact rules retain exact query identity. Encoded query data cannot
alter the parsed host/path, and encoded path escapes remain refused. Legacy rows authorize
only their stored fetch target, never old host aliases or implicit descendants.

The fetcher resolves once, rejects every mixed/special-use address answer, then
pins a fresh Reqwest client to the validated addresses with a refusing fallback
resolver. Public TLS verification stays enabled; proxies, redirects, retries,
compression and connection reuse are disabled. Deadlines include queueing and
DNS; response headers, bodies and concurrent fetches have finite limits.

Policy is reloaded before dispatch and before storage or delivery. Revoked or
unproven saved evidence is inaccessible, including old shared plain-text context.
Revocation prevents future retrieval/injection; it cannot erase historical chat
or content already sent to a model. No atomicity against arbitrary external file
edits between an authorization check and an OS operation is claimed.

Chat exposes only two read-only model tools while web is enabled: Google keyword
search and permitted URL fetch. The dispatcher bounds calls, tokens, time and
arguments and rechecks current account/URL permissions before reads and evidence
delivery. It cannot mutate policy, credentials, accounts or repositories, and
`/loop` exposes no tools. Search-provider snippets are listings, not fetched
linked pages; each content fetch still requires its own URL permission. A
configured SerpAPI key selects the fixed Google-results API transport, while no
active key selects public Google. SerpAPI listings require enabled web and current
account authority, independently of page URL grants; public Google and linked-page
fetches retain URL policy checks. Saved SerpAPI changes apply on the next search;
explicit environment values retain precedence. Challenges and provider failures never become
fabricated results. Keys remain server-side and never enter model context.

Content permission grants neither account authority nor provider configuration
authority. None of these authorities substitutes for coding/write/publication
gates. Page text is data, never a tool command.

- Locked by: `server::web_policy::tests`, `server::web_search::tests`, and web
  integration tests in `tests/panels.rs`.

## The browser never supplies a command

`POST /api/agent/run` carries check profile NAMES; `src/server/agent_policy.rs`
maps them to fixed argv. Free-text values cross the shim as `--opt=value`
single elements or temp files, never as separate argv tokens.

- Locked by: `tests/shim_and_agent_routes.rs` (hostile-argv matrix, run_id
  `\A[0-9a-f]{32}\z` fullmatch, branch namespace, `confirm` never defaulted).

## Every write to a repository is gated

Repository mutation boundaries (checkout, file write, add, commit, push) and
PR execution reload `config.yaml` and require `agentic.enabled`, write mode,
`writes_enabled`, `deepagent_github.enabled`, `allow_git_write_tools`, a nonblank
human reason, and explicit confirmation. The emergency disable switch is AND-ed
with `EXECUTION_ENABLED`; it cannot arm writes. Missing/invalid config refuses.
A changed repository, workspace root, protected scope, scanner setting, or budget
requires a new invocation instead of continuing under a stale snapshot.

Run confirmation authorizes isolated candidate edits only. Approval verifies the
manifest and commits locally with fresh reason/confirmation. Push and draft PR
publication each require separate actions and fresh intent; combined
`decide --push/--publish` is refused. Default master, deepagent, and clone-write
flags remain false (mode/write-enabled defaults alone cannot arm writes).
The established master-disabled CLI banner/no-op exit 0 remains; actual mutation
boundary denials use exit 4. Invalid/missing config uses exit 3.

Read-only diff/status do not require write enablement. Git optional index refresh,
fsmonitor, external diff and textconv are disabled for inspection. Reject/discard
remain available with writes disabled while the master layer is enabled.

Environment is inherited at process creation. Exporting the disable variable in
another shell does **not** alter an existing server/child. Restart with the switch
set to affect new children, or revoke YAML write policy to block later mutation
boundaries in an active child. Neither mechanism interrupts an already-running
Git command/check; process-tree cancellation limitations remain separately tracked.
No atomic transaction between an external policy edit and a syscall is claimed.

- Locked by: `tests/write_policy.rs`, `tests/real_repo_loop.rs`, and
  `tests/invariant_guard.rs::shipped_config_enforces_accounts_tls_and_keeps_execution_gates_closed`.

## The clone jail

Writes: canonical path -> per-segment `.git` name-equivalence refusal
(trailing dot/space, NTFS `git~1`, HFS-ignored codepoints, case folding) ->
resolve (dangling-leaf aware) -> containment -> landed-path vs the real
`.git` dir -> report the LANDED path (so a symlink onto a protected file is
judged by where it lands). Reads: after `canonical_repo_relative_path`
succeeds, refuse if any path segment is `.git` name-equivalent (same
`is_dotgit_name` helper as writes), then resolve inside a `cap_std::fs::Dir`
capability held open on the clone: each path component is opened relative to
the previous one (`openat` semantics), so a symlink pointing outside the clone
fails to resolve rather than being followed; the leaf is additionally opened
with `O_NOFOLLOW` (unix). 256 KB cap, UTF-8 required.

- Locked by: `tests/agentic_foundations.rs::write_jail_refuses_escapes_and_reports_landed_paths`,
  `read_jail_refuses_symlink_escapes_without_following_the_leaf`,
  `read_jail_refuses_dotgit_metadata`.
- This closes the canonicalize-then-open TOCTOU window the port previously
  carried as a documented residual: capability-based resolution has no window
  to race, because there is never a bare path handed to the OS a second time.

Clone jail ≠ secrets. After a selector canonicalizes inside the jail, local
planner `=== READ ===` requests and operator `--read-file` values are refused
when the final path segment matches `agentic.deepagent_github.denied_read_basenames`
(name-equivalence folded; audit `agentic_real_repo_read_request_refused` /
`sensitive_basename`). Cloud proposers still refuse every model-requested read
(`cloud_proposer`). The deny-list is not a secret scanner: secrets can use
arbitrary names, and hiding a basename from the next prompt does not remediate
repository history.

- Locked by: `src/agentic/real_repo_loop.rs` (`denied_read_basename`,
  `apply_model_read_request`) and `tests/real_repo_loop.rs`.

Opt-in local repository retrieval reuses these clone reads and basename
refusals. `agentic.deepagent_github.retrieval.enabled` must be literal true;
cloud proposers cannot expand their declared read set through retrieval.
A bounded Git inventory feeds a per-step Tantivy RAM index; ignored untracked
files, metadata aliases, unsafe selectors, binaries, oversized files and
injection-scanner hits are omitted. It never shares the public web index.
Opened file descriptors must be regular files, and reads remain bounded even
if a file grows after its metadata check. Each retrieved excerpt is re-read,
bound to the indexed full-file SHA-256, scanned again and charged against
existing read limits plus its configured UTF-8-byte token reservation.
A changed hash omits that hit. Every new iteration rebuilds from current
content, including edits from the prior attempt. Explicit selections keep
priority. Run/audit traces contain path, line selection, hash and counts,
not query or source bytes. Existing exact-edit, check and approval gates apply.

- Locked by: `src/agentic/repo_retrieval.rs`, `src/agentic/edits.rs`,
  `tests/real_repo_loop.rs` and the clone-jail regression suite.

## Nothing lands before it is judged

Every proposed file is injection-scanned and code-shape-scanned, scope-checked
against `protected_write_paths` (name-equivalence folded), and budget-checked
BEFORE candidate content is changed. Whole-proposal preflight precedes staging;
application failures roll back installed replacements, and failed rollback is
fatal/quarantined with recovery backups retained. This is not crash-atomic or
an atomic compare-and-swap against a concurrent external writer. A file that
exists and was not shown in full through declared or local retrieved context
is refused for whole-file
replacement rather than blindly replaced. Exact
edits require a fresh full-file hash and unique original text in the displayed
excerpt. Every attempt takes new snapshots; earlier writes grant no exemption.
Verification runs only inside a
hard sandbox (Seatbelt / bubblewrap or `unshare --net` / Job Object); no backend
means exit 3.

- Locked by: `tests/real_repo_loop.rs`, `tests/exact_edits.rs`, workspace transaction tests,
  `tests/agentic_foundations.rs::sandbox_*`.

### Native macOS Cargo boundary

Cargo sources are prepared explicitly against a Cargo.lock digest before model
execution. Verification uses the selected installed toolchain, vendored sources,
`--frozen`, an empty Cargo home, and fresh build/temp directories. Seatbelt denies
all network operations and file data reads outside candidate, prepared inputs,
specific OS/SDK/runtime roots, and scratch. Candidate source and `.git` are
read-only to checks; only owned scratch is writable. Shared compiler/source
caches are never writable by checks. This is a deliberate restriction on tests
that formerly wrote into the candidate; use the provided temporary directory.

Metadata discovery remains allowed. Seatbelt is not a memory/disk quota, and the
current process group implementation does not contain every escaped descendant.
Linux prefers bubblewrap with an allowlisted read-only filesystem, writable
scratch, tmpfs `/tmp`, and `--unshare-net`. Binding the host root (`--ro-bind / /`)
is forbidden because it still exposes SSH keys and the harness env credential
file. When `bwrap` is missing or its probe fails, Linux falls back to
`unshare --net` only (`name()` is `linux-netns`): network isolation without
filesystem confinement. That fallback is explicit in the backend name, an
info-level `linux hard sandbox backend selected` line, the self-test line, and
the audit `sandbox` field. `/api/status` does not report the backend. The
console must not import `crate::agentic`. Both backends missing is
`HARD_SANDBOX_UNAVAILABLE` (exit 3). Linux CI (`CI=true`) fails if `bwrap` is
missing. The ordinary test matrix may report a classified confinement skip
when a GitHub-hosted runner refuses `RTM_NEWADDR`; that is not confinement proof.
The separate `Linux bubblewrap confinement (required)` job runs the same suite
in a pinned container with namespace/mount capabilities. Setting
`CGAH_REQUIRE_LINUX_BWRAP` (any value) makes every probe failure fatal and also
refuses a non-Linux host; it is a test-only requirement, not a production gate.
Python 3 probes must execute successfully, demonstrate candidate reads and
scratch writes, and explicitly observe denied secret/symlink reads and candidate
writes. The network probe first connects to an owned host listener outside the
sandbox, then requires a different network namespace and connection denial
inside it. No public endpoint or generic nonzero exit serves as network proof.
The job changes no host sysctl and preserves the production backend ladder.
SIGKILL of the runner with a live grandchild is unverified.
It is the same process-group leftover already named for Seatbelt. Windows Job
Object remains a process-tree kill boundary without network or filesystem
isolation. See `docs/OFFLINE_CARGO.md` for preparation, required native tests,
and remaining process/resource limitations.

- Locked by: `tests/macos_cargo.rs` (no sandbox capability skip),
  `tests/linux_bwrap.rs`,
  `src/agentic/executor/sandbox.rs::prefer_linux_sandbox_falls_back_then_fails_closed`.

## Approval is bound to what was reviewed

`pending_decision` records carry an acceptance digest (`run_id + base HEAD +
path -> sha256 + file mode`). Approval rechecks live files and a disposable copy,
then builds an exact tree in a private Git index. A pre-staged change or existing
index lock refuses approval. The ordinary index lock remains held until the new
index is installed. No unrelated staged content is incorporated or erased.

The shared agentic Git boundary disables ambient Git configuration, executable
extensions, replacement objects and automatic maintenance. Unsupported local
configuration, graft/attribute/alternate metadata and content-transforming
attributes refuse the operation. Inspection uses the same boundary. No server
or shim imports this agentic helper.

Runs retain their origin from candidate creation and the exact approved commit.
Push checks those pins and uses an object-ID refspec; publication checks the
remote branch. Current policy and separate reason/confirmation remain required.
Older records missing the new bindings need a new reviewed run. No transaction
against arbitrary hostile filesystem races or later remote changes is claimed.
See `docs/GIT_APPROVAL.md` for compatibility changes and review limitations.

- Locked by: `tests/git_approval.rs`, `tests/write_policy.rs`,
  `tests/real_repo_loop.rs::loop_iterates_on_feedback_then_accepts_and_finalizes`,
  `tests/agentic_foundations.rs::manifest_digest_binds_files_and_head`.

## Secrets never reach a response or a log line

Audit records are redacted recursively (emails, IPs, configured secret shapes);
model error bodies are never echoed; `/api/keys` returns presence and a masked
tail only; the tool broker logs an argv digest, never argv; validation errors
substitute `(unexpected field)` for a caller-supplied key.

- Locked by: `tests/common_layer.rs`, `tests/panels.rs`, `tests/chat_and_sessions.rs`.

## A detached run cannot outlive its gates

`POST /api/agent/jobs` runs the identical validated request as
`POST /api/agent/run` in a tokio task, but the run gate and (when the local
model is also the planner) the chat gate are held by the TASK via
`GateGuard`'s `Drop`, not by the HTTP request. A closed tab, a proxy timeout,
or a client that never polls again cannot leave the gate stuck: cancelling
(`POST /api/agent/jobs/{job_id}/cancel`) aborts the task, dropping the guards and
the child (`kill_on_drop`). A job's terminal state is set exactly once —
`JobStore::finish` is a no-op if the job was already cancelled — so a slow
child finishing after cancellation can never resurrect a job the operator
already killed.

- Locked by: `tests/shim_and_agent_routes.rs::cancelling_a_running_job_aborts_it_and_a_finish_after_cancel_does_not_resurrect_it`,
  `::a_job_holds_the_run_gate_so_a_concurrent_sync_run_is_busy`,
  `src/server/agent_jobs.rs` unit tests.

## Local chat compaction preserves goal and the first user turn

The initial estimator is UTF-8 `bytes.div_ceil(4)`, not a Qwen/CJK tokenizer.
Successful local exchanges persist an observed/estimated prompt-token ratio
with the session, scoped to the requested model and backend URL. It starts at
1, stays within 1–3, responds immediately to increases and smooths decreases
with a half-weight EMA. Missing, zero or non-integer prompt usage leaves the
previous calibration unchanged. Web turns use the first model call's usage,
never their aggregate; summary usage and cloud replies do not train this ratio.
The send
window is stored prompt history (user and assistant turns, persist cap
`MAX_MESSAGES`). There is no 20-turn or 8000-char clip. Compaction owns
overflow. The effective trigger is
`max(chat.compact_prompt_tokens, effective_reply_reservation + 4096 +
calibrated_tool_definition_tokens)`, capped at 30000. Tool definitions are also
included in the calibrated input estimate. Web-enabled chat tightens the trigger
toward `web.total_tokens - 2 * effective_reply_reservation`, without going below
that floor; the web dispatcher independently enforces its total budget.
Resolved Ollama with explicit `reasoning_effort: "none"` reserves the reply
ceiling once. Other reasoning settings, missing settings and compatible
backends reserve it twice in startup validation, compaction and web dispatch.
Startup rejects local-chat or loop reply allowances above 25904 or 12952,
respectively. This is a conservative harness margin, not provider accounting.
The incoming user paste is never compacted. The server builds a candidate
summary with one local-model call (`compaction.summary_max_tokens`, default
768, clamped 128–2048). Input stays within 24000 characters and the calibrated
summary-call budget. Ordinary middle turns are clipped to 800 characters;
prior `[session-compacted]` summaries are reserved intact before selecting
recent ordinary turns. If the summaries alone cannot fit, compaction fails
without clipping them or rewriting history.
A failed, empty, timed-out, or aborted summary does not rewrite the session.
If compaction cannot bring that initial prompt below the trigger, the turn is
refused without rewriting the session. If it can, the summary is persisted atomically with the next
successful exchange, alongside calibration, using `write_json_atomic_mode` at `0o600`; failed or
cancelled model calls leave stored history unchanged. The system prompt is
composed each turn and is never stored in `messages`. `Session.goal` and the
first user message are preserved. Successful compaction is audited as
`chat_session_compacted` without message bodies. An irreducible prompt is
audited as `chat_prompt_too_large` with projected sizes only. A concurrent
local-model claim is audited as `chat_busy`. Neither event stores message
bodies.

- Locked by: `src/server/compaction.rs`,
  `src/server/sessions.rs::record_exchange_inner`,
  `src/server/routes/core.rs::prompt_history`,
  `tests/chat_and_sessions.rs::minimum_compaction_threshold_still_allows_an_ordinary_turn`,
  `tests/chat_and_sessions.rs::compaction_is_persisted_only_with_a_successful_exchange`,
  `tests/chat_and_sessions.rs::observed_cjk_usage_compacts_repeatedly_and_persists_only_successful_calibration`,
  `tests/chat_and_sessions.rs::reasoning_and_compatible_backends_enforce_the_effective_reply_reservation`,
  `tests/chat_and_sessions.rs::irreducible_prompt_is_rejected_without_rewriting_the_session`,
  `tests/chat_and_sessions.rs::cancel_aborts_the_in_flight_turn_and_releases_the_gate`,
  `tests/chat_and_sessions.rs::a_long_normal_session_compacts_instead_of_clipping_at_8000_chars`,
  `src/server/compaction.rs::projection_uses_next_prompt_not_lifetime_tally`.

## Signals weaker than their name

- `unslop.enabled` is a prose nudge, never a gate.
- `agentic/context` injection findings are advisory on reads; only the
  model-feeding commands refuse on them (selected by CODE, not severity).
- The Windows sandbox is a process-tree kill boundary, not a network namespace.
  A stronger Windows backend (for example AppContainer) is deferred.
- Linux `linux-netns` is network isolation only. Treat `linux-bwrap` as the
  filesystem-confined backend; do not claim Seatbelt parity when the fallback
  is selected.
- `security.api_key_optional` on a host fronted by a header-stripping proxy is
  indistinguishable from no proxy: explicitly set it false there.

Unix subprocess I/O shares an operation deadline and a fixed aggregate capture
ceiling. The direct child stays unreaped through process-group cleanup. macOS
also stops observed descendants across groups; this is best-effort ancestry
cleanup, not containment. See `docs/PROCESS_LIFECYCLE.md` for limits and tests.

## Native Ollama pull stays on loopback and never sends num_ctx

`GET /api/ollama/inventory` reuses the existing `/v1/models` readiness probe and
adds `GET /api/tags` for installed names. Both routes sit on the CSRF-guarded
router (operator/admin). `POST /api/ollama/pull` streams NDJSON progress (SSE
when `Accept: text/event-stream`) to a derived loopback origin (OpenAI `/v1`
stripped). The pull is single-flighted on its own gate, abortable
(`POST /api/ollama/pull/cancel` or disconnect), and never includes `num_ctx`.
Startup `keep_alive` warmup is `models.local_llm.warmup.enabled` (`flag_is_true`;
missing keys in old homes are off) and must not fail `serve`. Provider error
bodies are not echoed.

- Locked by: `src/llm/ollama.rs`,
  `src/server/routes/ollama.rs`,
  `tests/ollama_manage.rs`.

## Session export and transcript search stay on the machine

`GET /api/sessions` remains an open summary list with no message bodies and no
goal. Markdown export (`GET /api/sessions/{session_id}/export`) and transcript
search (`POST /api/sessions/search`) sit on the CSRF-guarded router like
`GET /api/sessions/{session_id}`. Export writes `{home}/exports/{id}.md` at
`0o600` and returns the same bytes. Search rebuilds a request-local Tantivy
RAM index over session JSON; it does not write the web cache or a second
search crate. Hits are session id plus a bounded snippet. Sessions remain
shared portal resources: any operator or admin who can load a session can
export or search it. Transcripts never leave the machine.

- Locked by: `src/server/session_export.rs`,
  `src/server/session_search.rs`,
  `src/server/routes/session_io.rs`,
  `tests/session_export.rs`.

## Scheduled agentic runs cannot skip reviewed-goal or write gates

A persisted schedule (`$CGAGENTHARNESS_HOME/data/agentic/console-schedules.json`,
`0o600`) fires the same validated request as `POST /api/agent/jobs` through
`prepare_run`. Creating a schedule without a bound/reviewed `goal_stage` fails
closed. `confirm` is never defaulted; write gates stay closed. Each occurrence
is consumed before the job starts (at-most-once): `next_fire_at` / `last_fired_at`
survive process restart, a restart mid-window does not fire, and missed windows
are skipped rather than caught up. Cancelled schedules do not fire; cancelling
the resulting job still makes `JobStore::finish` a no-op. Start, complete, and
fail are audited via `Audit::log` (JSONL, never raises).

- Locked by: `src/server/agent_schedules.rs`,
  `src/server/routes/agent.rs`,
  `tests/agent_schedules.rs`.

## Inference spend keeps recorded usage separate from prediction

Cloud Grok/Claude 2xx responses and local OpenAI-compat usage append one JSONL
row to `$CGAGENTHARNESS_HOME/logs/spend.jsonl` (or home-relative `logging.spend_file`;
absolute values and parent-directory components fall back to `logs/spend.jsonl`) through
the existing audit spend sink. Rows store provider, model, source
(`chat`/`loop`/`agentic`/`eval`), token counts, optional vendor ticks, and
`usage_missing`. They never store prompt/query/content/messages, credentials, or
`usd`. Dollars are derived at read time (`estimate_usd`): prefer Grok
`vendor_cost_ticks` (`TICKS_PER_USD = 10_000_000_000`); otherwise the dated
rate table; incomplete usage and unknown models stay unpriced; local rows are
always `local_unpriced`. `usage_reported` is true only when both input and
output counts parsed as JSON numbers. HTTP 2xx with empty text still appends
`outcome: failed_after_billing` then errors the chat. `TokenTally` and
local prompt/compaction token estimates are context budgets, not USD.
`PRICED_AS_OF` is the oldest `_RATE_VERIFIED` date; a table older than 30 days
warns once per process and does not fail the chat. Guarded
`GET /api/spend/summary` rolls up the same file by provider/model/UTC-day with
the same CSRF/authz class as a loaded session. Ollama pull, keep_alive warmup,
and MCP broker dispatch are excluded from the ledger by definition — they are
not billed inference. If a later MCP tool bills a cloud model, it must record
`source: "agentic"` through this same file. The console must not import
`crate::agentic` to write spend.

- Locked by: `src/llm/spend.rs`,
  `src/llm/cloud_chat.rs`,
  `src/server/routes/core.rs`,
  `tests/spend_ledger.rs`.

Cloud chat prediction is a separate pre-generation operation. Guarded
`POST /api/spend/predict` accepts only the explicit message/model and shares the
generation/cancellation gate. Claude counts the same model/message body at its
fixed count endpoint, with a 2-second default deadline and 4 MiB response cap;
Grok and count failures use a labelled UTF-8 bytes/4 heuristic. No history,
attachments, local context or tokenizer calibration reaches the cloud counter.
The dated ledger rate table prices full configured output with no cache credit.
A missing/null `models.cloud_chat.max_usd_per_call` leaves generation ungated;
otherwise it must be finite and positive. Vendor estimates enforce a known-rate
cap; heuristic estimates enforce it only with literal `budget_on_heuristic: true`.
Unknown rates remain unpriced. Prediction, count requests and budget refusal
create no spend rows. This is an estimate, never a guaranteed invoice ceiling.

## Completion notifications do not grant job or content authority

`notifications.enabled` is literal-true, default-off and restart-only. The shared
JobStore emits terminal transitions once per process; repeated finish/cancel calls
cannot create another event. An in-memory bounded queue delivers only job ID,
status and timestamps to the explicitly configured webhook. It never sends job
instructions/results, repository names, errors, or model context. Optional bearer
credentials use the private managed environment store and are stripped from MCP
child environments. Every DNS answer is validated and pinned before sending;
private destinations require an exact configured URL grant, and redirects are
never followed. Failed deliveries/overflow are audited and never change a job's
result. Retries are bounded to three with a stable batch ID. No durable delivery
or replay after restart is claimed. I6 and all execution/write gates are unchanged.

## Reload changes limits, not authority

The exact non-secret allowlist in `src/server/config_reload.rs::RELOADABLE`
contains 22 API/web limits. A bounded regular-file config read (no-follow on Unix) is parsed
and fully validated before one runtime snapshot is replaced. Any invalid limit,
malformed YAML, unknown or restart-only change refuses the entire candidate
and leaves running limits intact. Refusal audit/errors never echo YAML contents.
HTTP reload requires a current non-bootstrap admin session and CSRF, even in
legacy auth-disabled homes. Unix SIGHUP uses the same reload implementation.

TLS, authentication, secrets, sandbox, provider/model and write authority are
not reconfigured by reload. `web.concurrency` stays restart-only: web snapshots
share existing fetch permits, mutation locks and cancellation rather than
creating more capacity. New operations see the new limits; in-flight web
operations retain their snapshot. Rate counters survive reload; widening a
window cannot restore already expired history. A refusal does not rewrite the
operator's file. Existing fresh-disk coding-policy and URL-permission checks
remain independent and fail closed as before.

- Locked by: `tests/secure_portal.rs` (HTTP, actual SIGHUP, role/CSRF,
  atomic refusal, rate history and web response bounds), `config_reload` units,
  and the existing web/research tests.
