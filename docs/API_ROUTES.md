# HTTP route inventory

Every route the console serves, grouped by feature. The authoritative list is
`REGISTERED_PATHS` in `src/server/routes/mod.rs`; `GET /api/tools` reports the
same inventory at runtime as its "wired" set, and
`tests/invariant_guard.rs` fails the build if the router and the list diverge.
This page is a reading aid; when it disagrees with the code, the code wins.

All routes are loopback-only and pass the guard chain in
[INVARIANTS.md](../INVARIANTS.md): rate limit, same-origin, direct
loopback/no forwarding headers, account/RBAC, then mutation CSRF
(`X-CyClaw-CSRF`). Public routes still receive the early guards. The browser
never supplies a command; agent routes carry check-profile names and run ids.

Spend interpretation and completion-webhook configuration: [operator guide](SPEND_AND_NOTIFICATIONS.md). Webhooks are outbound notifications, not a new inbound route.

## Console and status

| Method | Path | Purpose |
|---|---|---|
| GET | `/` | Console page (`assets/static/harness.html` with CSRF/CSP placeholders filled) |
| GET | `/static/{name}` | Static console assets |
| GET | `/api/status` | Minimal status; public with early guards |
| GET | `/api/tools` | Wired-route and tool inventory |
| GET | `/api/registry` | Skill/persona registry listing |
| GET | `/api/audit` | Audit events (administrator) |
| GET | `/api/harness/runs` | Retained harness-optimizer runs (`/harness`) |
| GET | `/api/analytics/summary` | Composed spend/session/coding-run analytics for the account (`/analytics`; see [ANALYTICS.md](ANALYTICS.md)) |
| POST | `/api/config/reload` | Reload the non-secret limits allowlist (administrator, CSRF-guarded; see [CONFIG_RELOAD.md](CONFIG_RELOAD.md)) |

## Accounts and sessions

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/auth/setup-status` | Whether bootstrap password replacement is pending; public |
| POST | `/api/auth/bootstrap-password` | Replace the fresh `admin` password (12+ characters) |
| POST | `/api/auth/login` | Login; public with early guards |
| POST | `/api/auth/logout` | Revoke the current session |
| GET | `/api/auth/whoami` | Current account and role |
| POST | `/api/auth/password` | Change own password; revokes other sessions |
| GET, POST | `/api/auth/users` | List or create accounts (administrator; `/users`) |
| DELETE | `/api/auth/users/{username}` | Delete an account (administrator) |
| POST | `/api/auth/users/{username}/disabled` | Enable or disable an account (administrator) |
| POST | `/api/auth/users/{username}/password` | Administrative password reset |
| POST | `/api/auth/users/{username}/role` | Change a role (administrator) |
| GET, POST | `/api/keys` | Managed-key presence (masked tail) and `/api set <KEY> <value>` |

See [SECURE_RESEARCH.md](SECURE_RESEARCH.md) for roles, scrypt bounds and
session timeouts.

## Chat, model and chat sessions

| Method | Path | Purpose |
|---|---|---|
| POST | `/api/chat` | One turn; streams SSE when `Accept: text/event-stream` |
| POST | `/api/chat/attachments` | Store up to 3 text/DOCX files (txt/md/json/csv/log/docx, 15 MB each, 64 MB and 512-blob home quota; `attachments.max_concurrent_uploads` bodies in flight, else 503). UUID blobs at `0600`. Local chat persists owner-scoped blob ids on the session (cap 12) and may BM25-chunk those blobs into the existing fence; cloud, `/loop`, and `/agent` return `ATTACHMENT_SURFACE_FORBIDDEN` if those ids or request `attachment_ids` are present. |
| GET | `/api/notes-corpus` | List this owner's jailed `.md`/`.txt` notes (id, mime, sha prefix, bytes; no bodies, no host path). |
| POST | `/api/notes-corpus` | Copy+classify `.md`/`.txt` into `notes_corpus/<owner-digest>/` (caps in `notes_corpus.*`). Injected on local chat / preview only. |
| DELETE | `/api/notes-corpus/{id}` | Unlink one owner-owned note. |
| POST | `/api/chat/cancel` | Cancel the active turn (`/loop stop`) |
| POST | `/api/model` | Select the local model or `grok` / `claude` (`/model use`) |
| GET | `/api/ollama/inventory` | Live loopback tags plus configured-model readiness |
| POST | `/api/ollama/pull` | Operator/admin abortable pull; SSE when `Accept: text/event-stream` |
| POST | `/api/ollama/pull/cancel` | Abort the in-flight pull |
| POST | `/api/prompt/preview` | Show the assembled system prompt (`/prompt`) |
| POST | `/api/slash/parse` | Suggest-don't-guess slash normalizer; never executes mutations |
| GET | `/api/spend/summary` | Guarded retained-history spend rollup with completeness, file statuses and skipped-row counts (read-time USD) |
| POST | `/api/spend/predict` | Estimate an unsent cloud draft's cost before sending (Estimate draft; see [SPEND_AND_NOTIFICATIONS.md](SPEND_AND_NOTIFICATIONS.md)) |
| GET, POST | `/api/sessions` | List your sessions or create an owned one |
| GET | `/api/sessions/legacy` | Administrator metadata inventory of unassigned legacy sessions |
| POST | `/api/sessions/{session_id}/adopt` | Administrator adopts into own account with confirmation/reason; clears prior goal-stage approval |
| POST | `/api/sessions/search` | Local transcript search; case-insensitive matches with Unicode-safe snippets of at most 160 characters |
| POST | `/api/sessions/clear` | Delete owned sessions; retain foreign, legacy, corrupt and staged files |
| GET | `/api/sessions/{session_id}` | Load one owned session |
| GET | `/api/sessions/{session_id}/export` | Markdown export; also written 0o600 under home/exports |
| POST | `/api/sessions/{session_id}/rename` | Rename |
| POST | `/api/sessions/{session_id}/goal` | Set the session goal |
| GET, POST | `/api/sessions/{session_id}/goal-stage` | Goal staging status and transitions |
| GET, POST | `/api/sessions/{session_id}/skills` | Skill selection for the session |
| GET, POST | `/api/sessions/{session_id}/structured-facts` | Selected structured-memory facts (explicit include) |

See [CHAT_WORKFLOWS.md](CHAT_WORKFLOWS.md) and
[CHAT_STREAMING.md](CHAT_STREAMING.md).

## Persona, skills and pinned notes

| Method | Path | Purpose |
|---|---|---|
| GET, POST | `/api/soul` | Persona state and on/off toggle |
| GET, POST | `/api/style` | Output-style catalog and session select (`/style <name>` or `/style off`) |
| GET, POST | `/api/soul/document` | Read or edit the persona document |
| POST | `/api/soul/proposals` | Propose a persona change |
| GET, POST | `/api/soul/proposals/{id}` | Review or decide a proposal |
| GET | `/api/skills` | List skills |
| POST | `/api/skills/check` | Validate a skill |
| GET, POST | `/api/memory` | Pinned-note status and on/off toggle |
| POST | `/api/memory/add` | Save a literal note |
| POST | `/api/memory/forget` | Remove one note |
| POST | `/api/memory/clear` | Remove all notes |

## Structured memory (account-private)

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/structured-memory` | Truthful status and effective gates |
| POST | `/api/structured-memory/gates` | Administrator gate override for this process |
| GET | `/api/structured-memory/search` | Facts-only FTS candidates |
| GET, POST | `/api/structured-memory/facts` | List active facts or add one (confirm) |
| GET | `/api/structured-memory/facts/{id}` | Recall one fact, including inactive |
| POST | `/api/structured-memory/facts/{id}/deactivate` | Deactivate (confirm) |
| GET, POST | `/api/structured-memory/proposals` | List or create proposals |
| GET, POST | `/api/structured-memory/proposals/{id}` | Review or apply/reject with reason |
| GET | `/api/structured-memory/episodes` | List episodes |
| POST | `/api/structured-memory/episodes/purge` | Purge expired episodes |
| GET | `/api/structured-memory/episodes/{id}` | One episode |
| POST | `/api/structured-memory/episodes/{id}/summary` | Generate a semantic summary |
| POST | `/api/structured-memory/episodes/{id}/delete` | Delete one episode |
| GET, POST | `/api/structured-memory/consolidation` | List or start consolidation |
| GET | `/api/structured-memory/consolidation/{id}` | Consolidation status |
| POST | `/api/structured-memory/consolidation/{id}/cancel` | Cancel |
| GET | `/api/structured-memory/export` | Export the owner's memory |
| POST | `/api/structured-memory/purge` | Purge the owner's memory |

Confirmation and gate rules are in [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md).

## Web research

| Method | Path | Purpose |
|---|---|---|
| GET, POST | `/api/web` | Status and on/off toggle |
| POST | `/api/web/allow` | Atomically grant one or more exact URLs or explicit wildcards, with a group and optional seeds |
| POST | `/api/web/deny` | Remove a grant |
| POST | `/api/web/fetch` | Read one or more exact permitted URLs within shared resource limits |
| POST | `/api/web/check` | Check exact URL permissions locally, without fetching or granting access |
| POST | `/api/web/search` | Bounded Google search (`SERPAPI_API_KEY` optional) |
| POST | `/api/web/research` | Research permitted sources, optionally narrowing the group and concrete starting URLs |
| POST | `/api/web/research/cancel` | Cancel research |
| POST | `/api/web/inject` | Inject a read into the session context |
| POST | `/api/web/forget` | Drop injected reads |

The `cgagentharness web` CLI family drives these same routes through the
running portal.

## Private MCP memory server

The optional separate loopback listener accepts `POST /mcp` using dedicated
machine bearer keys, never console cookies. It is not a console route and is
not exposed by changing the portal's bind address. Default-off listener and
empty tool grants are independent. Read-only tools are `memory_list_facts`,
`memory_get_fact`, and literal-substring `memory_search`, each bound to the
key row's owner and `memory:read` scope. Local CLI `mcp-key create/list/revoke`
manages independent credentials. See [activation, limits and future exposure
contract](MCP_SERVER.md). `GET /api/mcp` and `/tools mcp` also show gateway
configuration; they do not probe or authorize that listener.

## MCP client

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/mcp` | Declared MCP servers, namespaced tools and versioned stdio capability policies; does not auto-discover |
| POST | `/api/mcp/call` | Call one declared MCP tool; `confirm` is never defaulted |

SSE MCP URLs are DNS-pinned. Loopback SSE requires `mcp.sse_allow_loopback: true`.
MCP tools are not attached to `/loop`. Stdio response headers are limited to 4 KiB,
including unterminated lines. Child stderr is drained through a pipe, retaining at most 2 KiB in memory and
512 Unicode characters in diagnostics. No stderr log file is created; the existing
call timeout still applies. Stdio protection follows the declared
[capability policy](MCP_CLIENT.md); no silent fallback to an unconfined process
or a weaker network mode is allowed. The response reports declared policy,
not a successful sandbox probe. `/tools mcp` renders that policy in the console.

## Coding agent

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/github/status` | GitHub credential and write-gate posture |
| GET | `/api/agent/checks` | Check-profile names the browser may request |
| POST | `/api/agent/run` | Synchronous run; check names map to fixed argv |
| GET, POST | `/api/agent/jobs` | List owned detached jobs or start one |
| GET | `/api/agent/jobs/{job_id}` | Job status |
| POST | `/api/agent/jobs/{job_id}/cancel` | Cancel a job |
| GET | `/api/notifications` | Owned destinations and retained delivery metadata; no URLs, credentials or job content |
| POST | `/api/notifications/{delivery_id}/replay` | Confirmed owner replay within current grants/limits; never reruns a job |
| POST | `/api/agent/schedules/preview` | Preview five interval/cron occurrences; return a bounded owner-bound activation receipt |
| GET, POST | `/api/agent/schedules` | List owned schedules or activate the exact previewed, owned reviewed-goal request |
| GET | `/api/agent/schedules/{schedule_id}` | One schedule |
| POST | `/api/agent/schedules/{schedule_id}/cancel` | Cancel a schedule |
| GET | `/api/agent/runs` | Retained run records |
| GET | `/api/agent/runs/{run_id}` | One run and its diff |
| POST | `/api/agent/runs/{run_id}/decision` | Approve locally (`--reason` + confirm) |
| POST | `/api/agent/runs/{run_id}/push` | Push the approved commit |
| POST | `/api/agent/runs/{run_id}/publish` | Open a draft PR |
| POST | `/api/agent/runs/{run_id}/discard` | Discard a run |

Run and job routes both go through `agent::prepare_run`, then cross the shim
into a child process; see [CONSOLE_JOBS.md](CONSOLE_JOBS.md) and
[GIT_APPROVAL.md](GIT_APPROVAL.md).

## Completion webhooks

`notifications.enabled: true` activates the private bounded outbox for explicitly
owned destinations and terminal-job subscriptions. It remains default-off and
restart-only. Configure schema 1 `destinations`; a nonempty legacy global
`webhook_url` refuses enablement until deliberately migrated. Account users cannot
supply another owner through the status/replay APIs.

Version-2 JSON batches contain only stable event/delivery IDs, job ID, terminal
status and timestamps. Every attempt rechecks current destination/account grants,
validates and pins DNS, refuses proxies/redirects and requires HTTPS for public
receivers. Exact private URL grants can permit private HTTP. Credentials are
selected explicitly from the protected managed environment and never enter status,
outbox records, payloads or audit.

Attempts and per-destination rate state persist before dispatch. Retained jobs
reconcile an enqueue crash gap on restart. Finite retries, retention, bounded queue
pressure and explicit replay use at-least-once delivery; receivers deduplicate by
stable delivery ID. Replay never restarts a job. See the complete configuration,
crash semantics and bounds in [SPEND_AND_NOTIFICATIONS.md](SPEND_AND_NOTIFICATIONS.md).
