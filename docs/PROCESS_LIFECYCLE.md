# Subprocess lifetime and capture

The synchronous argv runner and Unix shim share one subprocess owner. Nonblocking
pipe reads and stdin writes share the operation deadline. Capture refuses more
than 4 MiB of raw stdout and stderr instead of returning truncated machine data;
the same ceiling covers Git and `gh`. Verification overflow is a setup error.
Publication overflow is indeterminate because the command may have succeeded;
the writer records the operation and does not retry it.

The shim uses a blocking worker with an atomic cancellation flag. Aborting the
owning future sets that flag; the worker observes it during bounded I/O and
closes its pipes. No independent reader tasks are detached. Cancellation is
cooperative and cleanup may finish shortly after the HTTP acknowledgement.
The direct child remains unreaped until cleanup, so its process-group identity
cannot be reused before group signaling. This also covers ordinary background
children whose leader exits while they retain its pipes.

On macOS, public `libproc` interfaces record descendants across process groups.
Cleanup stops observed parents, rechecks process identity, and kills observed
descendants under time, count, and buffer ceilings. Seatbelt permissions do not change.

**Ancestry cleanup is best-effort, not process containment.** A child that forks
and reparents before observation may escape; enumeration can fail or truncate.
Start-time checking and signaling are separate syscalls. Server SIGKILL or a
crash does not run the owner's destructor. Linux keeps group cleanup without
macOS descendant enumeration. The non-Unix runners retain their prior behavior;
these Unix capture/lifetime claims do not apply to Windows.

Filesystem and network denial are separate boundaries. The synchronous runner
has no per-run disk quota or general memory or process-count limit. Kernel calls
and cleanup can extend the requested deadline. Use isolated
fixtures and inspect surviving work after interruption; the console correctly
keeps its descendant-survival warning.

## Child environments

`src/common/child_env.rs` owns opt-outs for three child kinds.
`src/common/mcp.rs` applies the MCP kind to stdio servers, which receive
`DO_NOT_TRACK=1`. `src/agentic/git.rs` and `src/agentic/gh_client.rs` apply the
GitHub CLI kind, which also receives `GH_TELEMETRY=false`,
`GH_NO_UPDATE_NOTIFIER=1`, and `GH_NO_EXTENSION_UPDATE_NOTIFIER=1`.
`src/agentic/executor/runner.rs` applies the verification-check kind, which
receives `GH_TELEMETRY=false`,
`HF_HUB_DISABLE_TELEMETRY=1`, and `ANONYMIZED_TELEMETRY=false` with
`DO_NOT_TRACK=1`. Each spawn site applies these values last and removes
case-insensitive aliases, so declared or inherited values cannot re-enable
telemetry on Windows. This proves delivery, not that an arbitrary child honors
`DO_NOT_TRACK`.

## MCP stdio diagnostics

MCP stdio has a separate Tokio child owner. It drains stderr continuously,
discards excess bytes, retains at most 2 KiB, and exposes at most 512 Unicode
characters at EOF. It creates no stderr log file. The 4 MiB synchronous-runner
limit does not apply.

The drain lets a noisy child finish a valid response without filling its stderr
pipe. Each response is one newline-delimited JSON line and is refused above
`mcp.max_result_bytes`, even if unterminated. `mcp.timeout_sec` and child cleanup
still apply. A diagnostic prefix proves neither complete capture nor descendant
containment. See [troubleshooting](TROUBLESHOOTING.md).

### Explicit MCP lifecycle policy

Every stdio server must declare [capabilities](MCP_CLIENT.md). In Linux strict
mode, a transient systemd service owns the supervisor and bubblewrap child in a
dedicated cgroup v2 hierarchy. The service sets aggregate process and memory
limits, disables delegation, protects cgroup control files, and kills the whole
group when its main process ends. A private, authenticated Unix socket detects
loss of the harness independently of MCP stdin. The worker checks its actual
cgroup membership and enforced limits before launching the tool. The external
manager also bounds service lifetime if either process stalls or dies.

Bubblewrap denies host service/control paths and creates a PID namespace;
`setsid()` cannot leave the service cgroup. Requested roots, working directory
and executable cannot expose host `/proc`, `/sys`, `/run`, `/var/run` or `/dev`.
The child receives its own `/proc` and minimal `/dev`. `network: unrestricted`
is a separate operator grant, not an HTTP hostname allowlist.

macOS refuses strict lifecycle containment. An explicit `process_group`
exception retains Seatbelt filesystem/network protection, but runner death and
detached descendants can escape cleanup. Windows refuses confined/strict stdio.
Its explicit `job_object` trusted-server exception atomically assigns an unnamed,
non-inheritable Job Object through `PROC_THREAD_ATTRIBUTE_JOB_LIST` during
`CreateProcessW` (Windows 10 / Server 2016 or newer). No child instruction runs
before membership exists. A handle allowlist contains only the three child pipe
ends; the Job Object has kill-on-close, active-process and aggregate memory
limits, with neither breakaway flag set. Cancellation/deadline/protocol failure
closes the client and kills the job. Abrupt runner exit closes the kernel handle
without relying on Rust destructors. Detached descendants remain members.

This exception grants unrestricted filesystem/network access and cannot contain
requests delegated to another Windows service (for example WMI/COM). It is for
trusted servers, not hostile-code isolation. There is no OS wall-clock timer on
the Windows job; the harness request deadline requires a responsive runtime.
The focused native acceptance observes live child/grandchild process handles,
heartbeat progress, denied breakaway, actual process/memory limits, nine exit
paths and a fresh successful call after each. It does not establish filesystem
or network isolation. The coding executor's check sandbox uses the same
`JobChild` path, so its checks are also assigned to the kill-on-close job during
`CreateProcess` (32-process limit, no job memory limit). It is likewise a
process-tree kill boundary only, not filesystem or network isolation.

Strict mode bounds process memory/count and lifetime; it does not impose a
scratch disk quota. A hard-killed harness may leave its private temporary
directories on disk. It does not automatically delete operator-granted outputs.

## Evidence

`tests/mcp_client.rs` checks allowed reads and writes plus denied secret, symlink,
and network probes on Linux and macOS. The required native Linux
`tests/mcp_lifecycle.rs` job exercises cancellation, Drop, protocol failure,
crashes, deadlines, and SIGKILL with a detached grandchild. It then checks an
empty cgroup, stopped heartbeat, successful reuse, limits, and migration denial.
Ordinary runners report that they did not execute this native acceptance.

`tests/process_lifecycle.rs` covers inherited pipes, blocked stdin, overflow,
complete output and status, and a short-lived leader with a background child.
Its required macOS case cancels a JobStore task with a Seatbelt check in another
process group; the wrapper and check stop while an unrelated sibling survives.
Missing native sandbox capability fails this required test.

`tests/process_writer_outcome.rs` uses a local fake `gh` that records one accepted
mutation then floods stdout. It verifies exactly one invocation, an indeterminate
error identifying the operation, and an audit record. No remote service is used.

The runner drains ready data while checking budget and cancellation, and sleeps
only when neither stream advances. Deadline tests retain separate 100 ms cases.

## Desktop ownership and recovery

The desktop parent owns a private inherited control pipe; EOF requests backend
shutdown and cancellation through the same owners described above. A server home
lease excludes a second Unix server. New coding workers hold a per-run lease;
`/agent runs` or status reconciliation marks a running record interrupted only
after that lease is demonstrably released. There is no PID-file kill authority,
automatic resume or approval replay. Legacy running state stays unknown. A
released worker lease proves the worker ended, not that every escaped descendant
ended. Native window semantics and remaining acceptance are documented in
[DESKTOP.md](DESKTOP.md).
