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
| POST | `/api/chat/attachments` | Store up to 3 text files (txt/md/json/csv/log, 15 MB each, 64 MB home quota). UUID blobs at `0600`. Inlined only on local chat/`/prompt`, never cloud, `/loop`, or `/agent`. |
| POST | `/api/chat/cancel` | Cancel the active turn (`/loop stop`) |
| POST | `/api/model` | Select the local model or `grok` / `claude` (`/model use`) |
| GET | `/api/ollama/inventory` | Live loopback tags plus configured-model readiness |
| POST | `/api/ollama/pull` | Operator/admin abortable pull; SSE when `Accept: text/event-stream` |
| POST | `/api/ollama/pull/cancel` | Abort the in-flight pull |
| POST | `/api/prompt/preview` | Show the assembled system prompt (`/prompt`) |
| POST | `/api/slash/parse` | Suggest-don't-guess slash normalizer; never executes mutations |
| GET | `/api/spend/summary` | Guarded per-provider/model/day spend rollup (read-time USD) |
| GET, POST | `/api/sessions` | List sessions or create one |
| POST | `/api/sessions/search` | Local transcript search; snippets only |
| POST | `/api/sessions/clear` | Delete all sessions |
| GET | `/api/sessions/{session_id}` | Load one session |
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
| POST | `/api/web/allow` | Add an exact URL or explicit wildcard |
| POST | `/api/web/deny` | Remove a grant |
| POST | `/api/web/fetch` | Fetch a permitted URL |
| POST | `/api/web/search` | Bounded Google search (`SERPAPI_API_KEY` optional) |
| POST | `/api/web/research` | Start a research run |
| POST | `/api/web/research/cancel` | Cancel research |
| POST | `/api/web/inject` | Inject a read into the session context |
| POST | `/api/web/forget` | Drop injected reads |

The `cgagentharness web` CLI family drives these same routes through the
running portal.

## MCP client

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/mcp` | Declared MCP servers and namespaced tools; does not auto-discover |
| POST | `/api/mcp/call` | Call one declared MCP tool; `confirm` is never defaulted |

SSE MCP URLs are DNS-pinned. Loopback SSE requires `mcp.sse_allow_loopback: true`.
MCP tools are not attached to `/loop`.

## Coding agent

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/github/status` | GitHub credential and write-gate posture |
| GET | `/api/agent/checks` | Check-profile names the browser may request |
| POST | `/api/agent/run` | Synchronous run; check names map to fixed argv |
| GET, POST | `/api/agent/jobs` | List detached jobs or start one |
| GET | `/api/agent/jobs/{job_id}` | Job status |
| POST | `/api/agent/jobs/{job_id}/cancel` | Cancel a job |
| GET, POST | `/api/agent/schedules` | List persisted schedules or create one (reviewed goal required) |
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
