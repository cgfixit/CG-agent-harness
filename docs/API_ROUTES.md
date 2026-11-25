# HTTP route inventory

[registered_paths()](../src/server/routes/mod.rs) and route registrations are
authoritative. `/api/tools` reports wiring. Guards enforce direct loopback,
same-origin, rate limits, account capability and mutation `X-CyClaw-CSRF`.
Public login and status retain early guards. The invariant guard checks the
method and path columns against `ROUTE_CAPS` in CI.

## Console and status

| Method | Path | Purpose |
|---|---|---|
| GET | `/`, `/static/{name}`, `/preview/html` | Console, assets, isolated preview |
| GET | `/api/status`, `/api/tools`, `/api/registry` | Status, wiring, catalog |
| GET | `/api/netconnect`, `/api/netconnect/diag` | LAN panel, configured probe |
| GET | `/api/audit`, `/api/diag`, `/api/diag/read` | Audit and allowlisted log reads |
| POST | `/api/diag/inject`, `/api/diag/forget` | Stage/drop local-turn excerpt |
| GET | `/api/harness/runs`, `/api/analytics/summary` | Optimizer and analytics summaries |
| POST | `/api/config/reload` | Admin [limit reload](INSTALL.md#reload-limits-without-a-restart) |

## Accounts and sessions

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/auth/setup-status`, `/api/auth/whoami` | Setup-code activity boolean, account identity |
| POST | `/api/auth/login`, `/api/auth/logout`, `/api/auth/password` | Sign in, revoke, change password |
| GET, POST | `/api/auth/users` | Admin list/create |
| DELETE | `/api/auth/users/{username}` | Admin delete |
| POST | `/api/auth/users/{username}/disabled`, `/api/auth/users/{username}/password`, `/api/auth/users/{username}/role` | Admin account changes |
| GET, POST | `/api/keys` | Masked presence, managed-key mutation |

## Chat, model and chat sessions

| Method | Path | Purpose |
|---|---|---|
| POST | `/api/chat`, `/api/chat/cancel` | Turn (JSON/SSE), cancel |
| POST | `/api/chat/fetch-confirm` | Decide owned composed fetch |
| POST | `/api/chat/attachments` | Upload attachments |
| GET, POST | `/api/notes-corpus` | List or ingest owned notes |
| DELETE | `/api/notes-corpus/{id}` | Delete owned note |
| POST | `/api/loop/start`, `/api/loop/stop` | Start/stop owned run |
| GET | `/api/loop/status`, `/api/loop/events/{run_id}` | Status, progress SSE |
| POST | `/api/model` | Chat selection |
| GET | `/api/ollama/inventory`, `/api/ollama/profile` | Exact tags, active profile |
| POST | `/api/ollama/pull`, `/api/ollama/pull/cancel` | Admin pull/abort |
| POST | `/api/prompt/preview`, `/api/slash/parse` | Prompt preview, command parse |
| GET | `/api/spend/summary` | Retained spend summary |
| POST | `/api/spend/predict` | Cloud draft estimate |
| GET, POST | `/api/sessions` | Owned list/create |
| GET | `/api/sessions/legacy` | Admin unassigned inventory |
| POST | `/api/sessions/search`, `/api/sessions/clear` | Owned search/clear |
| GET | `/api/sessions/{session_id}`, `/api/sessions/{session_id}/export` | Owned session, Markdown |
| POST | `/api/sessions/{session_id}/adopt`, `/api/sessions/{session_id}/rename` | Admin confirmed adoption, owner rename |
| POST | `/api/sessions/{session_id}/goal`, `/api/sessions/{session_id}/prompt-history` | Saved goal/mode, composer recall |
| GET, POST | `/api/sessions/{session_id}/goal-stage`, `/api/sessions/{session_id}/skills`, `/api/sessions/{session_id}/structured-facts` | Stage, skills, fact selection |
| DELETE | `/api/sessions/{session_id}/attachment-pins` | Clear pins |

## Persona, skills and pinned notes

| Method | Path | Purpose |
|---|---|---|
| GET, POST | `/api/soul`, `/api/soul/document`, `/api/style` | Persona state/document, session style |
| POST | `/api/soul/proposals` | Propose |
| GET, POST | `/api/soul/proposals/{id}` | Review/decide |
| GET | `/api/skills` | List skills |
| POST | `/api/skills/check` | Fixed check description |
| GET, POST | `/api/memory` | Pins status/toggle |
| POST | `/api/memory/add`, `/api/memory/forget`, `/api/memory/clear` | Pin mutations |

## Structured memory (account-private)

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/structured-memory`, `/api/structured-memory/search`, `/api/structured-memory/export` | Gates, FTS, export |
| GET, POST | `/api/structured-memory/facts`, `/api/structured-memory/proposals`, `/api/structured-memory/consolidation` | List/add, list/propose, list/start |
| GET | `/api/structured-memory/facts/{id}`, `/api/structured-memory/episodes`, `/api/structured-memory/episodes/{id}`, `/api/structured-memory/consolidation/{id}` | Record/status reads |
| GET, POST | `/api/structured-memory/proposals/{id}` | Review/decide |
| POST | `/api/structured-memory/gates`, `/api/structured-memory/facts/{id}/deactivate` | Admin override, confirmed deactivation |
| POST | `/api/structured-memory/episodes/{id}/summary`, `/api/structured-memory/episodes/{id}/delete`, `/api/structured-memory/episodes/purge`, `/api/structured-memory/purge` | Confirmed summary/delete/purge |
| POST | `/api/structured-memory/consolidation/{id}/cancel` | Cancel owned run |

See [memory confirmations](STRUCTURED_MEMORY.md#http-surfaces).

## Web research

| Method | Path | Purpose |
|---|---|---|
| GET, POST | `/api/web` | Status/toggle |
| POST | `/api/web/allow`, `/api/web/deny`, `/api/web/mode`, `/api/web/block`, `/api/web/unblock` | URL policy |
| POST | `/api/web/check`, `/api/web/fetch`, `/api/web/search` | Check, read, search |
| POST | `/api/web/research`, `/api/web/research/cancel` | Research/cancel |
| GET, POST | `/api/web/research/jobs` | List/start detached work |
| GET | `/api/web/research/jobs/{job_id}` | Owned job |
| POST | `/api/web/inject`, `/api/web/forget` | Stage/drop extract |

## Browser tasks

| Method | Path | Purpose |
|---|---|---|
| POST | `/api/browser/run`, `/api/browser/jobs` | Admin `{task, origins}`, sync/detached |
| GET | `/api/browser/jobs/{job_id}` | Report or pending request |
| POST | `/api/browser/jobs/{job_id}/cancel`, `/api/browser/jobs/{job_id}/deny` | Stop/deny |
| POST | `/api/browser/jobs/{job_id}/approve` | One-shot `{request, sha256, confirm, reason}` |

## MCP

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/mcp` | Declared capabilities |
| POST | `/api/mcp/call` | Confirmed declared tool call |

The private machine gateway serves `POST /mcp` on a
[separate listener](MCP.md#deliberate-local-activation).

## Coding agent

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/github/status`, `/api/agent/checks` | Git posture, fixed profiles |
| POST | `/api/agent/run` | Synchronous run |
| GET, POST | `/api/agent/jobs`, `/api/agent/schedules` | List/start owned work |
| GET | `/api/agent/jobs/{job_id}`, `/api/agent/schedules/{schedule_id}` | Owned status |
| POST | `/api/agent/jobs/{job_id}/cancel`, `/api/agent/schedules/{schedule_id}/cancel`, `/api/agent/schedules/preview` | Cancel job/schedule, preview schedule |
| GET | `/api/agent/runs`, `/api/agent/runs/{run_id}` | Retained runs, diff |
| POST | `/api/agent/runs/{run_id}/decision`, `/api/agent/runs/{run_id}/push`, `/api/agent/runs/{run_id}/publish`, `/api/agent/runs/{run_id}/discard` | Decide, push, draft, discard |
| GET | `/api/notifications` | Owned delivery metadata |
| POST | `/api/notifications/{delivery_id}/replay` | Admin replay only |
