# src/server/routes

Router assembly. `mod.rs` holds `REGISTERED_PATHS` and `build_router`; a route
missing from the list shows as unwired in `/api/tools`, and the module's tests
fail on drift. Handlers are grouped by surface: `agent.rs` (run, jobs,
schedules via `prepare_run`), `auth.rs`, `core.rs`, `panels.rs`, `ollama.rs`,
`session_io.rs`, `structured_memory.rs`, `notifications.rs`, `goals.rs`,
`persona.rs`, `skills.rs`, `style.rs`, `notes_corpus.rs`.
