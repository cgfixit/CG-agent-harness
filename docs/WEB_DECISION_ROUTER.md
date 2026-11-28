# Web decision router

## Routing design

The existing local chat tool loop routes evidence for `/goal` and `/loop`.
Use `/goal --web off <goal>` to enforce no web, `fetch` for exact reads and
`all` for search plus reads. The default is `web.goal_web_default`.
A turn with uploaded-notes evidence withholds search unless its current
message asks for outside, web or internet sources. A loop
inherits its goal's mode. The model decides whether evidence is necessary and
whether it answers the question. The server decides authority, provenance,
time and resource budgets. A prompt asking for no web is advisory. Standalone
`/web research` is a separate bounded discovery-and-synthesis job.

1. Use supplied or local context when sufficient. For external facts, search
   only when discovery is needed. Treat results as ranked leads, never as
   fetched source text.
2. Select relevant URLs and give `web_fetch` a focused query. Seek primary
   sources and qualifying or conflicting evidence. A small set of bounded
   passages can support a specific claim. Report gaps when coverage is weak.
3. Reuse fresh verified evidence in the current run. Refresh when the task
   needs newer material. A fetch receipt records retrieval time. Publication
   dates remain source claims.

## Implemented behavior and measured limits

### Provider selection

`web.search.provider: auto` chooses active Tavily, then SerpAPI, then public
Google. Explicit values are `tavily`, `serpapi` and `google-public`.
When enabled, fallback follows Tavily → keyed SerpAPI → public Google.
Both fallback switches ship true. Hops share one search slot and
`web.request_seconds` deadline. Cooling hops give up their time share.
The public hop gets its outstanding pacing wait plus one second when possible.
Transport failure, timeout, non-200, paid quota or unavailable units can move
to a fallback. Clean zero results, valid rows and missing or rejected keys
end the search. A cooling keyed hop may be skipped.
Public Google requires its generated search URL to be permitted and
applies a three-second pacing floor. Challenge/block cooldown ships
600 seconds. Paid quota cooldown defaults to the next clock hour.
`WEB_SEARCH_COOLDOWN` is a no-send outcome. Fallback audit reports the
hop and counts SerpAPI fallback bursts.

Paid hops reserve UTC-day units in `tools/web_units.sqlite` before send.
The restart-only ceilings count all requests on this home across turns,
jobs, CLI and restarts. Shipped ceilings are 30 Tavily credits, 8 SerpAPI
searches and 30 Firecrawl credits per day. An unavailable ledger refuses
paid dispatch. A request that was never sent returns its units. A sent
request retains the reservation after timeout or crash. Provider dashboards
may count other clients separately.

Listings carry rank, title, URL, snippet and bounded optional metadata.
SerpAPI spelling and related queries grant no reads. Provider answer boxes,
AI overviews, knowledge graphs and Tavily answers are excluded. Recognized
`site:` and `-site:` operators constrain results. A provider key anywhere in
a listing refuses it. Separate availability of a paid key from permission
to read listed content. Search sends the query and count. Firecrawl sends
a qualifying URL only. Neither receives the conversation as a search input.

### Read authority and fallback

An administrator's own listings can grant exact result URLs for the turn or
`/loop` run under `web.search.result_reads` (on if missing, off only via literal false).
`max_urls` defaults to 20. Optional
`arxiv_pdf` grants the exact PDF corresponding to a listed arXiv abstract
within the same URL cap, with a separate literal-true gate. These are
ephemeral grants, subject to policy, DNS,
redirect and account rechecks. Operators use URL policy only. A model-composed
URL allowed solely by a wildcard requires live administrator approval under
`web.fetch_provenance`. See [chat web tools](SECURE_RESEARCH.md#chat-web-tools).

Local reads use pinned egress, bundled roots and handle text-bearing PDFs in
`pdf-text-worker`. Opt-in Firecrawl sees only an exact eligible URL after
403, 429, 503, a qualifying script shell, oversize body or a PDF classified
textless, over limit or malformed. Busy, encrypted, timed-out or failed PDF
workers stay local. An abort is over limit only with a worker heap-cap marker.
For a script shell, the local result must be scripted HTML with extracted
text under 200 characters and under 5% of the markup. Firecrawl sends one
Markdown scrape with `maxAge: 0`, `storeInCache: false` and
a 10-page PDF cap. Each fallback reserves 10 units because Firecrawl fetches
the URL itself, so with Firecrawl enabled, startup refuses a daily ceiling
under 10. Final URL must match. Failed fallback keeps the local result
and adds its fallback code in `details.fallback`. Successful excerpts
carry `via: firecrawl` and contribute no links. See [fetch and research](SECURE_RESEARCH.md#fetch-discovery-and-research).

### Passage selection and loop carryover

[Evidence selection](../src/server/chat_web/evidence.rs) uses BM25 with a
focused query. A single page yields up to three passages. Multi-page ranking
keeps up to two per page. Deliver receipt IDs and offsets only for complete
passages. Omitted `refresh` can reuse fresh active-goal evidence. Outside
that case reads default fresh. `true` refreshes at most once per page per turn
and `false` permits policy-checked cache reuse.

A useful changed batch generates a digest of up to 1,024 tokens before the
next decision. Invalid digest JSON leaves valid passages and a visible
failure. Validation binds quotes to supplied passages; quotes above 160
characters drop their claim. Exact quotes still need semantic review.
The run retains nine URL/hash references (three passage IDs each), sixteen
prior queries and one validated packet. Restore checks current permissions,
age and hashes and includes citation-to-URL mappings. A new run starts empty.
The saved session's `web_sources` records bounded prior read metadata.
`web.carry_forward_sources` can list earlier permitted URLs and queries
in a new turn without their page text. This is distinct from a run's
hash-bound evidence restored through compaction.

The server validates IDs, offered tools, argument structure and call count
for the entire batch before any send. An individual field bound fails that
call and leaves valid siblings eligible. If every call fails, the tool loop
ends and the model gets a tools-off answer attempt. An empty answer
gets one tools-off retry. The server then reports the outcome itself.
`WEB_FINAL_EMPTY` can count toward loop failure limits when a read
succeeded. Per-call failures stay visible. A counted loop uses its last turn for a complete answer with gaps.
`GOAL_DONE` is a model marker, with no independent proof.

### Budgets and verification

At the shipped 32,768-token profile, `web.total_tokens` is 28,000,
`web.evidence_tokens` is 6,000 and `web.chat_tool_calls` is 30.
The tool loop reserves room for definitions, a reply, restored evidence and
minimal results before reads. `WEB_TOKEN_BUDGET` refuses a batch that cannot fit.
`WEB_TOOLS_WITHHELD` reserves room to answer after evidence.
Tool hops can answer directly. The goal digest is separately bounded. First-call
usage calibrates the estimate, restored evidence included. Auto-tuning
tightens below 32,768. `web.run_limits.searches` and `.reads` (shipped 0,
off) cap one run's calls, counted before each runs (a server `/loop` spans
turns): `WEB_RUN_BUDGET`. Verify the loaded window in [Models](MODELS.md).
Standalone research discovery (shipped 1,200 seconds) and synthesis
(shipped 1,800 seconds) have separate deadlines; discovery also bounds
pages, bytes, per-origin reads and time. Account, URL policy and
web rechecks after waits limit dispatch and delivery. See
[config](../assets/config.default.yaml) for active defaults and ranges.

Fixtures exercise provider fallback, malformed batches, policy revocation,
late passages, refresh, digest rejection and two-turn receipt reuse. A
historical 32K MLX run retrieved a late RFC section with one read across two
turns. Another run quoted IANA correctly but overstated what the text
established. Use separate receipts for authenticated-provider availability,
credit usage and semantic accuracy. See [invariants](../INVARIANTS.md#public-web-evidence-requires-current-content-permission).
