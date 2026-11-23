# tests/fixtures

Deterministic test inputs; no live-provider credentials required.

- [`cargo-sandbox/`](cargo-sandbox/): a tiny serde crate whose build script and test
  assert filesystem and loopback-network denial; its doctest verifies compilation. Used by [`../macos_cargo.rs`](../macos_cargo.rs).
- [`structured_memory/`](structured_memory/): gate, recall and consolidation step
  JSON for [`../structured_memory.rs`](../structured_memory.rs), plus the
  `phase7/corpus.json` evaluation corpus for
  [`../structured_memory_phase7.rs`](../structured_memory_phase7.rs) and
  [examples/structured_memory_eval.rs](../../examples/structured_memory_eval.rs).
  `d7/corpus.json` is the frozen #603 suggestion-qualification corpus, pinned by
  [`../structured_memory_suggestions.rs`](../structured_memory_suggestions.rs).
- [`web-research/`](web-research/): `corpus.json`, an offline virtual-site page
  and query corpus for [examples/web_research_benchmark.rs](../../examples/web_research_benchmark.rs).
- [`research-baseline/`](research-baseline/): the #772 12-question baseline corpus proposal (no labels yet), shape-checked by [`../research_baseline_corpus.rs`](../research_baseline_corpus.rs).
