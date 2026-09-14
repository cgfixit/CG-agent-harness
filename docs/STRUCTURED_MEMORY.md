# Structured memory (issue #87, M1 + M3 + M5 + Phase 4 + Phase 5 FTS)

This is the privacy-first structured-memory foundation. It is **not** the
existing pinned-note `/memory` feature and it is **not** the web-research
allowlist/corpus benchmark.

Related issue: [#87](https://github.com/cgfixit/CG-agent-harness/issues/87).

## Two memory systems

| Surface | Scope | Store | Who may write | Prompt inclusion |
|---|---|---|---|---|
| Pinned notes (`/memory`) | Shared home | `memory/notes.json` | Operator slash commands / `/api/memory/*` | `harness.json.memory_enabled` (`/memory on`) |
| Structured memory (M1/M3/M5 + Phase 4 + Phase 5) | Account-private | `memory/structured.sqlite3` | Human confirm+reason for facts; optional post-success episode staging | Facts enter `/prompt` only after an explicit pick (selected facts, `/memory retrieve`, or the per-request `retrieve` flag) **or** the separately gated `auto_retrieval` path, plus assembly recheck. Episodes are never injected. |

M1/M3/M5 do **not** migrate, reinterpret, or weaken pinned notes. Session
deletion continues to preserve notes. Structured facts and proposals are
independent of chat history. Clearing chats does **not** delete derived
episodes unless the operator confirms `delete_derived_episodes` on
`POST /api/sessions/clear`. That cascade still keeps facts, proposals, and
episodes referenced by pending proposals.

## What this slice ships

1. Manual structured **facts** (stable public id, content, category, digest,
   revision, active flag, timestamps).
2. Governed **proposals** (`add` / `update` / `deactivate`) that may only
   **suggest**. Optional `source_episode_ids` bind a suggestion to episode
   provenance without mutating facts.
3. Human apply/reject with persona semantics: deny unknown fields, bind the
   reviewed revision (and fact revision/digest for update/deactivate), require
   `confirm` + nonempty `reason`, re-check, apply once, audit metadata.
4. Optional **episode capture** after a successful `SessionStore::record_exchange`.
   Staging is non-fatal to chat. Episodes store bounded metadata and a
   privacy-filtered summary only — no raw query, no full answer, no hidden
   reasoning, no tool secrets.
5. Manual semantic-summary attach (`POST .../episodes/{id}/summary`) before any
   automatic consolidator. Clipped assistant output is not labeled a summary.
6. TTL, per-owner row/byte quotas, and deterministic oldest-first pruning that
   preserves episodes referenced by pending proposals.
7. Owner list/get/delete, expired purge, owner purge, and bounded local HTML
   export (escaped against stored XSS).
8. Tests and the `tests/fixtures/structured_memory/propose-confirm-recall.json`
   fixture (M1 list-after-apply; not prompt injection).
9. Phase 4 **explicit recall**: bounded fact search/list, session/request
   “include these facts” selection, reserved prompt budget, and assembly-time
   revalidation. Prompt preview reports the exact injected/dropped set.
10. Phase 5 **facts-only FTS5 retrieval**: co-transactional contentless index
    on fact title/value/tags, safe MATCH (tokenize/bound/quote, field-prefixed),
    search API with stable ids/revisions/provenance/lexical score, enable-time
    backfill, fail-soft when the index is busy or missing. Search ≠ inject.

## What this slice does not ship

Automatic consolidation, embeddings, vector databases, RAG fusion, episode FTS,
episode prompt injection, console slash commands that write facts, or any new
cloud egress. Status flags for retrieval fusion, consolidation, and RAG remain
**false**. `retrieval` can be true while those stay false. `explicit_recall` is
independent of `retrieval`. Episode availability is true only when
`structured_memory.episode_capture` is on and the store is open.

Later phases in #87 remain independently gated.

## Gates

- Administrator capability: `structured_memory.enabled` in `config.yaml`,
  evaluated with `flag_is_true` (literal YAML `true` only; quoted `"true"` is
  off). Ships **false**. Disabled startup does not create or open the database.
- Episode capture: `structured_memory.episode_capture`, also `flag_is_true`,
  ships **false**. Capture off performs no episode writes even if the store is
  open. Capture cannot open the store by itself. `/memory capture on|off`
  persists an operator overlay with the same fail-closed rule.
- Explicit recall: `structured_memory.explicit_recall`, also `flag_is_true`,
  ships **false**. Recall off injects no selected facts even if IDs are selected.
  `/memory recall on|off` is the operator overlay. `/memory on` still includes
  pinned notes only.
- Retrieval: `structured_memory.retrieval`, also `flag_is_true`, ships **false**,
  independent of `/memory on` and `explicit_recall`. Off means no FTS search and
  no force-include. `/memory retrieval on|off` is the operator overlay.
- Auto-retrieval: `structured_memory.auto_retrieval`, also `flag_is_true`, ships
  **false**. Requires retrieval. When on, chat may FTS the user message and
  inject rechecked top-k without a per-request flag. This is the Advisor-sensitive
  silent path. `/memory auto-retrieve on|off` is the overlay.
- Prompt budget: combined pinned-note + selected-fact body is capped at 3000
  characters. When both are present the reserved split is
  `pinned_prompt_chars` / `selected_fact_prompt_chars` (default 1500/1500);
  unused reserved capacity is not transferred. When only one source is present
  it may use the full 3000. Headings sit outside that body cap, matching
  existing note/web/goal sections.
- `harness.json.memory_enabled` stays pinned-note prompt inclusion only.

## Ownership

Rows are keyed by `owner_id`. That is the authenticated account `user_id`
(32 hex digits, generated at runtime), the documented `local` namespace from
`context_owner` when accounts are disabled, or a labeled `user_*` id used by
fixtures (`user_alice`, `user_bob`). Tests must not embed 32-hex token-shaped
literals. A second owner cannot list, get, export, or mutate a known id. Admin
capability does not grant inspection of another owner's structured memory.

## Episode contract

- Staging happens **after** successful exchange persistence only.
- Storage, busy, and write errors become truthful `episode.health` on the
  successful chat response. They do not fail the chat.
- `session_ref` / `turn_ref` are random opaque ids, not the raw session
  transcript and not a content hash of the query.
- Auto-staged `privacy_summary` is local metadata (outcome, model, char
  counts, sensitivity). It is not a semantic summary of the answer.
- Retention: oldest `created_ts`, then `public_id`. Pending proposal
  references are not pruned.
- Feature-off means no episode inserts. Existing leftover rows remain
  listable/exportable/purgeable while the store is open (rollback).

## Proposal contract (persona semantics, SQLite store)

Reuse persona review rules, not `soul-history/*.json`:

- `#[serde(deny_unknown_fields)]` on every mutation body.
- Propose does **not** require `confirm` (suggest).
- Decide/direct add/deactivate **do** require `confirm` + `reason`.
- Apply uses `BEGIN IMMEDIATE`, re-validates content, binds owner + proposal
  revision + expected fact revision/digest, updates the FTS row, applies once,
  and writes non-content audit metadata.

## Explicit recall (Phase 4)

Selection binds stable `public_id` values plus `expected_revision` on a
session (`POST /api/sessions/{id}/structured-facts`) or a single
`/api/chat` / `/api/prompt/preview` request. At prompt assembly the store is
re-read with the current owner filter. Missing, inactive, stale-revision, and
cross-owner facts are dropped; only survivors are formatted as untrusted
background context. Preview `structured_facts.injected` / `dropped` is the same
set chat would send. Suggest still does not mutate canonical facts.

## FTS retrieval (Phase 5)

- **Facts only.** Episode summaries are not indexed.
- **Contentless / field-prefixed FTS5** (`title:` / `value:` / `tags:`). MATCH
  input is tokenized, bounded, and quoted. Punctuation, operators, quotes, and
  column selectors in the user string cannot become FTS syntax.
- **Search ≠ inject.** `GET /api/structured-memory/search` returns a small
  top-k with stable ids, revisions, provenance (`fts5`), and lexical score.
  Hits are not written onto the shared session.
- **Force-include** (`retrieve: true` / `/memory retrieve <query>`) is the
  explicit pick for that request: run bounded FTS, recheck survivors, inject
  under the Phase 4 budget. The flag does not persist.
- **auto_retrieval** may run FTS on the user message without a flag. It is
  separately gated and ships false.
- Co-updated with fact add/update/deactivate/purge. Enable-time backfill
  rewrites the virtual table from current active facts in one Immediate
  transaction. Busy/corrupt index is fail-soft for chat.
- Audit records counts and error class, never raw query or fact text.

## Threat model (at rest)

The database is a regular file under the harness home with owner-private mode
(0600) and NOFOLLOW/ownership checks on open. That is **OS access control**,
not encryption. A process running as the home owner can read the SQLite bytes.
Application-managed encryption needs a separate key-lifecycle design; this
release does not place a key beside the database and call the store encrypted.

Audit events record owner-safe ids, revisions, counts, and status. They must
not record fact text, proposal payloads, episode summaries, or reasons that
embed content.

Export is generated locally, bounded by `max_export_bytes`, HTML-escaped, and
owner-filtered. It is not a prompt surface.

## HTTP surfaces

All of these use the existing guard chain and return `Cache-Control: no-store`
when they carry memory content:

| Method | Path | Confirm? |
|---|---|---|
| GET | `/api/structured-memory` | n/a (truthful status) |
| POST | `/api/structured-memory/gates` | operator overlay; not a fact mutation |
| GET | `/api/structured-memory/search` | n/a (FTS candidates; retrieval gate required) |
| GET | `/api/structured-memory/facts` | n/a (active facts; optional `q`/`category`/`limit` literal substring search, not FTS) |
| GET/POST | `/api/sessions/{session_id}/structured-facts` | selection = explicit include; not a fact mutation |
| GET | `/api/structured-memory/facts/{id}` | n/a (recall, including inactive) |
| POST | `/api/structured-memory/facts` | required (direct human add) |
| POST | `/api/structured-memory/facts/{id}/deactivate` | required |
| GET/POST | `/api/structured-memory/proposals` | propose = suggest |
| GET/POST | `/api/structured-memory/proposals/{id}` | decide requires confirm |
| GET | `/api/structured-memory/episodes` | n/a |
| GET | `/api/structured-memory/episodes/{id}` | n/a |
| POST | `/api/structured-memory/episodes/{id}/summary` | required (manual semantic summary) |
| POST | `/api/structured-memory/episodes/{id}/delete` | required |
| POST | `/api/structured-memory/episodes/purge` | required (expired only) |
| GET | `/api/structured-memory/export` | n/a (escaped HTML) |
| POST | `/api/structured-memory/purge` | required (owner facts+episodes+proposals+run metadata) |

Existing `/api/memory*` routes are unchanged. `POST /api/sessions/clear`
accepts optional `delete_derived_episodes`. `/api/chat` and
`/api/prompt/preview` accept `retrieve` / `retrieve_query` for per-request
force-include.

## Verify

```text
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --test structured_memory -- --nocapture
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --lib structured_memory -- --nocapture
```
