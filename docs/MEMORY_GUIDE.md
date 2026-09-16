# Memory: save, suggest, review and retrieve

The Harness can suggest **summaries, durable insights, or both** from completed
Harness chat turns and coding runs. These suggestions persist as pending
proposals. **Only your Apply action with a reason saves a canonical fact.**
Every structured-memory boolean ships true in fresh configuration. Existing
config files and explicit administrator off overrides are preserved. Fresh pinned
notes use `memory.enabled: true`; saved `harness.json` choices take precedence. Ordinary chat-history persistence
is separate and already happens without these flags.

## What counts as memory

Paths below are relative to the active Harness home (`CGAGENTHARNESS_HOME`).

| Kind | How it is saved / scope | How it is used later |
|---|---|---|
| Chat history | Successful exchanges are written to `sessions/*.json`; shared portal resource | Bounded recent messages from the selected session accompany subsequent chat. This is not a durable-fact extractor. |
| Pinned notes | `/memory add <literal text>` writes `memory/notes.json`; shared home | `/memory on` includes notes in the prompt; `/memory off` retains them but stops inclusion. Maximum 20 notes, 500 characters each. |
| Persona / soul | `soul.md`; shared home; edit/save or reviewed `/soul apply <id> <reason>` | `/soul on|off` controls prompt inclusion. Persona guidance is distinct from account-private facts. |
| Canonical structured facts | Private `memory/structured.sqlite3`; `/memory save`, governed fact HTTP writes, or approved proposals | List through the facts API; select for explicit recall or use separately gated facts-only retrieval. Saving does not itself enable prompt inclusion. |
| Pending proposals | Same private SQLite store; a model or operator may propose | Memory panel shows content, action, category, sources and revision. Pending text is never recalled into chat. Apply/Reject requires a reason. |
| Episodes | Private bounded metadata from successful captured chat; coding metadata when coding suggestions are enabled; optional human `semantic_summary` | Provenance and consolidation input. Never injected into chat or indexed by FTS. Metadata is not a durable fact. |
| Facts search index | Derived, contentless FTS5 index in the structured store | Lexical search over facts only. No embeddings, vector database, episode search or RAG fusion. |
| Coding jobs / run records | Existing shared operational records | Evidence about a coding run, not automatically injected memory. The new hook only uses the initiating account's current completed run. |

`session_summary` and `insight` are fact/proposal categories, not new storage
systems. A generated summary covers **one completed turn or run**, not an unseen
whole session. Applying it creates a fact; it does not silently fill an episode's
human `semantic_summary`. External Codex conversations and old Harness archives
are not scanned or imported.

## Gates and configuration

Set keys under `structured_memory:` in the active `config.yaml`. File changes
require a server restart/full app relaunch. Literal YAML `true` is required;
quoted `"true"` remains off. Administrator-only slash overrides persist in
`memory/structured_gates.json` and override their corresponding config values,
including explicit `off` over config `true`.
Inspect `GET /api/structured-memory` for effective gates and queue status.

| Key (all default `true`) | Effect / dependency | Live slash overlay |
|---|---|---|
| `enabled` | Opens the private store at startup; required by every structured feature | None; restart required |
| `episode_capture` | Allows episode staging; required by automatic completion suggestions | `/memory capture on|off` |
| `explicit_recall` | Allows selected facts into a prompt after owner/active/revision checks | `/memory recall on|off` |
| `retrieval` | Enables facts-only FTS search and explicit per-request retrieval | `/memory retrieval on|off` |
| `auto_retrieval` | With retrieval, searches/injects for chat without a per-request pick | `/memory auto-retrieve on|off` |
| `consolidation` | Allows the existing selected-episode consolidator | `/memory consolidation on|off` |
| `auto_consolidation` | With consolidation, processes eligible human-summarized episodes when idle | `/memory auto-consolidate on|off` |
| `auto_suggest_chat` | With store + capture, queues the completed current chat turn for proposals | `/memory auto-suggest-chat on\|off` |
| `auto_suggest_coding` | With store + capture, queues successful synchronous/detached coding runs for proposals | `/memory auto-suggest-coding on\|off` |

`/memory on` only enables pinned-note inclusion. It opens none of these gates.
`auto_retrieval` controls use of saved facts, not suggestion quality; it remains
independent and on in fresh configuration.

### Fresh defaults: automatic suggestions, approval before saving

Fresh homes already have these defaults. To enable them in an existing home,
merge this into its existing block; do not create duplicate YAML keys:

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

If an existing `/memory capture off` overlay is present, use `/memory capture on`
after restart. Enable existing consolidation/recall/retrieval off overrides too if you
want the exact recipe above. Either source switch can be enabled alone. Invalid
mode values disable suggestions. No setting auto-approves them.

Complete a chat turn or coding run, open **Memory** or `/memory proposals`, and
use **Refresh proposals** after generation. Expand the full proposal, then Apply
or Reject with a nonblank reason. Each action confirms the displayed revision
through the existing proposal-decide API. Check the content and repository scope;
model confidence does not establish correctness.

### Manual hard-save with automation disabled

Keep `enabled: true`; run `/memory auto-suggest-chat off`,
`/memory auto-suggest-coding off`, and `/memory auto-consolidate off`. Capture
may also be off. Then:

```text
/memory save For repository example, use metric units in examples. :: My reviewed standing preference
```

The command itself explicitly confirms a private fact write, requires a visible
reason after the last `::`, and uses the existing facts API with category `manual`.
It needs neither capture nor model inference. The store must already be open.
Plain language such as “remember this” does not execute this command: with
automatic suggestions enabled it may produce a draft, but cannot approve one.

For a shared literal pinned note instead, use `/memory add <text>`; that follows
the existing pinned-note contract. For a human summary attached to an episode:

```text
/memory remember Prefer metric units in examples. :: My standing preference
/memory consolidation on
/memory consolidate <returned-episode-id>
/memory proposals
```

`remember` only attaches the sentence to your latest completed episode. It saves
no fact and starts no model run. Auto-consolidation requires nonblank human
summaries, unexpired `none`/`pending` episodes and no running owner batch. A
completed suggestion batch marks its episode done; later summary edits do not
reset it for automatic replay. Explicit manual consolidation remains available.
Manual selection of summary-less episodes is allowed but produces worse input.

### Disable derived automation or all structured memory

To retain manual facts but stop derived capture and suggestions: leave the store
enabled, disable both `auto_suggest_*` switches and `auto_consolidation`, and close
capture/auto-consolidation overlays. Existing saved facts and pending proposals
remain. To avoid opening the structured database at all, set `enabled: false`
and restart. This does not delete it or disable pinned notes, persona, or ordinary
chat-history persistence. There is currently no structured-memory flag that
disables saving successful chat exchanges to the session store.

## Retrieve and inspect

- `GET /api/structured-memory/facts` lists your facts even with prompt recall off.
- With `explicit_recall` on, the existing request/session selected-fact API accepts
  IDs and revisions. Assembly rechecks ownership, active state and revision.
- `/memory retrieval on`, then `/memory search <query>` returns candidates only.
- `/memory retrieve <query>` sends that query as a chat turn and force-includes
  rechecked matches for that request only. It does not persist session selection.
- `/prompt` previews actual selected facts, pinned notes and persona. To preview
  retrieval, use `POST /api/prompt/preview` with `retrieve`/`retrieve_query`.

The combined notes/facts body budget is 3000 characters (reserved 1500/1500 when
both are present). Pending proposals and episode summaries never enter that
budget. A saved fact can therefore exist without appearing in a particular prompt.

## Bounds, privacy and failure behavior

Completion suggestions receive bounded, redacted current input/output in RAM.
They do not reread shared session/job archives. Coding input is the submitted
instruction plus a projection of the child result (repository, status, run id,
changed files, push/PR fields and commit message), not stdout/stderr, file bodies
or tool logs. Metadata alone cannot prove a coding lesson or successful tests.

| Setting | Default | Clamp / meaning |
|---|---|---|
| `suggestion_max_input_chars` | 8000 | 256–32000 total, split equally between input/output; character-safe clipping |
| `suggestion_max_queue` | 8 | 1–32 waiting jobs globally, plus at most one active generation |
| `suggestion_queue_ttl_secs` | 300 | 1–3600 seconds for waiting input; active inference uses the chat-client timeout |
| `suggestion_idle_ms` | 2000 | 10–30000 ms between worker attempts |
| `min_consolidation_confidence` | 0.40 | 0–1, shared by both generators; supplied lower values drop, omitted confidence remains accepted |
| `max_consolidation_candidates` | 8 | Existing candidate bound, also used for completion suggestions |
| `max_proposals_per_owner` | 32 | Pending proposal capacity; review existing proposals before generating more |

Input scanner hits, a full/expired queue, disabled capture, model errors or store
quotas can prevent suggestions. Successful chat/coding results stay successful.
There is no guarantee of a proposal for every event; empty output is valid.
Restart discards waiting input without replay/backfill. Durable pending proposals
survive restart. Session clear invalidates queued/in-flight chat suggestions;
existing facts/proposals survive under the existing deletion contract. Review
`automatic_suggestions` status, consolidation-run state and metadata audit events
for failures. Cancelling a running batch is allowed while the manual consolidation
gate is off, provided the store is open and the run belongs to the caller.

The worker waits while the generation gate is held; it does not preempt chat.
A new chat arriving during an already-running suggestion may receive `CHAT_BUSY`.
The local OpenAI-compatible model receives no tools/web or recalled facts, with
temperature 0 and max_tokens 1024. Redactors/scanners are bounded safeguards,
not a guarantee that arbitrary secrets or hallucinations are detected. Human
review remains mandatory. Private SQLite permissions are not encryption at rest.

See [the contract](STRUCTURED_MEMORY.md), [operator commands](USER_MANUAL.md),
[issue #87 close-out](memory/ISSUE_87_CLOSEOUT.md), and
[the next-task benchmark plan](MEMORY_BENCHMARK_PLAN.md). Phase 7 remains the
locked regression bar. No default flip or completion of issue #87 is implied.
