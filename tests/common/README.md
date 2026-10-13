# tests/common

Shared test fixtures, `#[path]`-included by the integration tests.

- [`mod.rs`](mod.rs): `TestServer` and `ServerOptions` (a temp harness home and an
  in-process server on `127.0.0.1:0`), `MockModel` (OpenAI-compatible),
  `real_bare_repo`, `install_fake_gh`, `run_agentic`, and
  `isolate_write_kill_switch`, which clears `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE`
  so write-gate tests fail on the gate under test, not the operator's environment.

Not to be confused with [`../common_layer.rs`](../common_layer.rs), which tests `src/common`.
