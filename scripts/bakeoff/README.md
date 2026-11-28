# bakeoff10: research routing bakeoff

C0 scores an HTML answer from supplied notes. C1–C5 grade tool use against `cases.json`. A routing pass requires a completed answer, the requested route, permitted calls and recorded results for the required sources. C2 requires consulting private notes before reporting a gap.

**Historical correction.** The October 30 matrix in `results.json` records six incorrect C3 passes. Across three runs, A listed private files and B also read them in a web-only case. The tables in `report.md` preserve the original grading. The summaries lack raw transcripts for a complete re-score or revised model ranking.

## Run
```
python3 bakeoff10.py --dry-run
python3 bakeoff10.py --quick
python3 bakeoff10.py --cases C1,C5 --models a,b --out out2
python3 bakeoff10.py --score out/A
```
Writes `out/<label>/<case>.score.json` and `summary.json`.

Judge on route_ok and leak first, then cites and facts. Wall time is secondary.

CI self-test invokes the archived harness runner at `scripts/bakeoff/archive/bakeoff_harness.py`.
