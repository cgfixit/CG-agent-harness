# Memory: save, suggest, review and retrieve

This how-to covers memory operations. [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md)
defines gates, ownership, storage, limits, and automatic behavior. The command
table is [CONSOLE.md §7.8](CONSOLE.md#78-slash-command-quick-reference).

Memory has shared-home pinned notes and an account-private structured store,
without embeddings or vector search.
The Harness proposes **summaries, durable insights, or both** from completed
chat turns and coding runs. **Only Save memory with a reason saves a fact.**
Recalled text is untrusted background context and cannot authorize tools,
coding, or network.

Fresh configuration enables pinned-note inclusion (`memory.enabled: true`) and
every structured-memory gate. Upgrades preserve existing config, saved
`harness.json` choices, and administrator overrides. Chat history persists
independently of these flags.

## What counts as memory

Paths are relative to `CGAGENTHARNESS_HOME` (default `~/.CGagentHarness`).

| Kind | How it is saved / scope | How it is used later |
|---|---|---|
| Chat history | Successful exchanges are written to `sessions/*.json`; account-owned, with unassigned legacy data quarantined | Bounded recent messages from the selected session accompany subsequent chat. This is not a durable-fact extractor. |
| Pinned notes | `/memory add <literal text>` writes `memory/notes.json`; shared home | Included in the prompt while `/memory on`; see [Pinned notes](#pinned-notes). |
| Persona / soul | `soul.md`; shared home; edit/save or reviewed `/soul apply <id> <reason>` | `/soul on\|off` controls prompt inclusion. Persona guidance is distinct from account-private facts. |
| Canonical structured facts | Private `memory/structured.sqlite3`; `/memory save`, governed fact HTTP writes, or approved proposals | List with `/memory facts`; select for explicit recall or use separately gated facts-only retrieval. Saving does not itself enable prompt inclusion. |
| Pending proposals | Same private SQLite store; a model or operator may propose | Memory cards show suggested text; details contain sources and revision. Pending text never enters chat. Saving or dismissing requires a reason. |
| Episodes | Private bounded metadata from successful captured chat; coding metadata when coding suggestions are enabled; optional human `semantic_summary` | Provenance and consolidation input. Never injected into chat or indexed by FTS. Metadata is not a durable fact. |
| Facts search index | Derived, contentless FTS5 index in the structured store | Lexical search over facts only; no episode search. |
| Detached coding jobs / run records | Owner-scoped jobs; shared underlying pipeline run records | Evidence about a coding run, not automatically injected memory. The completion-suggestion hook only uses the initiating account's current completed run. |

`session_summary` and `insight` are categories, not storage systems. A generated
summary covers **one completed turn or run**, not an unseen session. Applying it
creates a fact without filling an episode's human `semantic_summary`. The
Harness does not scan or import external Codex conversations or old archives.
Use pinned notes for shared preferences and reviewed structured facts for
account-private memory.

## Pinned notes

```text
/memory
/memory add Prefer metric units in examples.
/memory on
/prompt
```

- `/memory` shows inclusion state, note IDs, structured-memory gates and fact/proposal counts.
- `/memory add <note>` stores those exact words. For example,
  `/memory add save all session history` stores the sentence. It does not load
  or summarize other sessions.
- Adding a note does **not** turn inclusion on. `/memory on` includes saved notes
  in later chat; `/memory off` keeps the notes and excludes them.
- `/memory forget <id>` deletes one note; `/memory clear` deletes all notes.
- Changes apply on the next chat request without a restart and affect every
  session in this home.
- `/memory on` includes pinned notes only. It opens no structured-memory gate.

`memory/notes.json` holds at most 20 notes, 500 characters per note, and 3,000
characters of assembled prompt context. The prompt may omit notes from a full
store. The code rejects empty, oversized, and blocked instruction-override
content; these limits are constants, not YAML settings. If enabled notes are
absent from `/prompt`, inspect `/memory` for a store error and back up before
manual repair.

Chat can list notes included in its prompt but cannot save or delete them.

## Gates and configuration

Set keys under `structured_memory:` in `config.yaml`, then restart the server or
app. Gates require literal YAML `true`; quoted `"true"` and missing keys are off.
Administrators can change sub-gates live, such as `/memory auto-retrieve off`.
These overlays persist in `memory/structured_gates.json` and override config,
but cannot open the store. Only `enabled: true` plus a restart creates and opens
`memory/structured.sqlite3`. `/memory` shows the effective gates;
`GET /api/structured-memory` adds queue status and `config_drift` (shipped-on keys
your `config.yaml` omits; they read as off). Tunables: `assets/config.default.yaml`.

Every gate, its overlay, its dependencies and what its off state guarantees are
listed once in [the gate table](STRUCTURED_MEMORY.md#gates).

### Enable in an existing home

Fresh homes have these defaults. For an existing home, merge these keys into
the existing block without duplicating keys, then restart:

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

Slash overrides take precedence over these keys; after a restart, run
`/memory capture on` and any other overlay this recipe needs. To enable gates
separately, follow the [enable order](STRUCTURED_MEMORY.md#enable-order).

## Automatic suggestions and review

With store, capture and source enabled, completed work can produce pending
`session_summary`, `insight`, or both according to `suggestion_mode`. Invalid
modes disable suggestions. Local chat and `/prompt` receive source eligibility,
separate from pinned notes. Empty results are valid; no setting approves
proposals. Human-summary consolidation and retrieval remain separate.

Open **Memory** or `/memory proposals`. The pane refreshes proposals,
queue state and outcomes without clearing review reasons. Cards show full text
and affected memories. Collapsed **Suggestion activity** and card details contain
source IDs, models, counts, and failures. **Refresh** reloads manually.

Review content, action, sources, and repository scope; confidence is not proof.
Enter a reason and choose **Save memory** or **Dismiss**. Both confirm the displayed
revision; blank reasons fail. Refresh after a stale-revision error. Applied
proposals appear under **Saved facts**; a closed store has no decision controls.

The HTTP equivalent is POST `/api/structured-memory/proposals/{id}` with
`revision`, `confirm: true`, nonblank `reason`, and `apply: true|false`, using
`X-CyClaw-CSRF`. Proposing needs no confirmation; direct fact writes require
confirmation and a reason. Verify facts with `/memory facts` and `/prompt`.

An empty result is valid. Inspect completion attempts and run history: gates, injection checks, queue limits, model/schema failures, filtering,
and proposal capacity can prevent drafts without failing completed work. Review
pending proposals to free capacity. Queued inputs remain RAM-only: expiry, clear,
or restart can discard them. Receipts explain the loss; repeat relevant input
or write a reviewed `/memory remember` summary. Old conversations are never
replayed. See [completion suggestions](STRUCTURED_MEMORY.md#completion-suggestions).

## Save a fact manually

Keep `enabled: true`. Automatic generation and capture may stay off:

```text
/memory save For repository example, use metric units in examples. :: My reviewed standing preference
```

The command confirms a private fact write and requires a visible reason after
the last `::`. It uses category `manual` and needs an open store, but no capture
or model inference. Plain language such as "remember this" does not run the
command. It may produce a draft when suggestions are enabled, but cannot approve
one. Saving and retrieval remain separate.

For manual-only facts, see [turn automation off](#turn-automation-or-the-store-off).
For a shared literal note, use `/memory add <text>`.

## Summarize and consolidate episodes

Capture stages metadata, not durable facts. After a successful captured turn,
write the one durable sentence you want considered, then consolidate it:

```text
/memory remember Prefer metric units in examples. :: My standing preference
/memory consolidation on
/memory consolidate <returned-episode-id> [episode-id...]
/memory proposals
```

`remember` confirms attaching the bounded, scanned sentence to your latest
completed episode and prints its ID. It requires a reason and an open store;
capture may be off if the episode exists. It saves no fact and starts no model
run. This is **not chat autosave**.

`consolidate` claims the local generation gate, sends only those episode
summaries to the local model (no tools, no web, no recalled facts), and writes
pending proposals. It does **not** create or update facts; review them as above.
Manual selection of summary-less episodes still runs but has worse quality:
metadata alone is not a source of facts.

With `auto_consolidation` on, a bounded idle worker may claim summarized
episodes ([eligibility](STRUCTURED_MEMORY.md#consolidation)); it never preempts
chat, and later summary edits do not reset a completed batch for automatic
replay. Manual consolidation remains available.

## Retrieve and inspect

- `/memory facts` lists your facts even with prompt recall off.
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

To keep manual facts but stop capture and suggestions, leave the store enabled
and run `/memory auto-suggest-chat off`, `/memory auto-suggest-coding off`,
`/memory auto-consolidate off` and `/memory capture off` (or set their config
keys false and restart).
Saved facts and pending proposals remain. To stop opening the database, set
`enabled: false` and restart. This does not delete it or disable pinned notes,
persona, or chat-history persistence. Successful exchanges still enter the
account-owned `sessions/*.json`, and the next turn uses bounded recent messages.
No structured-memory flag disables session persistence.

`/clear` clears the display. Sessions Clear has its separate deletion flow: it
cancels waiting/in-flight chat suggestions and keeps facts and proposals; derived
episodes are kept unless you also confirm `delete_derived_episodes`. Export,
purge and what disabling stops: [rollback](STRUCTURED_MEMORY.md#rollback).
