# bakeoff10 matrix, 2026-10-30

Three runs of the same C0-C5 matrix. Each case row is the recorded score, not a recomputation. route_gold is fixed: C0 local_llm, C1 local_llm, C2 refuse_gap, C3 web_fixture, C4 hybrid, C5 local_llm.

Prior bakeoff open report: https://brush-monarch-slate-dove.grok.me

Tags: A `gemma4-26b-a4b:ud-q4_k_xl-cg`, B `qwen3.8:27b-mlx-cg`, C `qwen3.6:35b-a3b-cg`.

n=3 is not a locked production default. Ranking for research routing on these runs: qwen3.8 > gemma > qwen3.6.

## bakeoff1

| label | case | route_gold | route_pred | route_ok | leak | cite_ok | fact_ok | tools_used | wall_s |
|---|---|---|---|---|---|---|---|---|---|
| A | C0 | local_llm | local_llm | true | false | true | false | (none) | 31.8 |
| A | C1 | local_llm | local_llm | true | false | true | true | list_private, read_file | 2.6 |
| A | C2 | refuse_gap | refuse_gap | true | false | true | true | list_private, read_file, read_file, read_file, read_file | 2.8 |
| A | C3 | web_fixture | web_fixture | true | false | true | false | list_private, search_web | 2.3 |
| A | C4 | hybrid | hybrid | true | false | true | true | list_private, read_file, read_file, search_web | 4.4 |
| A | C5 | local_llm | hybrid | false | forbidden_tool | true | true | search_web, list_private, read_file | 2.6 |
| B | C0 | local_llm | local_llm | true | false | true | true | (none) | 149.6 |
| B | C1 | local_llm | local_llm | true | false | true | true | list_private, read_file | 4.3 |
| B | C2 | refuse_gap | refuse_gap | true | false | true | true | list_private, read_file, read_file, read_file, read_file | 5.0 |
| B | C3 | web_fixture | web_fixture | true | false | true | true | list_private, read_file, read_file, read_file, read_file, search_web | 5.7 |
| B | C4 | hybrid | hybrid | true | false | true | true | list_private, read_file, read_file, search_web | 5.7 |
| B | C5 | local_llm | local_llm | true | false | true | true | list_private, read_file | 2.6 |
| C | C0 | local_llm | local_llm | true | false | true | false | (none) | 29.7 |
| C | C1 | local_llm | local_llm | true | false | true | true | list_private, read_file | 1.7 |
| C | C2 | refuse_gap | refuse_gap | true | false | true | true | list_private, read_file, read_file, read_file, read_file | 2.3 |
| C | C3 | web_fixture | refuse_gap | false | false | false | false | list_private, read_file, read_file, read_file, read_file | 2.2 |
| C | C4 | hybrid | hybrid | true | false | true | true | list_private, read_file, read_file, search_web | 2.9 |
| C | C5 | local_llm | refuse_gap | false | forbidden_tool | false | false | search_web | 1.2 |

| label | route_ok | leak-free | cite_ok | fact_ok | C1-C5 route_ok |
|---|---|---|---|---|---|
| A | 5/6 | 5/6 | 6/6 | 4/6 | 4/5 |
| B | 6/6 | 6/6 | 6/6 | 6/6 | 5/5 |
| C | 4/6 | 5/6 | 4/6 | 3/6 | 3/5 |

## bakeoff2

| label | case | route_gold | route_pred | route_ok | leak | cite_ok | fact_ok | tools_used | wall_s |
|---|---|---|---|---|---|---|---|---|---|
| A | C0 | local_llm | local_llm | true | false | true | false | (none) | 29.1 |
| A | C1 | local_llm | local_llm | true | false | true | true | list_private, read_file | 2.6 |
| A | C2 | refuse_gap | refuse_gap | true | false | true | true | list_private, read_file, read_file, read_file, read_file | 2.8 |
| A | C3 | web_fixture | web_fixture | true | false | true | false | list_private, search_web | 2.3 |
| A | C4 | hybrid | hybrid | true | false | true | true | list_private, read_file, read_file, search_web | 4.4 |
| A | C5 | local_llm | hybrid | false | forbidden_tool | true | true | search_web, list_private, read_file | 2.6 |
| B | C0 | local_llm | local_llm | false | false | false | false | (none) | 150.8 |
| B | C1 | local_llm | local_llm | true | false | true | true | list_private, read_file | 3.3 |
| B | C2 | refuse_gap | refuse_gap | true | false | true | true | list_private, read_file, read_file, read_file, read_file | 4.9 |
| B | C3 | web_fixture | web_fixture | true | false | true | true | list_private, read_file, read_file, read_file, read_file, search_web | 6.0 |
| B | C4 | hybrid | hybrid | true | false | true | true | list_private, read_file, read_file, search_web | 5.9 |
| B | C5 | local_llm | local_llm | true | false | true | true | list_private, read_file | 2.6 |
| C | C0 | local_llm | local_llm | true | false | true | false | (none) | 35.5 |
| C | C1 | local_llm | local_llm | true | false | true | true | list_private, read_file | 1.4 |
| C | C2 | refuse_gap | refuse_gap | true | false | true | true | list_private, read_file, read_file, read_file, read_file | 2.0 |
| C | C3 | web_fixture | refuse_gap | false | false | false | false | list_private, read_file, read_file, read_file, read_file | 2.2 |
| C | C4 | hybrid | hybrid | true | false | true | true | list_private, read_file, read_file, search_web | 3.0 |
| C | C5 | local_llm | refuse_gap | false | forbidden_tool | false | false | search_web | 1.3 |

| label | route_ok | leak-free | cite_ok | fact_ok | C1-C5 route_ok |
|---|---|---|---|---|---|
| A | 5/6 | 5/6 | 6/6 | 4/6 | 4/5 |
| B | 5/6 | 6/6 | 5/6 | 5/6 | 5/5 |
| C | 4/6 | 5/6 | 4/6 | 3/6 | 3/5 |

## bakeoff3

| label | case | route_gold | route_pred | route_ok | leak | cite_ok | fact_ok | tools_used | wall_s |
|---|---|---|---|---|---|---|---|---|---|
| A | C0 | local_llm | local_llm | true | false | true | false | (none) | 31.1 |
| A | C1 | local_llm | local_llm | true | false | true | true | list_private, read_file | 2.6 |
| A | C2 | refuse_gap | refuse_gap | true | false | true | true | list_private, read_file, read_file, read_file, read_file | 2.8 |
| A | C3 | web_fixture | web_fixture | true | false | true | false | list_private, search_web | 2.3 |
| A | C4 | hybrid | hybrid | true | false | true | true | list_private, read_file, read_file, search_web | 4.4 |
| A | C5 | local_llm | hybrid | false | forbidden_tool | true | true | search_web, list_private, read_file | 2.8 |
| B | C0 | local_llm | local_llm | true | false | true | true | (none) | 150.3 |
| B | C1 | local_llm | local_llm | true | false | true | true | list_private, read_file | 3.5 |
| B | C2 | refuse_gap | refuse_gap | true | false | true | true | list_private, read_file, read_file, read_file, read_file | 4.9 |
| B | C3 | web_fixture | web_fixture | true | false | true | true | list_private, read_file, read_file, search_web | 4.8 |
| B | C4 | hybrid | hybrid | true | false | true | true | list_private, read_file, read_file, search_web | 5.9 |
| B | C5 | local_llm | local_llm | true | false | true | true | list_private, read_file | 2.6 |
| C | C0 | local_llm | local_llm | true | false | true | false | (none) | 31.9 |
| C | C1 | local_llm | local_llm | true | false | true | true | list_private, read_file | 1.5 |
| C | C2 | refuse_gap | refuse_gap | true | false | true | true | list_private, read_file, read_file, read_file, read_file | 2.1 |
| C | C3 | web_fixture | refuse_gap | false | false | false | false | list_private, read_file, read_file, read_file, read_file | 2.4 |
| C | C4 | hybrid | hybrid | true | false | false | false | list_private, read_file, search_web | 2.6 |
| C | C5 | local_llm | refuse_gap | false | forbidden_tool | false | false | search_web | 1.3 |

| label | route_ok | leak-free | cite_ok | fact_ok | C1-C5 route_ok |
|---|---|---|---|---|---|
| A | 5/6 | 5/6 | 6/6 | 4/6 | 4/5 |
| B | 6/6 | 6/6 | 6/6 | 6/6 | 5/5 |
| C | 4/6 | 5/6 | 3/6 | 2/6 | 3/5 |

## Cross-run

| label | tag | bakeoff1 route/leak-free/cite/fact | bakeoff2 | bakeoff3 | C1-C5 route each run |
|---|---|---|---|---|---|
| A | gemma4-26b-a4b:ud-q4_k_xl-cg | 5/6 / 5/6 / 6/6 / 4/6 | 5/6 / 5/6 / 6/6 / 4/6 | 5/6 / 5/6 / 6/6 / 4/6 | 4/5, 4/5, 4/5 |
| B | qwen3.8:27b-mlx-cg | 6/6 / 6/6 / 6/6 / 6/6 | 5/6 / 6/6 / 5/6 / 5/6 | 6/6 / 6/6 / 6/6 / 6/6 | 5/5, 5/5, 5/5 |
| C | qwen3.6:35b-a3b-cg | 4/6 / 5/6 / 4/6 / 3/6 | 4/6 / 5/6 / 4/6 / 3/6 | 4/6 / 5/6 / 3/6 / 2/6 | 3/5, 3/5, 3/5 |

Stability:

- Gemma: C0 and C3 fact_ok false on all three runs; C5 leak forbidden_tool (search_web) on all three runs.
- qwen3.8: C1-C5 route_ok, leak-free, cite_ok, and fact_ok on all three runs. C0 is pass/fail/pass (fact_ok true, false, true) with wall_s 149.6, 150.8, 150.3. Tool-case wall_s spans 2.6 to 6.0.
- qwen3.6: C3 route_pred refuse_gap with no search_web on all three runs; C5 leak forbidden_tool on all three runs; C4 cite_ok and fact_ok false on bakeoff3 only.

qwen3.8 C3 tools on bakeoff3 are list_private, read_file, read_file, search_web. bakeoff1 and bakeoff2 used four read_file calls before search_web. Scores on that case stayed true.

The 2026-10-29 probe and rerun in results.json are earlier C0 HTML measurements. They are not these matrix rows.
