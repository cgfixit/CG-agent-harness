# scripts/bakeoff

Local-model bench, versions 7.1 / h1.2. [Recorded results](../../docs/bakeoff/local-model-bakeoff-2026-10-10.md)
explain the measured window guidance; rerun before making current model claims.

- [`bakeoff_7.py`](bakeoff_7.py): direct Ollama lanes (tag contract, speed,
  long context, the `/v1` wire check) plus the scripted fake Ollama and the
  probe corpus the harness lane imports. Pulls nothing, picks no winner.
- [`bakeoff_harness.py`](bakeoff_harness.py): the product gate through a
  disposable `cgagentharness serve`: setup-code login, `auto_tune` at a pinned
  32768 window, a long session that must compact and keep its needle, web
  injection, HVAC research. `--self-test` runs it against the fake Ollama; the
  `bakeoff-self-test` CI job runs that backend gate, not native UI acceptance.

Run on a Mac: `python3 scripts/bakeoff/bakeoff_harness.py --models <tag> --bin
target/release/cgagentharness`. `--models` only filters the built-in profile
list; a new tag needs `--extra-model <tag> --models <tag>`. Uses Python's standard
library and the built binary; real runs also need running Ollama and installed tags.
