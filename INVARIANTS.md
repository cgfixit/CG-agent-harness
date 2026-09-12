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
selection; shared portal sessions/jobs/notes/persona remain explicitly shared.

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

Web reads remain disabled initially. `tools/web_allowlist.json` is a bounded,
versioned document; malformed, missing, unreadable or partially invalid policy
refuses access. Exact HTTP(S) URLs retain scheme, port, path and query identity.
Only explicit `*.host` and `/path/*` rules broaden scope. Legacy rows authorize
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
judged by where it lands). Reads resolve inside a `cap_std::fs::Dir` capability
held open on the clone: each path component is opened relative to the
previous one (`openat` semantics), so a symlink pointing outside the clone
fails to resolve rather than being followed; the leaf is additionally opened
with `O_NOFOLLOW` (unix). 256 KB cap, UTF-8 required.

- Locked by: `tests/agentic_foundations.rs::write_jail_refuses_escapes_and_reports_landed_paths`.
- This closes the canonicalize-then-open TOCTOU window the port previously
  carried as a documented residual: capability-based resolution has no window
  to race, because there is never a bare path handed to the OS a second time.

## Nothing lands before it is judged

Every proposed file is injection-scanned and code-shape-scanned, scope-checked
against `protected_write_paths` (name-equivalence folded), and budget-checked
BEFORE candidate content is changed. Whole-proposal preflight precedes staging;
application failures roll back installed replacements, and failed rollback is
fatal/quarantined with recovery backups retained. This is not crash-atomic or
an atomic compare-and-swap against a concurrent external writer. A file that
exists and was not shown in full via `--read-file` is refused for whole-file
replacement rather than blindly replaced. Exact
edits require a fresh full-file hash and unique original text in the displayed
excerpt. Every attempt takes new snapshots; earlier writes grant no exemption.
Verification runs only inside a
hard sandbox (Seatbelt / `unshare --net` / Job Object); no backend means exit 3.

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
Linux namespace and Windows Job Object backends do not establish equivalent
filesystem or network confinement. See `docs/OFFLINE_CARGO.md` for preparation,
required native tests, and remaining process/resource limitations.

- Locked by: `tests/macos_cargo.rs` (no sandbox capability skip).

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
(`POST /api/agent/jobs/{id}/cancel`) aborts the task, dropping the guards and
the child (`kill_on_drop`). A job's terminal state is set exactly once —
`JobStore::finish` is a no-op if the job was already cancelled — so a slow
child finishing after cancellation can never resurrect a job the operator
already killed.

- Locked by: `tests/shim_and_agent_routes.rs::cancelling_a_running_job_aborts_it_and_a_finish_after_cancel_does_not_resurrect_it`,
  `::a_job_holds_the_run_gate_so_a_concurrent_sync_run_is_busy`,
  `src/server/agent_jobs.rs` unit tests.

## Signals weaker than their name

- `unslop.enabled` is a prose nudge, never a gate.
- `agentic/context` injection findings are advisory on reads; only the
  model-feeding commands refuse on them (selected by CODE, not severity).
- The Windows sandbox is a process-tree kill boundary, not a network namespace.
- `security.api_key_optional` on a host fronted by a header-stripping proxy is
  indistinguishable from no proxy: explicitly set it false there.

Unix subprocess I/O shares an operation deadline and a fixed aggregate capture
ceiling. The direct child stays unreaped through process-group cleanup. macOS
also stops observed descendants across groups; this is best-effort ancestry
cleanup, not containment. See `docs/PROCESS_LIFECYCLE.md` for limits and tests.
