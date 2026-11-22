# Accounts, HTTPS and permitted web research

Fresh homes enable HTTPS, role authentication and web operations. Web content
needs an explicit URL grant; the allowlist is seeded with
`https://www.google.com/*` for public Google search. Upgrades keep
`config.yaml`, where missing legacy auth or TLS switches stay off; to adopt the
defaults, merge these literal values and restart:

```yaml
auth:
  enabled: true
tls:
  enabled: true
  auto_generate: true
  certificate_days: 365
  cert_file: ""
  key_file: ""
security:
  api_key_optional: true
```

Malformed auth/TLS booleans refuse configuration. The old
`security.api_key_optional: false` mode is ignored, and a harness key neither
blocks a valid account nor grants access.

## Accounts and roles

### First login and passwords

A fresh store's `admin` password is a random one-time code in owner-only
`<home>/setup-code`, deleted once replaced; other local accounts cannot claim
it. Homes still on `admin` / `admin` rotate at startup. Sign in and replace it
with at least 12 characters; until then the account can only inspect its
identity, change its password or log out. Replacement checks the current
password and CSRF token, then invalidates old sessions.

Every role can **change password** with its current password. An
administrator uses **USERS** (`/users`) to create users or reset passwords.
Password reset, role change, disable and deletion revoke that account's
sessions; disable, deletion and loss of coding access also stop its coding
work. The last enabled administrator cannot be deleted, disabled or
demoted.

Authentication probes time out after five seconds and account
mutations after fifteen, body included. A timeout does not prove a
change was rejected; refresh first.

### Roles

| Role | Allowed | Refused |
|---|---|---|
| Administrator (`admin`) | Normal harness work; users, roles, disable/reset; global credentials; web enablement and URL policy | Every operation lacking existing repository/write/approval/publication authorization |
| Portal operator (`operator`) | Chat; permitted fetch/search/research; own sessions/jobs/schedules/notes/style; persona read/proposals; coding workflows; read-only MCP servers; password/logout | Users/roles, keys, web/provider security, model pulls/selection, persona edits/backups, shared memory, audit/log reads, privileged MCP servers |
| Auditor (`auditor`) | Local chat in own sessions, prompt preview, `/api/audit`, `/diag` reads, opt-in log chat tools | Memory, attachments, web tools, cloud, loops, model choice, shared data, keys, administration |

Schema 3 folds retired roles into `auditor`: `portal_auditor`
accounts sign in again, `audit` accounts once an admin enables them. The account bar
shows the signed-in role; an auditor gets its own sessions and a restricted
`/status`. Audit records name the actor and route, never passwords, session
tokens, query bodies or keys.

### API Keys

The **API Keys** tab lists `env_keys::MANAGED_KEYS` except the harness and
planner keys (`/api` manages those; [managed
keys](INSTALL.md#supported-managed-keys) covers masks, restarts and precedence).
Administrators save, replace or clear values and see only masks and status,
through the OS credential store; without it they fail closed unless literal
`security.allow_plaintext_key_file: true` keeps the
[private legacy file](INSTALL.md#8-persistence-optional-keys-and-recovery). A
missing credential affects only its provider; unset `GH_TOKEN` uses `gh auth login`. Webhook bearers are
[configured separately](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook).

### Account storage and recovery

Versioned `auth.sqlite3` stores accounts and hashed session tokens
transactionally; private `auth.initialized` stops a missing database from
becoming a default-password bootstrap. A legacy `auth.json` migrates once
(`auth.json.pre-sqlite` keeps a copy). Corrupt, empty, unsafe or unsupported
stores refuse startup and never reset. Back up or recover with the server
stopped, keep database and marker together, and never delete them to recreate `admin`.

Each scrypt derivation uses about 128 MiB, so `auth.max_concurrent_operations`
bounds login and password work; extra attempts wait. Sessions expire after
`auth.session.idle_timeout_sec` without use and `auth.session.absolute_timeout_sec`
regardless, each at most 30 days. Chat session files under `sessions/` are written atomically
with mode 0600 because history can carry pasted secrets;
`memory/structured.sqlite3` follows the same owner-only policy
([at-rest contract](STRUCTURED_MEMORY.md#threat-model-at-rest)).

### Session ownership and legacy adoption

New sessions carry the account's random `user_id`, which listing, loading,
search, export, chat, prompt preview, goals, skills and style enforce;
administrators inherit no other account's sessions. Auth-disabled homes use
owner `local`, and enabling auth does not reassign it.

Sessions without an owner are unassigned legacy data: kept on disk, hidden
from reads, search and export, untouched by **Clear my session history**. An
administrator adopts one (**Sessions → Review unassigned legacy sessions**) with
a reason and confirmation; adoption keeps the transcript and blob pin owners but
clears the goal-stage coding approval. Unknown-schema, unreadable and
interrupted files stay for manual recovery.

Jobs and schedules use the same owner boundary
([contract](../INVARIANTS.md#session-export-and-transcript-search-stay-on-the-machine));
each schedule occurrence rechecks the current account.

## HTTPS and native trust

`serve` and the desktop sidecar share one transport setup: a unique P-256 key
and certificate in private `tls/server.pem` (0600, directory 0700), SANs
`localhost`, `127.0.0.1` and `::1`, reused across restarts and checked at start.
Invalid or expired material refuses startup with no HTTP fallback; explicit
`tls.enabled: false` is the legacy HTTP choice. The listener accepts TLS 1.3 only.

The desktop app pins that leaf for its own WKWebView and changes no global
trust ([native trust](DESKTOP.md#ownership-ports-and-security-boundaries)). The
server checks origin, HTTP/2 `:authority` and HTTP/1 Host; HTTPS cookies are
Secure, HttpOnly and SameSite=Strict. Browsers do not isolate cookies by port,
so another loopback service the browser visits receives the session cookie and,
on a shared host, could replay it.

Print the public certificate, or renew generated material:

```bash
./target/release/cgagentharness tls certificate > local-public.pem
# Stop this home's desktop/serve process before renewal:
./target/release/cgagentharness tls renew
```

Renewal leaves an operator certificate (`tls.cert_file` and `tls.key_file`,
valid chain, key and loopback SANs) to the operator. CLI sessions log in again
after a renewal.

Browsers need separate trust: **Setup → Export public browser certificate**
saves the leaf and shows its SHA-256. Safari and Chrome on macOS use Keychain
SSL trust; Firefox may need its own exception. Review trust after a renewal;
never disable validation.

## Terminal access

`account` and `web` use the running home's authenticated API (no offline
policy mutation); set an absolute `CGAGENTHARNESS_HOME` if needed and pass a
desktop or custom-port origin with `--url`.

```bash
./target/release/cgagentharness account --url https://127.0.0.1:8790 login admin
./target/release/cgagentharness web status
./target/release/cgagentharness web allow 'https://example.com/docs/*' --group docs --seed https://example.com/docs/
./target/release/cgagentharness web research 'How does widget_open fail?' --group docs
./target/release/cgagentharness account logout
```

Passwords come from hidden terminal input (or a private stdin pipe); never put
secrets in argv or chat. Private `cli-session.json` binds the cookie to the
origin and certificate; logout removes it. The CLI refuses foreign origins,
redirects, proxies and certificate substitution before sending credentials.

## Web permissions

Web settings persist (an absent or invalid legacy `web_enabled` stays off);
`/web on|off` controls future reads and injection. Enablement and policy are
**shared-home**; selections are account-private. Default allowlist mode needs
URL grants; an administrator can confirm `/web mode denylist` to permit
unblocked public URLs. Blocks (`/web block`, `/web unblock`) win in both modes
and across HTTP/HTTPS, and a host without a path blocks its subdomains. Switching
modes keeps both lists; named groups still restrict reads to their allow rules.
`/web status` shows policy; `/web check URL` checks permission without networking.

`tools/web_allowlist.json` is the v1 policy: up to `web.max_rules` entries per
allow/block list (batches name 1–32), capped at `max(64 KiB, web.max_rules × 4 KiB)`;
a policy over a lowered cap is refused whole. `allow` validates groups and
seeds, assigns stable IDs and writes atomically. A missing policy refuses reads
until an authorized mutation creates it; invalid files never reset. Stop the
server before manual edits.

| Rule | Meaning |
|---|---|
| `https://example.com/article` | That canonical URL only, no children or additional query |
| `https://example.com/docs/*` | `/docs/` and descendants, excluding `/docs` |
| `https://example.com/*` | Paths and query strings on that exact HTTPS origin, including `/` |
| `https://*.example.com/docs/*` | Explicit subdomains below the named domain; apex excluded |
| `https://*.example.com` | Each subdomain's `/` only; add `/*` for paths |
| `https://example.com/article?q=one` | Exact query identity/order; no arbitrary parameters |

Console `/web` adds only `https://` to scheme-less operands. Default ports,
IDNA and hostname case normalize; schemes never downgrade; fragments are
stripped. Trailing-dot hosts, credentials, alternate IP spellings, dot
segments, encoded separators or controls, malformed or nested escapes and
ambiguous authorities are refused. Encoded
punctuation/space in query data keeps its identity and cannot move
host or path. Path wildcards include query strings, but never suffix
impostors or alternate ports. Blocked rules match unreserved escapes decoded and slashes merged. `www.example.org` and `example.org` are distinct.
Discovered links require current permission.

HTML extraction shares the request deadline and stops on cancellation;
`web.html_parser_handles` bounds parser state, and excessive nesting returns
`WEB_HTML_COMPLEXITY` without saving partial evidence.

Each rule belongs to one source group, `default` unless `--group NAME` names
another; repeatable `--seed URL` records starting pages that must fit a pattern
in the same grant. `--group` on `check`, `fetch`, `pages`, `search` or
`research` limits that request to the group's allow rules. `/web deny` removes
an allow rule ID or matching pattern across groups; it adds no block.

Legacy rows migrate as exact URLs; one invalid row fails the migration. Legacy
`web_context.txt`/`web_last.json` are never injected; selections live in
`tools/web_<account-hash>_{last,context}.json`.

## Search, fetch and research

[Google search](#google-search) covers listing permissions. In default
allowlist mode, an administrator first grants the example page:

```text
/web allow https://doc.rust-lang.org/book/ch01-01-installation.html
```

Then ask in chat ("Read that page and tell me which command checks the
installed Rust version") or use direct commands:

```text
/web search --count 3 veeam software cve
/web fetch https://doc.rust-lang.org/book/ch01-01-installation.html
/web allow https://www.veeam.com/* https://example.org/* --group vendors
/web research --group vendors Compare the backup approaches
/web pages --group vendors backup retention
```

Double-quoted phrases stay exact. Put `--` before query text that resembles
flags (`/web search -- --help`), or single-quote it
([slash reference](CONSOLE.md#78-slash-command-quick-reference)).

### Chat web tools

Local chat and `/loop` offer only `web_search` (configured-provider listings)
and `web_fetch` (an exact permitted URL), no repository, shell, policy, account,
key or mutation tool; `web.chat_tool_calls` bounds all rounds. Listings grant admin-turn exact-URL reads
(`web.search.result_reads.enabled`, on unless literal `false`, even in a home
whose config predates the key). A listed `https://arxiv.org/abs/<id>` also
grants exactly `https://arxiv.org/pdf/<id>`, its `full_text_url`, within
`max_urls` (`result_reads.arxiv_pdf`, literal `true` only); a PDF past that cap
is not offered. A URL only a `/*`, `*.` rule or denylist permits, unlisted and
not named in the message, pauses a live admin turn for Approve; elsewhere it is
refused (`WEB_FETCH_COMPOSED`, `web.fetch_provenance`).

A malformed batch (bad identifier or call type, unoffered tool, unparsable
arguments, too many calls) is refused whole before any call runs; a bound one
call breaks (a `query` over 200 characters) fails that call alone as `ok: false`
(#772). Each call fits prompt, definitions, one reply and minimal results inside
`web.total_tokens`; a batch without room runs no reads (`WEB_TOKEN_BUDGET`;
`WEB_TOOLS_WITHHELD` reserves an answer after evidence). A batch that fails
entirely ends the tool rounds: the model answers once without tools, retried
once if empty, then harness-written (`WEB_FINAL_EMPTY`). Usage counts completed
model calls. Cancellation aborts the turn.

`web_fetch` selects up to three BM25 passages from its page by `query`
(1–200 characters), two per page across several pages; only whole delivered
passages get receipt IDs and offsets. Omitted `refresh` reuses fresh
evidence already read for this goal, other reads default fresh;
`refresh:false` allows permitted cache within `web.stale_seconds`, and `true`
forces one fresh read per page per turn (a repeat re-selects from it,
`refresh_reused`).

Before further tools, new useful passages get a bounded digest (≤1,024 output
tokens) with validated quotes and source IDs: provenance, not logical support.
A quote over 160 characters (`QUOTE_MAX_CHARS`) drops its claim
(`quote_too_long`), never trimmed. Server-managed goal runs retain at most nine
URL/hash receipts, three passage IDs per URL and sixteen queries, plus the
digest; reuse rechecks age, hashes and permissions. References survive
compaction, not completion, cancellation or restart.

`POST /api/chat` returns `web_tools`, reply and usage. Each entry records its
call's `query` and `refresh`, or `count`; a digest entry lists one
`drop_reasons` code per dropped claim. Session
`web_sources` is a bounded metadata ledger, never a second page store.
`web.carry_forward_sources` ships 5: permitted earlier URLs/queries (no page
text), cited when used. Design: [router](WEB_DECISION_ROUTER.md);
summary: [README](../README.md#how-the-research-loop-decides).

### Google search

In **API Keys**, save a [SerpAPI](https://serpapi.com/search-api) key (not a
Google Cloud key) under **Google results (SerpAPI)** or a Tavily key under
**Tavily search**, then **Test search**; changes apply to the next search, and
an inherited environment value wins ([managed keys](INSTALL.md#supported-managed-keys)).

Restart-only `web.search.provider` is `auto`, `google-public`, `serpapi` or
`tavily`; `auto` follows the active keys: Tavily, then SerpAPI, then public
Google. Keyed providers receive the query, result count and their own key,
never chat history or account credentials. Keys never appear in URLs, model
context, metadata or errors; a listing reflecting a key is refused. With web enabled, keyed listings need no Google
URL grant.

- **SerpAPI** uses the fixed `https://serpapi.com/search.json` Google backend,
  key in the request query.
- **Tavily** sends one basic-depth request (one credit) to the fixed
  `https://api.tavily.com/search`, key in the Authorization header. A key
  changed during DNS refuses before sending; HTTP 429, 432 and 433 are quota
  refusals.
- **Public Google**, with no active key, parses the permitted search page's
  organic results in order, each with the text Google shows under it as its
  snippet. It never runs JavaScript, solves CAPTCHA, adopts cookies or invents
  results; challenges, redirects and transport failures are explicit.

Provider answers and page bodies never enter a listing; its row and query
fields are in the [router](WEB_DECISION_ROUTER.md#implemented-behavior-and-measured-limits).

Fresh homes seed the Google grant; a home that removed it re-grants it:

```text
/web allow https://www.google.com/*
/web search veeam software cve
```

`web.search.fallback_to_public_google` and
`fallback_to_serpapi` (after Tavily; both ship true) try the next hop only when a provider is
unusable or its quota or daily ceiling is spent; a rejected key, a refused query
and a clean zero-result answer never fall through. The answer carries
`fallback_from`, each hop logs `web_search_fallback`, and more than
`fallback_alert_per_hour` SerpAPI fallbacks in a clock hour add one
`web_search_fallback_burst` line. A removed `provider: searxng` or a non-empty
`web.search.searxng_url` fails boot.

Each Tavily search, SerpAPI search and Firecrawl read first reserves its units in
`tools/web_units.sqlite`, a private ledger every turn, loop, job and CLI process
on the home shares, against restart-only daily ceilings
(`web.search.tavily_units_per_day` 30, `serpapi_units_per_day` 8,
`web.firecrawl.units_per_day` 30, UTC days; #772 D10). Units return only when
nothing was sent. A spent search ceiling (`WEB_SEARCH_DAILY_CAP`) moves on to
the next hop; an unreadable or deleted ledger refuses the paid hop
(`WEB_UNITS_UNAVAILABLE`); both audit `web_units_refused`. The ceilings count
this home only, not a dashboard balance.

An exact generated search URL also grants; an exact homepage grant does not,
`https://google.com/*` does not grant `www`, and redirects stay refused.
Allowlist mode answers `WEB_ALLOWLIST_EMPTY` for an empty policy and
`WEB_GOOGLE_PERMISSION` for a non-Google one; denylist mode needs no grant but
still refuses blocked Google URLs.

A challenge or block pauses public Google for `web.search_cooldown_public_sec`
(ships 600); a quota refusal pauses SerpAPI or Tavily for
`web.search_cooldown_quota_sec` (0: next clock hour). A paused search answers
`WEB_SEARCH_COOLDOWN` ("cooling down until HH:MM", local time) without a
request, which `/loop` does not count; restart clears it, and rejected keys
never pause.

`/web search` means Google in the console and CLI; `/web pages`, or a
source-group argument, selects permitted-page passage search.
`POST /api/web/search` without `engine` is still page search; send
`engine: "google"` and optional `count` (1–10, default 5) for listings.
`/api/web/research` accepts only page search.

### Fetch, discovery and research

Fetch accepts exact URLs only; wildcard patterns belong in `allow`.
An `application/pdf` response within `web.pdf_response_bytes` (8 MiB; a budgeted
run's remaining `web.run_bytes` if less) is read locally as
[page text](CONSOLE.md#pdf-text); its links are never collected.
A `fetch` batch validates all targets before any request and shares page, byte,
time and per-site limits. Partial reads report coverage and failures.

`/web off` stops running reads at their next dispatch, storage or delivery
check; queued and paced requests, search providers and the Firecrawl fallback
recheck it before sending. A response arriving while web is off is discarded
(`WEB_DISABLED`); cached pages stay on disk.

Every content request is DNS-pinned to checked public addresses with TLS hostname
validation and no reuse, retries, proxies, redirects, compression or cookies
([contract](../INVARIANTS.md#public-web-evidence-requires-current-content-permission)).

Opt-in `web.firecrawl.enabled` (ships off; needs **Firecrawl page reader** in API
Keys) is the one exception, for exact reads only (`fetch`, chat `web_fetch`,
batches; never discovery, crawl or search). After a pinned 403, 429 or 503, a
script shell (scripted HTML whose text is under 200 characters and 5% of the
markup), an oversized body, or a PDF local [text extraction](CONSOLE.md#pdf-text)
refuses as textless, over a limit or malformed (10 pages at most; a busy,
timed-out, failed or encrypted read stays local), the authorized URL leaves the
machine once, to the fixed `https://api.firecrawl.dev/v2/scrape`: basic proxy,
markdown only, `storeInCache: false`. Another final URL is refused as a
redirect; no links are kept; the page carries `via: firecrawl`.
`web.firecrawl.max_per_hour` (default 20) caps reads (a read refused before it
is sent gives its slot back), each reserves daily units (1 for a known non-PDF,
else the 10-page PDF cap), batches charge both reads to `web.run_bytes`, and HTTP
402 or 429 pauses it like a search quota. A failed fallback keeps the original
result, its code in `details.fallback`.

The HTML parser drops scripts, styles, navigation, headers, footers, forms and
`hidden` or `aria-hidden` elements; text hidden only by CSS stays. Extracts keep
canonical URLs, hashes, fetch age, validators, extraction version and passage
offsets.

The shipped YAML lists every `web.*` limit and range; a home missing those keys
gets smaller code defaults ([reloadable](INSTALL.md#reload-limits-without-a-restart)).
A failed request can charge the full response allowance.

For `research`, repeat `--url` for permitted starts, which narrow one run but
grant nothing; otherwise traversal uses group seeds and permitted literal
prefixes such as `/docs/`. A host wildcard without seeds uses up to four
matching, currently permitted cached pages, else reports `WEB_SEED_REQUIRED`.
Origin `/robots.txt` is readable when an allow rule covers that origin, even
path-restricted, or in ungrouped denylist mode; blocks still win. Unreadable
robots stops link traversal, not seeds; robots costs requests and supplies no
evidence. Links are deduplicated, paced and policy-checked; there is no sitemap
discovery, provider key or cloud fallback.

Tantivy rebuilds a bounded in-memory BM25 index from permitted cached extracts:
exact phrase and identifier matching (`a_b`, `a::b`; term ranking alone when no
passage holds the identifier, as PDFs print `d_model` as `d model`), title and
heading boosts, deduplication. Derived cache corruption grants nothing;
validators reduce bytes, not requests.

The local model plans at most `web.subqueries` subqueries (ships 3) in
`web.rounds` rounds (ships 2); synthesis cites passage IDs with exact quotes,
and a claim whose ID or quote is not in its passage is dropped
(`WEB_CITATION_DROPPED`).

Shipped budgets are 1200s for discovery including cold load, then 1800s
answering (300s each if the keys are absent); cancellation includes loading
([token budgets](MODELS.md#4-select-an-installed-model-and-check-ollama)).
Provider usage wins, else bytes/4 plus overhead is estimated. Synthesis drops
unfitting low-ranked passages (`WEB_EVIDENCE_TRIMMED`); retrieval interleaves
question and subqueries without embeddings or reranking. Console research
(`/web job`, `/web jobs`) outlives the tab; `/web cancel` stops it.

Citation IDs and quotes are checked against passages, but semantic support still
needs judgment. Rank is not truth, and fetch age is not publication date.

### Saved web selection

`/web inject` selects the last fetched or page-search extract, or a batch's
first successful page, as a bounded excerpt for local chat; `/prompt` previews
it and `/web forget` clears it without altering old chats. Listings never enter
the cache or selection; `/web research` reuses the cache but never replaces the
selection.

## Security boundaries

With auth on, every operational API read and write needs an account; public
endpoints are static login assets, minimal status, setup status and login.
Mutation CSRF is `X-CyClaw-CSRF`; forwarding headers are refused and reverse
proxies unsupported. The dedicated
[machine gateway](MCP.md#private-read-only-memory-gateway) does not inherit console authority.

Ownership follows [session ownership](#session-ownership-and-legacy-adoption)
and the shared-resource list in [SECURITY.md](../SECURITY.md#local-authority-and-shared-resources).
Research questions, answers, controller state and web selections belong to the
initiating random account identity, so a recreated username inherits nothing;
a deleted account's selections can remain on disk, inaccessible.

Beyond admin-turn reads, listings grant nothing: every other read independently
passes current policy, account and public-address checks. Policy is rechecked at
dispatch, cache/index/context reads, storage, model use and delivery;
revocation discards in-flight evidence and blocks revoked cache, but cannot
erase earlier conversation text or model requests
([contract](../INVARIANTS.md#public-web-evidence-requires-current-content-permission)).
URL policy covers only web, not OS, model or GitHub connections.

## Reproducible evidence

`tests/chat_web.rs`, `tests/web_research.rs`, `tests/web_phase2_redteam.rs` and
`tests/auth_guards.rs` pin the contracts above.

```bash
cargo run --locked --example web_research_benchmark -- /absolute/report.json
cargo run --locked --example web_research_benchmark -- /absolute/live-report.json --live
```

The fixed corpus mixes identifiers, paraphrases, stale and contradictory claims,
no-answer cases and hostile text; `--live` uses an installed local model and
downloads nothing.
