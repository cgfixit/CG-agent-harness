# bakeoff10 — private-corpus research infographic

Compares three local `-cg` Ollama tags on the cg-researcher private-document workflow: fixed corpus → cited claims → bounded connections → one research task → single-file HTML infographic.

## Models
- `gemma4-26b-a4b:ud-q4_k_xl-cg`
- `qwen3.8:27b-mlx-cg`
- `qwen3.6:35b-a3b-cg`

## Wire
Ollama `POST /v1/chat/completions` at temperature 0.2. Do not send `reasoning_effort`. Strip `<think>` and channel leaks after. Context stays at the Modelfile 32k unless truncation is observed.

## Run
```bash
cd scripts/bakeoff
python3 bakeoff10.py --dry-run
python3 bakeoff10.py                 # all three, serial
python3 bakeoff10.py --quick         # first model only
python3 bakeoff10.py --score out
```
Prints `starting A=...` immediately, then a JSON meta line after each model. Expect ~3–4 minutes total on M5 Pro 48GB (ballpark from one probe run).

## Scoring (judge-facing)
0–3 each: citation correctness, useful synthesis, restraint, task selection, format compliance, operational cost.
Pass gate for promoting a model: at least 2 of 3 clear citation+restraint+valid HTML on the fixed corpus. Auto `--score` is a hint only; read trap hits in context.

## Pending results
A Mac re-run of `python bakeoff10.py` was started ~2026-10-29 19:54 ET. Update `results.json` and the PR once that `out/` lands. Probe baseline numbers live in `results.json` under status `probe_complete_pending_rerun`.
