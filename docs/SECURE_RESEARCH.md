# Accounts, HTTPS and permitted web research

Fresh homes enable HTTPS, role authentication, and web operations. Web content
needs an explicit URL grant, and the allowlist starts empty.
Upgrades preserve `config.yaml`; missing legacy auth or TLS switches remain off.
To adopt the defaults, merge these literal booleans into the active mappings,
then restart the server:

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

Malformed auth/TLS booleans refuse configuration, preserving protection.
The old `security.api_key_optional: false` mode is ignored. Upgrade such homes
with `auth.enabled: true` to require account access.
A stale harness key neither blocks a valid account nor grants access.

## Accounts and roles

### First login and passwords

Only a fresh account store creates `admin` / `admin`. Its password uses the
normal scrypt hash with a bootstrap exception. Sign in and replace it with at
least 12 characters. Until then, the account can inspect its identity, change
its password, or log out, but cannot use chat, research, coding, administration,
or keys. Replacement checks the current password and CSRF token, then invalidates
old sessions. Administrative resets retain the ordinary password policy.

Every role can use **change password** with its current password. An administrator
uses **USERS** (`/users`) to create users or reset passwords. Password reset,
role change, disable, and deletion revoke that account's sessions. The last
enabled administrator cannot be deleted, disabled, or demoted. **logout** revokes
the session and clears private displayed state.

Console authentication probes have a five-second deadline; login, legacy
bootstrap, logout, and Users panel requests have a fifteen-second deadline.
These cover the complete response body as well as headers, so a stalled JSON
reply cannot leave the operation waiting indefinitely. A timeout does not prove
that a submitted account change was rejected; refresh before retrying it.

### Roles

| Role | Allowed | Refused |
|---|---|---|
| Administrator (`admin`) | Normal harness work; users, roles, disable/reset; global credentials; web enablement and URL policy | Every operation lacking existing repository/write/approval/publication authorization |
| Portal operator (`operator`) | Chat; enabled permitted fetch/search/research; own sessions/jobs/schedules, shared notes/persona and authorized coding workflows; own password/logout | Users/roles, global keys, web enablement/allowlist changes, provider security configuration |
| Auditor (`audit`) | Minimal status, designated redacted `/api/audit`, own password/logout | Chat/research/jobs, shared operational data, keys, user administration, other mutations |

The account bar displays the signed-in role. An Auditor sees a permission refusal
in **Sessions** and a read-only `/status` with minimal details; chat, web, users
and API Keys remain unavailable to that role. This presentation does not grant
additional backend permissions. Audit request records identify the actor and
matched route, without passwords, session tokens, query bodies, or keys; the
auditor projection exposes only those redacted fields.

### API Keys

The left tab is **API Keys**. Its rows come from `env_keys::MANAGED_KEYS`, the
existing list of values actually consumed by the binary; [supported managed
keys](INSTALL.md#supported-managed-keys) lists them and explains saved/active
masks, restarts and environment precedence. Administrators can
paste/save/replace/clear saved values. GET/POST responses contain only masks and
status. Save and clear use the OS credential store and fail closed when it is
unavailable. Literal `security.allow_plaintext_key_file: true` keeps the
[private legacy file](INSTALL.md#8-persistence-optional-keys-and-recovery). Values are never stored in SQLite. Missing provider credentials affect only the selected provider's
operation. Leave `GH_TOKEN` unset to use existing `gh auth login`. Completion
webhook bearers are configured separately; see the
[webhook guide](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook).

### Account storage and recovery

Versioned `auth.sqlite3` stores accounts and hashed session tokens with
transactional updates. Private `auth.initialized` prevents a missing database
from becoming a fresh default-password bootstrap.
A valid legacy `auth.json` migrates its user hashes, roles, disabled state,
lockouts, and timestamps; `auth.json.pre-sqlite` preserves the exact private
recovery copy. Legacy sessions require a new login. The original JSON remains a
recovery artifact, not the live store. A legacy pending-password account retains
its existing setup flow. Corrupt, empty, unsafe, or unsupported initialized stores
refuse startup; no automatic reset occurs. Failed writes do not publish successful
in-memory account changes. Stop the server before an operator-controlled backup
or recovery; preserve the database and initialization marker together, and do
not delete them to recreate `admin` / `admin`.

Each scrypt derivation uses about 128 MiB. `auth.max_concurrent_operations`
(default 2, range 1–4) bounds login and password work; extra attempts wait.
Sessions expire after `auth.session.idle_timeout_sec` (default 43200) without
use and `auth.session.absolute_timeout_sec` (default 604800) regardless. Chat
session files under `sessions/` are written atomically with mode 0600 because
history can carry pasted secrets.

Structured memory's separate `memory/structured.sqlite3` uses the same class of
open policy (owned parent, `SQLITE_OPEN_NOFOLLOW`, `foreign_keys=ON`,
`trusted_schema=OFF`, rollback journal, `synchronous=FULL`). Missing initialized
or corrupt files refuse startup rather than bootstrapping. Its gates and at-rest
limits are in [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md).

### Session ownership and legacy adoption

New sessions use schema version 1 and the account's random `user_id`. Listing,
loading, search, export, chat, prompt preview, goals, skills, and style enforce
that owner. Administrators do not inherit other accounts' sessions. Auth-disabled
homes use owner `local`; enabling auth does not reassign that data.

Sessions without an owner are unassigned legacy shared data. They remain on disk,
are hidden from ordinary reads/search/export, and survive **Clear my session
history**. In **Sessions**, an administrator can choose **Review unassigned legacy
sessions (admin)**, inspect metadata, enter a reason, confirm authorization and
choose **Adopt into my account**. Adoption preserves the transcript and old blob
pin owners but clears the prior goal-stage coding approval. The administrator
must stage and review coding work again. There is no implicit sharing or account
reassignment. Unknown-schema, unreadable and interrupted staged files remain for
manual recovery; clearing history cannot invent their owner.

Detached job and schedule management uses the same owner boundary. Old ownerless
job records remain on disk outside account APIs. Schedule dispatch rechecks the
current account role and disabled/password-change state; revocation stops later
occurrences. Account deletion does not reassign retained records to a newly
created account with the same username.

## HTTPS and native trust

Both `serve` and the desktop sidecar use the same transport setup. Fresh generated
material is a unique P-256 key and certificate, persisted together atomically as
private `tls/server.pem` (0600, directory 0700). SANs cover `localhost`,
`127.0.0.1`, and `::1`. The material is reused across restarts and checked for
name, time validity and key consistency. Invalid/expired material refuses startup;
there is no HTTP fallback. Explicit `tls.enabled: false` is the legacy HTTP choice.
The loopback HTTPS listener accepts TLS 1.3 only. This policy is explicit on the
server configuration, independent of dependency feature unification. Outbound
public-web clients retain their existing verified TLS compatibility.

The desktop receives certificate DER and fingerprint through its owned sidecar's
challenge and PID handshake. Readiness verifies that leaf before HTTP. WKWebView
uses it as the sole connection anchor with hostname, time and trust checks.
The server checks origin, HTTP/2 `:authority` and HTTP/1 Host, taking the scheme
from trusted listener metadata. The app changes no global trust. HTTPS
cookies are Secure, HttpOnly, and SameSite=Strict; logout uses the same attributes.
Public fetch uses public trust, and the model URL remains separate.

Print only the public certificate, or explicitly renew generated material:

```bash
./target/release/cgagentharness tls certificate > local-public.pem
# Stop this home's desktop/serve process before renewal:
./target/release/cgagentharness tls renew
```

Renewal acquires home ownership and preserves operator-supplied paths. For an
operator certificate, set both `tls.cert_file` and `tls.key_file` to absolute or
home-relative paths, with a valid chain, key, and loopback SANs. The operator
renews it. Old certificate-bound CLI sessions must log in again.

Browsers do not trust generated certificates automatically. Use the native app
or CLI, supply a browser-trusted operator certificate, or review and install the
exported public certificate through the OS or browser UI. The app never installs
it. `curl --cacert local-public.pem https://127.0.0.1:8790/api/status` trusts only
that file for one command. Never bypass certificate validation.

## Terminal access

Public `account` and `web` commands call the console's protected HTTP handlers,
so the home server must run. They cannot bypass login or mutate policy offline.
Set an absolute `CGAGENTHARNESS_HOME` when needed. For an explicit port or desktop
listener, pass its origin with `--url` after `account` or `web`.

```bash
./target/release/cgagentharness account --url https://127.0.0.1:8790 login admin
./target/release/cgagentharness account --url https://127.0.0.1:8790 password
./target/release/cgagentharness web status
./target/release/cgagentharness web allow 'https://example.com/docs/*' --group docs --seed https://example.com/docs/
./target/release/cgagentharness web allow https://example.com/robots.txt --group docs
./target/release/cgagentharness web fetch https://example.com/docs/start
./target/release/cgagentharness web search 'veeam software cve' --count 5
./target/release/cgagentharness web search 'widget_open' --group docs
./target/release/cgagentharness web research 'How does widget_open fail?' --group docs
./target/release/cgagentharness web cancel
./target/release/cgagentharness web deny 'https://example.com/docs/*'
./target/release/cgagentharness account logout
```

Login reads one password; replacement reads current and new passwords. Unix hides
terminal input; automation may supply stdin. Windows interaction needs a private
stdin pipe. Never put secrets in argv or chat. Private `cli-session.json` binds
the cookie to the origin and certificate; logout revokes and removes it. Before
sending credentials, the CLI refuses foreign origins, redirects, proxies, and
certificate substitution.

## Web permissions

Explicit web settings persist; absent/invalid legacy `web_enabled` stays off.
`/web on|off` (also terminal `web on|off`) controls future reads/injection;
URL grants remain required. Enablement/allowlist are **shared-home** settings;
saved selections are account-private. `/web status` shows groups/seeds;
`/web check URL` checks exact permission without network access.

`tools/web_allowlist.json` is the v1 policy (64 KiB, 32 rules). `/web allow`
and terminal `web allow` validate groups/seeds and assign stable IDs. Batches
write atomically. Missing policy refuses reads; a validated admin grant can
initialize it with that rule only. Invalid/unreadable files never reset.
Stop the server before manual edits; otherwise use serialized atomic API/CLI mutations.

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
remain refused. Path wildcards include query strings, but never suffix impostors
or alternate ports. `www.example.org` and `example.org` are distinct; neither is
inferred from the other. Scheme-relative discovered links resolve against the
source and still require current permission.

HTML extraction shares the request deadline and stops cooperatively on cancellation.
`web.html_parser_handles` (default 512, range 64–4096, restart-only) bounds live
open-element and formatting state; excessive nesting returns `WEB_HTML_COMPLEXITY`
without saving partial evidence. Byte and link limits still apply.

Each rule belongs to one source group, `default` unless `--group NAME` names
another. Repeatable `--seed URL` records concrete starting pages; each seed must
fit a pattern in the same grant. `--group` on `check`, `fetch`, `pages`, `search`
or `research` limits that request to the group's rules; without it, any rule can
permit. `/web deny` removes a rule ID or matching pattern across groups.

Legacy rows migrate only to the actual stored URL targets the old fetcher could
request. A formerly implicit prefix becomes exact; add an explicit wildcard to
permit discovery (a host wildcard also needs a concrete seed). Unknown legacy fields/invalid rows fail the whole
migration. Reading legacy data normalizes it in memory; the next authorized
mutation persists v1. Legacy unproven `web_context.txt`/`web_last.json` are never
injected. Current selections use `tools/web_<account-hash>_{last,context}.json`.

## Search, fetch and research

The app and browser console share the backend. Enter slash commands as plain text
starting with `/`. Google needs a [SerpAPI key or Google grant](#google-search).
To read the example page, an administrator first grants it:

```text
/web allow https://doc.rust-lang.org/book/ch01-01-installation.html
```

Now ask in ordinary chat:

```text
Search Google for the top 5 results for "veeam software cve".
Read https://doc.rust-lang.org/book/ch01-01-installation.html and tell me which command checks the installed Rust compiler version.
```

Direct commands:

```text
/web search --count 3 veeam software cve
/web fetch https://doc.rust-lang.org/book/ch01-01-installation.html
/web allow https://www.veeam.com/* https://example.org/* --group vendors
/web check https://www.veeam.com/ https://example.org/ --group vendors
/web fetch https://www.veeam.com/ https://example.org/ --group vendors
/web research --group vendors Compare the backup approaches
/web research --url https://www.veeam.com/ Summarize products and permitted internal links
/web allow https://example.com/docs/* https://example.com/robots.txt --group docs
/web pages --group docs widget_open
/web research --group docs How does widget_open fail?
/web cancel
/web inject
/web forget
```

Double-quoted query phrases retain their exact-phrase meaning. Use `--` before
query text that resembles flags, such as `/web search -- --help`, or single-quote
that literal argument. Other flag rules are in the
[slash-command quick reference](CONSOLE.md#78-slash-command-quick-reference).

### Chat web tools

With web enabled, ordinary chat can invoke `web_search` (Google keyword listings)
and `web_fetch` (an exact permitted public URL). They use OpenAI-compatible tool
calls, which the configured local model must support; it receives the retrieved
content. No repository, shell, policy, account, key or other mutation tool is
exposed. `/loop` remains tool-free. Invalid names,
arguments and excess tool requests are refused; batches are validated before
their first read and executed sequentially. A turn allows at most
`web.chat_tool_calls` (1–10, default 10) across all rounds, shares the chat
timeout, and reserves estimated tokens against `web.total_tokens` before each
model call. A tool call resends the prompt with its results, so tools are offered
only while both calls, both replies and a minimal result (≥256 calibrated tokens)
fit. A batch without that minimum per call runs no read; the model answers
without tools. Results are cut to the room left, keeping that minimum per later
call: pages keep their longest prefix, listings drop trailing results. Withheld
tools report `WEB_TOKEN_BUDGET`. Chat starts with a focused contextual query,
reuses duplicate searches, and stops when the evidence answers the question.
Reported usage sums all completed model calls; absent upstream usage stays marked
unreported, never invented.
`/loop stop` or the chat cancellation endpoint aborts the whole turn, including
an outstanding content request. Direct chat uses `POST /api/chat` and returns
`web_tools` alongside the reply and aggregate usage.

Existing homes keep explicitly configured smaller call budgets. To use ten there,
set `web.chat_tool_calls: 10` in the active home's `config.yaml` and use
**Reload limits** (or restart). Raising the call ceiling does not raise token,
time, byte, or URL-permission limits.

The console separates provider and source data from the model answer. Tool calls
show links, outcomes, and failures. Listings and snippets come from the search
provider; linked pages have not been fetched. Model answers are not validated
research citations; use `/web research` when checked quote references are
required.

### Google search

Open **API Keys** in the left pane, paste a **SerpAPI** key into **Google results
(SerpAPI)**, save, then click **Test Google search**. This is a SerpAPI
Google-results key, not a Google Cloud/Custom Search key; obtain it through
[SerpAPI](https://serpapi.com/search-api). No key belongs in the chat transcript
or `config.yaml`. The field manages `SERPAPI_API_KEY` in the OS credential store
(legacy `.env` only when the plaintext opt-in is literal true). Saving/clearing
it applies to the next search immediately; process environment overrides still apply.

An active nonempty key selects the fixed `https://serpapi.com/search.json` Google
backend. The API receives the chosen keyword query, engine, result count and its key;
it receives neither the full chat history nor harness account credentials.
The key appears only in the fixed provider's HTTPS request query, never in
user-facing URLs, model context, returned metadata or error messages. Responses reflecting the credential in extracted listings are refused.
Returned destination URLs never receive the key. The API client retains public
DNS pinning, shared concurrency, no proxy/redirect/retry/pooling, and finite
header/body/time bounds. With web enabled, SerpAPI listings need no Google URL
grant.

With no active key, the app requests the permitted public Google search URL and
parses recognizable organic result links in their returned order. It does not
execute JavaScript, solve CAPTCHA, adopt browser cookies, manufacture missing
results, or silently switch search engines. Challenges, redirects, unreadable
output and transport failures are explicit failures, so credential-free success
is not promised. A keyed request's invalid-key, quota or upstream failure also
stays explicit; public fallback occurs only when the active key is absent. For
that keyless fallback only, an administrator first grants the Google page:

```text
/web allow https://www.google.com/*
/web search veeam software cve
```

The exact generated search URL also works; an exact homepage grant is not enough.
`https://google.com/*` does not grant `www`; the apex endpoint may redirect and
redirects remain refused. Without a Google grant, an empty policy gives
`WEB_ALLOWLIST_EMPTY` and a non-Google policy gives `WEB_GOOGLE_PERMISSION`.

`/web search` defaults to Google in the console and CLI; `/web pages` selects the
existing permitted-page passage search, as does a source-group argument
(including the legacy `/web search group=docs ...` form). For API compatibility,
`POST /api/web/search` without `engine` still means page search; explicitly send
`engine: "google"` and optional `count` (1–10, default 5) for Google listings.
Dedicated `/api/web/research` only accepts the page-search engine.

### Fetch, discovery and research

Fetch accepts exact URLs only; wildcard patterns belong in `allow`, not `fetch`.
A `fetch` batch validates all targets before any request and shares page, byte,
time and per-site limits. Partial reads show coverage/failures rather than
claiming complete success.

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
list validated bounds; [configuration reload](CONFIG_RELOAD.md) lists the ones an
administrator can reload without a restart. Failed requests can conservatively
charge the full response allowance. Reports expose partial coverage, failures and
unvisited work.

For `research`, repeat `--url` for permitted starts. Starts narrow one run but
grant nothing. Without them, traversal begins at group seeds and permitted
literal prefixes such as `/docs/`; host wildcards need concrete seeds. Every run
attempts bounded discovery. Robots is fetched only with content permission. If
unreadable, seeds remain readable but link traversal stops. HTML links are
deduplicated, paced, and group-restricted. Robots consumes requests but supplies
no evidence. Sitemap discovery is absent. Page research needs no provider key
and has no cloud fallback.

Tantivy rebuilds a bounded in-memory BM25 index from permitted cached extracts.
Phrases, identifiers, title and heading boosts improve ranking; diversity and
deduplication reduce repetition. Derived cache corruption grants nothing.
Conditional refresh repeats network checks. Validators reduce bytes, not requests.

Research asks the local model for at most 3 subqueries in 2 rounds, runs at most
one bounded crawl, and synthesizes about 6,000 evidence tokens. Defaults are
28,000 total model tokens and 300 seconds. Without a verified 32768 context,
configure `web.total_tokens: 16000` and `web.evidence_tokens: 3000`; see
[MODELS.md](MODELS.md#4-select-an-installed-model-and-check-ollama). Provider
usage wins; otherwise bytes/4 plus overhead is estimated, charging incomplete
output conservatively. Bounded JSON output has no commands or privileged tools.
Retrieval interleaves the question and subqueries without embeddings or reranking.
`/web cancel` cancels dedicated research.

Answers label support, conflicts, inference, and missing or stale evidence.
Citation IDs and quotes are checked against passages, but semantic support still
needs judgment. Rank is not truth, and fetch age is not publication date. Bounded
runs never claim completeness; no-answer results abstain and failures stay visible.

### Saved web selection

`/web inject` selects the last fetched or page-search extract, or the first
successful batch page, for local chat. It stores a bounded excerpt, not a summary.
`/prompt` previews it; `/web forget` clears selection without altering old chats.
Google listings never enter the page cache or selection. `/web research` is a
request-scoped multi-source synthesis and does not replace selection, although
its fetched pages reuse the cache.

## Security boundaries

When auth is enabled, all operational API reads and writes require an account.
Public endpoints are static login assets, minimal status, setup status, and login
or legacy bootstrap. Mutation CSRF remains `X-CyClaw-CSRF`. Forwarding headers
are refused, and reverse proxies are unsupported. The dedicated
[machine gateway](MCP_SERVER.md) does not inherit console authority.

Sessions, transcript search/export, jobs, and schedules are account scoped; see
[session ownership](#session-ownership-and-legacy-adoption). Run records, spend,
persona, pinned notes, model selection, and public-page cache are shared among
authorized operators and administrators. This is not universal tenant isolation.
Research questions, answers, controller state, and web selections belong to the
initiating random account identity, so a recreated username inherits nothing. Auth-off mode uses owner `local`. Deleted
accounts can leave private selections on disk, inaccessible to replacements.

A non-empty allowlist is an armed content surface: every granted origin or path
is reachable by chat `web_fetch`, still bounded by `web.chat_tool_calls`. Fresh
homes refuse page fetching with `WEB_ALLOWLIST_EMPTY`. Search listings never
authorize their destinations: a follow-up `web_fetch` must independently pass the
current allowlist and account checks.

Policy is rechecked at dispatch, cache/index/context reads, storage, model use,
and delivery. Revocation discards in-flight evidence and blocks revoked cache,
even under overlapping rules. Research rechecks the account before model calls
and delivery. It cannot erase earlier conversation text or model requests. URL
policy covers only web, not OS, model, or GitHub connections. Content, provider,
and account permissions remain separate.

## Reproducible evidence

The [test suite](INSTALL.md#tests-and-cicd) covers policy, pinned fetch,
revocation, accounts, TLS, RBAC and the existing coding invariants.
`tests/chat_web.rs` pins the chat search and page-permission contracts above, and
`scripts/test-desktop-backend.py` exercises both real serving entrypoints. Native
trust unit tests and actual WKWebView acceptance are separate evidence.

```bash
cargo run --locked --example web_research_benchmark -- /absolute/report.json
cargo run --locked --example web_research_benchmark -- /absolute/live-report.json --live
```

The fixed corpus has nested pages, two sites, identifiers, paraphrases, stale
and contradictory claims, no answer, and hostile text. The old literal baseline
measures matching only on roots; its request count is an algorithmic count, not
an old-network latency measurement. The live mode uses an already installed local
model and records all results/usage. It performs no model download or publication.
