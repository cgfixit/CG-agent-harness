# src

The `cgagentharness` crate. `main.rs` is the CLI; `lib.rs` wires the modules.
Isolation (I6): `server/`, `shim/`, `llm/`, `common/` and `netconnect/` never
reference `agentic/`, and `agentic/` never references the server or shim. The
only crossing is `shim/`, which spawns `cgagentharness agentic <action>` as a
child process. `tests/invariant_guard.rs` fails the build if that changes.
Contracts: [INVARIANTS.md](../INVARIANTS.md).
