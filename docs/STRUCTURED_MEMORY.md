# Structured memory contract

This is the privacy-first structured-memory contract. It is **not** the
existing pinned-note `/memory` feature and it is **not** the web-research
allowlist/corpus benchmark. How to use memory: [MEMORY_GUIDE.md](MEMORY_GUIDE.md).

## Two memory systems

| Surface | Scope | Store | Who may write | Prompt inclusion |
|---|---|---|---|---|
| Pinned notes (`/memory`) | Shared home | `memory/notes.json` | Operator slash commands / `/api/memory/*` | `harness.json.memory_enabled` (`/memory on`) |
| Structured memory | Account-private | `memory/structured.sqlite3` | Human confirm+reason for facts; optional post-success episode staging; optional consolidation or completion suggestions into pending proposals | Facts enter `/prompt` only after an explicit pick (selected facts, `/memory retrieve`, or the per-request `retrieve` flag) **or** the separately gated `auto_retrieval` path, plus assembly recheck. Episodes are never injected. |

Structured memory does **not** migrate, reinterpret, or weaken pinned notes. Session
deletion continues to preserve notes. Structured facts and proposals are
independent of chat history. Clearing chats does **not** delete derived
episodes unless the operator confirms `delete_derived_episodes` on
`POST /api/sessions/clear`. That cascade still keeps facts, proposals, and
episodes referenced by pending proposals.

## What ships

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
5. Manual semantic-summary attach (`POST .../episodes/{id}/summary`), which the
   episode auto-consolidator requires. Clipped assistant output is not labeled a summary.
6. TTL, per-owner row/byte quotas, and deterministic oldest-first pruning that
   preserves episodes referenced by pending proposals.
7. Owner list/get/delete, expired purge, owner purge, and bounded local HTML
   export (escaped against stored XSS).
8. Tests and the `tests/fixtures/structured_memory/propose-confirm-recall.json`
   fixture (list-after-apply; not prompt injection).
9. **Explicit recall**: bounded fact search/list, session/request
   “include these facts” selection, reserved prompt budget, and assembly-time
   revalidation. Prompt preview reports the exact injected/dropped set.
10. **Facts-only FTS5 retrieval**: co-transactional contentless index
    on fact title/value/tags, safe MATCH (tokenize/bound/quote, field-prefixed),
    search API with stable ids/revisions/provenance/lexical score, enable-time
    backfill, fail-soft when the index is busy or missing. Search ≠ inject.
11. **Manual consolidation**: operator-selected episode IDs are sent to
    the local model (tools/web disabled, no recalled-fact prompt input). Strict
    JSON candidates become **pending proposals only**. Apply/deactivate still
    require confirm+reason. Runs are durable and idempotent over owner + ordered
    episode set + summarizer version. Restart recovers interrupted `running`
    rows without duplicating proposals.
12. **Optional automatic consolidation**: when
    `auto_consolidation` is on **and** consolidation is available, a bounded
    idle worker may enqueue eligible episodes and reuse the manual runner.
    Feature-off starts no worker. Chat wins the generation gate. Output remains
    pending proposals only.
13. **Evaluation corpus** under
    `tests/fixtures/structured_memory/phase7/`: synthetic cases for stable
    preferences, temporary statements, negation, corrections, secrets,
    prompt injection, conflicting facts, and cross-owner attempts. A
    fixture-model measurement path asserts provisional baselines and shipped
    enabled memory defaults. Live reviewer rates and latency percentiles
    stay documented-only.

## What does not ship

Embeddings, vector databases, RAG fusion, harness-writable RAG ingestion,
episode FTS, episode prompt injection, silent or autonomous canonical fact
apply, or any new cloud egress. `/memory save <text> :: <reason>` is an
explicit human fact write through the existing API.
Status flags for retrieval fusion and RAG remain **false**. `retrieval` or
`consolidation` can be true while those stay false.
Episode availability is true only when
`structured_memory.episode_capture` is on and the store is open.

## Gates

Every gate is evaluated with `flag_is_true`: literal YAML `true` only. Quoted
`"true"`, missing keys and invalid values stay **off**. All nine ship **true** in
fresh configuration; existing config files keep their own values. Config edits
need a server restart or full app relaunch.

A sub-gate is available only when the store is open **and** its administrator
override, if set, or otherwise its config value is true. Overrides are
administrator-only when accounts are enabled, persist versioned booleans in
`<home>/memory/structured_gates.json`, and take effect immediately; an explicit
`off` overrides config `true`, and explicit `on` enables a config-false gate.
Overrides **cannot open the store**: the store-opening gate has no slash
override.

| Gate (`structured_memory.*`) | Administrator override | Requires | Off means |
|---|---|---|---|
| `enabled` | None; config + restart only | — | Startup does not create or open the database. |
| `episode_capture` | `/memory capture on\|off` | store | No episode writes even if the store is open. Capture cannot open the store by itself. |
| `explicit_recall` | `/memory recall on\|off` | store | No selected facts are injected even if IDs are selected. |
| `retrieval` | `/memory retrieval on\|off` | store | No FTS search and no force-include. Independent of `explicit_recall`. |
| `auto_retrieval` | `/memory auto-retrieve on\|off` | `retrieval` | Chat never searches without an explicit pick. When on, chat may FTS the user message and inject rechecked top-k without a per-request flag; this is the Advisor-sensitive silent path. |
| `consolidation` | `/memory consolidation on\|off` | store | No selected-episode summarizer call (`/memory consolidate <id...>`). The completion-suggestion path may still create its own run records. |
| `auto_consolidation` | `/memory auto-consolidate on\|off` | `consolidation` (AND) | No idle worker starts. Eligibility and bounds: [Consolidation](#consolidation). |
| `auto_suggest_chat` | `/memory auto-suggest-chat on\|off` | store + `episode_capture` | Completed chat turns are not queued for suggestions. |
| `auto_suggest_coding` | `/memory auto-suggest-coding on\|off` | store + `episode_capture` | Successful coding runs are not queued for suggestions. |

Consolidation is independent of capture, recall, retrieval, and `auto_retrieval`;
`explicit_recall` is independent of `retrieval` and consolidation.
`harness.json.memory_enabled` (`/memory on`) stays pinned-note prompt inclusion
only and opens none of these gates.

## Prompt budget

The combined pinned-note + selected-fact body is capped at 3000 characters.
When both are present the reserved split is `pinned_prompt_chars` /
`selected_fact_prompt_chars` (default 1500/1500); unused reserved capacity is
not transferred. When only one source is present it may use the full 3000.
Headings sit outside that body cap, matching existing note/web/goal sections.

## Write-time injection checks and their limits

Fact and proposal writes use `Scanner::core()` through
`StructuredMemoryStore::clean_text`. Both content and category are checked on
direct fact creation, add/update proposal creation (including consolidation and
completion suggestions), and again when a pending add/update proposal is
applied. A core-pattern hit such as `ignore previous instructions` returns
`STRUCTURED_MEMORY_INJECTION` (HTTP 400) before the write. A refused apply
leaves the proposal pending and canonical facts unchanged; the operator can
still reject it. Confirmation, reason, ownership and revision checks remain
separate requirements.

This is the fixed core pattern set, not the configured
`policy.prompt_filter.banned_patterns` or the full coding governance scan.
Matching is case-insensitive and recognizes whitespace variants, but this
path does not use `scan_normalized`. It is not a secret scanner or a general
prompt-injection detector. Individually clean fragments may pass, and the
assembled recalled-facts block has no injection scan. Recalled facts remain
untrusted background context; the label does not guarantee model obedience.
Review pending proposals together for suspicious fragments before applying
them.

Regression coverage: `tests/structured_memory.rs` exercises HTTP fact/proposal
refusals; the unit tests in `src/server/structured_memory.rs` exercise
consolidation staging and apply-time rescanning of a legacy unsafe proposal
with a valid revision. Existing benign propose/apply/recall and evaluation-corpus
tests cover legitimate writes and the distinction from secret detection.

## Completion suggestions

`auto_suggest_chat` and `auto_suggest_coding` are two independent default-true
gates with administrator-only slash overrides. Both require an open store and episode capture;
neither requires or opens consolidation, recall or retrieval. `suggestion_mode`
selects `summaries`, `insights`, or `both` (default); invalid values disable them.
The `completion-suggestions-v1` worker reads only bounded redacted current
completion evidence in RAM, using the initiating account captured by the chat or
coding route. It does not scan shared archives or attach generated text as a
human semantic summary. Successful synchronous and detached coding routes share
the hook after the existing shim boundary; I6 is unchanged. Coding input is the
submitted instruction plus a projection of the child result (repository, status,
run id, changed files, push/PR fields and commit message), not stdout/stderr,
file bodies or tool logs. Metadata alone cannot prove a coding lesson or
successful tests.

The local model returns `session_summary` / `insight` candidates. Strict parsing,
source membership, sensitivity rejection, the confidence floor and existing
binding produce pending proposals through existing idempotent run records.
Canonical facts remain unchanged until Apply with confirm+reason. There is no
schema bump, tool/web access, recalled-fact payload or new cloud egress. Generation
uses temperature 0 and max_tokens 1024. The episode consolidator remains
`consolidator-v2` and its automatic path still requires human semantic summaries.

| Setting | Default | Clamp / meaning |
|---|---|---|
| `suggestion_max_input_chars` | 8000 | 256–32000 total, split equally between input/output; character-safe clipping |
| `suggestion_max_queue` | 8 | 1–32 waiting jobs globally, plus at most one active generation |
| `suggestion_queue_ttl_secs` | 300 | 1–3600 seconds for waiting input; active inference uses the chat-client timeout |
| `suggestion_idle_ms` | 2000 | 10–30000 ms between worker attempts |
| `min_consolidation_confidence` | 0.40 | 0–1, shared by both generators; supplied lower values drop, omitted confidence remains accepted |
| `max_consolidation_candidates` | 8 | Existing candidate bound, also used for completion suggestions |
| `max_proposals_per_owner` | 32 | Pending proposal capacity |

Waiting input is lost on restart and never backfilled from history; durable
pending proposals survive restart. Input scanner hits, a full/expired queue,
disabled capture, model errors, low confidence or store quotas can prevent
suggestions; empty output is valid. Failures do not fail completed chat/coding
results. Session clear invalidates waiting/in-flight
chat suggestions. Existing proposals/facts retain their existing deletion rules.
The worker waits for the generation gate and never interrupts chat.
Chat preempts an active suggestion instead of receiving `CHAT_BUSY`: its model call
is aborted, its run stored as `state=cancelled`, `error_class=preempted` (an owner
cancel ends it), and its job returns to the queue front. Run
cancellation requires the open store and owner match, even if manual consolidation
is off.
Redactors/scanners are bounded safeguards, not a guarantee that arbitrary secrets
or hallucinations are detected; human review remains mandatory.

## Ownership

Rows are keyed by `owner_id`. That is the authenticated account `user_id`
(32 hex digits, generated at runtime), the documented `local` namespace from
`context_owner` when accounts are disabled, or a labeled `user_*` id used by
fixtures (`user_alice`, `user_bob`). Tests must not embed 32-hex token-shaped
literals. A second owner cannot list, get, export, or mutate a known id. Admin
capability does not grant inspection of another owner's structured memory.

## Episode contract

- Chat staging happens **after** successful exchange persistence only. Coding
  suggestion staging happens after a successful shim result and keeps only
  metadata; detached jobs must also have persisted the finished result.
- Storage, busy, and write errors become truthful `episode.health` on the
  successful chat response. They do not fail the chat.
- `session_ref` / `turn_ref` are random opaque ids, not the raw session
  transcript and not a content hash of the query.
- Auto-staged `privacy_summary` is local metadata (outcome, model, char
  counts, sensitivity). It is not a semantic summary of the answer.
- `/memory remember <sentence> :: <reason>` confirms attaching your trimmed
  summary to this owner's latest completed episode (newest `created_ts`, then
  `public_id` ascending). It uses the existing summary API, its character bound
  (`max_episode_summary_chars`, default 500), and its NUL/injection checks.
  A visible nonblank reason is required, like `/soul apply`; the command is the
  explicit confirmation. It never creates the store, writes facts, or starts
  consolidation. Capture may be off if a completed episode already exists.
- `GET /api/structured-memory/episodes?latest_completed=true` selects that one
  episode in SQL, before the ordinary list's 256-row cap. No match returns an
  empty list; a closed store returns the existing disabled error.
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

The console **Memory** tab (also `/memory proposals`) lists pending proposals
from the existing list API with `?status=pending`, filtered in SQL before its
128-row cap so decided history cannot hide pending work. Its **Apply** and
**Reject** buttons send the displayed proposal revision to the existing
`POST /api/structured-memory/proposals/{id}` route; there is no separate decide
endpoint or extra mutation rule.

## Explicit recall

Selection binds stable `public_id` values plus `expected_revision` on a
session (`POST /api/sessions/{id}/structured-facts`) or a single
`/api/chat` / `/api/prompt/preview` request. At prompt assembly the store is
re-read with the current owner filter. Missing, inactive, stale-revision, and
cross-owner facts are dropped; only survivors are formatted as untrusted
background context. Preview `structured_facts.injected` / `dropped` is the same
set chat would send. Suggest still does not mutate canonical facts.

## FTS retrieval

- **Facts only.** Episode summaries are not indexed.
- **Contentless / field-prefixed FTS5** (`title:` / `value:` / `tags:`). MATCH
  input is tokenized, bounded, and quoted. Punctuation, operators, quotes, and
  column selectors in the user string cannot become FTS syntax.
- **Search ≠ inject.** `GET /api/structured-memory/search` returns a small
  top-k with stable ids, revisions, provenance (`fts5`), and lexical score.
  Hits are not written onto the owned session.
- **Force-include** (`retrieve: true` / `/memory retrieve <query>`) is the
  explicit pick for that request: run bounded FTS, recheck survivors, inject
  under the prompt budget. The flag does not persist.
- Co-updated with fact add/update/deactivate/purge. Enable-time backfill
  rewrites the virtual table from current active facts in one Immediate
  transaction. Busy/corrupt index is fail-soft for chat.
- Audit records counts and error class, never raw query or fact text.

## Consolidation

- **Selected episodes only.** The operator supplies episode public IDs.
  Manual runs still accept explicitly selected summary-less episodes; this is
  a worse quality path because metadata cannot support durable facts. The v2
  prompt asks for no candidates from metadata alone. Auto-batching requires a
  non-NULL, nonblank semantic summary and an unexpired `none`/`pending` row;
  owners with a running consolidation are skipped.
- **Suggestion quality.** `consolidator-v2` asks for durable preferences,
  identity, corrections, or standing constraints supported by semantic summaries.
  Temporary plans, jokes, tool/status dumps, errands, and unsupported guesses
  are non-candidates; an empty candidate array is valid. Negation and attribution
  must survive. Secrets/injection-like text remain reject sensitivity.
  `structured_memory.min_consolidation_confidence` defaults to **0.40**, clamps
  to [0, 1], and falls back to 0.40 for invalid/nonfinite configuration. A supplied
  confidence below the floor is counted as rejected. Missing confidence remains
  accepted without inventing a value. Confidence never bypasses human review.
- **Local model only.** The existing generation gate is claimed; tools and web
  are not attached. Interactive chat and consolidation contend for that gate
  and report busy truthfully.
- **No recalled-memory feedback.** Current facts are loaded only after
  extraction, to bind update/deactivate revision+digest or convert an exact
  content match from add → update.
- **Pending proposals only.** A successful run never creates, updates, or
  deletes canonical facts. Confirm+reason apply is unchanged.
- **Bounded.** `max_consolidation_episodes` / `max_consolidation_candidates`
  (default 8/8). Invalid schema, cancellation, model failure, and storage
  failure become run `failed`/`cancelled` with a non-content `error_class`.
- **Idempotent.** The same owner + sorted episode set + `consolidator-v2` key
  returns the completed run and does not insert a second proposal batch.
  A completed v1 key does not suppress a v2 run on the same episode set. Retrying
  v2 reuses its completed run; it does not create another proposal batch.
  Interrupted `running` rows become `failed`/`interrupted` on reopen and may
  be retried on the same key.
- Audit records run id, counts, state, and error class. It does not record
  episode summaries, proposal payloads, or raw model output.

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
| POST | `/api/structured-memory/gates` | administrator-only gate override; body `{"gate": "<config name>", "enabled": bool}` where the name is one of `episode_capture`, `explicit_recall`, `retrieval`, `auto_retrieval`, `consolidation`, `auto_consolidation`, `auto_suggest_chat`, `auto_suggest_coding`; not a fact mutation |
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
| GET | `/api/structured-memory/consolidation` | n/a (this owner's run metadata) |
| POST | `/api/structured-memory/consolidation` | start = suggest; selected episode IDs |
| GET | `/api/structured-memory/consolidation/{id}` | n/a |
| POST | `/api/structured-memory/consolidation/{id}/cancel` | abort in-flight generation |

Existing `/api/memory*` routes are unchanged. `POST /api/sessions/clear`
accepts optional `delete_derived_episodes`. `/api/chat` and
`/api/prompt/preview` accept `retrieve` / `retrieve_query` for per-request
force-include.

## Evaluation corpus

The corpus is local and synthetic. Re-run it on the source/build you intend to
use:

```text
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --test structured_memory_phase7 -- --nocapture
cargo run --locked --example structured_memory_eval -- /tmp/phase7-report.json
```

The test and example share `tests/fixtures/structured_memory/phase7/corpus.json`.
They exercise the store and fixture-model consolidator JSON (the same pattern as
`manual-consolidate-pending-only.json`). No live cloud LLM is used.
The low-confidence temporary-plan and `unsupported-paris` cases expect
rejection at the 0.40 floor. This fixture path measures
parsing/binding/retention, not live prompt quality; live model quality is not
established by deterministic fixtures.

### Asserted baselines (provisional)

These thresholds live in the corpus `thresholds` object and are locked by
`tests/structured_memory_phase7.rs`. They describe this labeled fixture mix,
not a live-model SLA.

| Metric | How it is measured | Provisional bar |
|---|---|---|
| Candidate proposal precision | Supported retained pending proposals / all retained pending proposals | ≥ 0.80 |
| Unsupported / hallucinated candidate rate | Unsupported retained / all retained | ≤ 0.20 |
| Sensitive-content retention rate | Retained proposals from `secret` / `prompt_injection` cases | **0** |
| Useful recall at small top-k | Labeled relevant facts in FTS hits / relevant | 1.0 on this corpus |
| False / stale / conflicting recall | Deactivated or cross-owner facts in FTS hits | **0** |
| Prompt-size overhead | `assemble_memory_sections` with reserved 1500/1500 | combined body ≤ 3000 |
| Database growth / pruning | Oldest-first episode prune with `max_episodes_per_owner: 2` | unreferenced row dropped; pending-proposal refs kept |
| Chat / feature-combination regression | Existing `tests/structured_memory.rs` + shipped-gate scan | extend those tests; do not add live E2E |

The fixture mix **intentionally** includes one unsupported candidate
(`unsupported-paris`) so the scorer is not tautological. A later live local
model should aim for precision 1.0 and unsupported rate 0 on a reviewed set.

Rejected-class retention is **zero** for consolidator `sensitivity: reject`
and for core injection-scanner hits (`STRUCTURED_MEMORY_INJECTION`). There is
still no secret scanner: an operator can confirm+reason add non-injection
secret-shaped text. That is a human-governed write, not a rejected class.

### Documented-only (do not invent numbers)

| Metric | Measurement method |
|---|---|
| Reviewer accept / reject rate | Count apply vs reject on `POST /api/structured-memory/proposals/{id}` in an owned home over a review window. |
| Prompt latency p50 / p95 | In an owned temp `CGAGENTHARNESS_HOME`, time `POST /api/chat` and `POST /api/prompt/preview` for the same selected-fact set (n≥30). Host-dependent. |
| Live-model quality | Swap fixture-model JSON for a local OpenAI-compatible backend only after the fixture baseline is green. Do not send this corpus to a cloud provider. |

## Enable order

In a home whose gates are off, enable one gate at a time in this order, keeping
each later gate off until the earlier one is useful and the locked bars are
green:

1. Manual facts and governed proposals (`structured_memory.enabled`)
2. Optional episode capture (`episode_capture`)
3. Explicit recall (`explicit_recall`)
4. Optional FTS5 recall (`retrieval`; `auto_retrieval` stays last among read paths)
5. Manual consolidation (`consolidation`)
6. Optional automatic pending-proposal generation (`auto_consolidation`, requires consolidation)

Completion suggestions (`auto_suggest_chat` / `auto_suggest_coding`) are
enabled separately, beside that list. They still write pending proposals only.

Suggested fixture bars before considering a gate: sensitive retention 0,
stale/cross-owner recall 0, prompt body ≤ 3000, no silent fact apply, and
precision / unsupported rate at or better than the corpus thresholds. Human
review rate and latency remain operator-owned.

## Rollback

Disable the gate in `config.yaml` (literal `false`) and/or the matching
`/memory … off` overlay. That stops new reads, writes, FTS inject, consolidator
calls, and the idle worker. It does **not** delete `memory/structured.sqlite3`.
Existing rows stay listable/exportable/purgeable while the store remains open
(`structured_memory.enabled` still true). Closing the store gate refuses new
structured-memory mutations and does not open/create the database on the next
start; leftover files remain until the operator exports or
`POST /api/structured-memory/purge` with confirm+reason.

## Verify

```text
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --test structured_memory -- --nocapture
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --lib structured_memory -- --nocapture
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --test structured_memory_phase7 -- --nocapture
```
