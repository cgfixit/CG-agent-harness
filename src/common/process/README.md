# src/common/process

The Unix half of the argv-list subprocess runner in [`../process.rs`](../process.rs).
There is no shell-string path; MCP stdio children have their own owner in
[`../mcp.rs`](../mcp.rs).

- [`unix.rs`](unix.rs): `run`: nonblocking pipe capture under one deadline and
  byte ceiling, process-group kill, cooperative cancel flag.

Related: [docs/PROCESS_LIFECYCLE.md](../../../docs/PROCESS_LIFECYCLE.md),
[tests/process_lifecycle.rs](../../../tests/process_lifecycle.rs).
