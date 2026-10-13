# tests/netconnect_support

- [`flag_table.rs`](flag_table.rs): `FlagCase` and `flag_cases()`: every
  combination of the netconnect master switch, one tier flag and scope, built
  from `Tier::ALL` so there is no second flag list. `#[path]`-included and
  iterated by [`../netconnect_flags.rs`](../netconnect_flags.rs), which checks
  tool registration and refusals per combination.

Doc: [docs/netconnect.md](../../docs/netconnect.md#gates).
