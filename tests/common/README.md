# tests/common

Shared test fixtures, `#[path]`-included by the integration tests.

- [`mod.rs`](mod.rs): `TestServer` and `ServerOptions` (a temp harness home and an
  in-process server on `127.0.0.1:0`), `MockModel` (OpenAI-compatible),
  `real_bare_repo`, `install_fake_gh`, `run_agentic`, and
  `isolate_write_kill_switch`, which clears `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE`
  so write-gate tests fail on the gate under test, not the operator's environment.

`config_with` starts from the shipped YAML but overrides tuning and compaction
for stable protocol fixtures. [`../shipped_config.rs`](../shipped_config.rs) checks
actual defaults separately, and fails by name on a quoted shipped model tag in
tests or scripts. A test asserting a default pins it with `.with(...)` or reads it
with `shipped_str`; scripts edit config by key, never by matching a value. `prepare_server` builds options without listening;
`spawn_server` starts the owned loopback server.

Not to be confused with [`../common_layer.rs`](../common_layer.rs), which tests `src/common`.
