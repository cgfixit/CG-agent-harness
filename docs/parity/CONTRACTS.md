# Non-RAG parity contracts

The fixed 48 actions are tracked in [actions.json](actions.json). This document
defines intended adaptations; it does not claim they are implemented. Current
source pins and implementation/verification states belong to the ledger.

Memory facts are explicitly reviewed operator data, separate from short notes.
Proposals bind an action, payload and expected fact version; application needs
an operator reason and scan. Non-RAG episodes describe bounded chat/coding
outcomes, including failure/cancellation, without retrieval scores. Raw query
storage stays off. Optional recording must not turn successful chat into failure.
There is no automatic FTS/vector injection; selected facts enter context only
under an explicit operator action and the existing prompt budget.

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

The current desktop already owns the backend, checks exact model inventories,
loads private dotenv data and recovers bounded jobs. Preserve those paths.
Exports require a narrow operator-selected save workflow; the remote console
must not gain generic native filesystem capabilities or unrestricted downloads.

The credential-free local default remains. Optional identity is a distinct
role-bearing principal; an API key is not connector scope or action consent.
New filesystem roots and optional network deployment deliberately extend the
existing home/clone and loopback invariants only behind explicit configuration
and their own enforcement/tests. They must not weaken the default boundaries.

Automatic memory consolidation is a stub at the pinned source
(`memory/consolidation.py:8`); it is excluded. Retired DeepAgents graph/optimizer
work stays excluded (`CLAUDE.md` describes its retirement; builder/scaffold
presence does not make it an active parity requirement). RAG ingestion,
retrieval-only MCP, retrieval fusion and RAG grounding remain excluded. Catalog
entries alone are never callable capability evidence.
