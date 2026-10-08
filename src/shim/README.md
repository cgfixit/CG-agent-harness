# src/shim

The one crossing of the I6 boundary. `ACTIONS` is the 12-entry whitelist of
`cgagentharness agentic <action>` children the server may spawn; `JSON_ACTIONS`
names the ones whose stdout is parsed. Argv is a fixed list, never a shell
string; timeouts here are duplicated on purpose in `agentic/`. A new action
touches this file, `agentic/commands.rs::dispatch` and `tests/invariant_guard.rs`
together. Core path: read [INVARIANTS.md](../../INVARIANTS.md) first.
