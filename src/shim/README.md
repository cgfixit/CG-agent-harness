# src/shim

The one crossing of the I6 boundary. **Core path**: read
[INVARIANTS.md](../../INVARIANTS.md) first.

- [`mod.rs`](mod.rs): `ACTIONS`, the 12-entry whitelist of `cgagentharness agentic <action>`
  children the server may spawn, and `JSON_ACTIONS` (10; `status` and `test` are
  text-only). Builds a fixed argv list, never a shell string; free text travels as
  one `--opt=value` element or a temp file. Timeouts here are duplicated on
  purpose in `agentic/`.

A new action touches this file, `agentic/commands.rs::dispatch` and
[tests/invariant_guard.rs](../../tests/invariant_guard.rs) together. Tests:
[tests/shim_and_agent_routes.rs](../../tests/shim_and_agent_routes.rs).
