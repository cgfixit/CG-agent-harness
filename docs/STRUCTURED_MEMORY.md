# Structured memory contract

## Two memory systems

Pinned notes are shared within a home (`memory/notes.json`) and controlled by
`/memory on`. The structured store (`memory/structured.sqlite3`) owns
account-private facts, pending proposals, episodes and runs. Uploaded notes
have a separate [corpus](MEMORY_GUIDE.md#what-counts-as-memory). Only active,
eligible facts enter model context from the structured store. Fact mutation
requires a human decision. Retrieval uses lexical FTS5.

## Gates

All nine `structured_memory.*` flags ship true. Missing and quoted values are
off. Opening the store requires `enabled: true` at restart. With the store
open, administrators can override each sub-gate live via `/memory <name>
on|off`. `memory/structured_gates.json` persists the override. Malformed
override state fails startup.

| YAML gate | Slash name | Additional prerequisite |
|---|---|---|
| `episode_capture` | `capture` | — |
| `explicit_recall` | `recall` | — |
| `retrieval` | `retrieval` | — |
| `auto_retrieval` | `auto-retrieve` | `retrieval` |
| `consolidation` | `consolidation` | — |
| `auto_consolidation` | `auto-consolidate` | `consolidation` |
| `auto_suggest_chat` | `auto-suggest-chat` | `episode_capture` |
| `auto_suggest_coding` | `auto-suggest-coding` | `episode_capture` |

## Prompt budget

Pinned notes and facts share 3,000 body characters. When both are present,
`pinned_prompt_chars` and `selected_fact_prompt_chars` reserve their portions
(shipped 1,500 each). One source alone may take the full budget. Headings sit
outside the cap.

## Completion suggestions

`completion-suggestions-v1` uses bounded, redacted completion input in RAM.
Chat stages after persistence. Coding stages after an eligible shim result
(including `pending_decision`), with submitted instruction and projected
metadata. File bodies, stdout and tool logs are excluded. In `/loop` only a
reply with standalone `GOAL_DONE` qualifies, paired with its human goal.
This marker provides no completion verification.

The exact local model and `suggestion_mode` must be in
`suggestion_supported`. Missing lists support nothing. Ollama alone normalizes
bare names to `:latest`. An explicit `"*"` opts out of the model restriction.
Invalid lists or unsupported pairs refuse generation with a reason.
Suggestions use local JSON generation without tools, web or recalled facts.
Cloud selection resolves to the configured local backend for memory.

Candidates must satisfy schema, source membership, sensitivity, confidence
and fact bindings. Duplicate normalized facts and pending or rejected
proposals are filtered. Full capacity refuses generation or trims a batch to
free slots. Successful runs create pending `session_summary` or `insight`
proposals. They leave the episode's human summary and consolidation state
alone.

Metadata receipts distinguish queued, running, done, failed and cancelled
outcomes. Chat preempts active generation and requeues it. Clear,
cancellation and expiry discard input. Restart marks lost input or interrupted
inference without replay. Pending proposals persist. See
[configuration](../assets/config.default.yaml) for queue, retention and
proposal bounds. The queue bound counts waiting inputs plus at most one
active generation. Accepting a queue entry produces a metadata-only
`idle` receipt. Worker admission checks capacity before model calls.
Generation requests local JSON-object format, temperature zero and a
bounded output. A valid empty candidate list is a completed run. Model,
schema and storage errors remain separate from the originating work.

## Ownership

Each operation uses the authenticated account ID, or `local` when accounts
are disabled. Administrators have no cross-owner read or mutation grant.
Episode IDs and session/turn references are opaque. Captured episodes contain
bounded metadata. Human `semantic_summary` comes from `/memory remember`
or the summary route. TTL and quotas prune oldest first, preserving episodes
referenced by pending proposals. Sessions Clear only prunes derived episodes
with explicit `delete_derived_episodes`.
`GET /api/structured-memory/episodes?latest_completed=true` selects the
owner's latest completed episode before ordinary listing bounds. Adding
a manual summary requires nonblank reason and enforces text bounds and
scanning. It works with capture off if the completed row still exists.

## Proposal contract (persona semantics, SQLite store)

Propose creates pending state. Apply, reject, direct add and deactivate
require confirmation and a reason. Apply checks owner, proposal revision
and affected fact revision/digest in one immediate transaction, updates FTS
and applies once. Fact content and category pass the core injection scanner
on create, propose and apply. Refused apply leaves the proposal pending.
Scanner coverage is limited to core patterns. Treat recalled content as
untrusted even when it passes. Mutations deny unknown fields. Apply
binds a pending proposal's revision and, for affected existing facts,
their expected revision and digest. Direct confirmation supplies authority
to write the fact, while confidence and source IDs only describe the draft.

## Explicit recall

Select `public_id` and `expected_revision` on a session or a single chat or
prompt preview. Assembly rechecks owner, active state and revision.
For local chat, facts are fenced before the new message and excluded from
stored history. Pinned notes go in the system prompt. `/loop` puts both
there. Cloud chat excludes memory.

## FTS retrieval

Contentless FTS5 indexes active facts and co-updates on mutation. Bounded,
quoted tokens keep input out of MATCH syntax. Explicit searches match all
selected terms. Automatic retrieval can match any meaningful bounded term.
`/search` returns candidates only. `retrieve: true` or `/memory retrieve`
force-includes rechecked candidates for one turn. `auto_retrieval` can include
top hits without an explicit pick. Busy or corrupt retrieval fails soft for
chat. Audit records status and counts. The bounded automatic path ignores
common query words. `max_retrieval_results` governs the effective result
limit. `/facts?q=…` is a separate literal substring lookup.
Indexes are rebuilt from active facts when enabling retrieval.

## Consolidation

Manual runs accept selected episodes. Auto runs require an eligible,
nonblank human summary and idle model access. Extraction uses only local
episode evidence with no tools, web or recalled facts. Facts are loaded
after generation for bindings. Results are pending proposals only.
The owner, sorted episode IDs and consolidator version key make completed
batches idempotent. Failed, cancelled or interrupted batches need manual
retry, except eligible owner-retired work when the account returns.

## Threat model (at rest)

Unix storage uses private file permissions and no-follow checks. Windows
has [platform limits](../INVARIANTS.md#signals-weaker-than-their-name).
Filesystem permissions protect the plaintext database. Owner-scoped export
is escaped, bounded HTML. Audit excludes content and reasons.

## HTTP surfaces

Read under `/api/structured-memory` for status, `/facts`, `/search`,
`/proposals`, `/episodes`, `/consolidation` and `/export`. Content is
`no-store`. `POST /gates` sets a sub-gate. Mutation routes require
confirmation and reason for deciding proposals, direct fact writes,
episode summary/delete and purge. Proposing and starting consolidation are
suggestion operations. `POST /consolidation/{id}/cancel` stops owned pending
work. `/api/sessions/{id}/structured-facts` stores explicit selection.
See [routes](../src/server/routes/mod.rs).

## Enable order

Open the store, then enable capture, explicit recall, retrieval and
consolidation as needed. Enable automatic retrieval, consolidation and
completion suggestions after their respective prerequisites.

## Rollback

Turn off individual gates to stop the associated path while preserving rows.
`enabled: false` at restart keeps the database closed. Export or purge
requires opening the store.

## Verify

`cargo test --locked --test structured_memory` and
`cargo test --locked --test structured_memory_phase7` exercise fixtures.
The synthetic corpus measures binding, recall, rejection and retention.
Live model quality requires separate evidence.
