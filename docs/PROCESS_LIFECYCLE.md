# Unix subprocess lifetime and capture

The synchronous argv runner and Unix server shim share one subprocess owner.
Pipe reads and stdin writes are nonblocking and share the operation deadline.
Capture refuses more than 4 MiB of aggregate raw stdout/stderr bytes instead of
returning successful truncated machine data. This fixed internal safety ceiling
also applies to Git/gh output. Verification capture failure is a setup/resource
error, not a request for another model attempt. A publishing command can have
succeeded before capture fails: the writer reports an indeterminate outcome,
records its operation and does not retry it.

The shim uses a blocking worker with an atomic cancellation flag. Aborting the
owning future sets that flag; the worker observes it during bounded I/O and
closes its pipes. No independent reader tasks are detached. Cancellation is
cooperative and cleanup may finish shortly after the HTTP acknowledgement.
The direct child remains unreaped until cleanup, so its process-group identity
cannot be reused before group signaling. This also covers ordinary background
children whose leader exits while they retain its pipes.

On macOS, the owner records descendants through public `libproc` interfaces,
including children in separate process groups. Cleanup stops observed parents,
checks process start identity again, then kills observed descendants. Traversal
has time, count and per-parent buffer ceilings. This improves actual nested
check cancellation without changing Seatbelt permissions.

**Ancestry cleanup is best-effort, not process containment.** A child that forks
and reparents before observation may escape; enumeration can fail or truncate.
Start-time checking and signaling are separate syscalls. Server SIGKILL or a
crash does not run the owner's destructor. Linux keeps group cleanup without
macOS descendant enumeration. The non-Unix runners retain their prior behavior;
these Unix capture/lifetime claims do not apply to Windows.

Filesystem and network denial are separate boundaries. There is still no
per-run disk quota or general memory/process-count limit. Kernel calls and
cleanup/reaping can add time beyond the requested deadline. Use isolated
fixtures and inspect surviving work after interruption; the console correctly
keeps its descendant-survival warning.

## Evidence

`tests/process_lifecycle.rs` exercises inherited pipes in both runners, blocked
stdin, output overflow, complete JSON/diagnostic output and exit status, and a
short-lived leader with a background child. Its required macOS test cancels a
JobStore task owning an actual Seatbelt check in a different process group and
asserts that both wrapper/check stop while an unrelated sibling survives.
Missing native sandbox capability fails that test.

`tests/process_writer_outcome.rs` uses a local fake `gh` that records one accepted
mutation then floods stdout. It verifies exactly one invocation, an indeterminate
error identifying the operation, and an audit record. No remote service is used.

CI exposed timer throttling from sleeping after ready output chunks. The runner
now continues draining ready data while retaining per-chunk budget/cancellation
checks; it sleeps only when neither stream advances. The early-exit fixture
uses one second for process startup and a five-second descendant, preserving
its termination assertion; the separate 100 ms deadline regressions remain.
