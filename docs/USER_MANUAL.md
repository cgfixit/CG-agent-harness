# Operator memory manual

Day-to-day howto for the two memory systems on this tree. It is **not** the
deep contract — that is [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md). Enable
steps and home layout are also in
[setup-guide §7.5](../setup-guide.md#75-operator-memory-notes). Overview:
[README](../README.md).

Neither system is embeddings, a vector database or RAG fusion. Optional
completion suggestions still need human approval before they become facts. Recalled text is untrusted background context and cannot
authorize tools, coding, or network.

## Two systems

| System | Who sees it | Store | How it enters `/prompt` |
|---|---|---|---|
| Pinned notes | Shared home | `memory/notes.json` | `/memory on` only |
| Structured memory | Your account | `memory/structured.sqlite3` | Explicit pick (selected facts, `/memory retrieve`, or `retrieve`) **or** the separately gated `auto_retrieval` path. Episodes never inject. |

`/memory on` includes pins only. It does not open the structured store or any
structured gate.

## Pinned notes

```text
/memory
/memory add Prefer metric units in examples.
/memory on
/prompt
```

- `/memory add <note>` stores those exact words. It does not summarize sessions
  or import history.
- `/memory forget <id>` deletes one note; `/memory clear` deletes all notes.
- `/memory on` includes saved notes in later chat. `/memory off` keeps the
  notes and excludes them.
- Adding a note does **not** turn inclusion on. Changes apply on the next chat
  without a restart and affect every session in this home.

Notes are injection-scanned literals. Current code limits are 20 notes, 500
characters each, and 3,000 characters of assembled memory body. Inspect
`/memory` if inclusion is on but `/prompt` shows nothing.

## Structured memory

Account-private **facts**, governed **proposals**, and optional **episodes**
(issue #87). Models may **suggest**. Applying, deactivating, or otherwise
mutating a fact still requires `confirm` and a nonempty `reason`.
`/memory save <text> :: <reason>` explicitly confirms a private fact write.

Episodes stage after a successful chat exchange when capture is on. Staging
never writes facts, never injects episode text, and never fails an
already-successful chat. File mode 0600 is OS access control, not encryption.

### Files under harness home

Default home is `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`).

| Path | Role |
|---|---|
| `memory/notes.json` | Pinned notes |
| `memory/structured.sqlite3` | Structured store (created only when the store gate is on) |
| `memory/structured_gates.json` | Slash overlays for the capture/recall/retrieval/auto-retrieve/consolidation/auto-consolidate sub-gates |

### Defaults and explicit overrides

Every gate uses `flag_is_true`: literal YAML `true` only. Quoted `"true"` stays
off. Config changes need a full quit/relaunch or `serve` restart. Slash
overlays take effect immediately once the store is open; they **cannot open
the store**.

Fresh homes ship pinned-note inclusion and all nine structured gates on. Existing
explicit off settings remain off. To enable an existing opted-out home:

1. Set `structured_memory.enabled: true` in the active home's `config.yaml` and
   restart.
2. Turn only the sub-gates you need. Administrator accounts can persist an
   explicit slash override for every sub-gate:

| Gate | Slash | What it does |
|---|---|---|
| `episode_capture` | `/memory capture on\|off` | Stage metadata-only episodes after a successful chat |
| `explicit_recall` | `/memory recall on\|off` | Allow operator-selected facts into `/prompt` after recheck |
| `retrieval` | `/memory retrieval on\|off` | Allow facts-only FTS search and per-request force-include |
| `auto_retrieval` | `/memory auto-retrieve on\|off` | Automatic top-k inject when this is on **and** retrieval is on |
| `consolidation` | `/memory consolidation on\|off` | Allow manual selected-episode consolidation into **pending proposals only** |
| `auto_consolidation` | `/memory auto-consolidate on\|off` | Bounded idle worker. Requires consolidation (AND). Pending proposals only. Chat wins the generation gate |
| `auto_suggest_chat` | `/memory auto-suggest-chat on\|off` | With store + capture, completed current chat can queue pending summaries/insights |
| `auto_suggest_coding` | `/memory auto-suggest-coding on\|off` | With store + capture, successful coding runs can queue pending summaries/insights |

All of these ship **true** for fresh homes; explicit operator off overrides win. `/memory` reports store and gate state. Tunables live
in `assets/config.default.yaml`; do not invent extra flags.

## Explicit recall

Facts enter chat only after an explicit pick **and** `explicit_recall` is on
(unless you use the retrieval force-include or `auto_retrieval` paths below).

Select by stable `public_id` plus `expected_revision`:

- Session: `POST /api/sessions/{session_id}/structured-facts`
- One request: `selected_facts` on `/api/chat` or `/api/prompt/preview`

There is no `/memory select` slash. At assembly the store is re-read for the
current owner. Missing, inactive, stale-revision, and cross-owner facts are
dropped. `/prompt` preview `structured_facts.injected` / `dropped` is the same
set chat would send.

**Budget.** Combined pinned-note + selected-fact body is 3000 characters. When
both are present the reserved split is 1500/1500
(`pinned_prompt_chars` / `selected_fact_prompt_chars`). Unused reserved
capacity is not transferred. When only one source is present it may use the
full 3000.

## FTS (search ≠ inject)

Facts only. Episode summaries are not indexed.

```text
/memory search rust test strategy
/memory retrieve rust test strategy
```

- `/memory search <query>` (or `GET /api/structured-memory/search`) returns a
  small top-k of candidates. It does **not** write session selection and does
  **not** inject. Retrieval off is HTTP 409, not a silent empty inject.
- `/memory retrieve <query>` sends that text as the chat turn with
  `retrieve` / `retrieve_query` set. Same fields on `/api/chat` and
  `/api/prompt/preview` are the explicit pick **for that prompt**: bounded
  FTS, recheck survivors, inject under the recall budget. Hits do not stick
  on the shared session.

`/prompt` shows session-selected facts when recall is on. A retrieve preview
is `POST /api/prompt/preview` with `retrieve` / `retrieve_query` — the same
set that chat would inject for that request.

## Automatic retrieval

Ships **true** for fresh homes. When it is on **and** `retrieval` is on, chat
searches bounded meaningful terms from the user message and injects rechecked
top-k facts without a per-request flag. Common question words are ignored and
FTS5/BM25 ranks candidates; explicit keyword searches still require all terms.
This is lexical retrieval, so unrelated synonyms may not match.

`/memory auto-retrieve off` (or the config literal `false`) closes it. Turning
`/memory on` does not enable it.

## Manual consolidation

Default **on** for fresh homes. Capture stages metadata, not durable facts. After a successful
captured turn, write the one durable sentence you want considered:

```text
/memory remember Prefer metric units in examples. :: My standing preference
```

This command explicitly confirms saving your summary on your latest completed
episode. The visible reason is required. It prints the episode id; it does not itself
run consolidation or write facts. An enabled idle worker may claim the episode. This is **not chat autosave**. An open store
and a completed episode are required; capture may be off if that episode exists.
Summaries are bounded by `max_episode_summary_chars` (default 500) and scanned.
With automatic consolidation enabled, wait for pending proposals in Memory.
To request consolidation manually:

```text
/memory consolidation on
/memory consolidate <episode-id> [episode-id...]
```

That claims the local generation gate, sends only those episode summaries to
the local model (no tools, no web, no recalled facts), and writes pending
proposals. It does **not** create or update facts. Review and apply use the
Memory panel below. Manually selecting an episode without a semantic summary
still runs, but has worse quality: metadata alone is not a source of facts.
`/memory on` does not open this gate.

## Automatic consolidation

Default **off**. Also requires `consolidation`. After capture has staged
episodes:

```text
/memory consolidation on
/memory auto-consolidate on
```

A bounded idle worker may pick unexpired `none`/`pending` episodes with a
nonblank human semantic summary (owner-scoped,
capped by `max_consolidation_episodes`) and reuse the manual consolidator.
It writes **pending proposals only**. It does not create, update, or delete
facts. Feature-off starts no worker. Disabling stops new claims; leftover
`running` rows recover like manual. Interactive chat wins the generation
gate. Restart reuses the same idempotency key.

The `consolidator-v2` prompt prefers fewer durable, supported suggestions.
`structured_memory.min_consolidation_confidence` defaults to 0.40 (clamped to
0–1); supplied confidence below it is rejected, while omitted confidence remains
accepted. Fresh gates ship on; confidence is not a review substitute.

## Review and apply (console and API)

Open **Memory** in the console sidebar or run `/memory proposals`. The panel
shows pending action, category, content, source episode ids, and proposal id.
Expand **Review full proposal**, enter your reason, then choose **Apply** or
**Reject**. That button explicitly confirms your decision for the displayed
revision through the existing API. Empty/blank reasons are refused. Refresh to
reload after a stale-proposal error. A closed store has no decision controls.

HTTP equivalent: review `GET /api/structured-memory/proposals/{id}`, then POST
to that same path with `revision`, `confirm: true`, a nonblank `reason`, and
`apply: true` (apply) or `apply: false` (reject), using `X-CyClaw-CSRF`.

The `/memory save` command is the explicit direct-write alternative. Proposal path:

1. Model or operator `POST /api/structured-memory/proposals` (suggest; no
   `confirm`).
2. Review, then decide or add/deactivate with `confirm: true` and `reason`.
3. Verify with `GET /api/structured-memory/facts` and `/prompt`.

Clearing session history keeps derived episodes unless you also confirm
`delete_derived_episodes`. That cascade still keeps facts, proposals, and
episodes referenced by pending proposals.

## Not shipped

Embeddings, vector DB, RAG fusion, episode FTS, episode prompt injection, or
automatic canonical fact approval. Status can report `retrieval: true`,
`consolidation: true`, or `auto_consolidation: true` while fusion and RAG
stay false. `auto_consolidation` ships true for fresh homes, but requires
consolidation and a reviewed semantic episode summary.

## Rollout and rollback

Fresh defaults are on; existing homes keep their explicit choices. The Phase 7
fixture bars in [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md#phase-7-evaluation-rollout-and-rollback)
remain regression checks for facts, capture, recall, retrieval and consolidation:


```text
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --test structured_memory_phase7 -- --nocapture
cargo run --locked --example structured_memory_eval -- /tmp/phase7-report.json
```

Rollback is disable-only. `/memory … off` or `config.yaml` literal `false`
stops new reads, writes, and workers. It does not delete
`memory/structured.sqlite3`. Export and purge remain.

## See also

- [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md) — gates, HTTP surfaces, ownership, Phase 7 eval
- [memory/ISSUE_87_CLOSEOUT.md](memory/ISSUE_87_CLOSEOUT.md) — issue #87 close-out
- [setup-guide §7.5](../setup-guide.md#75-operator-memory-notes) — enable steps
- [README](../README.md) — console overview
- [CHAT_WORKFLOWS.md](CHAT_WORKFLOWS.md) — sessions, `/prompt`, persona

## Automatic completion suggestions and manual hard-save

See [Memory guide](MEMORY_GUIDE.md) for the complete type/flag table and recipes.
Enable `structured_memory.enabled` in config and restart, then enable capture
and either suggestion source in config or with the administrator slash controls. Choose
`suggestion_mode: summaries|insights|both`. These source switches
ship false. The completed current turn/run can generate pending drafts; this
is not a whole-session archive or automatic canonical fact save. Review in
Memory and Apply/Reject with a reason. The older human-summary auto-consolidator
remains a separate path. Neither path enables retrieval.

With the store open, automatic generation and capture may remain off:

```text
/memory save For repository example, prefer metric examples. :: Reviewed standing preference
```

This is an explicit, immediate private fact write through the existing HTTP API;
the visible reason and command supply reason and confirmation. Plain-language
requests do not execute it. `/memory add` remains shared literal pinned notes;
`/memory remember` remains a summary attachment. Disabling structured automation
does not disable ordinary chat-history persistence. The next benchmark task is
specified in [MEMORY_BENCHMARK_PLAN.md](MEMORY_BENCHMARK_PLAN.md).
