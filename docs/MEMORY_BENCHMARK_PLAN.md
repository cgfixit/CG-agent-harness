# Next task: memory suggestion benchmark

Status: implementation plan, not an implemented live benchmark. The current
fixture tests prove parsing, binding, ownership and approval rules; they do not
measure an installed model's suggestion quality. No default should change from
a good-looking local chat or a canned fixture score.

## Reuse the web benchmark pattern

Inspect live `origin/main` and the relevant PR before editing. Read
`examples/web_research_benchmark.rs`, `examples/structured_memory_eval.rs`,
`tests/structured_memory_phase7.rs`, `tests/structured_memory_suggestions.rs`,
and the [memory contract](STRUCTURED_MEMORY.md). Extend the existing memory eval
example with fixture and explicit opt-in local-model modes. Reuse its report
shape where practical; no new service, SPA, dependencies, provider SDK or schema.

Measure two distinct production paths:

1. **Human episode summaries → pending durable candidates:** `consolidator-v2`.
   Seed owner-scoped episodes, attach summaries through validated semantics, then
   call the existing consolidation HTTP route so the generation gate is exercised.
2. **Completed current turn/run → pending summaries/insights:**
   `completion-suggestions-v1`. Drive the chat completion hook and the coding
   completion projection; test `summaries`, `insights`, and `both`. Use the existing
   fixture child for route tests, not a real repository mutation. Distinguish
   generated summaries of a single turn/run from whole-session summarization.

Use a fresh disposable home per run, synthetic account owners, installed local
OpenAI-compatible backend, cloud keys empty, tools/web off, temperature 0 and
max_tokens 1024. Default CI stays deterministic/offline. Local-model execution
must be explicitly selected, with endpoint/model recorded and validated by the
existing local client. No automatic model download or cloud judge.

## Fix labels before measuring live output

The Phase 7 case-level `supported` flag describes canned candidate output; it is
not always a label for the input. In particular, `unsupported-paris` supplies a
valid preference but intentionally emits a bad Paris claim. Preserve it unchanged.
A live model emitting the supported preference should receive credit. Temporary
but grounded text is also not necessarily a durable insight.

Add a small companion corpus/label file with input-derived expected propositions,
forbidden claims, source evidence and permitted category. Keep these labels and
seeded fact IDs **outside** the model payload. The correction fixture adapter can
know existing fact IDs that the actual model cannot see; do not inject a facts
array or gold IDs to improve target binding. Score correction meaning separately
from automatic conflict resolution, which remains unproved.

Suggested coverage: durable preference/constraint/identity, negation, speaker
attribution, correction, repository-scoped lesson, requested-but-unimplemented
change, failed check, temporary plan, joke, errand, tool/status-only evidence,
metadata-only episode, truncation, empty output, secret/injection, wrong owner and
fabricated source. For summary mode, temporary events can be faithful summaries;
for insight mode, the same events should usually be omitted. Do not score both
categories with one durability label.

## Report and score

Write one local JSON report containing source SHA and dirty state, corpus hash,
prompt hash/version, path/mode, configured/returned model, known backend version,
effective limits, per-case run status, emitted/retained/rejected candidates,
source refs, human labels and monotonic elapsed time. Use unavailable for missing
token/backend fields; do not add production columns just to fill the report.

Each emitted proposition needs an explicit human label and supporting source:
supported durable insight, faithful summary, temporary/non-durable, unsupported,
contradicted, misattributed, or sensitive/injection-like. Keep unreviewed outputs
unscored. The generating model must not grade itself.

| Metric | Required interpretation |
|---|---|
| Durable-insight precision | Supported, durable retained insight propositions / all retained insight propositions |
| Summary faithfulness | Fully supported summary propositions / all retained summary propositions; attribution and uncertainty must survive |
| Unsupported rate | Unsupported retained propositions / all retained propositions, reported separately by category |
| Positive gold recall | Distinct expected propositions represented / expected propositions; duplicates cannot inflate it |
| Negative-case abstention | Successful negative cases with no inappropriate candidates / successful negative cases |
| Safety / governance | Zero retained secrets/injection, foreign references, cross-owner recall, or unsolicited canonical fact changes |
| Runtime | Success/error/invalid-JSON counts plus per-case elapsed time; failures are not abstentions |

Compare complete fact ID/revision/content-digest snapshots before and after
suggestion generation, not just row counts. Do not call proposal decide during
quality scoring. Approval workflow tests remain separate. Assert no recalled
facts or gold labels in either model payload and retain
`prompt_contains_recalled_facts: false` for the episode consolidator.

Preserve locked Phase 7 bars: precision ≥0.80, unsupported ≤0.20,
secret/injection retention 0, stale/cross-owner recall 0. Add positive recall
reporting so all-empty output cannot win on precision. Agree a live recall target
on reviewed held-out labels before tuning; do not invent a passing threshold
after observing results. Report p50/p95 only with stated repeated sample counts
and cold/warm separation. Keep tuning and held-out cases separate.

## Bounded implementation prompt

Implement the above in one focused draft PR based on main. Expected files: the
existing memory example, one companion corpus/labels file, focused evaluator
tests, and memory docs. Use existing route/client/store helpers. Add a small
shared helper only when required to exercise the actual production path; do not
copy the generator into the evaluator. Do not implement embeddings, transcript
archives, new recall behavior, automatic approval, or default flips.

The evaluator's tests must reject all-empty positive results, mixed supported
and unsupported outputs, negation/attribution loss, summary-as-insight errors,
invented coding success, fabricated refs, and model errors disguised as
abstention. Existing queue-expiry/clear/account/mode tests stay green. Run the
full repository quality bar with cloud keys empty and the unchanged Phase 7
suite. Only run an installed local model when explicitly selected; disclose
model provenance and human review status. Leave #87 open.

Deliver the fixture report, a reproducible local command, review-ready unscored
output if human labels are pending, and a before/after comparison on identical
held-out inputs. No claim of live quality without a real model run and review.

## Research context

The [LongMemEval upstream documentation](https://github.com/xiaowu0162/LongMemEval/blob/main/README.md)
offers extraction, updates, temporal reasoning and abstention categories.
[LoCoMo's upstream repository](https://github.com/snap-research/locomo) illustrates
speaker/time/evidence annotations. These inform coverage, not compatible scores:
Harness is evaluating pending suggestions from bounded inputs, not the complete
long-conversation tasks. Do not import their cloud-judge workflows or download
their corpora as an implicit part of this task. Original source review was on
September 15, 2026; recheck upstream scope before relying on changed tooling.
