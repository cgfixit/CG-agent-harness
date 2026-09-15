# Operator memory manual

Day-to-day howto for the two memory systems on this tree. It is **not** the
deep contract — that is [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md). Enable
steps and home layout are also in
[setup-guide §7.5](../setup-guide.md#75-operator-memory-notes). Overview:
[README](../README.md).

Neither system is embeddings, a vector database, RAG fusion, or automatic
learning from chat. Recalled text is untrusted background context and cannot
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
mutating a fact still requires `confirm` and a nonempty `reason`. There is no
slash command that writes a fact.

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

### Enable (fail-closed)

Every gate uses `flag_is_true`: literal YAML `true` only. Quoted `"true"` stays
off. Config changes need a full quit/relaunch or `serve` restart. Slash
overlays take effect immediately once the store is open; they **cannot open
the store**.

1. Set `structured_memory.enabled: true` in the active home's `config.yaml` and
   restart.
2. Turn only the sub-gates you need, in config **or** via slash:

| Gate | Slash | What it does |
|---|---|---|
| `episode_capture` | `/memory capture on\|off` | Stage metadata-only episodes after a successful chat |
| `explicit_recall` | `/memory recall on\|off` | Allow operator-selected facts into `/prompt` after recheck |
| `retrieval` | `/memory retrieval on\|off` | Allow facts-only FTS search and per-request force-include |
| `auto_retrieval` | `/memory auto-retrieve on\|off` | **High-risk.** Silent top-k inject when this is on **and** retrieval is on |
| `consolidation` | `/memory consolidation on\|off` | Allow manual selected-episode consolidation into **pending proposals only** |
| `auto_consolidation` | `/memory auto-consolidate on\|off` | Bounded idle worker. Requires consolidation (AND). Pending proposals only. Chat wins the generation gate |

All of these ship **false**. `/memory` reports store and gate state. Tunables live
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

## `auto_retrieval` (high-risk)

Ships **false**. When it is on **and** `retrieval` is on, chat may FTS the user
message and inject rechecked top-k **without** a per-request flag. That is an
intentional silent path. Leave it off unless you mean every subsequent chat
to search and inject.

`/memory auto-retrieve off` (or the config literal `false`) closes it. Turning
`/memory on` does not enable it.

## Manual consolidation

Default **off**. After capture has staged episodes:

```text
/memory consolidation on
/memory consolidate <episode-id> [episode-id...]
```

That claims the local generation gate, sends only those episode summaries to
the local model (no tools, no web, no recalled facts), and writes pending
proposals. It does **not** create or update facts. Review and apply remain the
proposal path below. `/memory on` does not open this gate.

## Automatic consolidation

Default **off**. Also requires `consolidation`. After capture has staged
episodes:

```text
/memory consolidation on
/memory auto-consolidate on
```

A bounded idle worker may pick eligible `none`/`pending` episodes (owner-scoped,
capped by `max_consolidation_episodes`) and reuse the manual consolidator.
It writes **pending proposals only**. It does not create, update, or delete
facts. Feature-off starts no worker. Disabling stops new claims; leftover
`running` rows recover like manual. Interactive chat wins the generation
gate. Restart reuses the same idempotency key.

## Propose and apply (API)

Slash commands do not write facts. Typical operator path:

1. Model or operator `POST /api/structured-memory/proposals` (suggest; no
   `confirm`).
2. Review, then decide or add/deactivate with `confirm: true` and `reason`.
3. Verify with `GET /api/structured-memory/facts` and `/prompt`.

Clearing session history keeps derived episodes unless you also confirm
`delete_derived_episodes`. That cascade still keeps facts, proposals, and
episodes referenced by pending proposals.

## Not shipped

Embeddings, vector DB, RAG fusion, episode FTS, episode prompt injection, and
any slash that writes a fact. Status can report `retrieval: true`,
`consolidation: true`, or `auto_consolidation: true` while fusion and RAG
stay false. `auto_consolidation` ships false.

## See also

- [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md) — gates, HTTP surfaces, ownership
- [setup-guide §7.5](../setup-guide.md#75-operator-memory-notes) — enable steps
- [README](../README.md) — console overview
- [CHAT_WORKFLOWS.md](CHAT_WORKFLOWS.md) — sessions, `/prompt`, persona
