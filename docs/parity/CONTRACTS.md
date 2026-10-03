# Non-RAG parity contracts

The fixed 48 actions in [actions.json](actions.json) record source pins and
implementation/verification states. These contracts define intended adaptations,
not implementation claims. Ledger validation covers its pinned baseline, not
current releases. Current memory controls: [MEMORY_GUIDE.md](../MEMORY_GUIDE.md).

Memory facts are explicitly reviewed operator data, separate from short notes.
Harness memory implements account-private facts, governed proposals, bounded
episodes, selected-fact recall, facts-only FTS5 retrieval and consolidation behind `structured_memory.enabled`,
`episode_capture`, `explicit_recall`, `retrieval`, `auto_retrieval`,
`consolidation`, and `auto_consolidation`
(literal booleans, true for fresh homes; explicit off choices survive).
Proposals bind an action, payload and expected fact version; application needs
an operator reason, confirmation, and scan. Episode v1 stores opaque session/turn refs and privacy-filtered metadata,
excluding raw queries, full answers and query-content hashes. Post-success
staging is non-fatal.
Pinned `/memory` notes are unchanged. FTS search is not prompt injection:
hits require `/memory retrieve` / the per-request `retrieve` flag, or the
separately gated `auto_retrieval` silent path, plus assembly-time
owner/active/revision recheck. Episode FTS, embeddings, retrieval fusion,
and episode prompt injection are not shipped; those status flags remain
false. Manual consolidation writes pending proposals only. The harness-only default-on idle auto-consolidator reuses that runner and
never applies facts. The 3000-character budget reserves 1500/1500 when notes and facts compete.

Sync operates on approved non-RAG roots and one approved remote. It excludes
credentials, soul and Git internals. Active managed coding workspaces cannot be
mutated concurrently by sync. Pull/copy is the default; bisync needs separate
intent and deletion fuses. Transport success never triggers indexing.

Telegram uses allowlisted private-chat identity and a non-RAG local conversation
adapter. A reusable confirmation binds principal, destination, exact payload,
expiry and policy state, and can be claimed once. Media stays untrusted and
staged until an approved filesystem save. Cursor persistence does not promise
exactly-once external sends; ambiguous outcomes require reconciliation.

OpenTweet creates a local topic-based preview first. Remote draft creation,
scheduling and posting are external writes with distinct explicit intent.
Generated text cannot grant that intent. A timeout after transmission is an
unknown outcome, not permission to retry a non-idempotent create.

Conversational cloud routing is independent of retrieval. Default generation
stays local. Selection, enablement, credentials, destination, current policy
and per-call consent are independent gates; a pre-action hook may only deny.
Local inference mode does not authorize connector egress. Passive status must
never probe optional remote providers.

CEL and sequence analysis are bounded optional observation of redacted events.
Neither replaces authoritative audit nor changes policy verdicts. Retrieval-only
rules are excluded; coding/connector rules beyond upstream are labeled additions.

Desktop owns the backend, checks exact model inventories, loads OS-stored keys
and recovers bounded jobs. Preserve these paths.
Exports require a narrow operator-selected save workflow; the remote console
must not gain generic native filesystem capabilities or unrestricted downloads.

The credential-free local default remains. Optional identity is a distinct
role-bearing principal; an API key is not connector scope or action consent.
New filesystem roots or network deployment require explicit configuration,
enforcement and tests while preserving default home/clone and loopback boundaries.

At the ledger's CyClaw pin, `memory/consolidation.py` is a no-op stub.
Harness consolidation only creates pending proposals. Retired DeepAgents
graph/optimizer work, RAG ingestion, upstream retrieval-only MCP, retrieval
fusion and RAG grounding remain excluded. The harness's separate
[read-only MCP memory gateway](../MCP_SERVER.md) does not change that parity
scope. Catalog entries alone never prove callable capabilities.
