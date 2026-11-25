# Subprocess lifetime and capture

The Unix shim and synchronous argv runner share a subprocess owner and
deadline for nonblocking pipe I/O. Stdin is written alongside stdout/stderr
capture to avoid pipe deadlock. Cancellation can complete shortly after its
HTTP acknowledgement. The 4 MiB capture bound refuses oversized
stdout/stderr. Verification overflow is a setup error. Publication overflow
records an indeterminate outcome and must not be retried blindly.
Cancellation signals a blocking worker with no detached readers. The direct
child stays unreaped until group signaling completes.

macOS uses bounded `libproc` enumeration and identity checks for observed
descendants across groups. Linux uses process-group cleanup. Reparenting,
enumeration failure or a hard-killed owner can leave descendants. Filesystem
and network restrictions are separate. Browser shutdown closes the
sidecar's stdin and kills a lingering sidecar after five seconds.
The sidecar owns Chrome. No process-tree containment is claimed by this
shutdown path. See [browser policy](Browser-Use.md).

## Child environments

[Child environment policy](../src/common/child_env.rs) applies telemetry
opt-outs last for MCP, GitHub CLI and verification. The PDF worker has an
empty environment, deadlines and resource ceilings. A heap-cap marker plus
`SIGABRT` reports `PDF_LIMIT`. Other aborts remain local failures.
macOS detached research synthesis uses a scoped `caffeinate` guard for idle
sleep. The guard ends with its owner. Signals report `signal` with
`status` unset in `process::Output`.

## MCP stdio diagnostics

Stderr is drained and discarded. Early stdout close returns a fixed
`MCP_STDIO` error. Reproduce child diagnostics in a controlled environment.
Newline-delimited JSON replies are bounded by `mcp.max_result_bytes`,
including unterminated lines. `mcp.timeout_sec` bounds the call.

### Explicit MCP lifecycle policy

- **Linux strict.** systemd owns a nondelegated cgroup v2 with aggregate
  process and memory limits. Bubblewrap adds a PID namespace. The supervisor
  verifies membership and uses a private liveness socket. Scratch disk is
  unmetered. Unrestricted network requires a separate grant.
- **macOS.** Explicit `process_group` exception permits Seatbelt confinement
  with best-effort cleanup. Strict mode refuses.
- **Windows.** Explicit `job_object` trusted-server exception assigns the
  child at creation. Kill-on-close and process/memory limits cover members
  without breakaway. The runtime owns wall time. See
  [MCP platform requirements](MCP.md#explicit-windows-trusted-server-exception).

## Desktop ownership and recovery

The desktop's inherited control pipe requests backend shutdown on EOF.
Unix `flock` and Windows `LockFileEx` serialize home ownership. Coding
operations reload under the per-run lease. Reconciliation marks an
interrupted worker only when its lease is demonstrably released. Legacy
running state stays unknown. Lease release establishes worker termination.
See [desktop recovery](DESKTOP.md#closing-quitting-and-recovery).
Verification lives in `tests/process_lifecycle.rs`,
`tests/process_writer_outcome.rs` and the platform MCP lifecycle tests.
