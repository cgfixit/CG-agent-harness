//! Router assembly. Route paths listed in `REGISTERED_PATHS` feed `/api/tools`'s
//! `wired` computation (the same role `app.routes` plays in CyClaw).

pub mod agent;
pub mod auth;
pub mod browser;
pub mod core;
pub mod diag;
pub mod goals;
pub mod loop_runner;
pub mod notes_corpus;
pub mod notifications;
pub mod ollama;
pub mod panels;
pub mod persona;
pub mod session_io;
pub mod skills;
pub mod structured_memory;
pub mod style;

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::http::Method;
use axum::middleware;
use axum::routing::{delete, get, post};
use axum::Router;

use super::guards;
use super::state::AppState;
use crate::common::authz::{Capability, Role};

/// Every path the router registers (axum template syntax).
pub const REGISTERED_PATHS: [&str; 125] = [
    "/api/analytics/summary",
    "/api/config/reload",
    "/",
    "/preview/html",
    "/static/{name}",
    "/api/status",
    "/api/registry",
    "/api/tools",
    "/api/netconnect",
    "/api/netconnect/diag",
    "/api/diag",
    "/api/diag/read",
    "/api/diag/inject",
    "/api/diag/forget",
    "/api/skills",
    "/api/skills/check",
    "/api/sessions/{session_id}/skills",
    "/api/sessions/{session_id}/structured-facts",
    "/api/web",
    "/api/web/allow",
    "/api/web/deny",
    "/api/web/mode",
    "/api/web/block",
    "/api/web/unblock",
    "/api/web/fetch",
    "/api/web/check",
    "/api/web/search",
    "/api/web/research",
    "/api/web/research/cancel",
    "/api/web/research/jobs",
    "/api/web/research/jobs/{job_id}",
    "/api/web/inject",
    "/api/web/forget",
    "/api/mcp",
    "/api/mcp/call",
    "/api/memory",
    "/api/memory/add",
    "/api/memory/forget",
    "/api/memory/clear",
    "/api/structured-memory",
    "/api/structured-memory/gates",
    "/api/structured-memory/search",
    "/api/structured-memory/facts",
    "/api/structured-memory/facts/{id}",
    "/api/structured-memory/facts/{id}/deactivate",
    "/api/structured-memory/proposals",
    "/api/structured-memory/proposals/{id}",
    "/api/structured-memory/episodes",
    "/api/structured-memory/episodes/purge",
    "/api/structured-memory/episodes/{id}",
    "/api/structured-memory/episodes/{id}/summary",
    "/api/structured-memory/episodes/{id}/delete",
    "/api/structured-memory/export",
    "/api/structured-memory/purge",
    "/api/structured-memory/consolidation",
    "/api/structured-memory/consolidation/{id}",
    "/api/structured-memory/consolidation/{id}/cancel",
    "/api/spend/summary",
    "/api/spend/predict",
    "/api/sessions",
    "/api/sessions/search",
    "/api/sessions/legacy",
    "/api/sessions/{session_id}/adopt",
    "/api/sessions/clear",
    "/api/sessions/{session_id}",
    "/api/sessions/{session_id}/export",
    "/api/sessions/{session_id}/attachment-pins",
    "/api/sessions/{session_id}/rename",
    "/api/sessions/{session_id}/prompt-history",
    "/api/sessions/{session_id}/goal",
    "/api/sessions/{session_id}/goal-stage",
    "/api/prompt/preview",
    "/api/slash/parse",
    "/api/soul/document",
    "/api/soul/proposals",
    "/api/soul/proposals/{id}",
    "/api/soul",
    "/api/style",
    "/api/model",
    "/api/ollama/inventory",
    "/api/ollama/profile",
    "/api/ollama/pull",
    "/api/ollama/pull/cancel",
    "/api/keys",
    "/api/chat",
    "/api/chat/attachments",
    "/api/notes-corpus",
    "/api/notes-corpus/{id}",
    "/api/chat/cancel",
    "/api/chat/fetch-confirm",
    "/api/loop/start",
    "/api/loop/stop",
    "/api/loop/status",
    "/api/loop/events/{run_id}",
    "/api/github/status",
    "/api/agent/checks",
    "/api/agent/run",
    "/api/browser/run",
    "/api/browser/jobs",
    "/api/browser/jobs/{job_id}",
    "/api/browser/jobs/{job_id}/cancel",
    "/api/browser/jobs/{job_id}/approve",
    "/api/browser/jobs/{job_id}/deny",
    "/api/agent/runs",
    "/api/agent/runs/{run_id}",
    "/api/agent/runs/{run_id}/decision",
    "/api/agent/runs/{run_id}/push",
    "/api/agent/runs/{run_id}/publish",
    "/api/agent/runs/{run_id}/discard",
    "/api/agent/jobs",
    "/api/agent/jobs/{job_id}",
    "/api/agent/jobs/{job_id}/cancel",
    "/api/notifications",
    "/api/notifications/{delivery_id}/replay",
    "/api/agent/schedules",
    "/api/agent/schedules/preview",
    "/api/agent/schedules/{schedule_id}",
    "/api/agent/schedules/{schedule_id}/cancel",
    "/api/harness/runs",
    "/api/auth/setup-status",
    "/api/auth/login",
    "/api/auth/logout",
    "/api/auth/password",
    "/api/audit",
    "/api/auth/users/{username}/disabled",
];

/// Stream a worker's JSON events as SSE. Dropping the response (client gone)
/// aborts the worker task.
fn sse_from_channel(
    receiver: tokio::sync::mpsc::Receiver<serde_json::Value>,
    task: tokio::task::JoinHandle<()>,
) -> axum::response::Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use axum::response::IntoResponse;

    struct AbortOnDrop(tokio::task::AbortHandle);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let abort = AbortOnDrop(task.abort_handle());
    let stream = futures_util::stream::unfold((receiver, abort), |(mut receiver, abort)| async move {
        let event = receiver.recv().await?;
        Some((
            Ok::<_, std::convert::Infallible>(Event::default().data(event.to_string())),
            (receiver, abort),
        ))
    });
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

pub fn registered_paths() -> BTreeSet<String> {
    let mut set: BTreeSet<String> = REGISTERED_PATHS.iter().map(|s| s.to_string()).collect();
    for extra in [
        "/api/auth/whoami",
        "/api/auth/users",
        "/api/auth/users/{username}/password",
        "/api/auth/users/{username}/role",
        "/api/auth/users/{username}",
    ] {
        set.insert(extra.to_string());
    }
    set
}

/// The capability every registered `(method, route template)` requires (#965).
/// `guards::account_gate` resolves a request to one entry here and refuses
/// authenticated requests with no entry, so a new route is denied until this
/// table names it. Entries mirror `build_router` exactly; the coverage tests
/// below prove both directions.
pub const ROUTE_CAPS: &[(&str, Method, Capability)] = &[
    ("/", Method::GET, Capability::Public),
    ("/preview/html", Method::GET, Capability::Public),
    ("/static/{name}", Method::GET, Capability::Public),
    ("/api/status", Method::GET, Capability::Public),
    ("/api/auth/setup-status", Method::GET, Capability::Public),
    ("/api/auth/login", Method::POST, Capability::Public),
    ("/api/auth/whoami", Method::GET, Capability::OwnAccount),
    ("/api/auth/password", Method::POST, Capability::OwnAccount),
    ("/api/auth/logout", Method::POST, Capability::OwnAccount),
    ("/api/auth/users", Method::GET, Capability::ManageAccounts),
    ("/api/auth/users", Method::POST, Capability::ManageAccounts),
    ("/api/auth/users/{username}", Method::DELETE, Capability::ManageAccounts),
    (
        "/api/auth/users/{username}/disabled",
        Method::POST,
        Capability::ManageAccounts,
    ),
    (
        "/api/auth/users/{username}/password",
        Method::POST,
        Capability::ManageAccounts,
    ),
    (
        "/api/auth/users/{username}/role",
        Method::POST,
        Capability::ManageAccounts,
    ),
    ("/api/config/reload", Method::POST, Capability::ManageConfig),
    ("/api/structured-memory/gates", Method::POST, Capability::ManageConfig),
    ("/api/keys", Method::GET, Capability::ManageCredentials),
    ("/api/keys", Method::POST, Capability::ManageCredentials),
    ("/api/browser/run", Method::POST, Capability::BrowserUse),
    ("/api/browser/jobs", Method::POST, Capability::BrowserUse),
    ("/api/browser/jobs/{job_id}", Method::GET, Capability::BrowserUse),
    (
        "/api/browser/jobs/{job_id}/cancel",
        Method::POST,
        Capability::BrowserUse,
    ),
    (
        "/api/browser/jobs/{job_id}/approve",
        Method::POST,
        Capability::BrowserUse,
    ),
    ("/api/browser/jobs/{job_id}/deny", Method::POST, Capability::BrowserUse),
    ("/api/web", Method::POST, Capability::ManageWebPolicy),
    ("/api/web/allow", Method::POST, Capability::ManageWebPolicy),
    ("/api/web/deny", Method::POST, Capability::ManageWebPolicy),
    ("/api/web/mode", Method::POST, Capability::ManageWebPolicy),
    ("/api/web/block", Method::POST, Capability::ManageWebPolicy),
    ("/api/web/unblock", Method::POST, Capability::ManageWebPolicy),
    ("/api/audit", Method::GET, Capability::ReadAudit),
    ("/api/diag", Method::GET, Capability::ReadLogs),
    ("/api/diag/read", Method::GET, Capability::ReadLogs),
    ("/api/diag/inject", Method::POST, Capability::ReadLogs),
    ("/api/diag/forget", Method::POST, Capability::ReadLogs),
    ("/api/chat", Method::POST, Capability::Chat),
    ("/api/chat/cancel", Method::POST, Capability::Chat),
    ("/api/prompt/preview", Method::POST, Capability::Chat),
    ("/api/slash/parse", Method::POST, Capability::Chat),
    ("/api/chat/attachments", Method::POST, Capability::ChatAttachments),
    ("/api/sessions", Method::GET, Capability::OwnSessions),
    ("/api/sessions", Method::POST, Capability::OwnSessions),
    ("/api/sessions/{session_id}", Method::GET, Capability::OwnSessions),
    ("/api/sessions/search", Method::POST, Capability::OwnData),
    // Unassigned legacy sessions are shared data, not the caller's own:
    // administrator-only, as `session_io::legacy_owner` also enforces.
    ("/api/sessions/legacy", Method::GET, Capability::ManageSharedContent),
    ("/api/sessions/clear", Method::POST, Capability::OwnData),
    (
        "/api/sessions/{session_id}/adopt",
        Method::POST,
        Capability::ManageSharedContent,
    ),
    (
        "/api/sessions/{session_id}/attachment-pins",
        Method::DELETE,
        Capability::OwnData,
    ),
    ("/api/sessions/{session_id}/export", Method::GET, Capability::OwnData),
    ("/api/sessions/{session_id}/rename", Method::POST, Capability::OwnData),
    (
        "/api/sessions/{session_id}/prompt-history",
        Method::POST,
        Capability::OwnData,
    ),
    ("/api/sessions/{session_id}/goal", Method::POST, Capability::OwnData),
    (
        "/api/sessions/{session_id}/goal-stage",
        Method::GET,
        Capability::OwnData,
    ),
    (
        "/api/sessions/{session_id}/goal-stage",
        Method::POST,
        Capability::OwnData,
    ),
    ("/api/sessions/{session_id}/skills", Method::GET, Capability::OwnData),
    ("/api/sessions/{session_id}/skills", Method::POST, Capability::OwnData),
    (
        "/api/sessions/{session_id}/structured-facts",
        Method::GET,
        Capability::OwnData,
    ),
    (
        "/api/sessions/{session_id}/structured-facts",
        Method::POST,
        Capability::OwnData,
    ),
    ("/api/structured-memory", Method::GET, Capability::OwnData),
    ("/api/structured-memory/search", Method::GET, Capability::OwnData),
    ("/api/structured-memory/facts", Method::GET, Capability::OwnData),
    ("/api/structured-memory/facts", Method::POST, Capability::OwnData),
    ("/api/structured-memory/facts/{id}", Method::GET, Capability::OwnData),
    (
        "/api/structured-memory/facts/{id}/deactivate",
        Method::POST,
        Capability::OwnData,
    ),
    ("/api/structured-memory/proposals", Method::GET, Capability::OwnData),
    ("/api/structured-memory/proposals", Method::POST, Capability::OwnData),
    (
        "/api/structured-memory/proposals/{id}",
        Method::GET,
        Capability::OwnData,
    ),
    (
        "/api/structured-memory/proposals/{id}",
        Method::POST,
        Capability::OwnData,
    ),
    ("/api/structured-memory/episodes", Method::GET, Capability::OwnData),
    (
        "/api/structured-memory/episodes/purge",
        Method::POST,
        Capability::OwnData,
    ),
    ("/api/structured-memory/episodes/{id}", Method::GET, Capability::OwnData),
    (
        "/api/structured-memory/episodes/{id}/summary",
        Method::POST,
        Capability::OwnData,
    ),
    (
        "/api/structured-memory/episodes/{id}/delete",
        Method::POST,
        Capability::OwnData,
    ),
    ("/api/structured-memory/export", Method::GET, Capability::OwnData),
    ("/api/structured-memory/purge", Method::POST, Capability::OwnData),
    ("/api/structured-memory/consolidation", Method::GET, Capability::OwnData),
    (
        "/api/structured-memory/consolidation",
        Method::POST,
        Capability::OwnData,
    ),
    (
        "/api/structured-memory/consolidation/{id}",
        Method::GET,
        Capability::OwnData,
    ),
    (
        "/api/structured-memory/consolidation/{id}/cancel",
        Method::POST,
        Capability::OwnData,
    ),
    ("/api/notes-corpus", Method::GET, Capability::OwnData),
    ("/api/notes-corpus", Method::POST, Capability::OwnData),
    ("/api/notes-corpus/{id}", Method::DELETE, Capability::OwnData),
    ("/api/spend/summary", Method::GET, Capability::OwnData),
    ("/api/analytics/summary", Method::GET, Capability::OwnData),
    ("/api/spend/predict", Method::POST, Capability::OwnData),
    ("/api/loop/start", Method::POST, Capability::OwnData),
    ("/api/loop/stop", Method::POST, Capability::OwnData),
    ("/api/loop/status", Method::GET, Capability::OwnData),
    ("/api/loop/events/{run_id}", Method::GET, Capability::OwnData),
    ("/api/notifications", Method::GET, Capability::OwnData),
    (
        "/api/notifications/{delivery_id}/replay",
        Method::POST,
        Capability::NotificationReplay,
    ),
    ("/api/web/research/jobs", Method::GET, Capability::OwnData),
    ("/api/web/research/jobs", Method::POST, Capability::OwnData),
    ("/api/web/research/jobs/{job_id}", Method::GET, Capability::OwnData),
    ("/api/web", Method::GET, Capability::WebUse),
    ("/api/web/fetch", Method::POST, Capability::WebUse),
    ("/api/web/check", Method::POST, Capability::WebUse),
    ("/api/web/search", Method::POST, Capability::WebUse),
    ("/api/web/research", Method::POST, Capability::WebUse),
    ("/api/web/research/cancel", Method::POST, Capability::WebUse),
    ("/api/web/inject", Method::POST, Capability::WebUse),
    ("/api/web/forget", Method::POST, Capability::WebUse),
    ("/api/chat/fetch-confirm", Method::POST, Capability::WebUse),
    ("/api/mcp", Method::GET, Capability::UseMcp),
    ("/api/mcp/call", Method::POST, Capability::UseMcp),
    ("/api/soul", Method::GET, Capability::ManageSharedContent),
    ("/api/soul", Method::POST, Capability::ManageSharedContent),
    ("/api/soul/document", Method::GET, Capability::ManageSharedContent),
    ("/api/soul/document", Method::POST, Capability::ManageSharedContent),
    ("/api/soul/proposals", Method::POST, Capability::ProposePersona),
    ("/api/soul/proposals/{id}", Method::GET, Capability::ManageSharedContent),
    (
        "/api/soul/proposals/{id}",
        Method::POST,
        Capability::ManageSharedContent,
    ),
    ("/api/style", Method::GET, Capability::ReadPanels),
    // Style selection is per-owner per-session (`style::select` stores through
    // `store.for_owner`), not shared state, so it is own-data, not admin.
    ("/api/style", Method::POST, Capability::OwnData),
    ("/api/memory", Method::GET, Capability::ManageSharedContent),
    ("/api/memory", Method::POST, Capability::ManageSharedContent),
    ("/api/memory/add", Method::POST, Capability::ManageSharedContent),
    ("/api/memory/forget", Method::POST, Capability::ManageSharedContent),
    ("/api/memory/clear", Method::POST, Capability::ManageSharedContent),
    ("/api/model", Method::POST, Capability::ManageModels),
    ("/api/ollama/inventory", Method::GET, Capability::ManageModels),
    ("/api/ollama/profile", Method::GET, Capability::ManageModels),
    ("/api/ollama/pull", Method::POST, Capability::ManageModels),
    ("/api/ollama/pull/cancel", Method::POST, Capability::ManageModels),
    ("/api/registry", Method::GET, Capability::ReadPanels),
    ("/api/tools", Method::GET, Capability::ReadPanels),
    ("/api/skills", Method::GET, Capability::ReadPanels),
    ("/api/skills/check", Method::POST, Capability::ReadPanels),
    ("/api/netconnect", Method::GET, Capability::ReadPanels),
    ("/api/netconnect/diag", Method::GET, Capability::ReadPanels),
    ("/api/github/status", Method::GET, Capability::RunCoding),
    ("/api/agent/checks", Method::GET, Capability::RunCoding),
    ("/api/agent/run", Method::POST, Capability::RunCoding),
    ("/api/agent/runs", Method::GET, Capability::RunCoding),
    ("/api/agent/runs/{run_id}", Method::GET, Capability::RunCoding),
    ("/api/agent/runs/{run_id}/decision", Method::POST, Capability::RunCoding),
    ("/api/agent/runs/{run_id}/push", Method::POST, Capability::RunCoding),
    ("/api/agent/runs/{run_id}/publish", Method::POST, Capability::RunCoding),
    ("/api/agent/runs/{run_id}/discard", Method::POST, Capability::RunCoding),
    ("/api/agent/jobs", Method::GET, Capability::RunCoding),
    ("/api/agent/jobs", Method::POST, Capability::RunCoding),
    ("/api/agent/jobs/{job_id}", Method::GET, Capability::RunCoding),
    ("/api/agent/jobs/{job_id}/cancel", Method::POST, Capability::RunCoding),
    ("/api/agent/schedules", Method::GET, Capability::RunCoding),
    ("/api/agent/schedules", Method::POST, Capability::RunCoding),
    ("/api/agent/schedules/preview", Method::POST, Capability::RunCoding),
    ("/api/agent/schedules/{schedule_id}", Method::GET, Capability::RunCoding),
    (
        "/api/agent/schedules/{schedule_id}/cancel",
        Method::POST,
        Capability::RunCoding,
    ),
    ("/api/harness/runs", Method::GET, Capability::RunCoding),
];

/// The capability a `(method, route template)` requires, or `None` when the
/// pair is unlisted (the gate refuses those by default).
pub fn route_capability(method: &Method, template: &str) -> Option<Capability> {
    ROUTE_CAPS
        .iter()
        .find(|(path, m, _)| *path == template && m == method)
        .map(|(_, _, cap)| *cap)
}

/// True when a wrong-method request on a listed template keeps the router's
/// 405 instead of the gate's 403: the template is the own-account surface
/// (reachable under a bootstrap password), or the role's surface is not
/// method-exact (`authz::denies_wrong_method`) and the role holds every
/// capability the template lists. Anything else fails closed.
pub fn wrong_method_passthrough(role: Role, template: &str) -> bool {
    let mut rows = ROUTE_CAPS.iter().filter(|(path, _, _)| *path == template).peekable();
    rows.peek().is_some()
        && (template_is_own_account(template)
            || (!crate::common::authz::denies_wrong_method(role)
                && rows.all(|(_, _, cap)| crate::common::authz::grants(role, *cap))))
}

/// True when every entry on the template is `OwnAccount` (whoami, own
/// password, logout), the only surface reachable before a bootstrap password
/// is replaced.
pub fn template_is_own_account(template: &str) -> bool {
    let mut rows = ROUTE_CAPS.iter().filter(|(path, _, _)| *path == template).peekable();
    rows.peek().is_some() && rows.all(|(_, _, cap)| *cap == Capability::OwnAccount)
}

pub fn build_router(state: Arc<AppState>) -> Router {
    // Request order, outermost first: request_log (when enabled) ->
    // security_headers -> trusted_host (loopback Host/authority) -> account_gate
    // -> the per-router CSRF layer -> handler.
    // account_gate covers every `/api/` path: same-origin -> direct
    // loopback/no proxy -> rate limit -> account/RBAC (with auth enabled, only
    // /api/status and the setup-status and login routes are reachable
    // without a session). `/` and `/static/*` skip account_gate.
    //
    // Reads: account_gate only, no CSRF.
    let open = Router::new()
        .route("/", get(super::console::console))
        .route("/preview/html", get(super::console::preview_html))
        .route("/static/{name}", get(super::console::static_asset))
        .route("/api/status", get(core::status))
        .route("/api/audit", get(auth::audit_events))
        .route("/api/registry", get(panels::registry))
        .route("/api/tools", get(panels::tools))
        .route("/api/skills", get(panels::skills))
        .route("/api/web", get(panels::web_status))
        .route("/api/sessions", get(core::list_sessions))
        .route("/api/soul", get(core::soul_state))
        .route("/api/style", get(style::catalog))
        .route("/api/agent/checks", get(agent::agent_checks))
        .route("/api/harness/runs", get(panels::harness_runs));

    // Mutations and guarded reads: account_gate, then the process CSRF token.
    let guarded = Router::new()
        .route("/api/netconnect", get(panels::netconnect_panel))
        .route("/api/netconnect/diag", get(panels::netconnect_diag))
        .route("/api/diag", get(diag::status))
        .route("/api/diag/read", get(diag::read))
        .route("/api/diag/inject", post(diag::inject))
        .route("/api/diag/forget", post(diag::forget))
        .route("/api/config/reload", post(core::reload_config))
        .route(
            "/api/sessions/{session_id}/goal-stage",
            get(goals::status).post(goals::stage),
        )
        .route("/api/skills/check", post(skills::check))
        .route(
            "/api/sessions/{session_id}/skills",
            get(skills::selection).post(skills::select),
        )
        .route(
            "/api/sessions/{session_id}/structured-facts",
            get(structured_memory::selection).post(structured_memory::select),
        )
        .route("/api/prompt/preview", post(persona::preview))
        .route("/api/slash/parse", post(core::slash_parse))
        .route("/api/soul/document", get(persona::document).post(persona::edit))
        .route("/api/soul/proposals", post(persona::propose))
        .route("/api/soul/proposals/{id}", get(persona::review).post(persona::decide))
        .route("/api/web", post(panels::web_toggle))
        .route("/api/web/allow", post(panels::web_allow))
        .route("/api/web/deny", post(panels::web_deny))
        .route("/api/web/mode", post(panels::web_mode))
        .route("/api/web/block", post(panels::web_block))
        .route("/api/web/unblock", post(panels::web_unblock))
        .route("/api/web/fetch", post(panels::web_fetch))
        .route("/api/web/check", post(panels::web_check))
        .route("/api/web/search", post(panels::web_search))
        .route("/api/web/research", post(panels::web_research))
        .route("/api/web/research/cancel", post(panels::web_cancel))
        .route(
            "/api/web/research/jobs",
            post(panels::web_research_job_start).get(panels::web_research_jobs),
        )
        .route("/api/web/research/jobs/{job_id}", get(panels::web_research_job))
        .route("/api/web/inject", post(panels::web_inject))
        .route("/api/web/forget", post(panels::web_forget))
        .route("/api/mcp", get(panels::mcp_status))
        .route("/api/mcp/call", post(panels::mcp_call))
        .route("/api/memory", get(panels::memory_status).post(panels::memory_toggle))
        .route("/api/memory/add", post(panels::memory_add))
        .route("/api/memory/forget", post(panels::memory_forget))
        .route("/api/memory/clear", post(panels::memory_clear))
        .route("/api/structured-memory", get(structured_memory::status))
        .route("/api/structured-memory/gates", post(structured_memory::set_gates))
        .route("/api/structured-memory/search", get(structured_memory::search_facts))
        .route(
            "/api/structured-memory/facts",
            get(structured_memory::list_facts).post(structured_memory::add_fact),
        )
        .route("/api/structured-memory/facts/{id}", get(structured_memory::get_fact))
        .route(
            "/api/structured-memory/facts/{id}/deactivate",
            post(structured_memory::deactivate_fact),
        )
        .route(
            "/api/structured-memory/proposals",
            get(structured_memory::list_proposals).post(structured_memory::propose),
        )
        .route(
            "/api/structured-memory/proposals/{id}",
            get(structured_memory::review).post(structured_memory::decide),
        )
        .route("/api/structured-memory/episodes", get(structured_memory::list_episodes))
        .route(
            "/api/structured-memory/episodes/purge",
            post(structured_memory::purge_expired),
        )
        .route(
            "/api/structured-memory/episodes/{id}",
            get(structured_memory::get_episode),
        )
        .route(
            "/api/structured-memory/episodes/{id}/summary",
            post(structured_memory::summarize_episode),
        )
        .route(
            "/api/structured-memory/episodes/{id}/delete",
            post(structured_memory::delete_episode),
        )
        .route("/api/structured-memory/export", get(structured_memory::export_owner))
        .route("/api/structured-memory/purge", post(structured_memory::purge_owner))
        .route(
            "/api/structured-memory/consolidation",
            get(structured_memory::list_consolidation).post(structured_memory::start_consolidation),
        )
        .route(
            "/api/structured-memory/consolidation/{id}",
            get(structured_memory::get_consolidation),
        )
        .route(
            "/api/structured-memory/consolidation/{id}/cancel",
            post(structured_memory::cancel_consolidation),
        )
        .route("/api/spend/summary", get(core::spend_summary))
        .route("/api/analytics/summary", get(core::analytics_summary))
        .route("/api/spend/predict", post(core::spend_predict))
        .route("/api/sessions", post(core::create_session))
        .route("/api/sessions/search", post(session_io::search_session_transcripts))
        .route("/api/sessions/legacy", get(session_io::legacy_sessions))
        .route("/api/sessions/{session_id}/adopt", post(session_io::adopt_session))
        .route("/api/sessions/clear", post(core::clear_sessions))
        .route("/api/sessions/{session_id}", get(core::get_session))
        .route(
            "/api/sessions/{session_id}/attachment-pins",
            axum::routing::delete(core::clear_attachment_pins),
        )
        .route("/api/sessions/{session_id}/export", get(session_io::export_session))
        .route("/api/sessions/{session_id}/rename", post(core::rename_session))
        .route("/api/sessions/{session_id}/prompt-history", post(core::record_prompt))
        .route("/api/sessions/{session_id}/goal", post(core::session_goal))
        .route("/api/soul", post(core::soul_toggle))
        .route("/api/style", post(style::select))
        .route("/api/model", post(core::model_select))
        .route("/api/ollama/inventory", get(ollama::inventory))
        .route("/api/ollama/profile", get(ollama::profile))
        .route("/api/ollama/pull", post(ollama::pull))
        .route("/api/ollama/pull/cancel", post(ollama::cancel_pull))
        .route("/api/keys", get(panels::api_keys_status).post(panels::api_keys_set))
        .route("/api/chat", post(core::chat))
        .route("/api/chat/attachments", post(core::upload_attachments))
        .route(
            "/api/notes-corpus",
            get(notes_corpus::list_notes).post(notes_corpus::ingest_notes),
        )
        .route("/api/notes-corpus/{id}", delete(notes_corpus::delete_note))
        .route("/api/chat/cancel", post(core::cancel_chat))
        .route("/api/chat/fetch-confirm", post(core::fetch_confirm))
        .route("/api/loop/start", post(loop_runner::start))
        .route("/api/loop/stop", post(loop_runner::stop))
        .route("/api/loop/status", get(loop_runner::status))
        .route("/api/loop/events/{run_id}", get(loop_runner::events))
        .route("/api/github/status", get(agent::github_status))
        .route("/api/agent/run", post(agent::agent_run))
        .route("/api/browser/run", post(browser::run))
        .route("/api/browser/jobs", post(browser::job_start))
        .route("/api/browser/jobs/{job_id}", get(browser::job_get))
        .route("/api/browser/jobs/{job_id}/cancel", post(browser::job_cancel))
        .route("/api/browser/jobs/{job_id}/approve", post(browser::job_approve))
        .route("/api/browser/jobs/{job_id}/deny", post(browser::job_deny))
        .route("/api/agent/runs", get(agent::agent_runs))
        .route("/api/agent/runs/{run_id}", get(agent::agent_run_status))
        .route("/api/agent/runs/{run_id}/decision", post(agent::agent_run_decision))
        .route("/api/agent/runs/{run_id}/push", post(agent::agent_run_push))
        .route("/api/agent/runs/{run_id}/publish", post(agent::agent_run_publish))
        .route("/api/agent/runs/{run_id}/discard", post(agent::agent_run_discard))
        .route(
            "/api/agent/jobs",
            get(agent::agent_jobs_list).post(agent::agent_job_create),
        )
        .route("/api/agent/jobs/{job_id}", get(agent::agent_job_get))
        .route("/api/agent/jobs/{job_id}/cancel", post(agent::agent_job_cancel))
        .route(
            "/api/agent/schedules",
            get(agent::schedules_list).post(agent::schedule_create),
        )
        .route("/api/notifications", get(notifications::status))
        .route("/api/notifications/{delivery_id}/replay", post(notifications::replay))
        .route("/api/agent/schedules/preview", post(agent::schedule_preview))
        .route("/api/agent/schedules/{schedule_id}", get(agent::schedule_get))
        .route(
            "/api/agent/schedules/{schedule_id}/cancel",
            post(agent::schedule_cancel),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), guards::guarded));

    // Session bootstrap and account reads: account_gate only, no CSRF.
    let auth_open = Router::new()
        .route("/api/auth/setup-status", get(auth::setup_status))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/whoami", get(auth::whoami))
        .route("/api/auth/users", get(auth::list_users));

    // Account mutations: account_gate, then the process CSRF token.
    let auth_guarded = Router::new()
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/password", post(auth::change_password))
        .route("/api/auth/users/{username}/disabled", post(auth::set_disabled))
        .route("/api/auth/users", post(auth::create_user))
        .route("/api/auth/users/{username}/password", post(auth::set_password))
        .route("/api/auth/users/{username}/role", post(auth::set_role))
        .route("/api/auth/users/{username}", delete(auth::delete_user))
        .route_layer(middleware::from_fn_with_state(state.clone(), guards::guarded));

    let request_log = state.request_log;
    let app = Router::new()
        .merge(open)
        .merge(guarded)
        .merge(auth_open)
        .merge(auth_guarded)
        .layer(middleware::from_fn_with_state(state.clone(), guards::account_gate))
        .layer(middleware::from_fn(super::headers::trusted_host))
        .layer(middleware::from_fn(super::headers::security_headers));
    // Outermost so the line records the status the client actually saw
    // (including trusted-host 400s and header-layer rewrites).
    let app = if request_log {
        app.layer(middleware::from_fn(super::request_log::request_log))
    } else {
        app
    };
    app.with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn router_route_paths() -> BTreeSet<String> {
        let src = include_str!("mod.rs");
        let start = src.find("pub fn build_router").expect("build_router must exist");
        let body = src[start..]
            .split("#[cfg(test)]")
            .next()
            .expect("build_router precedes the test module");
        let mut paths = BTreeSet::new();
        let mut rest = body;
        while let Some(idx) = rest.find(".route(") {
            rest = &rest[idx + ".route(".len()..];
            // `.route_layer(` also contains the `.route(` prefix.
            if rest.trim_start().starts_with("layer") {
                continue;
            }
            if let Some(q) = rest.find('"') {
                let after = &rest[q + 1..];
                if let Some(end) = after.find('"') {
                    paths.insert(after[..end].to_string());
                    rest = &after[end + 1..];
                    continue;
                }
            }
            break;
        }
        paths
    }

    #[test]
    fn registered_paths_are_unique_and_cover_every_router_route() {
        let mut listed = BTreeSet::new();
        for p in REGISTERED_PATHS {
            assert!(listed.insert(p), "duplicate REGISTERED_PATHS entry {p}");
        }
        assert_eq!(REGISTERED_PATHS.len(), 125);

        let all = registered_paths();
        assert!(
            all.len() > REGISTERED_PATHS.len(),
            "registered_paths() must include the auth extras"
        );
        for extra in [
            "/api/auth/whoami",
            "/api/auth/users",
            "/api/auth/users/{username}/password",
            "/api/auth/users/{username}/role",
            "/api/auth/users/{username}",
        ] {
            assert!(all.contains(extra), "missing extra {extra}");
            assert!(
                !REGISTERED_PATHS.contains(&extra),
                "{extra} belongs in registered_paths extras, not the const table"
            );
        }

        let routed = router_route_paths();
        assert!(!routed.is_empty(), "parser found no .route(...) paths");
        for path in &routed {
            assert!(
                all.contains(path),
                "{path} is routed in build_router but missing from registered_paths() — add it to REGISTERED_PATHS or the extras"
            );
        }
        for path in &all {
            assert!(
                routed.contains(path),
                "{path} is listed in registered_paths() but not routed in build_router"
            );
        }
    }

    /// `(path, method)` pairs parsed from each `.route(...)` call in
    /// `build_router`, the same source-scan approach as `router_route_paths`
    /// plus the method constructors in each call's handler expression.
    fn router_route_methods() -> BTreeSet<(String, String)> {
        let src = include_str!("mod.rs");
        let start = src.find("pub fn build_router").expect("build_router must exist");
        let body = src[start..]
            .split("#[cfg(test)]")
            .next()
            .expect("build_router precedes the test module");
        let mut routes = BTreeSet::new();
        let mut rest = body;
        while let Some(idx) = rest.find(".route(") {
            rest = &rest[idx + ".route(".len()..];
            // `.route_layer(` also contains the `.route(` prefix.
            if rest.trim_start().starts_with("layer") {
                continue;
            }
            let next = rest.find(".route(").unwrap_or(rest.len());
            let segment = &rest[..next];
            rest = &rest[next..];
            let Some(q) = segment.find('"') else { continue };
            let after = &segment[q + 1..];
            let Some(end) = after.find('"') else { continue };
            let path = after[..end].to_string();
            for (token, method) in [
                ("get(", "GET"),
                ("post(", "POST"),
                ("delete(", "DELETE"),
                ("put(", "PUT"),
                ("patch(", "PATCH"),
            ] {
                if segment.contains(token) {
                    routes.insert((path.clone(), method.to_string()));
                }
            }
        }
        routes
    }

    #[test]
    fn route_caps_cover_every_router_method_and_path() {
        let routed = router_route_methods();
        assert!(routed.len() > 100, "parser found only {} route methods", routed.len());
        let table: BTreeSet<(String, String)> = ROUTE_CAPS
            .iter()
            .map(|(path, method, _)| (path.to_string(), method.to_string()))
            .collect();
        assert_eq!(
            table.len(),
            ROUTE_CAPS.len(),
            "duplicate (path, method) rows in ROUTE_CAPS"
        );
        for (path, method) in &routed {
            assert!(
                table.contains(&(path.clone(), method.clone())),
                "{method} {path} is routed in build_router but missing from ROUTE_CAPS"
            );
        }
        for (path, method) in &table {
            assert!(
                routed.contains(&(path.clone(), method.clone())),
                "{method} {path} is listed in ROUTE_CAPS but not routed in build_router"
            );
        }
    }

    /// The gate lets `guards::PUBLIC_API_PATHS` through before it looks at
    /// this table, and never sees a path outside `/api/`. Both facts are a
    /// second copy of the `Public` rows: a path added to the gate's list
    /// whose row names another capability would reach its handler with no
    /// account at all.
    #[test]
    fn public_rows_are_exactly_what_the_gate_serves_without_an_account() {
        let gate: BTreeSet<&str> = guards::PUBLIC_API_PATHS.into_iter().collect();
        for (path, method, cap) in ROUTE_CAPS {
            let open = !path.starts_with("/api/") || gate.contains(path);
            assert_eq!(
                *cap == Capability::Public,
                open,
                "{method} {path}: the table says {cap:?}, the gate {}",
                if open {
                    "serves it without an account"
                } else {
                    "requires an account"
                }
            );
        }
        for path in gate {
            assert!(
                ROUTE_CAPS.iter().any(|(p, _, _)| *p == path),
                "{path} is public at the gate but has no ROUTE_CAPS row"
            );
        }
    }

    #[test]
    fn route_caps_rows_are_registered_paths() {
        let all = registered_paths();
        for (path, _, _) in ROUTE_CAPS {
            assert!(
                all.contains(*path),
                "{path} is listed in ROUTE_CAPS but missing from registered_paths()"
            );
        }
        for path in &all {
            assert!(
                ROUTE_CAPS.iter().any(|(p, _, _)| p == path),
                "{path} is registered but has no ROUTE_CAPS row"
            );
        }
    }

    #[test]
    fn route_capability_denies_unlisted_pairs() {
        assert_eq!(route_capability(&Method::POST, "/api/chat"), Some(Capability::Chat));
        // A registered template with an unlisted method resolves to nothing.
        assert_eq!(route_capability(&Method::POST, "/api/diag/read"), None);
        assert_eq!(route_capability(&Method::DELETE, "/api/chat"), None);
        // Unknown templates resolve to nothing: the gate refuses them.
        assert_eq!(route_capability(&Method::GET, "/api/not-a-route"), None);
        assert!(template_is_own_account("/api/auth/whoami"));
        assert!(!template_is_own_account("/api/chat"));
        assert!(!template_is_own_account("/api/not-a-route"));
        // Wrong-method passthrough keeps the 405 for roles a template fully
        // serves, and refuses the method-exact roles and unheld templates.
        assert!(wrong_method_passthrough(Role::Admin, "/api/diag/read"));
        assert!(wrong_method_passthrough(Role::Operator, "/api/sessions/clear"));
        assert!(!wrong_method_passthrough(Role::Auditor, "/api/diag/read"));
        assert!(!wrong_method_passthrough(Role::Operator, "/api/keys"));
        assert!(wrong_method_passthrough(Role::Auditor, "/api/auth/whoami"));
        assert!(!wrong_method_passthrough(Role::Admin, "/api/not-a-route"));
    }
}
