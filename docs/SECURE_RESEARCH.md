# Accounts, HTTPS and permitted web research

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
access.

## Accounts and roles

### First login and passwords

Only a fresh account store creates `admin` / `admin`. That short initial password
uses the normal scrypt hash with a narrowly scoped bootstrap exception. Sign in;
the login dialog then requires a replacement password of at least 12 characters.
Until replacement, the account can inspect its identity, change its own password, or log out; it
cannot run chat, research, coding, account administration, or key management.
Replacement checks the current password and CSRF token and invalidates all old
sessions. Normal administrative resets retain the ordinary password policy.

Every role can change its own password: use **change password** in the account
bar and provide the current password. An administrator creates users through
**USERS** (`/users`) and can reset another user's password. Password reset, role
change, disabling, and deletion revoke that account's sessions. The last enabled
administrator cannot be deleted, disabled, or demoted. **logout** revokes this
session and clears private displayed state; sign in again to resume.

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
status. Save and clear use serialized atomic private dotenv updates, preserving
unrelated lines; unsafe or unreadable files are refused. Values are never stored
in SQLite. Missing provider credentials affect only the selected provider's
operation. Leave `GH_TOKEN` unset to use existing `gh auth login`. Completion
webhook bearers are configured separately; see the
[webhook guide](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook).

### Account storage and recovery

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
or recovery; preserve the database and initialization marker together, and do
not delete them to recreate `admin` / `admin`.

Each scrypt derivation uses about 128 MiB, so login and password operations
are bounded by `auth.max_concurrent_operations` (default 2, accepted range 1–4);
extra concurrent attempts wait for a permit rather than exhausting memory.
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

New sessions use schema version 1 and the authenticated account's random `user_id`.
Listing, loading, search, export, chat, prompt preview, goals, skills and style
selection enforce that owner. An administrator does not automatically inherit
another account's sessions. Auth-disabled homes use the explicit `local` owner;
enabling authentication does not silently move that data into a human account.

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
handlers as the console, so this home's server must be running. They never bypass
login or mutate policy offline. Set `CGAGENTHARNESS_HOME` to the intended absolute
home when it differs from the default. Default URL uses that home's scheme and
configured port; pass the actual origin with `--url`, after `account` or `web`,
when using an explicit port or an ephemeral desktop listener.

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

Login reads one password; password replacement reads current and new passwords.
Terminal input is hidden on Unix; automation can supply stdin lines. Windows
interactive password entry requires a private stdin pipe. Never put secrets in
argv or paste them into chat. Private `cli-session.json` contains the local cookie,
bound to the exact origin and owned certificate. Logout revokes it and removes
the file. The CLI refuses foreign origins, redirects, proxies and certificate
substitution before sending account credentials.

## Web permissions

Existing explicit web true/false settings are preserved; absent or invalid legacy
`web_enabled` fields remain off. Use `/web on` (or terminal `web on`) to enable a
previously disabled setting; `/web off` suppresses future reads and injection.
URL permission remains required regardless of this switch. Web enablement and
allowlist rules are **shared by this home, not per session**; only saved web
selections are account-private. `/web status` shows groups and seeds; `/web check
URL` diagnoses exact permission without DNS or network access.

`tools/web_allowlist.json` is the authoritative whole-policy document (v1, 64 KiB,
32 rules). `/web allow` and terminal `web allow` create stable IDs and validate
source groups/seeds. An `allow` batch writes once or not at all. Missing policy
refuses reads; an explicit validated admin allow command can initialize a missing
policy with only that grant. Corrupt, unsupported, unreadable or partially invalid
files never silently reset. Manual edits use the same strict parser; stop the
server for coordinated file owner edits, or use serialized atomic API/CLI
mutations while it runs.

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

The bundled app and browser console use the same backend; no terminal is needed.
Enter slash commands as plain text starting with `/`, without Markdown backticks.
Google search needs a [SerpAPI key or a Google grant](#google-search). To read
the example page, an administrator first grants it:

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

The console renders actual provider/source information separately from the
model answer; successful tool calls show source links and outcomes, and failures
are shown explicitly. Listings and snippets are attributed to the search
provider; linked pages have not been fetched. Model answers are not validated
research citations; use `/web research` when checked quote references are
required.

### Google search

Open **API Keys** in the left pane, paste a **SerpAPI** key into **Google results
(SerpAPI)**, save, then click **Test Google search**. This is a SerpAPI
Google-results key, not a Google Cloud/Custom Search key; obtain it through
[SerpAPI](https://serpapi.com/search-api). No key belongs in the chat transcript
or `config.yaml`. The field manages `SERPAPI_API_KEY` through the existing private
dotenv store. Saving/clearing it applies to the next search immediately; process
environment overrides still apply.

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

For `research`, repeat `--url` to start from multiple permitted pages/sites.
Starts narrow discovery and evidence for that run, never grant access. Without
explicit starts, the selected group's seeds and the permitted literal prefix of a
concrete-host path wildcard (for example `/docs/`) start bounded traversal. Host
wildcards require explicit concrete seeds. Fresh research always attempts bounded
discovery, even with cached matches. Robots is fetched only if permitted by the
same content policy. Without permitted/readable robots, explicit seeds remain
readable but discovered-link traversal stops. Relative links are parsed from HTML,
deduplicated, paced per origin and restricted to the selected group. Robots
responses consume request budgets but never become indexed evidence. Optional
sitemap discovery is not implemented. Dedicated permitted-page research needs
no provider credentials and has no cloud-model fallback.

Tantivy rebuilds a bounded in-memory BM25 index from permitted cached extracts.
Literal phrases/identifiers and modest title/heading boosts improve ranking;
source diversity and passage deduplication avoid repeated evidence. Cache corruption
cannot authorize reads; the cache is derived and rebuildable. Conditional refresh
uses the same network checks. Search currently refreshes each bounded source set,
so validators reduce transferred bytes but do not eliminate request count.

Dedicated research searches the index, asks the configured local model for at
most 3 additional subqueries in 2 rounds, performs at most one bounded crawl,
and synthesizes at most about 6,000 evidence tokens. Default total model allowance
is 28,000 tokens and deadline 300 seconds. (A constrained window that cannot reach
`OLLAMA_CONTEXT_LENGTH=32768` should instead configure `web.total_tokens: 16000`
and `web.evidence_tokens: 3000`; see [MODELS.md](MODELS.md#4-select-an-installed-model-and-check-ollama).)
Provider-reported usage is counted;
otherwise UTF-8 bytes/4 plus overhead is explicitly estimated, with incomplete
output charged conservatively. Model output is bounded JSON, never executable
commands or privileged tools. Original question plus focused subqueries are
interleaved during retrieval; no new agent framework, embeddings or reranker.
`/web cancel` cancels dedicated research.

Answers distinguish supported claims, conflicts, inferences and missing/stale
evidence. Citation IDs and exact quotes are checked against original passages;
semantic support still requires judgment. Rank is not truth confidence. Fetch
age is not publication-date verification. Bounded runs never assert completeness.
No-answer results abstain, and planner/answer failures remain visible in warnings.

### Saved web selection

`/web inject` explicitly selects the last fetched/page-search extract (the first
successful page of a batch fetch) for later local chat. It adds a bounded source
excerpt, not a summary; `/prompt` previews it and `/web forget` clears the current
account's selection without erasing previously recorded conversations. Google
listings are not inserted into the page cache or that saved selection, and
`/web research` does not replace it; fetched pages reuse the existing cache and
current-account selection. Research is a separate request-scoped, multi-source
synthesis; do not inject afterward expecting its entire answer or all sources to
be selected.

## Security boundaries

All operational API reads and writes require an account when enabled. Public
surfaces are the static login page/assets, minimal status, setup status and login
(or legacy bootstrap). Mutation CSRF remains `X-CyClaw-CSRF`. Forwarding headers
are refused; a reverse proxy is unsupported. The dedicated
[machine gateway](MCP_SERVER.md) does not inherit console authority.

Sessions, transcript search/export, detached jobs and schedule management are
account scoped; see [session ownership](#session-ownership-and-legacy-adoption).
Agentic run records, aggregate spend, persona, pinned notes, model selection and
the public-page cache remain shared among authorized portal operators/admins.
This is not universal tenant isolation. Research questions,
model answers and transient controller state belong to the initiating request
and account. Last fetched/injected web selections are scoped by persistent random
account identity, so deleting and recreating a username cannot inherit them.
Auth-off legacy use has one explicit local-owner scope. Private selections may
remain on disk after account deletion, but are inaccessible to replacement users.

A non-empty allowlist is an armed content surface: every granted origin or path
is reachable by chat `web_fetch`, still bounded by `web.chat_tool_calls`. Fresh
homes refuse page fetching with `WEB_ALLOWLIST_EMPTY`. Search listings never
authorize their destinations: a follow-up `web_fetch` must independently pass the
current allowlist and account checks.

Current policy is rechecked on dispatch, cache/index/context reads, storage,
model consumption and result delivery. Revocation discards in-flight evidence and
makes revoked cached sources inaccessible, even through overlapping rules.
Research also rechecks the initiating account before model calls and delivery.
Revocation cannot erase already delivered historical conversation content or a
previous model request; ordinary historical conversations can contain previously
supplied web text. The URL policy applies only to this web subsystem, not the OS
or unrelated model/GitHub connections; content, provider and account permissions
are separate.

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
