# bakeoff10: research routing bakeoff

Scores whether a model answers from private notes, the frozen local "internet", both, or says the answer is absent.

Prior bakeoff open report: https://brush-monarch-slate-dove.grok.me

## Layout
- `corpus/` private notes D1 to D4 (.txt).
- `web_fixture/` offline web pages W1 to W3, searched by keyword. No network.
- `cases.json` cases C0 to C5 with gold route, allowed tools, expected cites.
- `gold.json` facts, traps, routes, web facts.
- `bakeoff10.py` runner and scorer. C0 is the original stuffed HTML task.
- `modelfiles/` the `-cg` Ollama Modelfiles under test.
- `results.json`, `report.md` last recorded run; `archive/` earlier runners.

## Run
```
python3 bakeoff10.py --dry-run
python3 bakeoff10.py --quick
python3 bakeoff10.py --cases C1,C5 --models a,b --out out2
python3 bakeoff10.py --score out/A
```
Runs write scores and `summary.json`. Re-scoring writes only `rescore.json` per label.

Judge routing and leaks before citation and fact scores.

CI exercises `archive/bakeoff_harness.py`.
