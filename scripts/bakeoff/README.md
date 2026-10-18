# scripts/bakeoff

The operator's local-model bench, at version 8.0 / h2.1. [`docs/bakeoff/`](../../docs/bakeoff/local-model-bakeoff-2026-10-10.md)
holds measured results and window guidance.

- `check_harness_sync.py`: run first. Fails when this checkout is behind
  `origin/main` or a constant the bench mirrors drifted.
- `bakeoff_8.py`: direct Ollama lanes, the exact echo-back token ruler, the
  indirect `read_file` injection probe, the fake Ollama.
- `bakeoff_harness.py`: the product gate through a disposable `serve`
  (setup-code login, pinned 32768 window, pinned and mid-session needles read
  from the stored session). CI runs its `--self-test`.
- `qwen_needle_test.py`: compaction layouts (legacy main, PR #513, #513 plus
  the fact ledger) times needle position, with where each miss died.
- `bakeoff_7.py`: the 2026-10-14 bench, kept for comparison.
