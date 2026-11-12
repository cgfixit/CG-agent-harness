# Web decision router: research and implementation

Research baseline and implementation design, 2026-11-07, for goal and loop
turns that read the web. The one-page summary and flow diagram are in the
[README](../README.md#how-the-research-loop-decides); the operator contract is
[chat web tools](SECURE_RESEARCH.md#chat-web-tools); remaining phases are
tracked in [#772](https://github.com/cgfixit/CG-agent-harness/issues/772).

The observations below describe the reviewed baseline, with later changes
noted inline; what shipped and what was measured is in
[Implemented behavior and measured limits](#implemented-behavior-and-measured-limits).

## Outcome and accepted scope

The implementation baseline is `8e14eb2be46419d97a77e38d5ad5fd780ba0d226`.
The operator chose to preserve provider opt-ins and defaults. Credit
purchasing, plan selection and free-tier accounting remain elsewhere; the
router consumes their availability/refusal decisions and keeps existing hard
caps. It must never change keys, enable a provider or buy capacity.

Design target: local long-form /goal and /loop research on M5 Pro, 48 GB,
qwen3.8:27b-mlx-cg, 32,768-token context. This is the server chat loop, not the
src/agentic coding child; I6 remains intact, and routing implies no new shell,
filesystem, MCP, cloud or repository permission.

## Observations that matter

1. Provider transport failover already exists. web_provider.rs::plan_hops and
   falls_through feed web_google.rs::google_search_at. One broker decision,
   search gate and deadline cover a search chain. There is no general semantic
   router over local knowledge, listings, pages and accumulated goal evidence.

2. Provider eligibility, for web enabled and goal mode all (current code;
   at the baseline a Tavily key never changed auto):

   provider=auto, active Tavily key:
     Tavily -> SerpAPI only if fallback_to_serpapi=true and its key is active
     -> public Google only if fallback_to_public_google=true.
   provider=auto, active SerpAPI key only:
     SerpAPI -> public Google if fallback_to_public_google=true.
   provider=auto, no active key, or provider=google-public:
     Public Google only.
   provider=serpapi:
     SerpAPI -> optional public Google. Missing forced key is an error.
   provider=tavily:
     Same chain as auto with a Tavily key. Missing forced key is an error.

   Each fallback flag independently removes its hop. Flags missing from older
   configs are false because flag_is_true requires literal YAML true. The
   shipped YAML sets provider=auto, fallback_to_serpapi=false,
   fallback_to_public_google=true. Invalid provider names/types fail
   configuration loading.

3. Global web off or goal mode off closes web. Goal mode fetch permits exact
   reads but no discovery search. Goal mode all enables both tools, not URL
   access. Keyed listings require account/web authority; public Google also
   requires current URL policy. A fresh home only seeds Google's URL rule.
   Search results grant an administrator's turn exact-URL reads (`web.search.result_reads.enabled`, on unless literal `false`), nothing more; other roles keep the URL policy.

4. Recoverable search codes are WEB_SEARCH_PROVIDER, WEB_TIMEOUT,
   WEB_SEARCH_COOLDOWN and WEB_SEARCH_QUOTA for keyed hops. Missing/rejected
   credentials, refused queries and clean zero-result answers do not fall
   through. WEB_SEARCH_RESPONSE is not in the fallback set: it covers both
   malformed data and reflected credentials, so adding it wholesale would
   erase an important distinction.

5. web.request_seconds is 8 in shipped config. At the baseline SerpAPI could
   take the whole deadline and leave none for public Google, whose pacing has
   a three-second floor; a timeout fixture reproduced it. Fixed by the
   deadline split below.

6. Baseline listings kept only rank, title, URL and snippet; Tavily scores
   and SerpAPI highlighted words were discarded. Titles are capped at 300
   characters and snippets at 1,000.

7. Baseline chat_web.rs gave the model a clipped page prefix, although
   web_index.rs already offered policy-filtered BM25 passage retrieval with
   source IDs, hashes, offsets, headings and fetch timestamps.

8. Firecrawl is an optional exact-page fallback, not a search provider or
   crawler, and ships disabled
   ([contract](SECURE_RESEARCH.md#fetch-discovery-and-research)). TLS,
   private-address, redirect and policy refusals are not reasons to send a URL
   to Firecrawl.

9. Baseline Firecrawl requests set storeInCache=false but omitted maxAge, so
   a response could still come from Firecrawl's own cache.

10. chat_web.rs::StickyRoute suppresses search after local notes are present
    unless the user asks for outside sources: a narrow substring rule, not a
    general sufficiency/freshness decision. routes/core.rs already retrieves
    permitted attachments, notes and selected/structured memory, and /loop has
    no local-file reading tool. Route only among data the current owner and
    surface are already allowed to use.

11. chat_web.rs::hop_kind assigns Synthesis to every loop hop, with a
    regression test enforcing it. At the baseline the loop answer cap was the
    smaller of the configured 8,192 and active auto-tuning (4,096 on the
    verified 32K Qwen profile), and ordinary tool hops used 512 tokens at
    temperature zero. Simply lowering the loop cap would truncate useful goal
    reasoning.

12. The session web ledger holds targets, counts, failures and truncation,
    not passages. carry_forward_sources ships zero, and enabling it carries
    only metadata. Search deduplication is local to one chat turn.
    /web research has checked Citation/Claim/Answer types worth reusing, but
    its runner takes generation and search gates: reuse its pure
    retrieval/validation helpers rather than calling it from chat.

13. The Mac is verified as Mac17,9, Apple M5 Pro, 48 GB, 18 CPU cores.
    Both repository Qwen Modelfiles match: num_ctx 32768, temperature 0,
    top_p 0.9, top_k 20, min_p 0, presence_penalty 0, repeat_penalty 1,
    draft_num_predict 2. Request-time reasoning is none in shipped config.
    An authorized live probe verified Ollama 0.40.15, the MLX runner,
    model digest `b957244f74776492aa8c7ebac67328d26f1068410c719a08733981871cca3afa`,
    and a loaded context of 32,768. The cold readiness probe took 7.14 seconds,
    including 5.52 seconds loading. This tiny probe is not a routing benchmark.
    Verification uses a disposable authenticated TLS home.

## Routing design

Keep execution deterministic and use the model only for semantic choices.
Reuse the two public model tools, web_search and web_fetch. Code calculates
provider eligibility; the model never selects credentials or endpoints. No
separate agent, vector store, plugin protocol, new dependency or generic
router framework is needed.

### A. Decide whether outside evidence is needed.

Derive effective capabilities from account authority, web setting, goal
off/fetch/all, current URL rules, selected provider/fallback flags, tool
support, deadlines and remaining budgets. Preserve the local-notes guard.
Carry explicit outside-source intent from the trusted stored goal/user
instruction; never infer that authority from a web page or model reply.

Use local context first for supplied files, personal/project facts or a
stable transformation/computation. Internal model knowledge serves stable
explanations, never claims about current releases, dates or external state.
Explicit search, verification, current facts, a supplied unread URL, a
material contradiction or an unresolved external fact justify web evidence
when authorized; model confidence alone does not.

Internal outcomes: answer_local, reuse_evidence, search, read_exact, or
report_gap. These are states, not additional model tools. Decode validated
tool calls into search/read actions, and use the existing model hop for
selection rather than a classifier call before every turn. Record a short
decision code and evidence need, not private chain-of-thought.

### B. Search the configured chain only.

Start with one focused query and five results. Use the existing hop plan,
cooldowns, bounded clients, key-change checks and external quota decisions.
Preserve forced-provider missing/bad-key errors. A disabled or missing optional
fallback is unavailable, not implicitly enabled. No round-robin, no parallel
requests to all providers, no speculative retry after a successful answer.

Divide the absolute deadline among remaining authorized hops, reserve Google
pacing plus useful request time, and return unused time to later hops. Never
restart the overall timeout for a fallback. A tiny operator-configured budget
cannot guarantee every hop a useful attempt.

A successful zero-result listing is not a transport failure. The router may
form one genuinely different query for a stated gap, counted as a new search.
Do not switch providers because the model dislikes valid results.

Treat a malformed provider response separately from credential reflection
before proposing broader failover. Fail closed on secrets, authority, SSRF,
policy, TLS and cancellation; add no blind retry loops.

### C. Select pages from bounded listings.

Extend SearchResult only with optional fields used by triage: provider rank,
provider score, highlighted terms and source date. Keep provider identity with
each listing and absent fields absent; reject invalid numeric values and bound
every string/list. Expose no provider answer or raw page content through the
listing adapter.

Enforce explicit host/site constraints locally on canonical result URLs;
a provider echoing site: does not prove compliance. Preserve the actual
number of accepted results, including zero, and keep query parameters that
may identify content.

Triage uses entity/topic match, original rank, title/snippet/highlights,
primary-source preference, recency when required, novelty and source diversity.
Provider scores are provider-specific hints, not cross-provider confidence
or factual correctness. Missing dates are unknown, not old or fresh.

Initially select at most three useful pages; add pages only for a concrete
uncovered part of the goal. A listing/navigation request may end at listings,
clearly labeled. Quotes, precise factual synthesis, comparisons requiring
context and important current claims need page evidence. Sparse snippets are
uncertainty, not proof that the destination is irrelevant.

### D. Read and extract relevant evidence.

Try a currently permitted, sufficiently fresh cached page before a network
reread when the task allows cached evidence. An explicit refresh must not be
satisfied from cache. Reuse Page validation and receipt identity. Cache-only
retrieval must not call discover() or crawl.

Otherwise use the pinned exact fetch. Allow the Firecrawl fallback only when
enabled, for a selected page with an unresolved evidence need; irrelevant but
readable text is not a reason to pay for a second extraction. If Firecrawl is
unavailable or fails, try another permitted selected source or return a
coverage gap. Preserve both transport outcomes.

Route fetched text through web_index::retrieve/retrieve_many on a blocking
worker, using the active subquestion. Retain coherent passages, headings and
nearby qualifiers/table context: two or three passages per source, never a
prefix-only window. Relevance selection saves model context, not the first
HTTP download. Oversized/complex pages remain explicit failures; do not raise
response caps or send those failures to Firecrawl under today's contract.
(2026-11-10: an oversized body or a PDF now goes to the enabled fallback; see
`INVARIANTS.md`.)

For explicitly fresh Firecrawl reads, validate maxAge=0 and keep
storeInCache=false. Distinguish local fetch timestamp, provider
retrieval/cache age and publication date: a fresh local receipt alone does
not establish fresh origin content.

### E. Return an evidence packet to the goal loop.

Reuse Passage and the existing Answer/Citation validator for a bounded packet:
supported findings, conflicting findings, labeled inferences and remaining
questions. Server-side receipts hold exact URL, reader/provider, fetch/cache
status, content hash, passage ID and offsets; Qwen gets short S1/S2
identifiers the server maps to them.

Validate quote membership and referenced passage IDs before acceptance. That
proves provenance, not that a quote entails the claim: evaluate semantic
support separately. Read status and source badges derive from receipts, never
from the assistant saying it read a page.

Keep a small bounded active-run record of resolved questions, selected
receipt references, attempted queries and unresolved gaps, scoped to owner,
session and goal and cleared on changed goal/cancel/end. Rehydrate permitted
passages from the cache after compaction, checking hashes, age and authority
again. Missing/evicted/revoked content becomes unavailable; a summary cannot
manufacture a receipt. Do not add page bodies to web_ledger, silently enable
carry_forward_sources, persist a second corpus, or promote web inferences
into user memory facts.

Permission revocation cannot erase historical chat text; prevent fresh
injection/delivery instead. Keep unsuccessful/blocked research distinct from
satisfied goal criteria, and keep the unverified GOAL_DONE label until
evidence-aware goal completion is independently implemented and checked.

## M5 pro / 32k starting profile, subject to measurement

Do not change the Modelfile, quantization, drafting or context size in this work,
and do not send num_ctx. One inference at a time through GenerationGate, with
sequential page reads; the global four-fetch ceiling is an I/O bound, not
permission for four simultaneous Qwen generations.

Keep web.total_tokens=28000 as the outer prompt-plus-reply allowance, not 28K
plus the reply. This leaves 4,768 tokens below a verified 32,768 window, but
calibrated estimation and live-window checks are still required.

With the shipped 4,096 loop answer cap, 23,904 estimated tokens remain for all
input, tool schemas, evidence and framing: an accounting ceiling, not a
target. Never add 6,000 evidence tokens on top of a full history window.

Candidate working targets:
- Listings: five initially, ten only if the gap justifies it.
- Selected pages: up to three initially; two/three passages each.
- New page evidence: aim for 3,000-4,000 tokens; never exceed the configured
  6,000 evidence cap or the remaining calibrated input room.
- One evidence digest per useful batch, about 768-1,024 output tokens, using
  the existing answer schema/validators and counted in usage and deadlines.
  No evidence, no digest.
- A normal cycle aims for one search, up to three reads, and at most one
  different-query refinement before returning to reasoning: a routing target,
  not a credit quota or a reduction of the configured 30-call ceiling.
- Keep compaction's 24K configured trigger and dynamic web/reply tightening.
  Preserve the goal and exact evidence references through compaction, not
  prose assertions that research already happened.

Base timeouts on measured prefill/decode and cold load, reusing
model_limits.rs's padded estimate and the outer turn deadline: a short output
can still have expensive prefill. Shipped per-call timeout is 1,800 seconds,
auto_tune may replace it, and web-turn allowance can double it.

A deferred experiment is request_seconds=20 with a roughly 6/6/8 second
three-provider allocation; it is a trial profile, not a default. Firecrawl's
upstream timeout must stay inside the local one.

## Acceptance and measurement

Use the current main revision as the baseline in an isolated Harness home.
Test deterministic routing with controlled HTTP fixtures, then the real
Ollama model, then explicitly authorized live-provider samples, and keep those
evidence classes separate. CI/model fixtures do not prove this Qwen tag works
on this Mac.

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
- A long loop answer still gets its full cap; every prompt plus reservation
  fits the measured window. Unknown upstream usage stays unknown. Refusal or
  partial evidence is not completion.

Measure per-stage p50/p95 time, first-token time, prefill/decode throughput,
tokens, selected-source coverage, grounded claim rate, unsupported-read
claims, repeated queries/reads, compactions, Firecrawl calls and fallback
success on the same prompt set and evidence budget, warm and cold separately.
Require zero unauthorized egress and zero invented read receipts.

Existing coverage lives in tests/chat_web.rs, tests/loop_runner.rs,
tests/web_research.rs, tests/web_phase2_redteam.rs, tests/retrieval_ranking.rs
and module unit tests. No browser/native acceptance is claimed by this plan.

## Primary external references checked

[Tavily Search API](https://docs.tavily.com/documentation/api-reference/endpoint/search)
(basic depth kept), [SerpAPI organic results](https://serpapi.com/organic-results)
(provider rank kept apart from list rank),
[Google snippets](https://developers.google.com/search/docs/appearance/snippet)
(triage signals, not read receipts),
[Firecrawl scrape](https://docs.firecrawl.dev/api-reference/endpoint/scrape)
(maxAge is separate from cache storage) and
[Ollama context](https://docs.ollama.com/context-length). No paid requests
were sent during this research.

## Implemented behavior and measured limits

The implementation follows the existing chat tool loop and adds no provider,
classifier service, dependency, config key, page-body store or nested research
runner. A routing instruction uses the full goal, bounded listings and focused
read queries. The model judges semantic sufficiency; permission and budget
decisions stay deterministic.

- `web_google` and `web_tavily` retain bounded optional provider ranks, scores,
  highlighted words and dates. Recognizable `site:` / `-site:` constraints filter
  off-host listings; this is not a full search-language parser.
- `chat_web/evidence` selects up to three relevant BM25 passages from the page
  a `web_fetch` read; a single-page lookup may fill that limit, while lookups
  ranking several pages keep two per page. Receipt IDs/offsets go only to
  passages delivered whole. Omitted `refresh` reuses fresh evidence already
  read for the active goal; other reads default fresh. Explicit `true`
  refreshes; `false` permits current-policy cache reuse.
- Server-managed runs hold nine URL/hash references, three passage IDs each,
  sixteen prior queries and one validated packet. Restoring after compaction
  rechecks permissions, age and hashes and supplies citation-to-URL mappings.
  Failed or missing content contributes no renewed evidence. A new run starts empty.
- A useful read batch produces a ≤1,024-token digest before the next tool choice,
  without generating and discarding an answer first. Unchanged passages skip it.
  Restored findings include bounded source passages for critical review. Invalid
  digest JSON retains valid passages and a visible failure. Transport/model
  failure still follows the ordinary failed-turn policy.
- A batch whose calls all fail, including a turn's first, ends the tool rounds:
  the model answers once more without tools, and the fixed "Web operation
  failed" line is the reply only when that answer and one retry are empty. Each failure stays
  an `ok: false` event, so a provider cool-down no longer repeats one canned
  reply across loop turns.
- Tool planning first ran at 512 tokens with truncated answers regenerated at
  full synthesis; a tool-offering hop now answers once, at the configured cap and temperature.
  The active Qwen profile allowed 4,096 answer tokens, 28,000 total prompt-plus-reply
  tokens, 6,000 evidence tokens, 24,000 compaction trigger and 30 tool calls.
  Auto-tuning reported a 1,140-second model deadline, 300-second synthesis limit
  and 1,200-second research limit. No Modelfile, context, timeout or provider
  default was changed. Firecrawl now sends `maxAge:0` alongside `storeInCache:false`.
- `auto_tune` tightens token budgets only for a window below 32,768; at or
  above it the configured values stand within their ranges. A tuned loop turn
  is held at the effective chat reply cap.
- Token calibration compares the first model call's usage with an estimate that
  counts the evidence (at most 4,000 tokens) a `/loop` turn restores ahead of
  that call, so restored evidence no longer inflates the ratio and shrinks
  later budgets.
- A server `/loop` turn waits, stoppable and for at most two hours, while
  another generation holds the model, instead of counting a failed turn.
- Operator decision, 2026-11-10: `auto` follows the saved keys (Tavily, else
  SerpAPI, else public Google; Tavily wins with both).
- The remaining search deadline is split across the remaining hops and, when
  time allows, reserves the pacing wait still due plus one second for a final
  public Google hop (`web_provider::hop_budget`); a cooling later hop takes no
  share (`web_provider::timed_hops`).

### Runtime observations

Both baseline and candidate used Rust 1.88 binaries, the same disposable TLS
home/configuration, fresh chat sessions, permitted public pages and the real MLX
model at 32,768 context, kept warm. Tool receipts, answers and usage were
inspected. These are individual observations, not p95 estimates or a hardware
throughput benchmark; background work and network variance limit latency
attribution. No structured facts were injected.

| Request | Baseline wall / first token | Candidate wall / first token | Observed result |
|---|---:|---:|---|
| Supplied-number arithmetic | 9.45 / 9.02 s | 10.62 / 9.81 s | Correct 80; zero web calls in both. |
| Late RFC section, deliberately questionable premise | 60.24 / 9.34 s | 20.19 / 11.44 s | Baseline sent 24,000 prefix characters then hit Google challenge. Candidate sent 2,398 relevant characters including the section at offset 63,432 and reported the unsupported distinction. |
| IANA registration/production guidance | 9.37 / 3.86 s | 12.50 / 5.24 s | Both read the page and quoted it, but overstated a recommendation as impossibility. |

The late-section prompt used 23,997 aggregate prompt tokens on baseline versus
9,105 on candidate; its 2,398 characters were two passages, the per-page cap
before single-page reads could fill three. A corrected extension-behavior
question completed in 12.86 s (first token 4.37 s), distinguishing caches that
understand an extension from those following the base directive. A
source-qualifier prompt adjustment did not fix the IANA overstatement
(10.37 s); semantic faithfulness is not certified.

A real two-turn server goal completed in 61.39 s with one page read, 1,316
completion tokens and no tool failures. Its second turn reviewed the retained
findings without another read; `GOAL_DONE` remained explicitly unverified.
The loaded MLX
allocation later reported 27.75 GB, so the earlier readiness allocation is not
a memory ceiling. VM counters showed pre-existing compression/swap; no
before/after swap claim is made.

Two reruns at `86b28ff` ended after one server turn with a second fetch that
rephrased the same query and returned the same cached passages. Moving the
digest before the next tool decision and framing each turn explicitly fixed it.

Five consecutive fresh-session RFC runs with that correction completed two server
turns, one fetch and one validated digest each, with no tool failures or first-turn
`GOAL_DONE`. Wall times were 62.35, 57.32, 43.26, 46.51 and 46.31 seconds; completion
tokens were 1,818, 1,763, 1,080, 1,022 and 982. This verifies the repeated workflow,
not semantic correctness. A startup attempt blocked by `model_tune` performed
no inference and was excluded. The temporary diagnostic endpoint was removed.

Two further IANA goals completed two turns with one read each (44.26/56.36 s),
distinguishing service guidance from technical impossibility, though policy
extrapolations still require review. A local arithmetic goal in fetch mode attempted
an unnecessary example.com read, which URL policy refused; stronger routing
wording did not correct that and was reverted. Natural-language exclusion of
web is not an enforced gate: use the goal web-off mode when required.

### Verification scope

Focused HTTP tests exercise off/fetch/all modes, batching, budgets, cancellation,
revocation, provider fallback and two-turn reference reuse (one read and one
digest across two turns with a source URL mapping). Cache fixtures
cover late passages, forced refresh, cache hits, hash tampering, eviction and
revocation; ownership checks reject another owner/run/goal. Provider fixtures
exercise slow-hop fallback, bounded listing metadata and Firecrawl request fields.
No authenticated Tavily, SerpAPI or Firecrawl request was made, so live
credit/provider availability is unverified. Public Google challenged a real
request honestly.

Safari private sign-in and console rendering passed after an operator
exception; warning-free Safari and Chrome/Firefox trust are unverified.

This is a measured 32K starting configuration, not universal tuning.
Longer real compaction cycles, a larger semantic-quality corpus, stable-load
latency distributions and paid-provider runs remain future acceptance work.
Unavailable authorized providers can still leave an honest gap; no disabled
provider or permission is silently enabled.
