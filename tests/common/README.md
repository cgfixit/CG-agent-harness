# tests/common

Shared test fixtures (`mod.rs`): a temp harness home, an in-process server on
`127.0.0.1:0`, a mock OpenAI-compatible model, a fake `gh` and a bare git
remote. It also isolates `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` so write-gate
tests fail on the gate under test, not on the operator's environment.
