# src/agentic/executor

Runs verification checks inside a hard sandbox. Nothing lands before it is
judged, and nothing here commits; see [INVARIANTS.md](../../../INVARIANTS.md)
and [docs/CODING_PIPELINE.md](../../../docs/CODING_PIPELINE.md#offline-cargo-verification).

- [`mod.rs`](mod.rs): declares the modules; re-exports `run_verification`, `Check`, `production_sandbox`, `HardSandbox`.
- [`apply.rs`](apply.rs): `prove_disposable_copy`: copy the candidate tree, re-verify its digest, destroy it.
- [`manifest.rs`](manifest.rs): acceptance digest over run id, HEAD, path hashes and modes; re-verified at approve.
- [`prepared.rs`](prepared.rs): operator-prepared, lock-bound Cargo sources (`CargoInputs`); no dependency retrieval.
- [`runner.rs`](runner.rs): runs fixed `Check` argv lists in the sandbox with a scrubbed env and timeouts.
- [`sandbox.rs`](sandbox.rs): **core path.** Seatbelt; Linux bubblewrap with network-only netns fallback; Windows Job Object process containment. No unconstrained fallback.

Tests: [tests/linux_bwrap.rs](../../../tests/linux_bwrap.rs), [tests/macos_cargo.rs](../../../tests/macos_cargo.rs),
[tests/exhausted_run.rs](../../../tests/exhausted_run.rs).
