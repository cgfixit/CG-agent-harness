# Web decision router: research and implementation

Research baseline and implementation design, 2026-11-07, for goal and loop
turns that read the web. The one-page summary and flow diagram are in the
[README](../README.md#how-the-research-loop-decides); the operator contract is
[chat web tools](SECURE_RESEARCH.md#chat-web-tools); remaining phases are
tracked in [#772](https://github.com/cgfixit/CG-agent-harness/issues/772).

Converted from the operator-requested research TXT. The observations below describe
the reviewed baseline (before #786 and #788 merged); what shipped and what was
measured is in [Implemented behavior and measured limits](#implemented-behavior-and-measured-limits).

## Outcome and accepted scope

The implementation baseline is `8e14eb2be46419d97a77e38d5ad5fd780ba0d226`,
refreshed from `origin/main` after the initial research at `8ecd9c8`.
The operator authorized focused draft PRs and explicitly chose to preserve
current provider opt-ins and defaults.
Credit purchasing, plan selection and free-tier accounting remain elsewhere;
the router consumes their availability/refusal decisions and retains existing
hard caps. It must never change keys, enable a provider or buy capacity.

Design target: local long-form /goal and /loop research on M5 Pro, 48 GB,
qwen3.8:27b-mlx-cg, 32,768-token context. This is the server chat loop, not the
separate src/agentic coding child. I6 remains intact. No new shell, filesystem,
MCP, cloud or repository permissions are implied by routing between evidence.

## Observations that matter

1. Provider transport failover already exists. web_provider.rs::plan_hops and
   falls_through feed web_google.rs::google_search_at. One broker decision,
   search gate and deadline cover a search chain. There is no general semantic
   router over local knowledge, listings, pages and accumulated goal evidence.

2. Exact current provider eligibility, for web enabled and goal mode all:

   provider=auto, active SerpAPI key:
     SerpAPI -> public Google if fallback_to_public_google=true.
     Tavily is ignored even if its key is saved.
   provider=auto, no active SerpAPI key:
     Public Google only, even if Tavily has a key.
   provider=google-public:
     Public Google only; other keys do not alter it.
   provider=serpapi:
     SerpAPI -> optional public Google. Missing forced key is an error.
   provider=tavily:
     Tavily -> SerpAPI only if fallback_to_serpapi=true and its key is active
     -> public Google only if fallback_to_public_google=true.
     Missing forced Tavily key is an error.

   The two fallback flags independently remove their hops. Both false means
   only the selected first provider. Missing flags in older configs are false
   because flag_is_true requires literal YAML true. The shipped YAML sets
   provider=auto, fallback_to_serpapi=false, fallback_to_public_google=true.
   Invalid provider names/types fail configuration loading.

3. Global web off or goal mode off closes web. Goal mode fetch permits exact
   reads but no discovery search. Goal mode all enables both tools, not URL
   access. Keyed listings require account/web authority; public Google also
   requires current URL policy. A fresh home only seeds Google's URL rule.
   Search results never grant permission to read their destinations.

4. Current recoverable search codes are WEB_SEARCH_PROVIDER, WEB_TIMEOUT,
   WEB_SEARCH_COOLDOWN and WEB_SEARCH_QUOTA for keyed hops. Missing/rejected
   credentials, refused queries and clean zero-result answers do not fall
   through. WEB_SEARCH_RESPONSE is also not currently in the fallback set;
   that code covers both malformed data and reflected credentials, so adding
   it wholesale would erase an important distinction.

5. web.request_seconds is 8 in shipped config. Tavily reserves two seconds
   for later work, but SerpAPI gets the remaining deadline and can leave no
   useful time for public Google. Google's public pacing has a three-second
   floor. The focused timeout fixture reproduced this: SerpAPI consumed eight seconds and
   public Google did not answer. This fixture is not a live-provider outage.

6. Listings currently keep only rank, title, URL and snippet. Rank is the
   filtered/deduplicated output position, not necessarily the provider's
   original rank. Tavily scores and SerpAPI highlighted words are discarded.
   Titles are capped at 300 characters and snippets at 1,000.

7. chat_web.rs fetches exact URLs through WebTool::fetch, then gives the model
   the text prefix, clipped first by evidence_tokens and again by available
   context. web_index.rs already offers policy-filtered BM25 passage retrieval
   with source IDs, hashes, offsets, headings and fetch timestamps. Chat does
   not use that retrieval path for fetched evidence today.

8. Firecrawl is already an optional exact-page fallback. It runs only after a
   direct 403/429/503 or a qualifying near-empty script shell. It is not a
   search provider or crawler. It requires its own enable flag and key, current
   URL authority, and an unchanged final URL; failure preserves the original
   direct outcome. TLS, private-address, redirect and policy refusals are not
   reasons to send a URL to Firecrawl. It ships disabled.

9. Current Firecrawl requests set storeInCache=false but omit maxAge. Current
   Firecrawl docs describe maxAge as permitting cached responses up to two
   days old by default. Therefore storeInCache=false does not by itself prove
   freshness. This is a documented API/default mismatch to validate before
   claiming a time-sensitive page was freshly retrieved.

10. chat_web.rs::StickyRoute suppresses search after local notes are present
    unless the user asks for outside sources. It is a narrow substring rule,
    not a general sufficiency/freshness decision. routes/core.rs already
    retrieves permitted attachments, notes and selected/structured memory.
    /loop has no arbitrary local-file reading tool. Route only among data the
    current owner and surface are already allowed to use.

11. chat_web.rs::hop_kind intentionally assigns Synthesis to every loop hop,
    with a regression test enforcing it. The loop answer cap is the smaller of the configured 8,192 and
    active auto-tuning; the verified 32K Qwen profile caps it at 4,096.
    Ordinary tool hops use 512 tokens and temperature zero. Changing this requires an explicit planning-versus-answer
    split and an updated behavioral check; simply lowering the loop cap would
    truncate useful goal reasoning.

12. The session web ledger holds targets, counts, failures and truncation,
    not passages. carry_forward_sources ships zero, and enabling it carries
    only metadata. Search deduplication is currently local to one chat turn.
    /web research has checked Citation/Claim/Answer types that can be reused,
    but its runner performs permitted-site discovery and takes generation and
    search gates. Calling that runner from inside chat is not the integration:
    reuse its pure retrieval/validation helpers, without nested gate claims.

13. The Mac is currently verified as Mac17,9, Apple M5 Pro, 48 GB, 18 CPU cores.
    Both repository Qwen Modelfiles match: num_ctx 32768, temperature 0,
    top_p 0.9, top_k 20, min_p 0, presence_penalty 0, repeat_penalty 1,
    draft_num_predict 2. Request-time reasoning is none in shipped config.
    An authorized live probe verified Ollama 0.40.15, the MLX runner,
    model digest `b957244f74776492aa8c7ebac67328d26f1068410c719a08733981871cca3afa`,
    and a loaded context of 32,768. The cold readiness probe took 7.14 seconds,
    including 5.52 seconds loading. This tiny probe is not a routing benchmark.
    Verification uses a disposable authenticated TLS home, leaving the user's
    normal home and shared Ollama process intact.

## Routing design

Keep execution deterministic and use the model only for semantic choices.
Reuse the two public model tools, web_search and web_fetch. Provider eligibility
is calculated by code; the model never selects credentials or arbitrary
provider endpoints. Small internal decision types are sufficient; no separate
agent, vector store, plugin protocol, new dependency or generic router framework.

### A. Decide whether outside evidence is needed.

Derive effective capabilities from account authority, web setting, goal
off/fetch/all, current URL rules, selected provider/fallback flags, tool
support, deadlines and remaining budgets. Preserve the local-notes guard.
Carry explicit outside-source intent from the trusted stored goal/user
instruction; never infer that authority from a web page or model reply.

Use existing local context first when the task is about supplied files,
personal/project facts or a stable transformation/computation. Use internal
model knowledge for stable explanations when no external evidence is needed.
It is not a source for claims about current releases, dates or external state.
Explicit search, verification, current facts, a supplied unread URL, a
material contradiction or an unresolved external fact are reasons to seek
web evidence when authorized. Mere model confidence is not a sufficient gate.

Internal outcomes: answer_local, reuse_evidence, search, read_exact, or
report_gap. These are states/actions, not additional model tools. Decode
existing validated tool calls into search/read actions; use the existing
model hop for selection rather than adding an unconditional classifier
call before every turn. A routing/planning hop must remain distinct from
the full-budget answer/continuation hop. Record a short decision code and
target evidence need, not private chain-of-thought.

### B. Search the configured chain only.

Start with one focused query and five results. Use the existing hop plan,
cooldowns, bounded clients, key-change checks and external quota decisions.
Preserve forced-provider missing/bad-key errors. A disabled/missing optional
fallback is unavailable, not implicitly enabled. No round-robin or parallel
requests to all providers and no speculative retry after a successful answer.

Divide the existing absolute deadline among remaining authorized hops,
reserve Google pacing plus useful request time, and return unused time to
later hops. If too little time remains, record exhausted_before_attempt.
Never restart the overall timeout for a fallback. A tiny operator-configured
budget cannot guarantee that every hop gets a useful attempt.

Preserve clean-empty semantics: a successful zero-result listing is not a
transport failure. The router may formulate one genuinely different query
for a stated gap; this is a new search, counted normally. Do not switch to
another provider merely because the model dislikes valid results.

Treat malformed provider response separately from credential reflection
before proposing broader failover. Fail closed on secrets, authority,
SSRF, policy, TLS and cancellation. Do not add blind retry loops or turn
clean denials into searches against another service.

### C. Select pages from bounded listings.

Extend the existing SearchResult only with optional fields used by triage:
provider rank, provider score, highlighted terms and source date when present.
Keep provider identity with each listing. Preserve absent fields as absent;
reject invalid numeric values and bound every string/list. Do not expose
provider answers or raw page content through the listing adapter.

Enforce explicit host/site constraints locally on canonical result URLs;
a provider echoing site: does not prove compliance. Preserve the actual
number of accepted results, including zero. Keep canonical URL semantics
rather than stripping arbitrary query parameters that may identify content.

Triage uses entity/topic match, original rank, title/snippet/highlights,
primary-source preference, recency when required, novelty and source diversity.
Provider scores are provider-specific hints, not cross-provider confidence
or factual correctness. Missing dates are unknown, not old or fresh.
Reuse the next bounded tool-selection hop for the whole candidate batch.

Initially select at most three useful pages; add pages only for a concrete
uncovered part of the goal. A listing/navigation request may end at listings,
clearly labeled. Quotes, precise factual synthesis, comparisons requiring
context and important current claims need page evidence. Sparse snippets are
uncertainty, not proof that the destination is irrelevant.

### D. Read and extract relevant evidence.

Try a currently permitted, sufficiently fresh cached page before a network
reread when the task allows cached evidence. An explicit request to refresh
must not be silently satisfied from cache. Reuse Page validation and receipt
identity. Cache-only retrieval must not call discover() or crawl.

Otherwise use the existing pinned exact fetch. If it qualifies for the
existing Firecrawl fallback and the provider is explicitly enabled, allow
that fallback only for a selected page with an unresolved evidence need.
Irrelevant but readable text is not a reason to pay for a second extraction.
If Firecrawl is disabled/unavailable or fails, try another permitted selected
source or return a coverage gap. Preserve both transport outcomes.

Route successfully fetched text through web_index::retrieve/ retrieve_many
on a blocking worker, using the active subquestion. Retain coherent passages,
headings and nearby qualifiers/table context. Start with two or three
passages per source; avoid a prefix-only context window. Relevance selection
saves model context, not the first HTTP download. Oversized/complex pages
remain explicit failures; do not automatically raise response caps or send
those failures to Firecrawl under today's contract.

For explicitly fresh Firecrawl reads, validate maxAge=0 or equivalent current
API behavior and keep storeInCache=false. Distinguish local fetch timestamp,
provider retrieval/cache age and publication date. A fresh local receipt
alone does not establish fresh origin content.

### E. Return an evidence packet to the goal loop.

Reuse Passage and the existing Answer/Citation validator for a bounded packet:
supported findings, conflicting findings, labeled inferences and remaining
questions. Server-side receipt data includes exact URL, reader/provider,
fetch/cache status, content hash, passage ID and offsets. Give Qwen short
S1/S2 identifiers mapped by the server to those receipts.

Validate quote membership and referenced passage IDs before acceptance.
Such validation proves provenance, not that a quote logically entails the
claim: evaluate semantic support separately. Do not claim a universal natural-
language hallucination detector. Read status and source badges derive from
receipts, never from the assistant saying it read a page.

Keep a small bounded active-run record of resolved questions, selected
receipt references, attempted queries and unresolved gaps. Scope it to owner,
session and goal; clear it on changed goal/cancel/end. Rehydrate permitted
passages from the existing cache after compaction, checking hashes, age and
authority again. Missing/evicted/revoked content becomes unavailable; a
summary cannot manufacture a receipt. Do not add page bodies to web_ledger,
silently enable carry_forward_sources, persist a second corpus, or promote
web inferences into user memory facts.

Existing historical chat text cannot be erased by permission revocation;
prevent fresh injection/delivery rather than promising retroactive removal.
Keep unsuccessful/blocked research distinct from satisfied goal criteria.
Preserve the current unverified GOAL_DONE label until evidence-aware goal
completion is independently implemented and checked.

## M5 pro / 32k starting profile, subject to measurement

Do not change the Modelfile, quantization, drafting or context size in this work.
Do not send num_ctx. One inference at a time through GenerationGate; start with
sequential page reads as the current chat path does. The global four-fetch ceiling
is an I/O bound, not permission for four simultaneous Qwen generations.

Keep web.total_tokens=28000 as the current outer prompt-plus-reply allowance.
This leaves 4,768 tokens below a verified 32,768 window, but calibrated estimation
and live-window checks are still required. It is not 28K plus the reply.

With the then-shipped 8,192 loop answer reservation, at most 19,808 estimated tokens
remain for all input, tool schemas, evidence and framing under that allowance
(the shipped loop cap is now 4,096, the value a tuned turn already ran at).
With a 4,096 answer, 23,904 remain. These are accounting ceilings, not targets.
Never add 6,000 evidence tokens on top of a full history window.

Candidate working targets:
- Routing/listing-selection output: 512 tokens, temperature 0, reasoning none.
  Preserve the effective loop answer ceiling (4,096 in the measured profile; configured 8,192) for actual goal continuation.
  Separate planning output from answer output rather than capping both at 512.
- Listings: five initially, ten only if the gap justifies it.
- Selected pages: up to three initially; two/three passages each.
- Newly injected page evidence: aim for 3,000-4,000 tokens; never exceed the
  configured 6,000 evidence cap or the actual remaining calibrated input room.
- One evidence digest per useful batch, approximately 768-1,024 output tokens,
  using existing answer schema/validators. Count this inference in usage and
  deadlines. Skip the digest if no evidence was obtained.
- A normal useful cycle aims for one search, up to three reads, and at most one
  different-query refinement before returning to reasoning. This is a routing
  target, not a new credit quota or a reduction of the configured 30-call ceiling.
- Keep compaction's 24K configured trigger and dynamic web/reply tightening.
  Preserve goal and exact evidence references through compaction, not prose-only
  assertions that research has already happened.

Timeouts must be based on measured prefill/decode and cold load. Reuse
model_limits.rs's padded estimate (load + prompt/prefill_tps + output/decode_tps,
with its safety factor and clamps) and the outer turn deadline. A 512-token
output can still have expensive prefill; a desktop model name does not justify
a hard 10-second inference deadline. Shipped per-call timeout is 1,800 seconds,
auto_tune may replace it, and web-turn allowance can double it. Report the actual
effective deadline during benchmarks.

A deferred network experiment is request_seconds=20, versus the preserved 8 seconds,
with a three-provider allocation roughly 6/6/8 seconds including pacing and
queueing, lending unused time forward. This changes no provider eligibility and
is a trial profile, not an applied or proven default. Exact-page direct and
Firecrawl reads currently each have their own bounded request stage; account
for both under the outer turn deadline. Firecrawl's upstream timeout must remain
inside the local timeout with response overhead. Never add an unbounded wait.

## Implementation sequence and affected files

1. Capability and deadline behavior: web_provider.rs, web_google.rs and focused
   existing provider tests. Keep default chains exact; fix deadline starvation
   before adding ranking behavior. A pure scheduling/eligibility helper is enough.

2. Listing triage and page passages: web_google.rs, web_tavily.rs, chat_web.rs,
   web_index.rs, web_search.rs; use existing retrieval_ranking helpers. Preserve
   panel/CLI/API listing compatibility with additive optional fields. Audit every
   caller of google_search, fetch, get_or_fallback and retrieve_many. Firecrawl
   freshness handling belongs in web_firecrawl.rs and its request fixtures.

3. Goal integration and evidence continuity: chat_web.rs, routes/core.rs,
   loop_runner.rs, prompts.rs and existing web_research pure validators. Keep
   active-run state bounded and owner-scoped. Change sessions.rs/web_ledger.rs
   only if durable reference metadata is actually needed; no page-body store.
   Review the console loop mirror in assets/static/harness.html if server stop
   codes or route events change. Do not call the research runner inside chat.

4. Reconcile owning docs/invariants after actual behavior is verified:
   INVARIANTS.md, docs/SECURE_RESEARCH.md, docs/CONSOLE.md and docs/MODELS.md as
   needed. assets/config.default.yaml only for a measured, justified tunable;
   no provider-default changes. Update config validation/reload only if a new
   setting is unavoidable. Regenerate assets/self/MANUAL.md from its script
   when an input changes. Respect existing Markdown budgets and PR template.

## Acceptance and measurement

Use the current main revision as the baseline in an isolated Harness home.
First test deterministic routing with controlled HTTP fixtures, then run the
real Ollama model, then explicitly authorized live-provider samples. Keep those
evidence classes separate. Existing broad CI/model fixtures are not proof that
this particular Qwen tag works on this Mac.

Required behavior cases:
- Web off; goal off/fetch/all; tools unsupported; absent/invalid policy.
- Every provider selection crossed with both fallback flags, key presence and
  cooldown/quota states. Forced missing/bad keys remain loud. All unavailable
  returns an honest gap, never disabled-provider traffic.
- First/second-hop timeout with a useful final-hop opportunity; insufficient
  total deadline; Google pacing; cancellation during every stage; key or URL
  policy changed while queued and before evidence delivery.
- Clean empty results; wrong-host site results; duplicate/missing fields;
  malformed JSON versus secret reflection; correct provider attempt receipts.
- Local-only/notes question makes zero searches. Explicit outside-source request
  in the full stored goal survives a generated loop reminder and compaction.
- Listing-only answer versus a claim that requires a page; a late-page answer
  anchor must survive passage selection at the same delivered token budget.
- Direct readable page avoids Firecrawl; qualifying block/script shell uses it
  only when enabled; redirects, private addresses, denied URLs and oversized
  pages never become extraction bypasses; failed fallback preserves provenance.
- Evidence quote/ID validation, semantic support review, conflicts, unknown
  freshness, cache eviction/revocation, different owners and changed goals.
- Tool-selection cap remains small while a long loop answer still gets its
  full cap; every prompt plus reservation fits the measured window. Unknown
  upstream usage stays unknown. Refusal or partial evidence is not completion.

Measure per-stage p50/p95 time, first-token time, prefill/decode throughput,
prompt/completion tokens, selected-source coverage, grounded claim rate,
unsupported-read claims, repeated queries/reads, compactions, Firecrawl calls,
fallback success and memory pressure/swap growth. Compare the same prompt set
and evidence budget, with warm/cold and warm/cold cache runs recorded separately.
Require zero unauthorized egress and zero invented successful-read receipts in
the acceptance corpus. Passage-anchor recall alone is not an answer-quality win.

Local verification should build/run affected paths and use only focused tests
needed for the matrix, not a full local suite. Relevant existing files include
tests/chat_web.rs, tests/loop_runner.rs, tests/web_research.rs,
tests/web_phase2_redteam.rs, tests/retrieval_ranking.rs and module unit tests.
Use pinned Rust 1.89 shims, scoped lint/build checks, and actionlint/zizmor only
if workflows actually change. Leave broad/platform coverage to hosted CI.
Browser/native checks become required if the delivered feature changes those
surfaces; no browser/native acceptance is claimed by this plan.

## Primary external references checked

[Tavily Search API](https://docs.tavily.com/documentation/api-reference/endpoint/search).
Current API docs expose content/score and describe bounded chunks for basic,
fast and advanced search. Older help text differs; retain basic depth initially
and validate actual response shape before tuning chunk use. No auto-depth or
raw-content upgrade is required for this router.

[SerpAPI Google organic results](https://serpapi.com/organic-results).
Documents position, snippet, highlighted words and optional date. Preserve
provider rank separately from the output list rank.

[Google snippets](https://developers.google.com/search/docs/appearance/snippet).
Snippets depend on the query and may use page content or a meta description.
They are useful triage signals and not a receipt for reading the target page.

[Firecrawl scrape](https://docs.firecrawl.dev/api-reference/endpoint/scrape).
Documents request timeout and cached-read maxAge separately from cache storage.
No paid requests were sent during this research.

[Ollama context](https://docs.ollama.com/context-length).
Larger windows require more memory; verify actual allocated context and compute
placement. This plan preserves the requested 32K profile rather than assuming
that a generic recommendation for a larger window proves a benefit here.

## Historical lead, not current acceptance

The 2026-11-02 local research record reported better answer-anchor presence from
BM25 passages than prefixes on five selected pages, but its final retrieved-
passage Qwen answer failed a thinking-leak guard. It also recorded false read
claims and an off-domain site-query result. These motivate the acceptance cases;
they were not rerun here and do not prove the new router's quality.

## Implemented behavior and measured limits

The implementation follows the existing chat tool loop. It adds no provider,
classifier service, dependency, config key, page-body store or nested research
runner. A routing instruction uses the full goal, bounded listings and focused
read queries. The model remains responsible for semantic sufficiency; permission
and budget decisions remain deterministic.

- `web_google` and `web_tavily` retain bounded optional provider ranks, scores,
  highlighted words and dates. Recognizable `site:` / `-site:` constraints filter
  off-host listings; this is not a full search-language parser.
- `chat_web/evidence` selects up to three relevant existing BM25 passages, with
  receipt IDs/offsets only for passages delivered whole. Omitted `refresh` reuses
  fresh evidence already read for the active goal; other reads default fresh.
  Explicit `true` refreshes; `false` permits current-policy cache reuse.
- Server-managed runs hold nine URL/hash references, three passage IDs each,
  sixteen prior queries and one validated packet. Restoring after compaction
  rechecks permissions, age and hashes and supplies citation-to-URL mappings.
  Failed or missing content contributes no renewed evidence. A new run starts empty.
- A useful read batch produces a ≤1,024-token digest before the next tool choice,
  without generating and discarding an answer first. Unchanged passages skip it.
  Restored findings include bounded source passages for critical review. Invalid
  digest JSON retains valid passages and a visible failure. Transport/model
  failure still follows the ordinary failed-turn policy.
- Tool planning first ran at 512 tokens with truncated answers regenerated at
  full synthesis; a hop that offers tools now writes up to the answer cap once.
  The active Qwen profile allowed 4,096 answer tokens, 28,000 total prompt-plus-reply
  tokens, 6,000 evidence tokens, 24,000 compaction trigger and 30 tool calls.
  Auto-tuning reported a 1,140-second model deadline, 300-second synthesis limit
  and 1,200-second research limit. No Modelfile, context, timeout or provider
  default was changed. Firecrawl now sends `maxAge:0` alongside `storeInCache:false`.
- #786 (merged) splits the remaining search deadline across the remaining hops
  and, when time allows, reserves pacing time plus one second for a final public
  Google hop (`web_provider::hop_budget`). The reserve is the pacing wait still
  due, and a cooling later hop takes no share (`web_provider::timed_hops`).

### Runtime observations

Both baseline and candidate used Rust 1.88 binaries, the same disposable TLS
home/configuration, fresh chat sessions, permitted public pages and the real MLX
model at 32,768 context. Tool receipts, answers and usage were inspected. The
model stayed warm; these are individual observations, not p95 estimates or an
isolated hardware throughput benchmark. Background development work and network
variance limit latency attribution. No structured facts were injected into these
comparison turns.

| Request | Baseline wall / first token | Candidate wall / first token | Observed result |
|---|---:|---:|---|
| Supplied-number arithmetic | 9.45 / 9.02 s | 10.62 / 9.81 s | Correct 80; zero web calls in both. |
| Late RFC section, deliberately questionable premise | 60.24 / 9.34 s | 20.19 / 11.44 s | Baseline sent 24,000 prefix characters then hit Google challenge. Candidate sent 2,398 relevant characters including the section at offset 63,432 and reported the unsupported distinction. |
| IANA registration/production guidance | 9.37 / 3.86 s | 12.50 / 5.24 s | Both read the page and quoted it, but overstated a recommendation as impossibility. |

The late-section prompt used 23,997 aggregate prompt tokens on baseline versus
9,105 on candidate. A corrected extension-behavior question completed in 12.86 s
(first token 4.37 s), distinguishing caches that understand an extension from
those following the base directive. A source-qualifier prompt adjustment did
not fix the IANA overstatement (10.37 s); semantic faithfulness is not certified.

A real two-turn server goal completed in 61.39 s with one page read, 1,316
completion tokens and no tool failures. Its second turn critically reviewed the
retained findings without another read. `GOAL_DONE` remained explicitly unverified.
Safari private mode rendered the session, and a further browser-driven loop rendered
the page receipt and three-finding digest separately. The loaded MLX allocation later reported
27.75 GB, so the earlier readiness allocation is not a memory ceiling. VM counters
showed pre-existing compression/swap; no controlled before/after swap claim is made.

Follow-up repetition exposed a control-flow defect: two reruns at `86b28ff`
ended after one server turn with two fetch calls (69.38/75.48 seconds). A diagnostic
trace showed the second call rephrasing the same section query and returning the
same cached passages. Evidence messages also lacked the current turn. Moving the
digest before the next tool decision removes the discarded draft answer; explicit
turn framing keeps the actual harness turn visible.
Restored findings now carry bounded passages for review. Fresh current-goal reads
reuse cache by default, explicit refresh remains effective, and invalid digests
retain valid references.

Five consecutive fresh-session RFC runs with that correction completed two server
turns, one fetch and one validated digest each, with no tool failures or first-turn
`GOAL_DONE`. Wall times were 62.35, 57.32, 43.26, 46.51 and 46.31 seconds; completion
tokens were 1,818, 1,763, 1,080, 1,022 and 982. This verifies the repeated workflow,
not universal semantic correctness. A startup attempt blocked by `model_tune`
performed no inference and was excluded; readiness was verified before repetition.
The temporary diagnostic endpoint was removed and its disposable configuration
restored byte for byte. Trust stores and model settings remained unchanged.

Two further IANA goals completed two turns with one read each (44.26/56.36 s),
distinguishing service guidance from technical impossibility, though policy
extrapolations still require review. A local arithmetic goal in fetch mode attempted
an unnecessary example.com read; current URL policy refused it. Stronger routing
wording did not correct that behavior and was reverted. Natural-language exclusion
of web is not an enforced gate; use the explicit goal web-off mode when required.

### Verification scope

Focused HTTP tests exercise off/fetch/all modes, batching, budgets, cancellation,
revocation, provider fallback and two-turn reference reuse. The latter proves one
read and one digest across two turns with a source URL mapping. Cache fixtures
cover late passages, forced refresh, cache hits, hash tampering, eviction and
revocation; ownership checks reject another owner/run/goal. Provider fixtures
exercise slow-hop fallback, bounded listing metadata and Firecrawl request fields.
No authenticated Tavily, SerpAPI or Firecrawl request was made; live credit/provider
availability is unverified. Public Google challenged a real request honestly.

Native setup/TLS work merged separately as #787. Safari private sign-in and console
rendering passed after an operator exception. The operator declined trust-store
changes, so warning-free Safari and Chrome/Firefox trust are unverified. The
in-app browser also refused the untrusted certificate. These are not provider
or routing failures.

This is a measured 32K starting configuration, not a claim of universal tuning.
Longer real compaction cycles, a larger semantic-quality corpus, stable-load
latency distributions and paid-provider runs remain future acceptance work.
Preserved opt-ins mean unavailable authorized providers can still leave an honest
gap; no disabled provider or permission is silently enabled.
