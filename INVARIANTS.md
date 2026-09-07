# CGagentHarness invariants

These are the guarantees the codebase enforces by structure, and where each one
lives. When code and this file disagree, code wins; fix this file.

## I6 — process isolation between the console and the agentic pipeline

The HTTP console (`src/server`, `src/shim`, `src/llm`, `src/common`) never links
or calls the agentic pipeline (`src/agentic`). The only edge is
`src/shim/mod.rs`, which builds an argv list from an 11-action whitelist and
spawns `current_exe() agentic <action>` as a CHILD PROCESS with a hard timeout
(`kill_on_drop`, process-group SIGKILL on unix). The agentic side never references
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
key is 401 even when CSRF is also missing.

- Locked by: `tests/auth_guards.rs`, `tests/security_headers.rs`.
- Fail-closed: an unset `CGAGENTHARNESS_API_KEY` refuses every guarded route.
- The Host header must be a loopback name (DNS-rebinding defense); the bind is
  refused for a non-loopback host in `serve`.

## The browser never supplies a command

`POST /api/agent/run` carries check profile NAMES; `src/server/agent_policy.rs`
maps them to fixed argv. Free-text values cross the shim as `--opt=value`
single elements or temp files, never as separate argv tokens.

- Locked by: `tests/shim_and_agent_routes.rs` (hostile-argv matrix, run_id
  `\A[0-9a-f]{32}\z` fullmatch, branch namespace, `confirm` never defaulted).

## Every write to a repository is gated

`deepagent_github.allow_git_write_tools` (ships false) gates `write_file`,
`add`, `commit`, `push_branch`. `agentic.enabled` (ships false) plus mode,
`writes_enabled`, a human reason and a per-call `confirm` gate the `gh pr create`
path (`src/agentic/writer.rs`); `EXECUTION_ENABLED` is true and only the env
kill switch `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` can turn it off (AND-ed,
never OR-ed).

- Locked by: `tests/real_repo_loop.rs::writer_gates_in_order_and_plan_integrity`,
  `tests/invariant_guard.rs::shipped_config_keeps_every_gate_closed`.

## The clone jail

Writes: canonical path -> per-segment `.git` name-equivalence refusal
(trailing dot/space, NTFS `git~1`, HFS-ignored codepoints, case folding) ->
resolve (dangling-leaf aware) -> containment -> landed-path vs the real
`.git` dir -> report the LANDED path (so a symlink onto a protected file is
judged by where it lands). Reads: canonicalize-and-contain, 256 KB cap,
UTF-8 required, `O_NOFOLLOW` on the leaf (unix).

- Locked by: `tests/agentic_foundations.rs::write_jail_refuses_escapes_and_reports_landed_paths`.
- Residual: CyClaw's reads use a held directory fd (`openat`), which Rust's
  standard library does not expose portably. The canonicalize-then-open window
  is narrowed by `O_NOFOLLOW`, not closed.

## Nothing lands before it is judged

Every proposed file is injection-scanned and code-shape-scanned, scope-checked
against `protected_write_paths` (name-equivalence folded), and budget-checked
BEFORE any byte is written; a quarantined iteration writes nothing. A file that
exists, was not shown in full via `--read-file`, and was not written by an earlier
iteration is refused rather than blindly replaced. Verification runs only inside a
hard sandbox (Seatbelt / `unshare --net` / Job Object); no backend means exit 3.

- Locked by: `tests/real_repo_loop.rs`, `tests/agentic_foundations.rs::sandbox_*`.

## Approval is bound to what was reviewed

`pending_decision` records carry an acceptance digest (`run_id + base HEAD +
path->sha256`). `decide approve` re-verifies it against the live worktree AND a
disposable copy with user/system git config disabled and hooks pinned to an
empty directory, then re-checks scope against the policy in force NOW, then
commits with `--no-verify` under the configured committer identity.

- Locked by: `tests/real_repo_loop.rs::loop_iterates_on_feedback_then_accepts_and_finalizes`,
  `tests/agentic_foundations.rs::manifest_digest_binds_files_and_head`.

## Secrets never reach a response or a log line

Audit records are redacted recursively (emails, IPs, configured secret shapes);
model error bodies are never echoed; `/api/keys` returns presence and a masked
tail only; the tool broker logs an argv digest, never argv; validation errors
substitute `(unexpected field)` for a caller-supplied key.

- Locked by: `tests/common_layer.rs`, `tests/panels.rs`, `tests/chat_and_sessions.rs`.

## Signals weaker than their name

- `unslop.enabled` is a prose nudge, never a gate.
- `agentic/context` injection findings are advisory on reads; only the
  model-feeding commands refuse on them (selected by CODE, not severity).
- The Windows sandbox is a process-tree kill boundary, not a network namespace.
- `security.api_key_optional` on a host fronted by a header-stripping proxy is
  indistinguishable from no proxy: do not enable it there.
