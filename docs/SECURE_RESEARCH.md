# Local HTTPS, accounts, and permitted web research

Fresh homes enable HTTPS, account authentication with role permissions, and web
fetch/search/research. Web content still requires an explicit URL grant; its
allowlist starts empty. Upgrades preserve the existing `config.yaml`; missing
legacy auth/TLS switches remain off. To adopt the secure
defaults, merge these literal booleans into the active home's existing mappings,
then stop and restart its server:

```yaml
auth:
  enabled: true
tls:
  enabled: true
  auto_generate: true
  certificate_days: 90
  cert_file: ""
  key_file: ""
security:
  api_key_optional: true
```

Malformed auth/TLS booleans refuse configuration rather than disabling protection.
The old `security.api_key_optional: false` key-enforcement mode is deprecated and
ignored. Upgrade such homes with `auth.enabled: true` to require account access.
A missing, wrong, or stale harness key neither blocks a valid account nor grants
access. Forwarding headers are refused; a reverse proxy is unsupported.

## First login and account storage

Only a fresh account store creates `admin` / `admin`. That short initial password
uses the normal scrypt hash with a narrowly scoped bootstrap exception. Sign in;
the login dialog then requires a replacement password of at least 12 characters.
Until replacement, the account can inspect its identity, change its own password, or log out; it
cannot run chat, research, coding, account administration, or key management.
Replacement checks the current password and CSRF token and invalidates all old
sessions. Normal administrative resets retain the ordinary password policy.

Accounts and hashed session tokens live in versioned SQLite `auth.sqlite3` under
the home, with transactional updates. Private `auth.initialized` prevents a
missing initialized database from becoming a fresh default-password bootstrap.
A valid legacy `auth.json` migrates its user hashes, roles, disabled state,
lockouts, and timestamps; `auth.json.pre-sqlite` preserves the exact private
recovery copy. Legacy sessions require a new login. The original JSON remains a
recovery artifact, not the live store. A legacy pending-password account retains
its existing setup flow. Corrupt, empty, unsafe, or unsupported initialized stores
refuse startup; no automatic reset occurs. Failed writes do not publish successful
in-memory account changes. Stop the server before an operator-controlled backup
or recovery; preserve the database and initialization marker together.

| Role | Allowed | Refused |
|---|---|---|
| Administrator (`admin`) | Normal harness work; users, roles, disable/reset; global credentials; web enablement and URL policy | Every operation lacking existing repository/write/approval/publication authorization |
| Portal operator (`operator`) | Chat; enabled permitted fetch/search/research; shared sessions, notes, persona and authorized coding workflows; own password/logout | Users/roles, global keys, web enablement/allowlist changes, provider security configuration |
| Auditor (`audit`) | Minimal status, designated redacted `/api/audit`, own password/logout | Chat/research, shared operational data, keys, user administration, other mutations |

All operational API reads and writes require an account when enabled. Public
surfaces are the static login page/assets, minimal status, setup status and login
(or legacy bootstrap). Mutation CSRF remains `X-CyClaw-CSRF`. The last enabled
administrator cannot be deleted, disabled, or demoted. Password reset, role
change, disabling, and deletion revoke that account's sessions. Audit request
records identify the actor and matched route, without passwords, session tokens,
query bodies, or keys; the auditor projection exposes only those redacted fields.

This is a shared local portal, not tenant isolation: chat sessions, goals, jobs,
run records, persona, notes, model selection and the public-page cache are shared
resources available to authorized administrators/operators. Research questions,
model answers and transient controller state belong to the initiating request
and account. Last fetched/injected web selections are scoped by persistent random
account identity, so deleting and recreating a username cannot inherit them.
Auth-off legacy use has one explicit local-owner scope. Private selections may
remain on disk after account deletion, but are inaccessible to replacement users.
Ordinary historical conversations can contain previously supplied web text.

## HTTPS and native trust

Both `serve` and the desktop sidecar use the same transport setup. Fresh generated
material is a unique P-256 key and certificate, persisted together atomically as
private `tls/server.pem` (0600, directory 0700). SANs cover `localhost`,
`127.0.0.1`, and `::1`. The material is reused across restarts and checked for
name, time validity and key consistency. Invalid/expired material refuses startup;
there is no HTTP fallback. Explicit `tls.enabled: false` is the legacy HTTP choice.

The native desktop receives the certificate DER and fingerprint over its owned
sidecar's private challenge/PID handshake. Readiness verifies that exact leaf
before sending HTTP. WKWebView validates its hostname/time/trust using that leaf
as the only per-connection anchor and rejects other certificates and origins.
No global validation switch or keychain/root installation is used. HTTP/2
`:authority` and HTTP/1 Host share validation, while scheme comes from trusted
listener metadata. Cookies use Secure under HTTPS, HttpOnly and SameSite=Strict;
logout clears with the same attributes. Public web fetches use ordinary public
TLS trust, and the local model's URL remains independently configured.

Print only the public certificate, or explicitly renew generated material:

```bash
./target/release/cgagentharness tls certificate > local-public.pem
# Stop this home's desktop/serve process before renewal:
./target/release/cgagentharness tls renew
```

Renewal acquires home ownership and refuses to overwrite operator-supplied paths.
To use an operator certificate, set both `tls.cert_file` and `tls.key_file` to
absolute paths or paths relative to the home. Supply a valid chain and private
key with the supported loopback SANs. The operator controls renewal. Existing
CLI sessions bound to the old certificate require login again after renewal.

External browsers do not automatically trust a generated certificate. Prefer the
native app or the CLI for zero system-trust changes. For an external browser,
supply an operator-managed certificate trusted by that browser, or deliberately
review/install the exported public certificate through that browser/OS's own
trust UI. The application never performs that installation. `curl --cacert
local-public.pem https://127.0.0.1:8790/api/status` trusts only the supplied file
for that command; do not use an invalid-certificate bypass.

## Terminal access

The public `account` and `web` command families call the same protected HTTP
handlers as the console. They never bypass login or mutate policy offline.
Set `CGAGENTHARNESS_HOME` to the intended absolute home when it differs from the
default. Default URL uses that home's scheme and configured port; pass the actual
origin with `--url` when using an explicit port or an ephemeral desktop listener.

```bash
./target/release/cgagentharness account --url https://127.0.0.1:8790 login admin
./target/release/cgagentharness account --url https://127.0.0.1:8790 password
./target/release/cgagentharness web status
./target/release/cgagentharness web allow 'https://example.com/docs/*' --group docs --seed https://example.com/docs/
./target/release/cgagentharness web allow https://example.com/robots.txt --group docs
./target/release/cgagentharness web fetch https://example.com/docs/start
./target/release/cgagentharness web search 'widget_open' --group docs
./target/release/cgagentharness web research 'How does widget_open fail?' --group docs
./target/release/cgagentharness web cancel
./target/release/cgagentharness web deny 'https://example.com/docs/*'
./target/release/cgagentharness account logout
```

Login reads one password; password replacement reads current and new passwords.
Terminal input is hidden on Unix; automation can supply stdin lines. Windows
interactive password entry requires a private stdin pipe. Never put secrets in
argv or paste them into chat. Private `cli-session.json` contains the local cookie,
bound to the exact origin and owned certificate. Logout revokes it and removes
the file. The CLI refuses foreign origins, redirects, proxies and certificate
substitution before sending account credentials.

## URL permission and migration

`tools/web_allowlist.json` is the authoritative whole-policy document (v1, 64 KiB,
32 rules). `/web allow` and terminal `web allow` create stable IDs and validate
source groups/seeds. Missing policy refuses reads; an explicit validated admin
allow command can initialize a missing policy with only that grant. Corrupt,
unsupported, unreadable or partially invalid files never silently reset.
Manual edits use the same strict parser; stop the server for coordinated file
owner edits, or use serialized atomic API/CLI mutations while it runs.

| Rule | Meaning |
|---|---|
| `https://example.com/article` | That canonical URL only, no children or additional query |
| `https://example.com/docs/*` | `/docs/` and descendants, excluding `/docs` |
| `https://example.com/*` | Paths and query strings on that exact HTTPS origin, including `/` |
| `https://*.example.com/docs/*` | Explicit subdomains below the named domain; apex excluded |
| `https://example.com/article?q=one` | Exact query identity/order; no arbitrary parameters |

Default ports normalize; schemes never downgrade. Fragments are stripped. IDNA
and hostname case normalize; trailing-dot hosts, credentials, alternate IP
spellings, dot segments, encoded path separators/dots/nested escapes, malformed escapes
and ambiguous authorities are refused. Encoded punctuation/space in query data
retains its identity; it cannot change the parsed host or path. Encoded controls
remain refused. Path wildcards include query strings, but never implicit `www`,
suffix impostors or alternate ports. Scheme-relative discovered links
resolve against the source and still require current permission.

Legacy rows migrate only to the actual stored URL targets the old fetcher could
request. A formerly implicit prefix becomes exact; add a wildcard and seed
explicitly to permit discovery. Unknown legacy fields/invalid rows fail the whole
migration. Reading legacy data normalizes it in memory; the next authorized
mutation persists v1. Legacy unproven `web_context.txt`/`web_last.json` are never
injected. Current selections use `tools/web_<account-hash>_{last,context}.json`.

## Google keyword search and chat tools

With web enabled, ordinary chat can invoke `web_search` (Google keyword listings)
and `web_fetch` (an exact permitted public URL). They use standard OpenAI tool
calls against the configured local model. No repository, shell, policy, account,
key or other mutation tool is exposed. `/loop` remains tool-free. Invalid names,
arguments, parallel calls and excess tool requests are refused. A turn allows
at most `web.chat_tool_calls` (default 3, range 1–5), shares the chat timeout,
and reserves estimated tokens against `web.total_tokens` before each model call.
Reported usage sums all completed model calls; absent upstream usage remains
marked unreported internally rather than being invented as actual token counts.
Cancellation aborts the whole turn, including an outstanding content request.

An administrator grants the generated search destination, normally:

```text
/web allow https://www.google.com/*
/web search veeam software cve
```

Natural language works in the same app: `Search Google for the top 5 results for
"veeam software cve"`. `/web search` defaults to Google in the console and CLI;
`/web pages` selects the existing permitted-page passage search. A source-group
argument retains the legacy page-search interpretation. For API compatibility,
`POST /api/web/search` without `engine` still means page search; explicitly send
`engine: "google"` and optional `count` (1–10, default 5) for Google listings.
Dedicated `/api/web/research` only accepts the page-search engine.

The **API Keys → Google results (SerpAPI)** field manages `SERPAPI_API_KEY` through
the existing private dotenv store. An active nonempty key selects the fixed
`https://serpapi.com/search.json` Google backend. This is a SerpAPI credential,
not a Google Custom Search/Cloud key. See [the provider's API](https://serpapi.com/search-api).
The API receives the chosen keyword query, engine, result count and its key;
it receives neither the full chat history nor harness account credentials.
The key appears only in the fixed provider's HTTPS request query as required by
that API, never in user-facing URLs, model context, returned metadata or error
messages. Responses reflecting the credential in extracted listings are refused.
Returned destination URLs never receive the key. The API client retains public
DNS pinning, shared concurrency, no proxy/redirect/retry/pooling, and finite
header/body/time bounds. Provider-key configuration and Google URL permission
are both required for this path; neither authorizes arbitrary destinations.

With no active key, the app requests the permitted public Google search URL and
parses recognizable organic result links in their returned order. It does not
execute JavaScript, solve CAPTCHA, adopt browser cookies, or silently switch
search engines. Challenges, redirects, unreadable output and transport failures
are explicit failures. A keyed request's invalid-key, quota or upstream failure
also stays explicit; public fallback occurs only when the active key is absent.
Saving/clearing keys takes effect after restart; process environment overrides
still apply. A test on this Mac returned a Google JavaScript challenge, so
credential-free success is not promised.

The console renders actual provider/source information separately from the
model answer. Listings and snippets are attributed to the search provider;
linked pages have not been fetched. A follow-up `web_fetch` must independently
pass the current allowlist and account checks. Permission is rechecked before
model consumption and final delivery. Provider listings are not inserted into
the page cache or saved `/web inject` selection. Fetched pages reuse the existing
cache and current-account selection. Previously recorded chat cannot be erased
by later revocation. Model answers are not validated research citations; use
dedicated `/web research` when checked quote references are required.

## Fetch, discovery and research

Fresh web settings are enabled. Existing explicit true/false settings are
preserved; absent or invalid legacy `web_enabled` fields remain off. Use `/web on`
(or terminal `web on`) to enable a previously disabled setting. URL permission
remains required regardless of this switch. In chat:

```text
/web allow https://example.com/docs/* docs https://example.com/docs/
/web allow https://example.com/robots.txt docs
/web pages group=docs widget_open
/web research group=docs How does widget_open fail?
/web cancel
/web inject
/web forget
```

Wildcards are permissions, not crawl targets. Explicit URLs and configured seeds
start bounded traversal. Robots is fetched only if separately permitted by the
same content policy. Without permitted/readable robots, explicit seeds remain
readable but discovered-link traversal stops. Relative links are parsed from HTML,
deduplicated, paced per origin and restricted to the selected group. Robots
responses consume request budgets but never become indexed evidence. Optional
sitemap discovery is not implemented. Dedicated permitted-page research needs
no provider credentials and has no cloud-model fallback. Google listings are a
separate search mode described below.

Every content request resolves once, rejects any non-public/mixed address set,
and pins only the checked addresses to the actual connection while preserving
TLS hostname validation. Connection reuse, resolver fallback, retries, proxies,
redirects, compression and cookies cannot broaden that request. The exact child
URL is fetched. Bounds cover queueing, DNS, headers, body, time and cancellation.
The HTML parser removes executable/hidden/navigation content and retains headings,
paragraphs, tables, lists and code. Extracts keep canonical URLs, hashes, fetch
age, validators, extraction version and original passage offsets.

Defaults: 20 page requests, 2 MiB charged bytes, 60 seconds per crawl; 8 seconds
and 256 KiB per response; 2 concurrent fetches, 250 ms per-origin pacing, 10
requests per origin; 128 cached pages/16 MiB. `web.*` fields in the shipped YAML
list validated bounds. Failed requests can conservatively charge the full
response allowance. Reports expose partial coverage, failures and unvisited work.

Tantivy rebuilds a bounded in-memory BM25 index from permitted cached extracts.
Literal phrases/identifiers and modest title/heading boosts improve ranking;
source diversity and passage deduplication avoid repeated evidence. Cache corruption
cannot authorize reads; the cache is derived and rebuildable. Conditional refresh
uses the same network checks. Search currently refreshes each bounded source set,
so validators reduce transferred bytes but do not eliminate request count.

Dedicated research searches the index, asks the configured local model for at
most 3 additional subqueries in 2 rounds, performs at most one bounded crawl,
and synthesizes at most about 3,000 evidence tokens. Default total model allowance
is 16,000 tokens and deadline 300 seconds. Provider-reported usage is counted;
otherwise UTF-8 bytes/4 plus overhead is explicitly estimated, with incomplete
output charged conservatively. Model output is bounded JSON, never executable
commands or privileged tools. Original question plus focused subqueries are
interleaved during retrieval; no new agent framework, embeddings or reranker.

Answers distinguish supported claims, conflicts, inferences and missing/stale
evidence. Citation IDs and exact quotes are checked against original passages;
semantic support still requires judgment. Rank is not truth confidence. Fetch
age is not publication-date verification. Bounded runs never assert completeness.
No-answer results abstain, and planner/answer failures remain visible in warnings.

Current policy is rechecked on dispatch, cache/index/context reads, storage and
result delivery. Revocation discards in-flight evidence and makes revoked cached
sources inaccessible, even through overlapping rules. Research also rechecks the
initiating account before model calls and delivery. Revocation cannot erase
already delivered historical conversation content or a previous model request.
The URL policy applies only to this web subsystem, not the OS or unrelated
model/GitHub connections; content, provider and account permissions are separate.

## API Keys

The left tab is **API Keys**; `/registry` and the panel's registry button preserve
inventory access. Its rows come from `env_keys::MANAGED_KEYS`, the existing list
of values actually consumed by the binary. Administrators can paste/save/replace/
clear saved values. GET/POST responses contain only masks and status. Save and
clear use serialized atomic private dotenv updates, preserving unrelated lines;
unsafe or unreadable files are refused. Values are never stored in SQLite.

Saved and active masks are separate. Explicit process environment values override
saved credentials, including after restart. Clearing a saved key does not erase
an inherited value or change a running provider. Restart to reload stored values;
remove an external environment override separately if that is the intent. Missing
provider credentials affect only the selected provider's operation. The optional
harness metadata key is never an account credential.

## Reproducible evidence

`cargo test --locked --all-targets --all-features` covers policy, pinned fetch,
revocation, accounts, TLS, RBAC and the existing coding invariants. Native trust
unit tests and actual WKWebView acceptance are separate evidence. The deterministic
browser suite is `node scripts/chat-browser-acceptance.mjs` (mock API, real Chrome).
`python3 scripts/test-desktop-backend.py` exercises both real serving entrypoints.

```bash
cargo run --locked --example web_research_benchmark -- /absolute/report.json
cargo run --locked --example web_research_benchmark -- /absolute/live-report.json --live
```

The fixed corpus has nested pages, two sites, identifiers, paraphrases, stale
and contradictory claims, no answer, and hostile text. The old literal baseline
measures matching only on roots; its request count is an algorithmic count, not
an old-network latency measurement. The live mode uses an already installed local
model and records all results/usage. It performs no model download or publication.

### Recorded local-model evaluation (September 12, 2026)

The fixed 13-page corpus uses two virtual sites on an owned local HTTP fixture,
with twelve permitted pages and one robots-disallowed private page. The installed
`qwen3.8:27b-mlx` model produced the results below; its tag does not prove its
inference backend. These measurements precede the final compatible lockfile sync;
they are a reproducible evaluation sample, not a release-wide quality guarantee.

| Case | Expected-source recall: roots / passages | Retrieved-source precision | Exact citation references | Total model tokens |
|---|---|---|---|---|
| Nested page | 0 / 1 | 0.25 | 2/2 | 1,275 |
| Identifier | 0 / 1 | 1.00 | 3/3 | 1,021 |
| Paraphrase | 0 / 1 | 0.20 | 2/2 | 1,517 |
| Same-version contradiction | 0 / 1 | 0.67 | 4/4 | 1,229 |
| Stale source | 0 / 1 | 0.40 | 5/5 | 2,180 |
| No answer | Abstained | No relevant source expected | No claims | 515 |
| Navigation/hostile noise | Abstained despite irrelevant retrieval | 0 | No claims | 696 |

All five answerable cases found their expected sources; 16/16 citation IDs and
quotes matched original passages. That mechanical check does not prove semantic
truth. Broader queries still retrieve irrelevant sources. Two invalid planning
responses were surfaced in the no-answer case; synthesis abstained. The stale
case retained one explicitly flagged old passage after a failed refresh.

Total reported model usage was 8,433 tokens. Median search latency was 934 ms;
median research latency was 11,908 ms, with one research case taking 52,458 ms.
Search made 98 fixture requests across seven queries; research expansion made
56 more. Conditional requests save transferred bytes, not these request counts.
The root literal baseline measured matching only and counted two hypothetical
root requests per query; it did not measure old-network latency or model usage.
No speedup, token-saving or broad accuracy claim follows from this sample.
