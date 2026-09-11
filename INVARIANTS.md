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

## Guard chain on every operator route

`rate limit -> same-origin (Sec-Fetch-Site + exact Origin scheme/host/port) ->
API key (constant-time; bypass only if security.api_key_optional AND loopback
peer AND no forwarding headers) -> CSRF (per-process token, absent header rejects)`.
Order is load-bearing: a wrong key against a spent budget is 429, and a missing
key is 401 even when CSRF is also missing in key-enforced mode.

- Locked by: `tests/auth_guards.rs`, `tests/security_headers.rs`.
- Direct local access ships with `security.api_key_optional: true`: no API key
  or account login is needed for harness operations. When explicitly set false,
  an unset `CGAGENTHARNESS_API_KEY` refuses every guarded route.
- Per-user sessions and roles protect account management only; enabling
  `auth.enabled` does not add a login requirement to harness operations.
- The Host header must be a loopback name (DNS-rebinding defense); the bind is
  refused for a non-loopback host in `serve`.

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
  `tests/invariant_guard.rs::shipped_config_keeps_every_gate_closed`.

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
