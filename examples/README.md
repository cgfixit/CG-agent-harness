# examples

Cargo examples, run with `cargo run --locked --example <name>`. All use fixture
or local models; none call a cloud LLM or flip a gate.

- [`structured_memory_eval.rs`](structured_memory_eval.rs): phase 7 memory
  evaluation on `tests/fixtures/structured_memory/phase7/corpus.json`; writes a
  results JSON ([STRUCTURED_MEMORY.md](../docs/STRUCTURED_MEMORY.md)).
- [`web_research_benchmark.rs`](web_research_benchmark.rs): research pipeline
  benchmark on `tests/fixtures/web-research/corpus.json`; `--live` drives the
  configured local model.
- [`web_retrieval_benchmark.rs`](web_retrieval_benchmark.rs): offline retrieval
  timing on synthetic cached pages; no network, model or index.
- [`web_extraction_probe.rs`](web_extraction_probe.rs): extraction acceptance;
  `--serve` opens a disposable authenticated console for manual checks.

Fixtures: [tests/fixtures/](../tests/fixtures/README.md).
