# Memory: save, suggest, review and retrieve

This is the memory how-to. The contract (gates, ownership, storage, limits and
what is never automatic) is [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md); the
command table is [CONSOLE.md §7.8](CONSOLE.md#78-slash-command-quick-reference).

There are two memory systems: shared-home pinned notes and an account-private
structured store. Neither is embeddings, a vector database or RAG fusion. The
Harness can suggest **summaries, durable insights, or both** from completed
Harness chat turns and coding runs. These suggestions persist as pending
proposals. **Only your Apply action with a reason saves a canonical fact.**
Recalled text is untrusted background context and cannot authorize tools,
coding, or network.

Fresh configuration turns on pinned-note inclusion (`memory.enabled: true`) and
every structured-memory gate. Existing config files, saved `harness.json`
choices and explicit administrator off overrides are preserved. Ordinary
chat-history persistence is separate and already happens without these flags.

## What counts as memory

Paths below are relative to the active Harness home (`CGAGENTHARNESS_HOME`,
default `~/.CGagentHarness`).

| Kind | How it is saved / scope | How it is used later |
|---|---|---|
| Chat history | Successful exchanges are written to `sessions/*.json`; account-owned, with unassigned legacy data quarantined | Bounded recent messages from the selected session accompany subsequent chat. This is not a durable-fact extractor. |
| Pinned notes | `/memory add <literal text>` writes `memory/notes.json`; shared home | Included in the prompt while `/memory on`; see [Pinned notes](#pinned-notes). |
| Persona / soul | `soul.md`; shared home; edit/save or reviewed `/soul apply <id> <reason>` | `/soul on\|off` controls prompt inclusion. Persona guidance is distinct from account-private facts. |
| Canonical structured facts | Private `memory/structured.sqlite3`; `/memory save`, governed fact HTTP writes, or approved proposals | List through the facts API; select for explicit recall or use separately gated facts-only retrieval. Saving does not itself enable prompt inclusion. |
| Pending proposals | Same private SQLite store; a model or operator may propose | Memory panel shows content, action, category, sources and revision. Pending text is never recalled into chat. Apply/Reject requires a reason. |
| Episodes | Private bounded metadata from successful captured chat; coding metadata when coding suggestions are enabled; optional human `semantic_summary` | Provenance and consolidation input. Never injected into chat or indexed by FTS. Metadata is not a durable fact. |
| Facts search index | Derived, contentless FTS5 index in the structured store | Lexical search over facts only. No embeddings, vector database, episode search or RAG fusion. |
| Detached coding jobs / run records | Owner-scoped jobs; shared underlying pipeline run records | Evidence about a coding run, not automatically injected memory. The completion-suggestion hook only uses the initiating account's current completed run. |

`session_summary` and `insight` are fact/proposal categories, not new storage
systems. A generated summary covers **one completed turn or run**, not an unseen
whole session. Applying it creates a fact; it does not silently fill an episode's
human `semantic_summary`. External Codex conversations and old Harness archives
are not scanned or imported. Use pinned notes for shared-home preferences; use
structured facts only after explicit review.

## Pinned notes

```text
/memory
/memory add Prefer metric units in examples.
/memory on
/prompt
```

- `/memory` shows the inclusion state, note IDs and structured-memory gate state.
- `/memory add <note>` stores those exact words:
  `/memory add save all session history` stores that sentence, not a summary or
  a reference that loads other sessions. Store concrete facts or a reviewed
  summary instead.
- Adding a note does **not** turn inclusion on. `/memory on` includes saved notes
  in later chat; `/memory off` keeps the notes and excludes them.
- `/memory forget <id>` deletes one note; `/memory clear` deletes all notes.
- Changes apply on the next chat request without a restart and affect every
  session in this home.
- `/memory on` includes pinned notes only. It opens no structured-memory gate.

Notes are stored in `memory/notes.json`. Current code limits are 20 notes, 500
characters per note and 3,000 characters of assembled prompt context; a full
store is not a promise every note fits in the prompt. Empty, oversized and
blocked instruction-override content is rejected. These note limits are code
constants, not documented YAML settings. If memory is on but absent from
`/prompt`, inspect `/memory` for an unreadable store rather than assuming it
loaded. Back up before manual repair. Pinned-note capability flags
(`rag.facts`, episodes, retrieval fusion) remain false.

Chat receives a guide to these controls plus current inclusion settings. It can
list notes actually included in its prompt, but cannot save or delete them.

## Gates and configuration

Set keys under `structured_memory:` in the active `config.yaml`, then restart
the server or fully relaunch the app. Literal YAML `true` is required; quoted
`"true"` and missing keys stay off. Administrators can also switch each
sub-gate live with its slash overlay, for example `/memory auto-retrieve off`.
Overlays persist in `memory/structured_gates.json` and override the config
value, including explicit `off` over config `true`, but they cannot open the
store: only `enabled: true` plus a restart creates and opens
`memory/structured.sqlite3`. `/memory` shows the effective gates;
`GET /api/structured-memory` also shows suggestion queue status. Tunables live in
`assets/config.default.yaml`; do not invent extra flags.

Every gate, its overlay, its dependencies and what its off state guarantees are
listed once in [the gate table](STRUCTURED_MEMORY.md#gates).

### Enable in an existing home

Fresh homes already have these defaults. To enable them in an existing home,
merge this into its existing block (do not create duplicate YAML keys) and
restart:

```yaml
structured_memory:
  enabled: true
  episode_capture: true
  auto_suggest_chat: true
  auto_suggest_coding: true
  suggestion_mode: "both" # "summaries", "insights", or "both"
  consolidation: true
  auto_consolidation: true
  explicit_recall: true
  retrieval: true
  auto_retrieval: true
```

Existing slash overrides take precedence over config: if a `/memory capture off`
overlay is present, use `/memory capture on` after restart, and turn on any other
off overlays you want to match this recipe. To enable gates one at a time, follow
the [enable order](STRUCTURED_MEMORY.md#enable-order).

## Automatic suggestions and review

With the store, capture and either suggestion source on, a completed chat turn
or successful coding run can produce pending `session_summary` and/or `insight`
proposals, chosen by `suggestion_mode`. Either source switch can be enabled
alone. Invalid modes, including non-string YAML values, disable suggestions. No
setting auto-approves them. This path is separate from the human-summary
auto-consolidator below; neither enables retrieval.

Complete a chat turn or coding run, open **Memory** or `/memory proposals`, and
use **Refresh proposals** after generation. The panel shows pending action,
category, content, source episode ids and proposal id. Expand **Review full
proposal**, check the content and repository scope (model confidence does not
establish correctness), enter your reason, then choose **Apply** or **Reject**.
That button explicitly confirms your decision for the displayed revision through
the existing proposal-decide API. Empty/blank reasons are refused. Refresh to
reload after a stale-proposal error. A closed store has no decision controls.

HTTP equivalent: review `GET /api/structured-memory/proposals/{id}`, then POST
to that same path with `revision`, `confirm: true`, a nonblank `reason`, and
`apply: true` (apply) or `apply: false` (reject), using `X-CyClaw-CSRF`. A model
or operator suggests with `POST /api/structured-memory/proposals` (no
`confirm`); direct add/deactivate also need `confirm: true` and `reason`. Verify
with `GET /api/structured-memory/facts` and `/prompt`.

There is no guarantee of a proposal for every event, and a suggestion that fails
never fails the original chat or run. Review `automatic_suggestions` status,
consolidation-run state and metadata audit events for failures, and review
existing proposals before generating more: pending capacity is per owner. Bounds
and failure behavior: [completion suggestions](STRUCTURED_MEMORY.md#completion-suggestions).

## Save a fact manually

Keep `enabled: true`. Automatic generation and capture may stay off:

```text
/memory save For repository example, use metric units in examples. :: My reviewed standing preference
```

The command itself explicitly confirms a private fact write, requires a visible
reason after the last `::`, and uses the existing facts API with category `manual`.
It needs neither capture nor model inference. The store must already be open.
Plain language such as “remember this” does not execute this command: with
automatic suggestions enabled it may produce a draft, but cannot approve one.
Saving and later prompt retrieval remain separate operations.

To save only by hand, run `/memory auto-suggest-chat off`,
`/memory auto-suggest-coding off`, and `/memory auto-consolidate off` (or set
those config values false and restart). For a shared literal note instead, use
`/memory add <text>`.

## Summarize and consolidate episodes

Capture stages metadata, not durable facts. After a successful captured turn,
write the one durable sentence you want considered, then consolidate it:

```text
/memory remember Prefer metric units in examples. :: My standing preference
/memory consolidation on
/memory consolidate <returned-episode-id> [episode-id...]
/memory proposals
```

`remember` explicitly confirms attaching your bounded, scanned sentence to your
latest completed episode and prints its id. It requires a visible reason and an
open store; capture may be off if that episode exists. It saves no fact and
starts no model run. This is **not chat autosave**.

`consolidate` claims the local generation gate, sends only those episode
summaries to the local model (no tools, no web, no recalled facts), and writes
pending proposals. It does **not** create or update facts; review them as above.
Manual selection of summary-less episodes still runs but has worse quality:
metadata alone is not a source of facts.

With `auto_consolidation` on as well, a bounded idle worker may claim the
episode instead, so you can wait for pending proposals in Memory. It requires
nonblank human summaries, unexpired `none`/`pending` episodes and no running
owner batch, waits for the generation gate, and never preempts chat. A completed
suggestion batch marks its episode done; later summary edits do not reset it for
automatic replay. Explicit manual consolidation remains available.

## Retrieve and inspect

- `GET /api/structured-memory/facts` lists your facts even with prompt recall off.
- With `explicit_recall` on, select facts by stable `public_id` plus
  `expected_revision` for a session (`POST /api/sessions/{session_id}/structured-facts`)
  or one request (`selected_facts` on `/api/chat` or `/api/prompt/preview`).
  There is no `/memory select` slash. Assembly rechecks ownership, active state
  and revision.
- `/memory search <query>` (or `GET /api/structured-memory/search`) returns
  candidates only; it does **not** write session selection or inject. Retrieval
  off is HTTP 409, not a silent empty inject.
- `/memory retrieve <query>` sends that query as a chat turn and force-includes
  rechecked matches for that request only. The same `retrieve` /
  `retrieve_query` fields work on `/api/chat` and `/api/prompt/preview`. It does
  not persist session selection.
- `/prompt` previews actual selected facts, pinned notes and persona; its
  `structured_facts.injected` / `dropped` is the set chat would send. To preview
  retrieval, use `POST /api/prompt/preview` with `retrieve`/`retrieve_query`.

With `auto_retrieval` and `retrieval` on, chat matches any meaningful word in the
bounded prompt prefix, ranks candidates with FTS5/BM25, and injects rechecked
top-k facts without a per-request flag. Common question words are ignored;
explicit keyword queries remain all-terms matches. This is lexical retrieval, so
unrelated synonyms may not match. `/memory auto-retrieve off` (or config literal
`false`) leaves only explicit picks. `auto_retrieval` controls use of saved
facts, not suggestion quality.

Owner, active status, revision and [prompt-budget](STRUCTURED_MEMORY.md#prompt-budget)
checks still apply before inclusion, so a saved fact can exist without appearing
in a particular prompt. Pending proposals and episode summaries never enter that
budget.

## Turn automation or the store off

To retain manual facts but stop derived capture and suggestions: leave the store
enabled, disable both `auto_suggest_*` switches, `auto_consolidation` and
capture (config or `/memory … off`). Existing saved facts and pending proposals
remain. To avoid opening the structured database at all, set `enabled: false`
and restart. This does not delete it or disable pinned notes, persona, or
ordinary chat-history persistence: successful exchanges still persist in
account-owned `sessions/*.json`, and bounded recent messages are used for the
next chat in that session. There is no structured-memory flag that disables
saving successful chat exchanges to the session store.

`/clear` clears the display. Sessions Clear has its separate deletion flow: it
cancels waiting/in-flight chat suggestions and keeps facts and proposals; derived
episodes are kept unless you also confirm `delete_derived_episodes`. Export,
purge and what disabling stops: [rollback](STRUCTURED_MEMORY.md#rollback).
