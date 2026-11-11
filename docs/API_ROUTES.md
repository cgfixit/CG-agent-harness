# HTTP route inventory

`registered_paths()` in `src/server/routes/mod.rs` is authoritative: it combines
`REGISTERED_PATHS` with the authentication routes, and `GET /api/tools` reports
what is wired.

Console routes pass the [guard chain](../INVARIANTS.md): same-origin, direct
loopback, rate limit, account/RBAC, then mutation CSRF (`X-CyClaw-CSRF`). Public
routes keep only the early guards. Agent requests carry check-profile names and
run IDs, never commands.

## Console and status

| Method | Path | Purpose |
|---|---|---|
| GET | `/` | Console page with CSRF/CSP placeholders filled |
| GET | `/preview/html` | Sandboxed inline-only HTML preview, no network |
| GET | `/static/{name}` | Static console assets |
| GET | `/api/status` | Public minimal status; model fields need an account |
| GET | `/api/tools` | Wired-route and tool inventory |
| GET | `/api/netconnect` | Read-only LAN panel, query ignored ([netconnect.md](netconnect.md)) |
| GET | `/api/netconnect/diag` | Budgeted TCP probe of configured targets; 409 unless the tier may run ([diagnostics](netconnect.md#diagnostics)) |
| GET | `/api/registry` | Skill/tool/connector catalog |
| GET | `/api/audit` | Audit events (any role) |
| GET | `/api/diag` | `/diag` gate, allowlisted logs, last read and staged excerpt (admin or `portal_auditor`; 409 `DIAG_DISABLED` while off) |
| GET | `/api/diag/read` | Bounded, redacted tail of one allowlisted log: `file`, optional `last`, `event`, `since`, `errors` |
| POST | `/api/diag/inject`, `/api/diag/forget` | Stage the last read for `session_id`'s next local chat turn, or drop it |
| GET | `/api/harness/runs` | Retained harness-optimizer runs (`/harness`) |
| GET | `/api/analytics/summary` | Owned sessions plus shared spend and coding analytics ([details](SPEND_AND_NOTIFICATIONS.md#analytics-sessions-tokens-and-coding-runs)) |
| POST | `/api/config/reload` | Administrator reload of the [limits allowlist](INSTALL.md#reload-limits-without-a-restart) |

## Accounts and sessions

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/auth/setup-status` | `setup_code` while the one-time code works; public |
| POST | `/api/auth/login` | Login; public with early guards |
| POST | `/api/auth/logout` | Revoke the current session |
| GET | `/api/auth/whoami` | Current account, role and `user_id` |
| POST | `/api/auth/password` | Change own password; revokes other sessions |
| GET, POST | `/api/auth/users` | Administrator list/create, with each `user_id` (`/users`) |
| DELETE | `/api/auth/users/{username}` | Administrator delete |
| POST | `/api/auth/users/{username}/disabled`, `/api/auth/users/{username}/password`, `/api/auth/users/{username}/role` | Administrator enable/disable, password reset, role change |
| GET, POST | `/api/keys` | Managed-key presence (masked tail) and `/api set <KEY> <value>` |

Roles, scrypt bounds and session timeouts: [SECURE_RESEARCH.md](SECURE_RESEARCH.md).

## Chat, model and chat sessions

| Method | Path | Purpose |
|---|---|---|
| POST | `/api/chat` | One turn; streams SSE when `Accept: text/event-stream` |
| POST | `/api/chat/cancel` | Cancel the active turn (`/loop stop`) |
| POST | `/api/chat/attachments` | Upload up to 3 txt/md/json/csv/log/docx files; 503 past `attachments.max_concurrent_uploads` ([limits and retention](CONSOLE.md#text-file-attachments-and-prompt-preview)) |
| GET, POST | `/api/notes-corpus` | List owned note metadata and upload limits, or ingest a `.md`/`.txt` batch for local prompt retrieval; file refusals carry a zero-based `file_index` |
| DELETE | `/api/notes-corpus/{id}` | Unlink one owned note |
| POST | `/api/loop/start` | Start an owner-bound server-side `/loop` run: `session_id`, optional `goal`/`web_mode`, limit overrides |
| POST | `/api/loop/stop` | Stop by `run_id` |
| GET | `/api/loop/status` | One run (`?run_id=`) or a session's run (`?session_id=`) |
| GET | `/api/loop/events/{run_id}` | SSE per-turn progress ([server loops](CONSOLE.md#71-sessions-and-bounded-chat-continuation)) |
| POST | `/api/model` | Select the local model, `grok` or `claude` (`/model use`) |
| GET | `/api/ollama/inventory` | Live loopback tags (`/model list`) plus configured-model readiness |
| GET | `/api/ollama/profile` | Selected-model facts (`/model profile`) |
| POST | `/api/ollama/pull`, `/api/ollama/pull/cancel` | Operator/admin pull (SSE when `Accept: text/event-stream`), or abort it |
| POST | `/api/prompt/preview` | Show the assembled prompt (`/prompt`) |
| POST | `/api/slash/parse` | Suggest-don't-guess slash normalizer; never executes mutations |
| GET | `/api/spend/summary` | Retained spend summary with completeness and read-time USD |
| POST | `/api/spend/predict` | Estimate unsent cloud cost ([Estimate draft](SPEND_AND_NOTIFICATIONS.md)) |
| GET, POST | `/api/sessions` | List your sessions or create an owned one |
| GET | `/api/sessions/legacy` | Administrator metadata inventory of unassigned legacy sessions |
| POST | `/api/sessions/search` | Owned local transcript search ([bounds](CONSOLE.md#search-and-export-session-transcripts)) |
| POST | `/api/sessions/clear` | Delete owned sessions, attachments and uploaded notes; foreign, legacy, corrupt and staged files stay |
| GET | `/api/sessions/{session_id}` | Load one owned session with its `web_sources` read ledger |
| POST | `/api/sessions/{session_id}/adopt` | Administrator adoption with confirmation and reason; clears prior goal-stage approval |
| GET | `/api/sessions/{session_id}/export` | Markdown export; also written 0o600 under home/exports |
| POST | `/api/sessions/{session_id}/rename` | Rename |
| POST | `/api/sessions/{session_id}/goal` | Set goal; optional `web_mode` `off`/`fetch`/`all`, else `web.goal_web_default` (ships `all`) |
| GET, POST | `/api/sessions/{session_id}/goal-stage` | Goal staging status and transitions |
| GET, POST | `/api/sessions/{session_id}/skills` | Session skill selection |
| GET, POST | `/api/sessions/{session_id}/structured-facts` | Structured-memory facts explicitly included in the session |
| POST | `/api/sessions/{session_id}/prompt-history` | Owner-scoped composer line (`{text}`) for ArrowUp recall; last 50, `/api` values dropped |
| DELETE | `/api/sessions/{session_id}/attachment-pins` | Owner-scoped unpin; keeps blobs; refused during generation |

## Persona, skills and pinned notes

| Method | Path | Purpose |
|---|---|---|
| GET, POST | `/api/soul`, `/api/soul/document` | Persona state and on/off toggle; read or edit the persona document |
| POST | `/api/soul/proposals` | Propose a persona change |
| GET, POST | `/api/soul/proposals/{id}` | Review or decide a proposal |
| GET, POST | `/api/style` | Output-style catalog and session select (`/style <name>`, `/style off`) |
| GET, POST | `/api/skills`, `/api/skills/check` | List skills; describe a `check:<profile>` ID |
| GET, POST | `/api/memory` | Pinned-note status and on/off toggle |
| POST | `/api/memory/add`, `/api/memory/forget`, `/api/memory/clear` | Save a literal note, remove one, remove all |

## Structured memory (account-private)

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/structured-memory` | Truthful status and effective gates |
| POST | `/api/structured-memory/gates` | Administrator gate override, persisted in `memory/structured_gates.json` |
| GET | `/api/structured-memory/search` | Facts-only FTS candidates |
| GET, POST | `/api/structured-memory/facts` | List active facts or add one (confirm) |
| GET | `/api/structured-memory/facts/{id}` | Recall one fact, including inactive |
| POST | `/api/structured-memory/facts/{id}/deactivate` | Deactivate (confirm) |
| GET, POST | `/api/structured-memory/proposals` | List or create proposals |
| GET, POST | `/api/structured-memory/proposals/{id}` | Review, or apply/reject with reason |
| GET | `/api/structured-memory/episodes`, `/api/structured-memory/episodes/{id}` | List episodes or read one |
| POST | `/api/structured-memory/episodes/purge` | Purge expired episodes |
| POST | `/api/structured-memory/episodes/{id}/summary` | Attach a human semantic summary (confirm + reason) |
| POST | `/api/structured-memory/episodes/{id}/delete` | Delete one episode |
| GET, POST | `/api/structured-memory/consolidation` | List or start consolidation |
| GET, POST | `/api/structured-memory/consolidation/{id}`, `/api/structured-memory/consolidation/{id}/cancel` | Consolidation status; cancel |
| GET, POST | `/api/structured-memory/export`, `/api/structured-memory/purge` | Export or purge the owner's memory |

Confirmation and gate rules: [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md).

## Web research

| Method | Path | Purpose |
|---|---|---|
| GET, POST | `/api/web` | Status and on/off toggle |
| POST | `/api/web/allow` | Atomically grant exact URLs or explicit wildcards with group and seeds |
| POST | `/api/web/deny` | Remove a grant |
| POST | `/api/web/mode` | Administrator selects allowlist/denylist; denylist requires confirmation |
| POST | `/api/web/block`, `/api/web/unblock` | Administrator adds/removes blocks |
| POST | `/api/web/check` | Check exact URL permissions locally, without fetching or granting |
| POST | `/api/web/fetch` | Read one or more exact permitted URLs within shared resource limits |
| POST | `/api/web/search` | Page search; `engine:"google"` uses `web.search.provider` |
| POST | `/api/web/research` | Research permitted sources; question ≤2,000 characters, optional group/URLs |
| POST | `/api/web/research/cancel` | Cancel the owner's running research, detached or not |
| POST, GET | `/api/web/research/jobs` | Start a detached owner-bound research job (`/web job`) or list owned jobs; `GET …/jobs/{job_id}` reads one ([research](SECURE_RESEARCH.md#fetch-discovery-and-research)) |
| POST | `/api/web/inject`, `/api/web/forget` | Include your last selection in chat, or drop injected reads |

The `cgagentharness web` CLI drives these routes.

## MCP

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/mcp` | Declared MCP servers, namespaced tools and versioned stdio capability policies; no auto-discovery |
| POST | `/api/mcp/call` | Call one declared MCP tool; `confirm` is never defaulted |

Bounds and capability policy: [MCP.md](MCP.md#outbound-client-capabilities);
`/loop` has no MCP tools. The private memory server is not a console route: a
separate default-off loopback listener accepts `POST /mcp` with machine bearer
keys minted by `mcp-key`, and console cookies never authorize it
([activation and limits](MCP.md#deliberate-local-activation)).

## Coding agent

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/github/status` | GitHub credential and write-gate posture |
| GET | `/api/agent/checks` | Check-profile names the browser may request |
| POST | `/api/agent/run` | Synchronous run; check names map to fixed argv |
| GET, POST | `/api/agent/jobs` | List owned detached jobs or start one |
| GET, POST | `/api/agent/jobs/{job_id}`, `/api/agent/jobs/{job_id}/cancel` | Job status; cancel a job |
| POST | `/api/agent/schedules/preview` | Preview five occurrences and return an owner-bound receipt |
| GET, POST | `/api/agent/schedules` | List owned schedules or activate the exact previewed request |
| GET, POST | `/api/agent/schedules/{schedule_id}`, `/api/agent/schedules/{schedule_id}/cancel` | One schedule; cancel it |
| GET | `/api/agent/runs`, `/api/agent/runs/{run_id}` | Retained run metadata; one run and its diff |
| POST | `/api/agent/runs/{run_id}/decision` | Approve (`reason`, confirm) or reject |
| POST | `/api/agent/runs/{run_id}/push`, `/api/agent/runs/{run_id}/publish`, `/api/agent/runs/{run_id}/discard` | Push the approved commit, open a draft PR, or discard the run |
| GET | `/api/notifications` | Owned destinations and retained delivery metadata; no URLs, credentials or job content |
| POST | `/api/notifications/{delivery_id}/replay` | Confirmed owner replay within current grants/limits; never reruns a job |

Run and job creation share `agent::prepare_run`, then spawn through the shim
([publication](CODING_PIPELINE.md#git-approval-and-publication)). Run routes
need literal `agentic.enabled: true`; otherwise they return HTTP 409
`AGENTIC_DISABLED` without reading or reconciling records, while
`GET /api/github/status` stays HTTP 200. Webhook destinations and payloads:
[SPEND_AND_NOTIFICATIONS.md](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook).
