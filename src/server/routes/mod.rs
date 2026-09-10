//! Router assembly. Route paths listed in `REGISTERED_PATHS` feed `/api/tools`'s
//! `wired` computation (the same role `app.routes` plays in CyClaw).

pub mod agent;
pub mod auth;
pub mod core;
pub mod panels;

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::middleware;
use axum::routing::{delete, get, post};
use axum::Router;

use super::guards;
use super::state::AppState;

/// Every path the router registers (axum template syntax).
pub const REGISTERED_PATHS: [&str; 43] = [
    "/",
    "/static/{name}",
    "/api/status",
    "/api/registry",
    "/api/tools",
    "/api/skills",
    "/api/web",
    "/api/web/allow",
    "/api/web/deny",
    "/api/web/fetch",
    "/api/web/search",
    "/api/web/inject",
    "/api/web/forget",
    "/api/memory",
    "/api/memory/add",
    "/api/memory/forget",
    "/api/memory/clear",
    "/api/sessions",
    "/api/sessions/{session_id}",
    "/api/sessions/{session_id}/rename",
    "/api/sessions/{session_id}/goal",
    "/api/soul",
    "/api/model",
    "/api/keys",
    "/api/chat",
    "/api/chat/cancel",
    "/api/github/status",
    "/api/agent/checks",
    "/api/agent/run",
    "/api/agent/runs",
    "/api/agent/runs/{run_id}",
    "/api/agent/runs/{run_id}/decision",
    "/api/agent/runs/{run_id}/push",
    "/api/agent/runs/{run_id}/publish",
    "/api/agent/runs/{run_id}/discard",
    "/api/agent/jobs",
    "/api/agent/jobs/{job_id}",
    "/api/agent/jobs/{job_id}/cancel",
    "/api/harness/runs",
    "/api/auth/setup-status",
    "/api/auth/bootstrap-password",
    "/api/auth/login",
    "/api/auth/logout",
];

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

pub fn build_router(state: Arc<AppState>) -> Router {
    // Open routes: no key, no origin check, no CSRF, no rate limit.
    let open = Router::new()
        .route("/", get(super::console::console))
        .route("/static/{name}", get(super::console::static_asset))
        .route("/api/status", get(core::status))
        .route("/api/registry", get(panels::registry))
        .route("/api/tools", get(panels::tools))
        .route("/api/skills", get(panels::skills))
        .route("/api/web", get(panels::web_status))
        .route("/api/sessions", get(core::list_sessions))
        .route("/api/soul", get(core::soul_state))
        .route("/api/agent/checks", get(agent::agent_checks))
        .route("/api/harness/runs", get(panels::harness_runs));

    // Guarded: rate limit -> same-origin -> API key -> CSRF.
    let guarded = Router::new()
        .route("/api/web", post(panels::web_toggle))
        .route("/api/web/allow", post(panels::web_allow))
        .route("/api/web/deny", post(panels::web_deny))
        .route("/api/web/fetch", post(panels::web_fetch))
        .route("/api/web/search", post(panels::web_search))
        .route("/api/web/inject", post(panels::web_inject))
        .route("/api/web/forget", post(panels::web_forget))
        .route("/api/memory", get(panels::memory_status).post(panels::memory_toggle))
        .route("/api/memory/add", post(panels::memory_add))
        .route("/api/memory/forget", post(panels::memory_forget))
        .route("/api/memory/clear", post(panels::memory_clear))
        .route("/api/sessions", post(core::create_session))
        .route("/api/sessions/{session_id}", get(core::get_session))
        .route("/api/sessions/{session_id}/rename", post(core::rename_session))
        .route("/api/sessions/{session_id}/goal", post(core::session_goal))
        .route("/api/soul", post(core::soul_toggle))
        .route("/api/model", post(core::model_select))
        .route("/api/keys", get(panels::api_keys_status).post(panels::api_keys_set))
        .route("/api/chat", post(core::chat))
        .route("/api/chat/cancel", post(core::cancel_chat))
        .route("/api/github/status", get(agent::github_status))
        .route("/api/agent/run", post(agent::agent_run))
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
        .route_layer(middleware::from_fn_with_state(state.clone(), guards::guarded));

    let auth_open = Router::new()
        .route("/api/auth/setup-status", get(auth::setup_status))
        .route("/api/auth/bootstrap-password", post(auth::bootstrap_password))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/whoami", get(auth::whoami))
        .route("/api/auth/users", get(auth::list_users))
        .route_layer(middleware::from_fn_with_state(state.clone(), guards::auth_open));

    let auth_sess = Router::new()
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/users", post(auth::create_user))
        .route("/api/auth/users/{username}/password", post(auth::set_password))
        .route("/api/auth/users/{username}/role", post(auth::set_role))
        .route("/api/auth/users/{username}", delete(auth::delete_user))
        .route_layer(middleware::from_fn_with_state(state.clone(), guards::auth_sess));

    let request_log = state.request_log;
    let app = Router::new()
        .merge(open)
        .merge(guarded)
        .merge(auth_open)
        .merge(auth_sess)
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
        assert_eq!(REGISTERED_PATHS.len(), 43);

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
}
