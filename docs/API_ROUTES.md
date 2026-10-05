# HTTP route inventory

Console routes, grouped by feature. `registered_paths()` in
`src/server/routes/mod.rs` combines `REGISTERED_PATHS` with authentication
extras. Its unit test checks router coverage; `GET /api/tools` reports catalog
wiring against that set. Code remains authoritative.

All console routes use the [guard chain](../INVARIANTS.md): rate limit,
same-origin, direct loopback/no forwarding headers, account/RBAC, mutation
CSRF (`X-CyClaw-CSRF`). Public routes retain early guards. Agent requests carry
check-profile names and run IDs, never browser-supplied commands.

Spend and outbound completion webhooks: [operator guide](SPEND_AND_NOTIFICATIONS.md).

## Console and status

| Method | Path | Purpose |
|---|---|---|
| GET | `/` | Console page (`assets/static/harness.html` with CSRF/CSP placeholders filled) |
| GET | `/static/{name}` | Static console assets |
| GET | `/api/status` | Minimal status; public with early guards |
| GET | `/api/tools` | Wired-route and tool inventory |
| GET | `/api/netconnect` | Read-only LAN panel (query ignored; see [netconnect.md](netconnect.md)) |
| GET | `/api/registry` | Skill/persona registry listing |
| GET | `/api/audit` | Audit events (administrator) |
| GET | `/api/harness/runs` | Retained harness-optimizer runs (`/harness`) |
| GET | `/api/analytics/summary` | Owned sessions plus shared spend/coding analytics (`/analytics`; [details](ANALYTICS.md)) |
| POST | `/api/config/reload` | Reload the non-secret limits allowlist (administrator, CSRF-guarded; see [CONFIG_RELOAD.md](CONFIG_RELOAD.md)) |

## Accounts and sessions

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/auth/setup-status` | Pending bootstrap replacement; `default_password` while `admin` / `admin` works; public |
| POST | `/api/auth/bootstrap-password` | Replace the fresh `admin` password (12+ characters) |
| POST | `/api/auth/login` | Login; public with early guards |
| POST | `/api/auth/logout` | Revoke the current session |
| GET | `/api/auth/whoami` | Current account, role, and `user_id` |
| POST | `/api/auth/password` | Change own password; revokes other sessions |
| GET, POST | `/api/auth/users` | List/create accounts, including each `user_id` (administrator; `/users`) |
| DELETE | `/api/auth/users/{username}` | Delete an account (administrator) |
| POST | `/api/auth/users/{username}/disabled` | Enable/disable an account (administrator) |
| POST | `/api/auth/users/{username}/password` | Administrative password reset |
| POST | `/api/auth/users/{username}/role` | Change a role (administrator) |
| GET, POST | `/api/keys` | Managed-key presence (masked tail) and `/api set <KEY> <value>` |

See [SECURE_RESEARCH.md](SECURE_RESEARCH.md) for roles, scrypt bounds and
session timeouts.

## Chat, model and chat sessions

| Method | Path | Purpose |
|---|---|---|
| POST | `/api/chat` | One turn; streams SSE when `Accept: text/event-stream` |
| POST | `/api/chat/attachments` | Upload up to 3 txt/md/json/csv/log/docx files, 15 MB each; home quota 64 MB/512 blobs. Concurrent bodies use `attachments.max_concurrent_uploads`, else 503. UUID blobs use `0600`. Local chat retains up to 12 owner-scoped IDs per session for fenced BM25 context. Cloud, `/loop` and `/agent` refuse saved/requested attachment IDs with `ATTACHMENT_SURFACE_FORBIDDEN`. |
| GET | `/api/notes-corpus` | List owned jailed `.md`/`.txt` metadata (ID, MIME, SHA prefix, bytes), without bodies or host paths. |
| POST | `/api/notes-corpus` | Copy/classify `.md`/`.txt` into `notes_corpus/<owner-digest>/` under `notes_corpus.*` caps; local chat/preview only. |
| DELETE | `/api/notes-corpus/{id}` | Unlink one owner-owned note. |
| POST | `/api/chat/cancel` | Cancel the active turn (`/loop stop`) |
| POST | `/api/model` | Select the local model or `grok` / `claude` (`/model use`) |
| GET | `/api/ollama/inventory` | Live loopback tags plus configured-model readiness |
| POST | `/api/ollama/pull` | Operator/admin abortable pull; SSE when `Accept: text/event-stream` |
| POST | `/api/ollama/pull/cancel` | Abort the in-flight pull |
| POST | `/api/prompt/preview` | Show the assembled system prompt (`/prompt`) |
| POST | `/api/slash/parse` | Suggest-don't-guess slash normalizer; never executes mutations |
| GET | `/api/spend/summary` | Guarded retained spend, completeness, file statuses, skipped rows and read-time USD |
| POST | `/api/spend/predict` | Estimate unsent cloud cost (Analytics → Tokens and cost → Estimate draft; [details](SPEND_AND_NOTIFICATIONS.md)) |
| GET, POST | `/api/sessions` | List your sessions or create an owned one |
| GET | `/api/sessions/legacy` | Administrator metadata inventory of unassigned legacy sessions |
| POST | `/api/sessions/{session_id}/adopt` | Administrator adopts into own account with confirmation/reason; clears prior goal-stage approval |
| POST | `/api/sessions/search` | Owned local transcript search off async workers; case-insensitive literal phrases cross chunk boundaries; Unicode-safe snippets ≤160 characters |
| POST | `/api/sessions/clear` | Delete owned sessions; retain foreign, legacy, corrupt and staged files |
| GET | `/api/sessions/{session_id}` | Load one owned session |
| GET | `/api/sessions/{session_id}/export` | Markdown export; also written 0o600 under home/exports |
| POST | `/api/sessions/{session_id}/rename` | Rename |
| POST | `/api/sessions/{session_id}/goal` | Set the session goal |
| GET, POST | `/api/sessions/{session_id}/goal-stage` | Goal staging status and transitions |
| GET, POST | `/api/sessions/{session_id}/skills` | Skill selection for the session |
| GET, POST | `/api/sessions/{session_id}/structured-facts` | Selected structured-memory facts (explicit include) |

See [CONSOLE.md](CONSOLE.md) and
[streaming chat](CONSOLE.md#streaming-chat-and-cancellation).

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
| POST | `/api/web/allow` | Atomically grant exact URLs or explicit wildcards, with group/optional seeds |
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

A separate default-off loopback listener accepts `POST /mcp` with dedicated
machine bearer keys, never console cookies. Portal binding does not expose it.
Empty tool grants independently restrict `memory_list_facts`, `memory_get_fact`
and literal-substring `memory_search` to the key owner and `memory:read` scope.
`mcp-key create/list/revoke` manages its credentials. `GET /api/mcp` and
`/tools mcp` display configuration without probing or authorizing it. See
[activation, limits and exposure contract](MCP_SERVER.md).

## MCP client

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/mcp` | Declared MCP servers, namespaced tools and versioned stdio capability policies; does not auto-discover |
| POST | `/api/mcp/call` | Call one declared MCP tool; `confirm` is never defaulted |

Bounds and capability policy: [MCP_CLIENT.md](MCP_CLIENT.md). `/loop` has no MCP tools.

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
| POST | `/api/agent/schedules/preview` | Preview five interval/cron occurrences; return bounded owner-bound receipt |
| GET, POST | `/api/agent/schedules` | List owned schedules or activate the exact previewed, owned reviewed-goal request |
| GET | `/api/agent/schedules/{schedule_id}` | One schedule |
| POST | `/api/agent/schedules/{schedule_id}/cancel` | Cancel a schedule |
| GET | `/api/agent/runs` | Retained run metadata while the coding layer is on; HTTP 409 `AGENTIC_DISABLED` while it is off |
| GET | `/api/agent/runs/{run_id}` | One run and its diff |
| POST | `/api/agent/runs/{run_id}/decision` | Approve locally (`--reason` + confirm) |
| POST | `/api/agent/runs/{run_id}/push` | Push the approved commit |
| POST | `/api/agent/runs/{run_id}/publish` | Open a draft PR |
| POST | `/api/agent/runs/{run_id}/discard` | Discard a run |

Run and job creation share `agent::prepare_run`, then spawn through the shim; see [CONSOLE_JOBS.md](CONSOLE_JOBS.md) and
[CODING_PIPELINE.md](CODING_PIPELINE.md#git-approval-and-publication).
`GET /api/agent/runs` lists metadata only while `agentic.enabled` is literal true. Otherwise it is HTTP 409 `AGENTIC_DISABLED` and does not read or reconcile records; detail, decision, push, and discard do the same. `GET /api/github/status` stays HTTP 200.

## Completion webhooks

Default-off, restart-only `notifications.enabled: true` activates a private
bounded outbox for owned destinations and terminal-job subscriptions. Schema 1
`destinations` replace the legacy `webhook_url`, which must be empty. Status and
replay cannot select another account owner.

Version-2 batches contain stable event/delivery IDs, job ID, status and
timestamps. Attempts recheck destination/account grants, pin validated DNS,
refuse proxies/redirects and require public HTTPS. Exact private URL grants can
permit private HTTP. Explicit managed credentials never enter status, outbox,
payloads or audit.

Attempts/rate state persist before dispatch. Retained jobs reconcile enqueue
crash gaps on restart. At-least-once delivery has finite retries, retention and
queue bounds; deduplicate by delivery ID. Replay never restarts jobs. See
[configuration and crash semantics](SPEND_AND_NOTIFICATIONS.md).
