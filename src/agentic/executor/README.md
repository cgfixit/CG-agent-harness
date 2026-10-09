# src/agentic/executor

Runs verification checks inside a sandbox: `sandbox.rs` (core path: Seatbelt on
macOS, bubblewrap on Linux, no unconfined fallback), `prepared.rs` and
`manifest.rs` (what a run may touch), `runner.rs` (bounded execution),
`apply.rs` (landing a judged result). Nothing lands before it is judged; see
[INVARIANTS.md](../../../INVARIANTS.md).
