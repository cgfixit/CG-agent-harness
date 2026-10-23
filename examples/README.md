# examples

Cargo examples: `cargo run --locked --example <name> -- <arguments>`. They use
disposable fixture state, not the operator's home; no cloud LLM is called.

- [`structured_memory_eval.rs`](structured_memory_eval.rs): phase 7 memory
  evaluation on `tests/fixtures/structured_memory/phase7/corpus.json`; requires
  `OUTPUT.json` ([STRUCTURED_MEMORY.md](../docs/STRUCTURED_MEMORY.md)).
- [`web_research_benchmark.rs`](web_research_benchmark.rs): research pipeline
  benchmark on `tests/fixtures/web-research/corpus.json`; requires `OUTPUT.json`.
  Add `--live` for the embedded default local-model configuration; content stays
  on loopback fixtures. Fixture configuration disables account/TLS gates.
- [`web_retrieval_benchmark.rs`](web_retrieval_benchmark.rs): offline retrieval
  timing on synthetic cached pages; no network, model or persistent index.
- [`web_extraction_probe.rs`](web_extraction_probe.rs): extraction acceptance;
  `--serve` opens a disposable authenticated HTTP console for manual checks,
  requiring `CGAGENTHARNESS_W1_PROBE_PASSWORD` for its admin login.

Fixtures: [tests/fixtures/](../tests/fixtures/README.md).
