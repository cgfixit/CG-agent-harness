# Memory: save, suggest, review and retrieve

This how-to covers memory operations. [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md)
defines gates, ownership, storage, limits, and automatic behavior. The command
table is [CONSOLE.md §7.8](CONSOLE.md#78-slash-command-quick-reference).

There are two memory systems: shared-home pinned notes and an account-private
structured store. Neither uses embeddings, a vector database, or RAG fusion.
The Harness can propose **summaries, durable insights, or both** from completed
chat turns and coding runs. **Only Apply with a reason saves a canonical fact.**
Recalled text is untrusted background context and cannot authorize tools,
coding, or network.

Fresh configuration enables pinned-note inclusion (`memory.enabled: true`) and
every structured-memory gate. Upgrades preserve existing config, saved
`harness.json` choices, and administrator overrides. Chat history persists
independently of these flags.

## What counts as memory

Paths below are relative to the active Harness home (`CGAGENTHARNESS_HOME`,
default `~/.CGagentHarness`).

| Kind | How it is saved / scope | How it is used later |
|---|---|---|
| Chat history | Successful exchanges are written to `sessions/*.json`; account-owned, with unassigned legacy data quarantined | Bounded recent messages from the selected session accompany subsequent chat. This is not a durable-fact extractor. |
| Pinned notes | `/memory add <literal text>` writes `memory/notes.json`; shared home | Included in the prompt while `/memory on`; see [Pinned notes](#pinned-notes). |
| Persona / soul | `soul.md`; shared home; edit/save or reviewed `/soul apply <id> <reason>` | `/soul on\|off` controls prompt inclusion. Persona guidance is distinct from account-private facts. |
| Canonical structured facts | Private `memory/structured.sqlite3`; `/memory save`, governed fact HTTP writes, or approved proposals | List with `/memory facts`; select for explicit recall or use separately gated facts-only retrieval. Saving does not itself enable prompt inclusion. |
| Pending proposals | Same private SQLite store; a model or operator may propose | Memory panel shows content, action, category, sources and revision. Pending text is never recalled into chat. Apply/Reject requires a reason. |
| Episodes | Private bounded metadata from successful captured chat; coding metadata when coding suggestions are enabled; optional human `semantic_summary` | Provenance and consolidation input. Never injected into chat or indexed by FTS. Metadata is not a durable fact. |
| Facts search index | Derived, contentless FTS5 index in the structured store | Lexical search over facts only. No embeddings, vector database, episode search or RAG fusion. |
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

Slash overrides take precedence. After restart, use `/memory capture on` and
enable any other overlays needed for this recipe. To enable gates separately,
follow the [enable order](STRUCTURED_MEMORY.md#enable-order).

## Automatic suggestions and review

With the store, capture, and either suggestion source enabled, a completed chat
turn or successful coding run can produce pending `session_summary`, `insight`,
or both according to `suggestion_mode`. Invalid modes, including non-string
YAML values, disable suggestions. No setting approves proposals. Suggestions,
human-summary consolidation, and retrieval are separate gates.

After a chat turn or coding run, open **Memory** or `/memory proposals`, then use
**Refresh proposals**. Review the action, category, content, source episode IDs,
proposal ID, and repository scope. Model confidence does not establish
correctness. Enter a reason and choose **Apply** or **Reject**. This confirms the
displayed revision through the proposal-decide API. Blank reasons fail. Refresh
after a stale-proposal error. A closed store has no decision controls.

HTTP equivalent: review `GET /api/structured-memory/proposals/{id}`, then POST
to that same path with `revision`, `confirm: true`, a nonblank `reason`, and
`apply: true` (apply) or `apply: false` (reject), using `X-CyClaw-CSRF`. A model
or operator suggests with `POST /api/structured-memory/proposals` (no
`confirm`); direct add/deactivate also need `confirm: true` and `reason`. Verify
with `GET /api/structured-memory/facts` and `/prompt`.

Not every event produces a proposal. Suggestion failure does not fail the chat
or run. For failures, inspect `automatic_suggestions`, consolidation state, and
metadata audit events. Review existing proposals before generating more because
pending capacity is per owner. See [completion suggestions](STRUCTURED_MEMORY.md#completion-suggestions).

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

For manual-only facts, run `/memory auto-suggest-chat off`,
`/memory auto-suggest-coding off`, and `/memory auto-consolidate off`, or set
their config values false and restart. For a shared literal note, use `/memory add <text>`.

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

With `auto_consolidation` on, a bounded idle worker may claim the episode. It requires
nonblank human summaries, unexpired `none`/`pending` episodes and no running
owner batch, waits for the generation gate, and never preempts chat. A completed
suggestion batch marks its episode done; later summary edits do not reset it for
automatic replay. Manual consolidation remains available.

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
and disable both `auto_suggest_*` switches, `auto_consolidation`, and capture.
Saved facts and pending proposals remain. To stop opening the database, set
`enabled: false` and restart. This does not delete it or disable pinned notes,
persona, or chat-history persistence. Successful exchanges still enter the
account-owned `sessions/*.json`, and the next turn uses bounded recent messages.
No structured-memory flag disables session persistence.

`/clear` clears the display. Sessions Clear has its separate deletion flow: it
cancels waiting/in-flight chat suggestions and keeps facts and proposals; derived
episodes are kept unless you also confirm `delete_derived_episodes`. Export,
purge and what disabling stops: [rollback](STRUCTURED_MEMORY.md#rollback).
