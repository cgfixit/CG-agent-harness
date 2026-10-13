# src/server/routes

Router assembly and handlers grouped by surface. A route missing from
`REGISTERED_PATHS` shows as unwired in `/api/tools`, and this module's tests fail
on drift. Every path sits behind the guard chain in [`../guards.rs`](../guards.rs).
Inventory: [docs/API_ROUTES.md](../../../docs/API_ROUTES.md).

- [`mod.rs`](mod.rs): `REGISTERED_PATHS`, `build_router` guard layering, the SSE helper.
- [`agent.rs`](agent.rs): `/api/github/status` and `/api/agent/*`: `prepare_run`, runs, jobs, schedules, decisions.
- [`auth.rs`](auth.rs): `/api/auth/*` sessions, RBAC user admin, the `/api/audit` reader.
- [`core.rs`](core.rs): `/api/status`, sessions, chat and cancel, model and soul toggles, spend, analytics, reload, uploads.
- [`goals.rs`](goals.rs): `/api/sessions/{id}/goal-stage`: goal-to-coding staging and completion state.
- [`notes_corpus.rs`](notes_corpus.rs): `/api/notes-corpus` list, multipart ingest, delete.
- [`notifications.rs`](notifications.rs): `/api/notifications` delivery status and bounded replay.
- [`ollama.rs`](ollama.rs): `/api/ollama/*` inventory, read-only profile, abortable pull, warmup status.
- [`panels.rs`](panels.rs): registry, tools and skills views, `/api/web/*`, `/api/mcp`, `/api/memory`, `/api/keys`, `/api/netconnect`.
- [`persona.rs`](persona.rs): `/api/soul/document`, proposals, `/api/prompt/preview`.
- [`session_io.rs`](session_io.rs): session export, transcript search, legacy listing and adopt.
- [`skills.rs`](skills.rs): `/api/skills/check` and per-session skill selection; context only, never executes.
- [`structured_memory.rs`](structured_memory.rs): `/api/structured-memory/*`; confirm-gated writes.
- [`style.rs`](style.rs): `/api/style` catalog and per-session selection.

Tests: [tests/shim_and_agent_routes.rs](../../../tests/shim_and_agent_routes.rs),
[tests/auth_guards.rs](../../../tests/auth_guards.rs), [tests/panels.rs](../../../tests/panels.rs),
[tests/smoke_wired_count.rs](../../../tests/smoke_wired_count.rs).
