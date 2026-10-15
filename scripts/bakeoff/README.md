# scripts/bakeoff

The operator's local-model bench, vendored at version 7.1 / h1.2 so CI can run
its product gate. [`docs/bakeoff/`](../../docs/bakeoff/local-model-bakeoff-2026-10-10.md)
holds the measured results and the window guidance.

- [`bakeoff_7.py`](bakeoff_7.py): direct Ollama lanes (tag contract, speed,
  long context, the `/v1` wire check) plus the scripted fake Ollama and the
  probe corpus the harness lane imports. Pulls nothing, picks no winner.
- [`bakeoff_harness.py`](bakeoff_harness.py): the product gate through a
  disposable `cgagentharness serve`: setup-code login, `auto_tune` at a pinned
  32768 window, a long session that must compact and keep its needle, web
  injection, HVAC research. `--self-test` runs it against the fake Ollama; the
  `bakeoff-self-test` job in `ci.yml` does that on every code change.

Run on a Mac: `python3 scripts/bakeoff/bakeoff_harness.py --models <tag> --bin
target/release/cgagentharness`. Needs only Python 3 and the built binary.
