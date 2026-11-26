# Accounts, HTTPS and permitted web research

Fresh homes seed HTTPS, account authentication and web access with a Google
search grant. Existing homes keep their configuration. See
[installation](INSTALL.md) and [invariants](../INVARIANTS.md#account-transport-and-request-boundaries).

## Accounts and roles

The private `<home>/setup-code` bootstraps `admin`. Replace it with a
password of at least 12 characters. Bootstrap sessions allow identity,
password change and logout. Administrators manage accounts in **USERS**.
Password reset, role change, disablement and deletion revoke sessions.
The last enabled administrator cannot be removed, disabled or demoted.

| Role | Capabilities |
|---|---|
| `admin` | Account, credential and web policy management, plus normal guarded operations |
| `operator` | Local chat, permitted web, owned sessions/jobs/schedules/notes, coding and permitted MCP |
| `auditor` | Own local chat and prompt preview, audit and `/diag`; excludes memory, attachments, cloud, web and loops |

Own-password change requires
the current password and revokes old sessions. A committed password change or
reset also clears the account's staged `/diag` state and logs an audit event,
even if the client disconnects. Authentication and account
mutations have distinct deadlines. A timed-out write needs readback before
retry. A harness API key cannot grant account access. See
[managed keys](INSTALL.md#supported-managed-keys).

### Account storage and recovery

`auth.sqlite3` stores accounts and hashed session tokens transactionally.
Preserve it with `auth.initialized` during offline recovery. A missing
initialized database, bad schema or unsafe file refuses startup. Legacy
`auth.json` migration keeps a backup.
The database also keeps sign-in failures for valid nonexistent names (HMAC-keyed,
capped at 16,384 names), so a restart does not reset their lockout.
Account checks read the last committed state: a write in flight never
delays them, and a change is visible only after commit.
`auth.session` sets idle and absolute expiry and the per-account cap.
A sign-in past `max_per_account` (shipped 8) ends that account's least
recently used session and logs `account.session_evicted`. Capacity refusals
return 409 `AUTH_LIMIT`.

### Session ownership and legacy adoption

Sessions, transcript search/export, jobs and schedules bind to the stable
account ID. Administrators have no cross-owner session access. Auth-disabled
records belong to `local`. Unassigned legacy sessions stay hidden until an
administrator adopts one with confirmation and reason. Adoption clears
goal-stage approval and preserves attachment ownership. Unreadable and
unknown-schema files remain for recovery. Clear-history touches owned
records only. Recreating a username issues a new owner ID.

## HTTPS and native trust

Generated `tls/server.pem` holds a private P-256 key and leaf for
`localhost`, `127.0.0.1` and `::1`. Invalid or expired material fails startup.
The HTTPS listener uses TLS 1.3. `tls.enabled: false` explicitly selects
HTTP. Native WKWebView pins its owned leaf. Browsers need separate trust.
Export with **Setup → Export public browser certificate** and compare the
shown SHA-256. CLI equivalents are `cgagentharness tls certificate` and
`cgagentharness tls renew`. Stop the home owner before renewal and renew
browser trust and CLI login afterward.

Host, origin and CSRF guards apply. HTTPS session cookies are Secure,
HttpOnly and SameSite=Strict. Browser cookies do not isolate ports. Use a
trusted loopback environment. Forwarding headers and reverse proxies are
unsupported.

## Terminal access

`cgagentharness account --url <origin> login <name>` reads the password
without echo. The private CLI session binds to the exact origin and
certificate. `cgagentharness web` calls that running server's guarded routes.
Pass `--url` for desktop and custom ports. Read
[account and web routes](API_ROUTES.md) for the request contract.

## Web permissions

Web enablement and URL policy are home-wide. Selections are account-owned.
`/web off` denies future dispatch, storage and delivery. In-flight and
queued requests recheck authority. Current policy also gates index, cache
and prompt reuse. Revocation cannot retract text already delivered.

- Default allowlist mode uses exact canonical grants or explicit
  wildcards. `https://example.com/docs/*` covers `/docs/` descendants
  and their queries. `*.example.com` excludes the apex. Exact URLs include
  the exact query. Schemes, nondefault ports and hostnames matter.
- An administrator can confirm `/web mode denylist` to allow unblocked
  public URLs. Blocks win in either mode and across HTTP/HTTPS. A host-only
  block includes subdomains. Named groups stay limited to their allow rules.
- `/web allow` validates the batch and each seed before writing
  `tools/web_allowlist.json`. `/web deny` removes an allow rule.
  `/web check` checks without networking. A missing or invalid policy
  refuses reads. Ambiguous authorities, credentials, dot segments, encoded
  separators and unsupported escapes refuse canonicalization.
- `--group` narrows reads and discovery. Repeat `--seed` when granting
  concrete start pages. Seeds must fit a requested rule. A lowered
  `web.max_rules` below the stored count refuses the policy whole.

## Search, fetch and research

Use `/web search` for provider listings, `/web fetch <URL>` for reads,
`/web pages` for page search and `/web research` for bounded synthesis.
Use `--group NAME` to narrow scope and repeat `--url URL` for research
start pages. [Console syntax](CONSOLE.md#78-slash-command-quick-reference).

### Chat web tools

`web_search` and `web_fetch` are the local chat and loop web tools.
An administrator's own listings can grant exact result URLs for the
turn or run, including the exact arXiv PDF when enabled, under
`web.search.result_reads`. A URL named neither by the operator nor in
eligible evidence and permitted solely by a wildcard or denylist needs
live administrator approval (`web.fetch_provenance`). Otherwise it
refuses. Empty allowlists, blocks and private-address checks still apply.
The [router](WEB_DECISION_ROUTER.md) owns passage, digest and budget rules.

### Google search

`web.search.provider` is restart-only. `auto` follows active Tavily,
SerpAPI, then public Google. Keys can be changed for the next search.
Keyed providers receive query, count and their own credential. Public
Google needs the generated search URL permitted (fresh homes seed
`https://www.google.com/*`). Challenges or blocks cause cooldown. Keyed
quota exhaustion pauses until the configured quota cooldown or next
clock hour. Failed eligible hops can fall back within the same deadline.
See [provider rules and daily units](WEB_DECISION_ROUTER.md#provider-selection).
The API defaults to permitted page search, and `engine: "google"` requests
listings. A group selects page search in console and CLI.

### Fetch, discovery and research

All exact fetch targets are checked before batch dispatch. Local fetch
pins checked public DNS, validates TLS hosts, and refuses redirects,
proxies, cookies and compression. That client trusts only the Mozilla roots in `webpki-root-certs`, so
the OS verifier makes no OCSP, CRL or CA request. Enterprise roots are
not trusted, and revocation waits for a release that updates the bundle.
Cloud clients still use the OS verifier. Policy still gates every discovered
link. Text PDFs are extracted in the [PDF worker](CONSOLE.md#pdf-text).
Optional Firecrawl sends only an eligible exact URL under the
[router fallback rules](WEB_DECISION_ROUTER.md#read-authority-and-fallback).
Its hourly slot is returned if cancelled or otherwise refused before send.
The local HTML parser discards scripts, styles, forms and marked-hidden
content. CSS-only hiding is outside extraction. HTML complexity limits
can refuse a result without saving partial evidence. Extract receipts
bind canonical URL, content hash, fetch time and passage offsets.

Research uses permitted start URLs, group seeds, literal prefixes or
bounded matching cached pages. Unseeded wildcards may report
`WEB_SEED_REQUIRED`. Robots reads follow current scope and count toward
budgets. Unreadable robots prevents link traversal while seeds remain
eligible. A bounded Tantivy BM25 index ranks permitted extracts. Discovery
and local synthesis have separate deadlines. Citation validation drops
unknown IDs and quotes absent from their passage. Read counts and partial
coverage matter when assessing answers.

`/web job` and `/web jobs` inspect owner-bound work that survives the tab.
`/web cancel` stops research. Restart does not resume it.

### Saved web selection

`/web inject` stages the owner's last eligible extract for local chat,
and `/web forget` clears it. `/prompt` shows the current selection.
Listings do not become extracts. Research may reuse cache without
changing the selection. Older selections are rechecked before prompt use,
and a copied extract does not establish continuing URL permission.

## Security boundaries

Operational routes require accounts when enabled. Mutations use
`X-CyClaw-CSRF`. Research context and selections are owner-bound.
URL grants apply only to web evidence. See
[the egress invariant](../INVARIANTS.md#public-web-evidence-requires-current-content-permission)
and [machine MCP authority](MCP.md#private-read-only-memory-gateway).
