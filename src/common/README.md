# src/common

Primitives shared by every side of the I6 boundary. No module here may import
`agentic`; [tests/invariant_guard.rs](../../tests/invariant_guard.rs) enforces it.
Contracts: [INVARIANTS.md](../../INVARIANTS.md); process rules:
[docs/PROCESS_LIFECYCLE.md](../../docs/PROCESS_LIFECYCLE.md).

- [`apikey.rs`](apikey.rs): the `API_KEY_ENV` name; presence is reported, never grants authority.
- [`atomic.rs`](atomic.rs): atomic bytes/JSON writes: stage, sync, rename, optional Unix mode.
- [`audit.rs`](audit.rs): redacted audit JSONL with bounded retention; the server normally queues appends to one writer thread.
- [`auth_sqlite.rs`](auth_sqlite.rs): SQLite schema and commit-before-snapshot writes, included by `auth_store.rs` via `#[path]`.
- [`auth_store.rs`](auth_store.rs): `AuthManager`: users, sessions, lockout, bootstrap admin, one-time `auth.json` migration.
- [`authn.rs`](authn.rs): scrypt n=2^17 password records, lockout math, session and CSRF tokens.
- [`bounded_log.rs`](bounded_log.rs): two-generation JSONL retention; `append_until` retries a busy lease to a deadline.
- [`child_env.rs`](child_env.rs): per-child-kind telemetry opt-outs (`DO_NOT_TRACK` and siblings), pinned by the otel skill.
- [`config.rs`](config.rs): private YAML loading and typed `AppConfig` views; `flag_is_true` requires literal `true`.
- [`credential_file_writer.rs`](credential_file_writer.rs): Windows-only owner-ACL file writer, included by `credential_store.rs`.
- [`credential_store.rs`](credential_store.rs): `CredentialStore` trait and the OS store; env wins; one-time `.env` migration.
- [`errors.rs`](errors.rs): `HarnessError {code, message, details}` and the constructors the console's `detail.code` relies on.
- [`file_lease.rs`](file_lease.rs): `FileLease`: OS-held advisory lock; files never deleted, PIDs never authority.
- [`home.rs`](home.rs): `~/.CGagentHarness` layout, seeding, `harness.json`, `LOOPBACK_HOSTS`, `validate_port`.
- [`home_lock.rs`](home_lock.rs): `HomeLock`: advisory single-server ownership of a home.
- [`identity.rs`](identity.rs): agent committer defaults and `TEMPLATE_BRANCH_PREFIXES`; env override fails closed.
- [`injection.rs`](injection.rs): `Scanner` over core, OWASP and configured prompt-injection patterns.
- [`local_tls.rs`](local_tls.rs): exact-certificate rustls trust for the loopback portal; also compiled into `desktop/`.
- [`mcp.rs`](mcp.rs): MCP JSON-RPC `StdioClient` and stdio spawn; `SECRET_ENV` names never reach children.
- [`mcp_policy.rs`](mcp_policy.rs): `StdioCapabilities`: operator-owned network, containment, filesystem and resource grants.
- [`mcp_worker.rs`](mcp_worker.rs): Linux strict-mode stdio supervisor, a real `main.rs` subcommand; exits 3 elsewhere.
- [`mod.rs`](mod.rs): module list plus timestamps, random tokens, SHA-256, `is_visibly_blank`, `MAX_PR_BODY_BYTES`.
- [`private_sqlite.rs`](private_sqlite.rs): owner-only, size-capped SQLite open shared by auth and memory.
- [`process/`](process/README.md): the Unix half of the argv-list runner.
- [`process.rs`](process.rs): `RunSpec`/`run`/`run_cancellable`: argv-only runner, `MAX_CAPTURE_BYTES`, `kill_tree`, `which`.
- [`ratelimit.rs`](ratelimit.rs): in-memory per-client sliding window with `retry_after_sec`.
- [`repo_paths.rs`](repo_paths.rs): `canonical_repo_relative_path`: rejects absolute, `..`, leading `-`, `:`, trailing dot or space.
- [`sandbox_wrap.rs`](sandbox_wrap.rs): Seatbelt profile and bubblewrap argv wrapping; refuses binding the host root.
- [`tool_broker.rs`](tool_broker.rs): tool-name allowlist `decide`/`assert_allowed`; empty allowlist denies.
- [`windows_job.rs`](windows_job.rs): Windows Job Object child (`#[cfg(windows)]`), no breakaway; containment, not isolation.
