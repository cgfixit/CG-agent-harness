# Structured memory contract

The privacy-first structured-memory contract: **not** the pinned-note
`/memory` feature and **not** the web-research corpus benchmark. How to use
memory: [MEMORY_GUIDE.md](MEMORY_GUIDE.md).

## Two memory systems

| Surface | Scope | Store | Who may write | Prompt inclusion |
|---|---|---|---|---|
| Pinned notes (`/memory`) | Shared home | `memory/notes.json` | Operator slash commands / `/api/memory/*` | `harness.json.memory_enabled` (`/memory on`) |
| Structured memory | Account-private | `memory/structured.sqlite3` | Human confirm+reason for facts; optional post-success episode staging; optional consolidation or completion suggestions into pending proposals | Facts enter `/prompt` only after an explicit pick (selected facts, `/memory retrieve`, or the per-request `retrieve` flag) **or** the separately gated `auto_retrieval` path, plus assembly recheck. Episodes are never injected. |

Structured memory does **not** migrate, reinterpret or weaken pinned notes, and
session deletion preserves them. Facts and proposals are independent of chat
history. Clearing chats deletes derived episodes only when the operator
confirms `delete_derived_episodes` on `POST /api/sessions/clear`; that cascade
still keeps facts, proposals and episodes referenced by pending proposals.

## What ships

1. Manual **facts** with stable IDs, content, category, digest, revision, active
   state and timestamps.
2. Governed `add`, `update` and `deactivate` **proposals**, applied or rejected
   by a human ([contract](#proposal-contract-persona-semantics-sqlite-store)).
   Optional `source_episode_ids` record provenance without changing facts.
3. Optional **episode capture** after a persisted exchange: bounded metadata
   and a filtered summary, without raw queries, full answers, hidden reasoning
   or tool secrets.
4. Manual semantic-summary attachment, which automatic consolidation requires.
   Clipped assistant output is not a semantic summary.
5. TTL, per-owner row and byte quotas, oldest-first pruning, owner operations
   and bounded escaped HTML export.
6. **Explicit recall**, **facts-only FTS5 retrieval** and **manual or automatic
   consolidation** into pending proposals, each specified below.
7. A synthetic [evaluation corpus](#evaluation-corpus) whose fixture metrics
   are provisional.

## What does not ship

Embeddings, vector databases, RAG fusion, harness-writable RAG ingestion,
episode FTS, episode prompt injection, silent or autonomous canonical fact
apply, or any new cloud egress. `/memory save <text> :: <reason>` is an
explicit human fact write. Status flags for retrieval fusion and RAG remain
**false** even when `retrieval` or `consolidation` is true. Episode
availability is true only when `episode_capture` is on and the store is open.

## Gates

Every gate is evaluated with `flag_is_true`: literal YAML `true` only. Quoted
`"true"`, missing keys and invalid values stay **off**. All nine ship **true** in
fresh configuration; existing config files keep their own values. Config edits
need a restart.

A sub-gate is available only when the store is open **and** its administrator
override, if set, or otherwise its config value is true. Overrides are
administrator-only when accounts are enabled, persist in
`<home>/memory/structured_gates.json` and take effect immediately: explicit
`off` overrides config `true`, and explicit `on` enables a config-false gate.
Overrides **cannot open the store**.

| Gate (`structured_memory.*`) | Administrator override | Requires | Off means |
|---|---|---|---|
| `enabled` | None; config + restart only | — | Startup does not create or open the database. |
| `episode_capture` | `/memory capture on\|off` | store | No episode writes even if the store is open. |
| `explicit_recall` | `/memory recall on\|off` | store | No selected facts are injected even if IDs are selected. |
| `retrieval` | `/memory retrieval on\|off` | store | No FTS search and no force-include. Independent of `explicit_recall`. |
| `auto_retrieval` | `/memory auto-retrieve on\|off` | `retrieval` | Chat never searches without an explicit pick. On, chat may FTS the user message and inject rechecked top-k without a per-request flag: the Advisor-sensitive silent path. |
| `consolidation` | `/memory consolidation on\|off` | store | No selected-episode summarizer call (`/memory consolidate <id...>`). The completion-suggestion path may still create its own run records. |
| `auto_consolidation` | `/memory auto-consolidate on\|off` | `consolidation` (AND) | No idle worker starts. Eligibility and bounds: [Consolidation](#consolidation). |
| `auto_suggest_chat` | `/memory auto-suggest-chat on\|off` | store + `episode_capture` | Completed chat turns are not queued for suggestions. |
| `auto_suggest_coding` | `/memory auto-suggest-coding on\|off` | store + `episode_capture` | Accepted coding runs are not queued for suggestions. |

Gates not linked in the Requires column are independent.
`harness.json.memory_enabled` (`/memory on`) stays pinned-note prompt inclusion
only and opens none of these gates.

## Prompt budget

The combined pinned-note + selected-fact body is capped at 3000 characters.
When both are present the reserved split is `pinned_prompt_chars` /
`selected_fact_prompt_chars` (default 1500/1500); unused reserved capacity is
not transferred. A source present alone may use the full 3000; headings sit outside the cap.

## Write-time injection checks and their limits

Fact and proposal writes use `Scanner::core()` through
`StructuredMemoryStore::clean_text`. Content and category are checked on
direct fact creation, add/update proposal creation (including consolidation and
completion suggestions), and again when a pending proposal is applied. A
core-pattern hit such as `ignore previous instructions` returns
`STRUCTURED_MEMORY_INJECTION` (HTTP 400) before the write. A refused apply
leaves the proposal pending and facts unchanged; the operator can still
reject it.

Only the fixed core patterns apply (not `policy.prompt_filter.banned_patterns`,
the coding scan or `scan_normalized`): this is neither a secret scanner nor a
general injection detector. Assembled recall is not scanned, and clean
fragments can combine into unsafe text, so treat facts as untrusted context.

## Completion suggestions

`auto_suggest_chat` and `auto_suggest_coding` are independent gates. Both
require an open store and episode capture; neither requires nor opens
consolidation, recall or retrieval.
`suggestion_mode` selects `summaries`, `insights` or `both` (default); invalid
values disable suggestions. The resolved local model and mode must also be
listed in `suggestion_supported`
([D7](MEMORY_GUIDE.md#automatic-suggestions-and-review)); otherwise the
receipt and `/api/structured-memory` report `unsupported_model_mode` and no
model is called. A missing key supports nothing, so existing homes add it:
`suggestion_supported: [{ model: "<tag>", modes: [summaries, insights, both] }]`.
Only Ollama aliases bare names to `:latest`; other local backends require exact IDs.
Fresh homes seed `qwen3.8:27b-mlx-cg`, the shipped chat default. `"*"` matches
any model and opts out of the corpus rule; a malformed list refuses
(`invalid_supported_models`).

`completion-suggestions-v1` reads bounded, redacted current completion evidence
in RAM for the initiating account. It neither scans shared archives nor writes
human semantic summaries. Synchronous and detached coding share the shim hook
(I6): `pending_decision` qualifies; exhausted runs do not. Coding input is the
submitted instruction and projected child result (repository, status,
run id, changed files, push/PR fields and commit message), not stdout/stderr,
file bodies or tool logs, so it cannot prove a coding lesson or passing
tests. Local `/loop` queues only replies carrying the standalone
`GOAL_DONE` marker (Markdown decoration allowed), paired with the stored human
goal; the marker is unverified model advice, not evidence of completion.

Strict parsing, source membership, sensitivity, confidence and fact bindings
govern idempotent `session_summary` or `insight` proposals. Identical normalized
candidates count as rejected duplicates within an output; different source
bindings stay distinct. Generation uses temperature 0 and 1024 output tokens,
without tools, web, recalled facts or cloud egress, and requests
`response_format: {"type":"json_object"}` locally, as consolidation does.
Backend refusals and malformed replies remain visible failures; schema and
source validation remain authoritative.

Suggestions record the local model of their turn or completion, consolidation
uses the current selection, and a cloud chat selection falls back to the
configured local backend: memory never calls a cloud provider.

The shipped YAML lists each bound with its clamp. `suggestion_max_input_chars`
(8000) is the total across input and output, split equally and clipped on
character boundaries. `suggestion_max_queue` (8) counts waiting jobs globally,
plus at most one active generation; waiting input expires after
`suggestion_queue_ttl_secs` (300), while active inference uses the chat-client
timeout. `status_poll_ms` (30000) is halved while generating.
`min_consolidation_confidence` and `max_consolidation_candidates` are shared
with consolidation, and `max_proposals_per_owner` (50; 32 if absent) is the
pending-proposal capacity.

Accepted inputs receive durable metadata-only `idle` receipts, then become
`running`. Expiry, disabled gates, source clear, cancellation and inference
errors keep distinct reasons. Reopen marks queued/preempted receipts
`failed/input_lost_on_restart` and interrupted generation `failed/interrupted`;
inputs are never replayed, and pending proposals persist.
Completion replies and detached jobs retain episode/run IDs or refusal reasons.
RAM status keeps latest attempts for `suggestion_max_queue` owners; durable
runs remain separately available.

Empty candidates and filtered results are valid; failures do not fail completed
work. Chat preempts active suggestion inference, records `cancelled/preempted`
and requeues it. Owner cancellation discards queued input at once and needs an
open store and matching owner, even when manual consolidation is off.
Redaction and scanning are bounded safeguards; human review remains mandatory.

## Ownership

Rows are keyed by `owner_id`: the authenticated account `user_id`
(32 hex digits, generated at runtime), the `local` namespace when accounts
are disabled, or a labeled `user_*` fixture id (tests must not embed 32-hex
token-shaped literals). A second owner cannot list, get, export or mutate a
known id, and admin capability grants no inspection of another owner's
structured memory.

## Episode contract

- Chat staging happens only **after** the exchange is persisted. Coding
  suggestion staging follows a successful shim result and keeps only
  metadata; detached jobs must also have persisted the finished result.
- Storage, busy and write errors become truthful `episode.health` on the
  successful chat response; they do not fail the chat.
- `session_ref` / `turn_ref` are random opaque ids, not the transcript or a
  content hash of the query.
- Auto-staged `privacy_summary` is local metadata (outcome, model, char
  counts, sensitivity), not a semantic summary.
- `/memory remember <sentence> :: <reason>` attaches your trimmed summary to
  this owner's latest completed episode (newest `created_ts`, then `public_id`
  ascending), within `max_episode_summary_chars` (default 500) and the
  NUL/injection checks. A nonblank reason is required; the command is the
  confirmation. It never creates the store, writes facts or starts
  consolidation, and works with capture off if a completed episode exists.
- `GET /api/structured-memory/episodes?latest_completed=true` selects that one
  episode in SQL, before the ordinary list's 256-row cap; no match returns an
  empty list.
- Retention: oldest `created_ts`, then `public_id`. Episodes referenced by
  pending proposals are not pruned.

## Proposal contract (persona semantics, SQLite store)

Reuse persona review rules, not `soul-history/*.json`:

- `#[serde(deny_unknown_fields)]` on every mutation body.
- Propose does **not** require `confirm` (suggest).
- Decide/direct add/deactivate **do** require `confirm` + nonempty `reason`.
- Apply uses `BEGIN IMMEDIATE`, re-validates content, binds owner + proposal
  revision + expected fact revision/digest, updates the FTS row, applies once,
  and writes non-content audit metadata.

The **Memory** tab (`/memory proposals`) lists pending proposals via
`?status=pending`, filtered before its 128-row cap, then saved facts. **Save memory**
and **Dismiss** send the proposal's revision to
`POST /api/structured-memory/proposals/{id}`, the only decide endpoint.
**Use in this session** sets [session selection](#explicit-recall).

## Explicit recall

Selection binds stable `public_id` values plus `expected_revision` on a
session (`POST /api/sessions/{id}/structured-facts`) or a single
`/api/chat` / `/api/prompt/preview` request. At prompt assembly the store is
re-read with the current owner filter. Missing, inactive, stale-revision and
cross-owner facts are dropped; only survivors are formatted as untrusted
background context. On a local chat turn they are fenced ahead of the new
message, counted with it and never stored in history, while pinned notes stay
in the system prompt; `/loop` turns keep both there. Preview describes local
chat. Cloud chat omits memory: `structured_facts.injected` stays empty and
selected facts are dropped as `cloud_chat`.

## FTS retrieval

- **Facts only.** Episode summaries are not indexed.
- **Contentless / field-prefixed FTS5** (`title:` / `value:` / `tags:`). MATCH
  input is tokenized, bounded and quoted, so punctuation, operators, quotes and
  column selectors in the user string cannot become FTS syntax.
- **Search ≠ inject.** `GET /api/structured-memory/search` returns a small
  top-k with stable ids, revisions, provenance (`fts5`) and lexical score.
  Hits are not written onto the session.
- **Force-include** (`retrieve: true` / `/memory retrieve <query>`) is the
  explicit pick for that request: bounded FTS, recheck, inject under the
  prompt budget. The flag does not persist.
- The index is co-updated with fact add/update/deactivate/purge and backfilled
  from active facts in one Immediate transaction when enabled. A busy or
  corrupt index is fail-soft for chat.
- Audit records counts and error class, never raw query or fact text.

## Consolidation

- **Selected episodes only.** The operator supplies episode public IDs.
  Manual runs accept summary-less episodes, a worse path because metadata
  cannot support durable facts; the v2 prompt asks for no candidates from
  metadata alone. Auto-batching requires a nonblank semantic summary and an
  unexpired `none`/`pending` row; running owners and episodes from
  failed/cancelled current-version runs are skipped until a manual retry,
  including after restart. Completion-suggestion failures do not block
  human-summary consolidation.
- **Suggestion quality.** `consolidator-v2` asks for durable preferences,
  identity, corrections or standing constraints supported by semantic summaries.
  Temporary plans, jokes, tool/status dumps, errands and unsupported guesses
  are non-candidates. Negation and attribution must survive. Secrets/injection-like text remain reject sensitivity.
  `structured_memory.min_consolidation_confidence` defaults to **0.40** and clamps
  to [0, 1]; a quoted, empty or non-finite `config.yaml` value refuses at load
  (`FLOAT_KEYS`). A supplied confidence below the floor is rejected; a missing
  one is accepted. Confidence never bypasses human review.
- **Local model only.** The generation gate is claimed, with no tools or web.
  Chat wins the gate, and both report busy truthfully.
- **No recalled-memory feedback.** Current facts are loaded only after
  extraction, to bind update/deactivate revision+digest or convert an exact
  content match from add → update.
- **Pending proposals only.** A successful run, manual or automatic, never
  creates, updates or deletes canonical facts. With `auto_consolidation` off,
  no worker starts.
- **Bounded.** `max_consolidation_episodes` / `max_consolidation_candidates`
  (default 8/8). Invalid schema, cancellation, model failure and storage
  failure become run `failed`/`cancelled` with a non-content `error_class`.
- **Idempotent.** The same owner + sorted episode set + `consolidator-v2` key
  returns the completed run without a second proposal batch; a completed v1
  key does not suppress a v2 run. Interrupted `running` rows become
  `failed`/`interrupted` on reopen and may be retried manually on the same key.

## Threat model (at rest)

The database is an owner-private (0600, NOFOLLOW-checked) file under the home:
**OS access control**, not encryption, so a process running as the owner can
read it.

Audit events record owner-safe ids, revisions, counts and status. They must
not record fact text, proposal payloads, episode summaries or reasons that
embed content.

Export is local, HTML-escaped, owner-filtered and marked `Truncated: true`
when `max_export_bytes` or a row limit (128 proposals, 256 episodes) clips it.
It is not a prompt surface.

## HTTP surfaces

All use the guard chain and return `Cache-Control: no-store`
when they carry memory content. Paths are under `/api/structured-memory`
unless shown in full. GET reads need no confirmation: the status root,
`/search` (FTS candidates; retrieval gate required), `/facts` (active facts;
optional `q`/`category`/`limit` literal substring search, not FTS),
`/facts/{id}` (including inactive), `/export` (escaped HTML), and
`/proposals`, `/episodes` and `/consolidation` (this owner's run metadata),
each also by `/{id}`.

| POST path | Confirm? |
|---|---|
| `/gates` | administrator-only gate override; body `{"gate": "<config name>", "enabled": bool}`, naming any sub-gate in [Gates](#gates); not a fact mutation |
| `/api/sessions/{session_id}/structured-facts` (also GET) | selection = explicit include; not a fact mutation |
| `/facts` | required (direct human add) |
| `/facts/{id}/deactivate` | required |
| `/proposals` | propose = suggest |
| `/proposals/{id}` | decide requires confirm |
| `/episodes/{id}/summary` | required (manual semantic summary) |
| `/episodes/{id}/delete` | required |
| `/episodes/purge` | required (expired only) |
| `/purge` | required (owner facts+episodes+proposals+run metadata) |
| `/consolidation` | start = suggest; selected episode IDs |
| `/consolidation/{id}/cancel` | stop queued/running work |

`/api/chat` and `/api/prompt/preview` accept `retrieve` / `retrieve_query` for per-request
force-include.

## Evaluation corpus

The corpus is local and synthetic:

```text
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --test structured_memory_phase7 -- --nocapture
cargo run --locked --example structured_memory_eval -- /tmp/phase7-report.json
```

Both use `tests/fixtures/structured_memory/phase7/corpus.json` and a
fixture-model consolidator; they measure parsing, binding and retention, not
live model quality.

### Asserted baselines (provisional)

These thresholds live in the corpus `thresholds` object, locked by
`tests/structured_memory_phase7.rs`. They describe this fixture mix, not a
live-model SLA.

| Metric | How it is measured | Provisional bar |
|---|---|---|
| Candidate proposal precision | Supported retained pending proposals / all retained pending proposals | ≥ 0.80 |
| Unsupported / hallucinated candidate rate | Unsupported retained / all retained | ≤ 0.20 |
| Sensitive-content retention rate | Retained proposals from `secret` / `prompt_injection` cases | **0** |
| Useful recall at small top-k | Labeled relevant facts in FTS hits / relevant | 1.0 on this corpus |
| False / stale / conflicting recall | Deactivated or cross-owner facts in FTS hits | **0** |
| Prompt-size overhead | `assemble_memory_sections` with reserved 1500/1500 | combined body ≤ 3000 |
| Database growth / pruning | Oldest-first episode prune with `max_episodes_per_owner: 2` | unreferenced row dropped; pending-proposal refs kept |
| Chat / feature-combination regression | `tests/structured_memory.rs` + shipped-gate scan | extend those tests; no live E2E |

Rejected-class retention is zero for `sensitivity: reject` and core injection
hits; a confirmed human write can still contain secret-shaped text.

### Documented-only (do not invent numbers)

Reviewer accept/reject rate, prompt latency (p50/p95 of `/api/chat` and
`/api/prompt/preview` with the same selected facts, n≥30) and live-model
quality (a local backend, after the fixture baseline is green) are operator
measurements, not asserted numbers.

## Enable order

In a home whose gates are off, enable one gate at a time in this order, each
only once the earlier one is useful and the
[asserted baselines](#asserted-baselines-provisional) hold:

1. Manual facts and governed proposals (`structured_memory.enabled`)
2. Episode capture (`episode_capture`)
3. Explicit recall (`explicit_recall`)
4. FTS5 recall (`retrieval`; `auto_retrieval` stays last among read paths)
5. Manual consolidation (`consolidation`)
6. Automatic pending-proposal generation (`auto_consolidation`)

Completion suggestions (`auto_suggest_chat` / `auto_suggest_coding`) are
enabled separately and still write pending proposals only.

## Rollback

Disable the gate in `config.yaml` (literal `false`) and/or the matching
`/memory … off` overlay. That stops new reads, writes, FTS inject, consolidator
calls and the idle worker. It does **not** delete `memory/structured.sqlite3`:
rows stay listable, exportable and purgeable while the store is open, and
`enabled: false` only stops opening the database on the next start.

## Verify

```text
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --test structured_memory -- --nocapture
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --lib structured_memory -- --nocapture
```

For a packaged backend, `scripts/test-desktop-backend.py
DesktopBoundary.test_completion_memory_uses_selected_model_and_requires_review`
(with `CGAH_TEST_BINARY` set) checks selected-model proposals, review refusals,
Apply/Dismiss and restart persistence against fixtures.
