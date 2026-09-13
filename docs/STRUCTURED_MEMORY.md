# Structured memory (issue #87, M1)

This is the privacy-first structured-memory foundation. It is **not** the
existing pinned-note `/memory` feature and it is **not** the web-research
allowlist/corpus benchmark.

Related issue: [#87](https://github.com/cgfixit/CG-agent-harness/issues/87).

## Two memory systems

| Surface | Scope | Store | Who may write | Prompt inclusion |
|---|---|---|---|---|
| Pinned notes (`/memory`) | Shared home | `memory/notes.json` | Operator slash commands / `/api/memory/*` | `harness.json.memory_enabled` (`/memory on`) |
| Structured memory (M1) | Account-private | `memory/structured.sqlite3` | Human confirm+reason only | Not injected in M1; list/recall APIs only |

M1 does **not** migrate, reinterpret, or weaken pinned notes. Session deletion
continues to preserve notes. Structured facts and proposals are independent:
clearing chats does not delete them. A separately confirmed purge is future
work (no implicit cascade).

## What M1 ships

1. Manual structured **facts** (stable public id, content, category, digest,
   revision, active flag, timestamps).
2. Governed **proposals** (`add` / `update` / `deactivate`) that may only
   **suggest**.
3. Human apply/reject with persona semantics: deny unknown fields, bind the
   reviewed revision (and fact revision/digest for update/deactivate), require
   `confirm` + nonempty `reason`, re-check, apply once, audit metadata.
4. Explicit status / list / recall APIs sufficient to verify
   propose → confirm → recall.
5. Tests and the `tests/fixtures/structured_memory/propose-confirm-recall.json`
   fixture.

## What M1 does not ship

Automatic episode capture, automatic consolidation, lexical/FTS retrieval,
embeddings, vector databases, RAG fusion, prompt injection of selected facts,
console slash commands, or any new cloud egress. Status flags for those
surfaces are **false**.

Later phases in #87 remain independently gated. Do not treat this PR as M2–M7.

## Gates

- Administrator capability: `structured_memory.enabled` in `config.yaml`,
  evaluated with `flag_is_true` (literal YAML `true` only; quoted `"true"` is
  off). Ships **false**. Disabled startup does not create or open the database.
- `harness.json.memory_enabled` stays pinned-note prompt inclusion only.

## Ownership

Rows are keyed by `owner_id`. That is the authenticated account `user_id`
(32 hex digits, generated at runtime), the documented `local` namespace from
`context_owner` when accounts are disabled, or a labeled `user_*` id used by
fixtures (`user_alice`, `user_bob`). Tests must not embed 32-hex token-shaped
literals. A second owner cannot list, get, or mutate a known id. Admin
capability does not grant inspection of another owner's structured memory.

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
not record fact text, proposal payloads, or reasons that embed content.

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

Existing `/api/memory*` routes are unchanged.

## Verify

```text
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --test structured_memory -- --nocapture
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --lib structured_memory -- --nocapture
```
