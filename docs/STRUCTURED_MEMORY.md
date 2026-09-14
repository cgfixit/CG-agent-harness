# Structured memory (issue #87, M1 + M3 + M5)

This is the privacy-first structured-memory foundation. It is **not** the
existing pinned-note `/memory` feature and it is **not** the web-research
allowlist/corpus benchmark.

Related issue: [#87](https://github.com/cgfixit/CG-agent-harness/issues/87).

## Two memory systems

| Surface | Scope | Store | Who may write | Prompt inclusion |
|---|---|---|---|---|
| Pinned notes (`/memory`) | Shared home | `memory/notes.json` | Operator slash commands / `/api/memory/*` | `harness.json.memory_enabled` (`/memory on`) |
| Structured memory (M1/M3/M5) | Account-private | `memory/structured.sqlite3` | Human confirm+reason for facts; optional post-success episode staging | **Not injected.** List/recall/export APIs only. Phase 4 is required before any `/prompt` inclusion. |

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
   fixture.

## What this slice does not ship

Automatic consolidation, lexical/FTS retrieval, embeddings, vector databases,
RAG fusion, **prompt injection of facts or episodes**, console slash commands
that write facts, or any new cloud egress. Status flags for retrieval,
consolidation, and RAG remain **false**. Episode availability is true only when
`structured_memory.episode_capture` is the literal YAML boolean `true` and the
store is open.

Later phases in #87 remain independently gated.

## Gates

- Administrator capability: `structured_memory.enabled` in `config.yaml`,
  evaluated with `flag_is_true` (literal YAML `true` only; quoted `"true"` is
  off). Ships **false**. Disabled startup does not create or open the database.
- Episode capture: `structured_memory.episode_capture`, also `flag_is_true`,
  ships **false**. Capture off performs no episode writes even if the store is
  open. Capture cannot open the store by itself.
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
  revision + expected fact revision/digest, applies once, and writes
  non-content audit metadata.

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
| GET | `/api/structured-memory/facts` | n/a (active facts) |
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
accepts optional `delete_derived_episodes`.

## Verify

```text
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --test structured_memory -- --nocapture
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --lib structured_memory -- --nocapture
```
