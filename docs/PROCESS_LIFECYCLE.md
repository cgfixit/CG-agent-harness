# Subprocess lifetime and capture

The synchronous argv runner and the Unix shim share one subprocess owner.
Nonblocking pipe reads and stdin writes share the operation deadline. Capture
refuses more than 4 MiB of raw stdout and stderr instead of returning truncated
machine data; the same ceiling covers Git and `gh`. Verification overflow is a
setup error. Publication overflow is indeterminate, because the command may
have succeeded: the writer records the operation and does not retry it.

The shim uses a blocking worker with an atomic cancellation flag. Aborting the
owning future sets the flag; the worker sees it during bounded I/O and closes
its pipes. No reader tasks are detached. Cancellation is cooperative, so cleanup
may finish shortly after the HTTP acknowledgement. The direct child stays
unreaped until cleanup, so its process-group identity cannot be reused before
group signaling; that also covers background children whose leader exits while
they hold its pipes.

On macOS, public `libproc` interfaces record descendants across process groups.
Cleanup stops observed parents, rechecks process identity and kills observed
descendants under time, count and buffer ceilings. Seatbelt permissions do not
change.

**Ancestry cleanup is best-effort, not process containment.** A child that forks
and reparents before observation may escape, and enumeration can fail or
truncate. Server SIGKILL or a crash does not run the owner's destructor. Linux
keeps group cleanup without descendant enumeration. These Unix claims do not
apply to the Windows runners.

Filesystem and network denial are separate boundaries. The synchronous runner
has no per-run disk quota and no general memory or process-count limit, and
cleanup can extend the requested deadline. Inspect surviving work after an
interruption; the console keeps its descendant-survival warning for that reason.

## Child environments

`src/common/child_env.rs` owns telemetry opt-outs for three child kinds:

| Kind | Applied in | Receives |
|---|---|---|
| MCP stdio server | `src/common/mcp.rs` | `DO_NOT_TRACK=1` |
| GitHub CLI | `src/agentic/git.rs`, `src/agentic/gh_client.rs` | that, plus `GH_TELEMETRY=false`, `GH_NO_UPDATE_NOTIFIER=1`, `GH_NO_EXTENSION_UPDATE_NOTIFIER=1` |
| Verification check | `src/agentic/executor/runner.rs` | `DO_NOT_TRACK=1`, `GH_TELEMETRY=false`, `HF_HUB_DISABLE_TELEMETRY=1`, `ANONYMIZED_TELEMETRY=false` |

Each spawn site applies these last and removes case-insensitive aliases, so a
declared or inherited value cannot re-enable telemetry on Windows. This proves
delivery, not that an arbitrary child honors `DO_NOT_TRACK`.

One more child carries no request data: on macOS `process::KeepAwake` runs
`/usr/bin/caffeinate -i -w <pid>` with a fixed argv and an empty environment
while a detached web research job answers, so idle sleep does not cut the
answer short. Dropping the guard kills it, `-w` ends it if the server dies
first, and it never blocks lid-close or manual sleep. A child ended by a signal
reports `signal` in `process::Output` with `status` unset.

## MCP stdio diagnostics

MCP stdio drains and discards stderr, so a noisy child cannot fill its pipe.
When stdout closes unexpectedly, the client waits up to 50 ms for the drain and
returns a fixed `MCP_STDIO` message. To see child diagnostics, reproduce the
call with the server in a controlled environment.

Each response is one newline-delimited JSON line, refused above
`mcp.max_result_bytes` even if unterminated; `mcp.timeout_sec` and child
cleanup still apply. See [troubleshooting](TROUBLESHOOTING.md).

### Explicit MCP lifecycle policy

Every stdio server must declare [capabilities](MCP.md#migrate-a-stdio-declaration).

**Linux strict mode.** A transient systemd service owns the supervisor and the
bubblewrap child in a dedicated cgroup v2 hierarchy. The service sets aggregate
process and memory limits, disables delegation, protects cgroup control files
and kills the whole group when its main process ends. A private, authenticated
Unix socket detects loss of the harness independently of MCP stdin, and the
worker checks its cgroup membership and limits before launching the tool.
Bubblewrap creates a PID namespace, so `setsid()` cannot leave the cgroup.
Requested roots, working directory and executable cannot expose host `/proc`,
`/sys`, `/run`, `/var/run` or `/dev`; the child gets its own `/proc` and a
minimal `/dev`. `network: unrestricted` is a separate operator grant, not an
HTTP hostname allowlist. Strict mode bounds memory, process count and lifetime
but imposes no scratch disk quota; a hard-killed harness may leave its private
temporary directories behind and never deletes operator-granted outputs.

**macOS** refuses strict lifecycle containment. An explicit `process_group`
exception keeps Seatbelt filesystem and network protection, but runner death
and detached descendants can escape cleanup.

**Windows** refuses confined and strict stdio. Its explicit `job_object`
[trusted-server exception](MCP.md#explicit-windows-trusted-server-exception)
assigns a Job Object during `CreateProcessW` (Windows 10 / Server 2016 or
newer), so no child instruction runs before membership exists. Only the three
child pipe ends are inherited; the job has kill-on-close, active-process and
aggregate memory limits and neither breakaway flag. Cancellation, a deadline or
a protocol failure kills the job, an abrupt runner exit closes the kernel
handle, and detached descendants stay members. There is no OS wall-clock timer
on the job: the request deadline needs a responsive runtime. The coding
executor's check sandbox uses the same `JobChild` path (32-process limit, no job
memory limit).

## Evidence

`tests/process_lifecycle.rs` covers inherited pipes, blocked stdin, overflow and
a short-lived leader with a background child; its required macOS case cancels a
Seatbelt check in another process group while an unrelated sibling survives.
`tests/process_writer_outcome.rs` proves the indeterminate publication overflow.
`tests/mcp_client.rs` checks allowed and denied reads, writes and network probes,
and the required Linux `tests/mcp_lifecycle.rs` job covers cancellation, crashes,
deadlines and SIGKILL with a detached grandchild, then limits and migration
denial. The native Windows acceptance observes child and grandchild handles,
denied breakaway, limits and nine exit paths; it does not establish filesystem
or network isolation.

## Desktop ownership and recovery

The desktop parent owns a private inherited control pipe; EOF requests backend
shutdown and cancellation through the owners above. A server home lease
excludes a second server on Unix (`flock`) and Windows (`LockFileEx`). New
coding workers hold a per-run lease; decide, push, publish and discard take it
and reload the record under it, and `/agent runs` or status reconciliation marks
a running record interrupted only after that lease is demonstrably released.
There is no PID-file kill authority, automatic resume or approval replay, and
legacy running state stays unknown. A released worker lease proves the worker
ended, not every escaped descendant. Native window behavior:
[DESKTOP.md](DESKTOP.md).
