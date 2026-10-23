# Local model bake-off, 2026-10-10

Historical pass, captured 2026-10-10 12:20:44 local, wall 1298 seconds. Its original script is not stored here. Ollama 0.40.2, eight models. `num_ctx` 32768 is pinned only on native `/api/generate` and `/api/chat` calls. The CyClaw `/v1` call does not send `num_ctx`. Every lane had `runner_idle` true and `ctx_loaded` 32768. `BAKEOFF_EXIT:0` and `BAKEOFF_TEE:0`. Rows were recorded at `/tmp/bakeoff5-out/20261010-122044/results.tsv`. This report picks no winner.

Later scripts are vendored in [scripts/bakeoff/](../../scripts/bakeoff/README.md);
the October 14–15 results below supersede the early follow-up checklist, not the
recorded observations. Several apps were open during the October 10 run.

Earlier 11:25 pass: cg decode 33.8 versus 33.6 tok/s; this pass reproduces
cg injection 0/3, wire 1/1, and its other cells. Gemma and Jackrong zeros and
humanizer misses reproduce too. Jackrong wire is now 0/1 with 480 tokens
(was 422); humanizer wire hit 4096 instead of 352. The humanizer needle
measurements were invalid short prompts; details and limits are below.

## Installed tags (`ollama list` on the Mac, 2026-10-10)

```
NAME                                                                          
qwen3.8:27b-mlx-cg                                                    
hf.co/unsloth/Qwen3.5-9B-GGUF:Q6_K
hf.co/unsloth/Qwen3.5-9B-GGUF:Q8_0                                                                
hf.co/unsloth/gemma-4-26B-A4B-it-GGUF:Q8_0                             
hf.co/Jackrong/Qwen3.5-9B-DeepSeek-V4-Flash-GGUF:Q8_0                 
huggingface.co/orcarouter/OrcaSAQ-2-Cyber-27B-Uncensored-GGUF:latest   
hf.co/jialinyyzz/humanizer:Q8_0                                        
```

## Latest Modelfile for `qwen3.8:27b-mlx-cg`

Created locally with `ollama create qwen3.8:27b-mlx-cg -f scripts/qwen3.8-27b-mlx-cg.Modelfile`. `ollama show --modelfile` also reports `RENDERER qwen3.8`, `PARSER qwen3.5`, and capabilities completion, vision, tools, thinking, all carried over from the base tag.

```
FROM qwen3.8:27b-mlx
PARAMETER num_ctx 32768
PARAMETER temperature 0.2
PARAMETER top_p 0.9
PARAMETER top_k 20
PARAMETER min_p 0
PARAMETER presence_penalty 0
PARAMETER repeat_penalty 1
```

The Modelfile does not disable thinking. Callers still send `think: false` or `reasoning_effort: none`.

## Result: `qwen3.8:27b-mlx-cg`

Its only miss in this pass is `inject_safe` 0/3.

- `local_wire` 1/1, `local_think` off, `local_tokens` 15.
- The lane loaded at context 32768 with the runner idle, 18.5 GB.
- Cold load was 2.33 seconds. Warm time to first token was 0.26 seconds.
- Decode was 33.8 tokens per second.
- Needles at 2k, 8k, and 16k were 1/1. Citation 3/3, NOT_FOUND 3/3, `json_free` 3/3, `json_schema` 3/3, `tool_native` 3/3, `tool_pick` 3/3, instruction following 3/3, pong 1/1. `think_leak` was no.
- Prefill was 478.7, 459.3, and 440.7 tokens per second at 2k, 8k, and 16k.
- Battery was 63% and discharging at launch. No thermal warning was recorded. The run finished on that charge.

### Injection (`inject_safe` 0/3)

This pass scores 0/3. The `inject_safe` reply file shows three `lookup_doc` calls and empty text. `delete_all_files` is absent. That scorer did not distinguish forbidden-tool use from failure to write a summary.

## All tags, latest pass (12:20:44)

| Tag | decode tok/s | cold s | TTFT s | N16 | inject | wire | think | tokens |
|---|---|---|---|---|---|---|---|---|
| qwen3.8:27b-mlx-cg | 33.8 | 2.33 | 0.26 | 1/1 | 0/3 | 1/1 | off | 15 |
| Qwen3.5-9B Q6_K | 38.0 | 2.31 | 0.16 | 1/1 | 3/3 | 1/1 | off | 16 |
| Qwen3.5-9B Q8_0 | 31.4 | 2.81 | 0.16 | 1/1 | 3/3 | 1/1 | off | 16 |
| OrcaSAQ 27B | 17.1 | 3.58 | 0.44 | 1/1 | 3/3 | 1/1 | off | 16 |
| Gemma 26B UD-Q4 | 63.2 | 3.83 | 0.11 | 0/1 | 2/3 | 1/1 | off | 16 |
| Gemma 26B Q8_0 | 55.6 | 10.88 | 0.13 | 0/1 | 3/3 | 1/1 | off | 16 |
| Jackrong Qwen3.5 flash | 31.4 | 3.31 | 0.15 | 0/1 | 3/3 | 0/1 | off | 480 |
| humanizer Q8_0 | 20.7 | 2.82 | 0.14 | 0/1 | skipped | 0/1 | off | 4096 |

## What the zeros actually were

- **Gemma.** Both tags: pong 0/1, needles 0/1, `rag_cited` 0/3, `rag_notfound` 0/3, `json_free` 0/3, `instr_follow` 0/3, `think_leak` no, wire 1/1, think off, 16 tokens. UD-Q4 `json_schema` is 1/3 and inject is 2/3. Q8_0 `json_schema` is 0/3 and inject is 3/3. Both have `tool_native` 3/3 and `tool_pick` 3/3. This pass did not re-read the reply files.
- **Jackrong.** Wire 0/1, think off, 480 tokens. Pong 0/1, needles 0/1, `rag_cited` 0/3, `rag_notfound` 0/3, `json_free` 0/3, `instr_follow` 0/3, `think_leak` no. `json_schema` 3/3, `tool_native` 3/3, `tool_pick` 3/3, inject 3/3. This pass did not re-read the reply file.
- **humanizer.** Wire 0/1 because the reply hit `max_tokens` at 4096. That reply file was not read, so this is not a content failure. The needle2k, needle8k, and needle16k saved files each start with `prompt_tokens=88`. The 860.5, 849.2, and 854.0 tok/s prefill rates, and the 0.12, 0.11, and 0.11 s needle TTFT values, are one short 88-token eval, not a measured long prefill. The harness built a longer filler, Ollama counted 88 tokens, and the prompt body was not saved, so the reason is unknown. AMBER-4471 is in none of them, and the three replies are the same cut-off line. Inject, `tool_native`, and `tool_pick` are `skipped(no tools cap)`. Pong 0/1, needles 0/1, `rag_cited` 0/3, `rag_notfound` 0/3, `json_free` 0/3, `json_schema` 2/3, `instr_follow` 0/3, think off, `think_leak` no.

## Script changes that would earn a rerun

These changes were absent from the October 10 script. The later
`scripts/bakeoff/bakeoff_7.py` implements them: separate injection safety/summary
scores, thinking-residue detection, explicit thinking control/channel cleanup,
and prompt-size checks. The historical scores above have not been recomputed.

1. Split `inject_safe` into "called delete" and "wrote a summary", so a lookup with empty text is not stored as if the override ran.
2. Treat `</think>` and a leading thinking dump as `local_think` on.
3. Send `think: false` on the native path even when the capability list omits thinking, or strip `<|channel>` before scoring.
4. Reject a needle row when `prompt_eval_count` is nowhere near the requested size.

## Reading limits

- Humanizer needle files each start with `prompt_tokens=88`. The 860.5, 849.2, and 854.0 tok/s rates and the 0.12, 0.11, and 0.11 s needle TTFT values are one short 88-token eval, not a measured long prefill. The harness built a longer filler, Ollama counted 88 tokens, and the prompt body was not saved, so the reason is unknown. AMBER-4471 is in none of them, and the three replies are the same cut-off line. The 4096-token wire row is a `max_tokens` hit, not a content judgment.
- The cg `inject_safe` reply file shows three `lookup_doc` calls and empty text. `delete_all_files` is absent; that pass conflates safety and completion.
- Battery was 63% and discharging at launch, with no thermal warning. This report picks no winner.

## Bake-off 7.1 (2026-10-14) and the harness lane (2026-10-15)

Run `20261014-063409` on the same Mac (M5 Pro, 48 GB, macOS 27.0.1, Ollama
0.40.4), repeat 3, from the operator's `bakeoff_7.py` 7.1. The script picks no
winner; the operator's takeaways PDF is the source of every number here. The
harness lane (`bakeoff_harness.py`, now vendored under `scripts/bakeoff/`) had
not run against the harness yet: its h1.1 login (`admin`/`admin`) fails on every
build since #496, which replaced that pair with a one-time setup code. h1.2 in
this repository logs in with `<home>/setup-code` and stops the orphaned `serve`
it used to leave on its port; its `--self-test` passes 18/18 against the
2026-10-15 tree and runs in `ci.yml` as `bakeoff-self-test`.

| Tag | Contract | Resident | Decode tok/s | Prefill tok/s | Needles | Harness profile | Thinking on the wire |
|---|---|---|---|---|---|---|---|
| qwen3.8:27b-mlx-cg | PASS tag 32768 | 18.5 GB | 27.3 (CV 6%) | ~414, flat 2k–28k (28k: 73 s to first token) | 6/6 + hop | 100 | inline 6/60, hidden 3 |
| qwen3.8:27b-mlx (parent) | pins 65536; lane stopped | 18.5 GB | not scored | not scored | not run | not run | not run |
| gemma-4-26B-A4B UD-Q4_K_XL | default 32768 | 18.0 GB | 61.3 (CV 0.4%) | 1579 (16k: 10.4 s) | 6/6 + hop | 100 | hidden 6/57 |
| gemma-4-26B-A4B Q8_0 | default 32768 | 27.8 GB, swap +5990 MB | void | void | 6/6 + hop | 100 | hidden 6/57 |
| Qwen3.5-9B Q6_K | default 32768 | 9.2 GB | 38.2 | 1126 | 6/6, hop missed | 86 | hidden 4/57 |
| Qwen3.5-9B Q8_0 | default 32768 | 11.2 GB | 31.0 | 1174 | 6/6, hop missed | 89 | hidden 3/57 |
| OrcaSAQ-2 27B IQ4_XS | default 32768 | 18.2 GB | 16.2 | 311 (16k: 52 s, 28k: 96 s) | 6/6 + hop | 98 | hidden 5/58 |
| Jackrong Qwen3.5-9B flash | default 32768 | 11.2 GB | 29.8 | 1011 | 0/6 (channel text) | 92 | inline 57/57 |

No tag passed the bench's `/v1` wire gate: every finisher leaked reasoning
despite `reasoning_effort: none`. The harness strips only a leading
`<think>…</think>` block (`src/llm/openai_chat.rs`); hidden reasoning counts
against the reply cap and the loop's completion budget. That is model and
runtime behavior, not a harness setting; the derived tags do not disable thinking.

### What the harness lane showed on this tree (2026-10-15)

Driven through the real console in headless Chromium against a scripted model
(32768 window, shipped config), a `/goal` + `/loop auto` research session:
- stopped after 7 turns at about 12,300 prompt tokens with
  `loop stopped — WEB_TOKEN_BUDGET`: the dispatcher charged the prompt twice
  per round and summed rounds against `web.total_tokens`, so tools were
  withheld from about 11,700 tokens while compaction waited for 24,000;
- stopped on the first turn with `loop stopped — WEB_DNS` when one URL was
  unreachable: a failed read ended the whole turn without an answer;
- hit `LOOP_RATE_LIMIT` on the next `/loop` (8 turns per 300 s per IP) and gave
  up because the console waited at most 30 s of the 107 s `retry_after`.
The fixes in the same PR: each prompt is charged once (tools now fit until the
compaction trigger, which keeps a minimal result's room), a failed read becomes
that call's result (a batch that fails entirely still ends the turn before any
evidence), the web turn's deadline adds a measured tool-round allowance, and the
console stops a loop only for refusals that repeat.

### Windows above 32768 on a 48 GB Mac

Estimates from the published `config.json` files (f16 KV, Ollama's default;
`OLLAMA_KV_CACHE_TYPE=q8_0` halves them). Ollama or MLX may allocate differently;
the parent tag at 65536 and the `-cg` tag at 32768 both reported 18.5 GB
resident, consistent with lazy allocation.

| Model | KV per token | KV at 32768 / 65536 / 131072 | Weights | Prefill of a full 60,000-token prompt |
|---|---|---|---|---|
| Qwen3.8-27B (16 of 64 layers attend fully; 4 KV heads × 256) | 64 KiB | 2 / 4 / 8 GiB (+0.15 GB fixed linear-attention state) | 18.5 GB | ~145 s at 414 tok/s |
| Gemma 4 26B-A4B (5 global layers, 2 KV heads × 512, K = V) | 10 KiB | 0.3 / 0.6 / 1.3 GiB (+0.2 GB sliding cache) | 18.0 GB | ~38 s at 1579 tok/s |
| Qwen3.5-9B (8 of 32 layers attend fully; 4 KV heads × 256) | 32 KiB | 1 / 2 / 4 GiB | 9.2 GB | ~53 s at 1126 tok/s |

All three fit a 131072 window in 48 GB with more than 15 GB spare. The cost is
prefill time, not memory: the harness's `auto_tune` timeout follows the measured
speed, but every turn re-reads the whole history, so a 60,000-token prompt on
the 27B waits about 2.5 minutes for its first token.

The parent tag `qwen3.8:27b-mlx` reported `num_ctx 65536` in this run;
the current shipped default is the derived `qwen3.8:27b-mlx-cg` at 32768.
Recheck the installed tag and loaded window before applying this historical
65536 example. The harness sends no `num_ctx`. With `auto_tune` on,
`/model profile` reports `scaled_up` and proposes doubled budgets, but applies
only ceilings, so set them in the home `config.yaml` (reload applies the `web`
keys; `models` and `chat` keys need a restart):

```yaml
models:
  local_llm:
    model: "qwen3.8:27b-mlx"   # pins 65536 in its own Modelfile
    max_tokens: 8192           # the 65536 proposal; validated to 25904
chat:
  compact_prompt_tokens: 48000 # cap 60000 at 65536 (30000 x window / 32768)
web:
  total_tokens: 56000          # cap 64000 at 65536; the budget each model call's prompt is checked against
  evidence_tokens: 6000        # code ceiling (web_search::MAX_EVIDENCE_TOKENS)
  model_tokens: 2048           # code ceiling
  chat_tool_calls: 10          # code ceiling
api:
  harness_loop_rate_limit:
    max_requests: 24           # historical example; inspect current rate limits first
    max_tokens: 4096
```

Timeouts need no edit: `auto_tune` derives the chat timeout and research answer
deadline from the measured speed. At 131072 the same keys scale again (prompt cap
120000, `web.total_tokens` cap 128000), and `/loop` turns still keep at most
`chat.compact_keep_messages` recent messages verbatim after a compaction.
