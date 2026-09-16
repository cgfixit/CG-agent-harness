# Issue #87 close-out (structured memory)

Operator close-out for
[#87](https://github.com/cgfixit/CG-agent-harness/issues/87): privacy-first
structured memory (facts, episodes, governed proposals, retrieval, and
consolidation). This note does **not** flip any gate and does **not** close the
GitHub issue by itself. Merge of the close-out docs PR is the docs step; the
issue stays open until an operator closes it after merge.

Deep contract: [STRUCTURED_MEMORY.md](../STRUCTURED_MEMORY.md).
Day-to-day howto: [USER_MANUAL.md](../USER_MANUAL.md).
Types and recipes: [MEMORY_GUIDE.md](../MEMORY_GUIDE.md).
Enable steps: [setup-guide §7.5](../../setup-guide.md#75-operator-memory-notes).

## Truth order

When prose and behavior disagree, trust this order:

1. Code under `src/` (and the locked tests / examples that exercise it)
2. Shipped defaults in `assets/config.default.yaml`
3. `INVARIANTS.md` and `AGENTS.md`
4. `README.md` and `setup-guide.md` (then this note and the memory docs)

Fix the lower layer in the same change set when you touch the behavior.

## Shipped gates (all default false)

Every structured-memory gate uses `flag_is_true`: literal YAML `true` only.
Quoted `"true"`, missing keys, and invalid values stay **off**. Slash overlays
persist under `<home>/memory/structured_gates.json` once the store is open; they
take effect immediately and **cannot open the store**. `/memory on` remains
pinned-note inclusion only and never opens these gates.

| Gate | Config key | Slash overlay | Ships |
|---|---|---|---|
| Store open | `structured_memory.enabled` | *(config + restart only)* | `false` |
| Episode capture | `structured_memory.episode_capture` | `/memory capture on\|off` | `false` |
| Explicit recall | `structured_memory.explicit_recall` | `/memory recall on\|off` | `false` |
| Facts-only FTS | `structured_memory.retrieval` | `/memory retrieval on\|off` | `false` |
| Silent FTS inject | `structured_memory.auto_retrieval` | `/memory auto-retrieve on\|off` | `false` |
| Manual consolidation | `structured_memory.consolidation` | `/memory consolidation on\|off` | `false` |
| Idle auto-consolidator | `structured_memory.auto_consolidation` | `/memory auto-consolidate on\|off` | `false` |
| Chat completion suggestions | `structured_memory.auto_suggest_chat` | `/memory auto-suggest-chat on\|off` | `false` |
| Coding completion suggestions | `structured_memory.auto_suggest_coding` | `/memory auto-suggest-coding on\|off` | `false` |

`auto_retrieval` requires `retrieval`. `auto_consolidation` requires
`consolidation`. Both suggestion sources require an open store and
`episode_capture`. Models may suggest; applying a canonical fact still needs
`confirm` and a nonempty `reason`.

## Enable order

Measure Phase 7 **before** recommending any gate ON:

```text
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --test structured_memory_phase7 -- --nocapture
cargo run --locked --example structured_memory_eval -- /tmp/phase7-report.json
```

Suggested rollout (keep every later gate false until the earlier one is useful
and the locked bars are green):

1. Manual facts and governed proposals (`enabled`)
2. Optional episode capture (`episode_capture`)
3. Explicit recall (`explicit_recall`)
4. Optional FTS5 recall (`retrieval`; leave `auto_retrieval` last among read paths)
5. Manual consolidation (`consolidation`)
6. Optional automatic pending-proposal generation (`auto_consolidation`)

Completion suggestions (`auto_suggest_chat` / `auto_suggest_coding`) are a
separate opt-in beside that list. They still write pending proposals only.

## Rollback

Availability is `store_open && override.unwrap_or(config_on)`. An administrator's
explicit slash `off` therefore disables a config-true sub-gate immediately;
explicit `on` enables it. Config edits still need a process restart. The store
opening gate has no slash override.

That does **not** delete `memory/structured.sqlite3`. Existing rows stay
listable/exportable/purgeable while the store remains open. Closing
`structured_memory.enabled` refuses new structured-memory mutations and does not
open the database on the next start; leftover files remain until the operator
exports or runs `POST /api/structured-memory/purge` with confirm+reason.

## Non-goals still true

This tree still does **not** ship:

- Embeddings or a vector database
- RAG fusion or harness-writable RAG ingestion
- Episode prompt injection or episode FTS
- Silent / autonomous canonical fact apply
- New cloud egress for memory summarization or consolidation

Status may report `retrieval: true` or `consolidation: true` while fusion and
RAG stay false. Recalled text is untrusted context and cannot authorize tools,
coding, or network.

## Related surfaces

- Locked Phase 7 bars and HTTP tables: [STRUCTURED_MEMORY.md](../STRUCTURED_MEMORY.md)
- Next-task live-model plan (not a live result): [MEMORY_BENCHMARK_PLAN.md](../MEMORY_BENCHMARK_PLAN.md)
- Latest release scope: [setup-guide.md](../../setup-guide.md) version block
  (Latest v0.1.12 at `22520f3`)
