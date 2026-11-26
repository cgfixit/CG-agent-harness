//! Registry/tools/skills views, /web, /memory, /api keys, harness runs.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{FromRequest, State};
use axum::http::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::common::errors::HarnessError;
use crate::server::env_keys;
use crate::server::errors::{ApiError, ApiResult};
use crate::server::memory_notes::rag_flags;
use crate::server::schemas::*;
use crate::server::state::AppState;
use crate::server::views;

const MAX_RUNS: usize = 50;

/// The registry file the agentic child would use now. It loads config.yaml at
/// each spawn, so read the file, not the startup snapshot.
fn governed_registry(state: &AppState) -> std::path::PathBuf {
    let live = crate::common::config::AppConfig::load(&state.home.config_path());
    state
        .home
        .governed_registry_path(live.as_ref().unwrap_or(state.cfg.as_ref()))
}

/// These panels read config.yaml and registry files from disk on every call:
/// off the async workers.
pub async fn registry(State(state): State<Arc<AppState>>) -> Json<Value> {
    super::structured_memory::off_worker(move || Json(views::full_registry(&state.home, &governed_registry(&state))))
        .await
}

pub async fn tools(State(state): State<Arc<AppState>>) -> Json<Value> {
    super::structured_memory::off_worker(move || tools_report(&state)).await
}

fn tools_report(state: &AppState) -> Json<Value> {
    let extras = crate::netconnect::NetconnectConfig::from_config(state.cfg.as_ref())
        .map(|cfg| crate::netconnect::tools::catalog_rows(&cfg))
        .unwrap_or_default();
    let mut report = views::list_wired_tools_with(&super::registered_paths(), &extras);
    let config = crate::common::config::AppConfig::load(&state.home.config_path());
    for row in report["tools"].as_array_mut().unwrap() {
        let path = row["path"].as_str().unwrap_or("");
        if path.starts_with("/api/agent/") && path != "/api/agent/checks" {
            let enabled = config.as_ref().is_ok_and(|cfg| cfg.flag_is_true("agentic.enabled"));
            row["enabled"] = json!(enabled);
            if !enabled {
                row["ready"] = json!(false);
                row["unavailable_reason"] = json!("agentic layer disabled or config unreadable");
            }
        }
    }
    Json(report)
}

/// Read-only LAN panel. The query string is ignored. No method but GET is routed.
///
/// Rows come only from `state.netconnect_sources` through `panel_sources`
/// (which calls `collect_passive`). Those sources are fixed at startup.
/// Collection runs on the blocking pool: it reads local files and, on macOS,
/// waits on fixed `arp`/`netstat` children, so no async worker waits on it.
pub async fn netconnect_panel(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    let cfg = crate::netconnect::NetconnectConfig::from_config(state.cfg.as_ref())
        .map_err(|err| ApiError::from_err(StatusCode::BAD_REQUEST, &err))?;
    let body = super::structured_memory::off_worker(move || {
        crate::netconnect::tools::panel_sources(&cfg, &state.netconnect_sources)
    })
    .await
    .map_err(|err| ApiError::from_err(StatusCode::BAD_GATEWAY, &err))?;
    Ok(Json(body))
}

/// Diagnostics tier: one TCP connect per configured target and port.
///
/// The query string is ignored and no method but GET is routed, so the
/// browser supplies nothing: targets, ports and limits come from config. The
/// dial loop runs inside this request future, so the server dropping the
/// request stops it; the run deadline (at most 60 seconds) bounds it
/// otherwise. A gate refusal is 409. A run cut short by the budget is 200
/// with `ok: false`, `code: NETCONNECT_BUDGET`, and the partial results.
pub async fn netconnect_diag(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    let cfg = crate::netconnect::NetconnectConfig::from_config(state.cfg.as_ref())
        .map_err(|err| ApiError::from_err(StatusCode::BAD_REQUEST, &err))?;
    let body = crate::netconnect::tools::call_diag(&cfg, &state.netconnect_rate, crate::netconnect::LiveConnector)
        .await
        .map_err(|err| {
            let status = if err.is("NETCONNECT_DISABLED") || err.is("NETCONNECT_REFUSED") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_GATEWAY
            };
            ApiError::from_err(status, &err)
        })?;
    Ok(Json(body))
}

pub async fn skills(State(state): State<Arc<AppState>>) -> Json<Value> {
    super::structured_memory::off_worker(move || {
        Json(views::list_wired_skills(&state.home, &governed_registry(&state)))
    })
    .await
}

// ---------------------------------------------------------------- web

fn web_err(e: &HarnessError) -> ApiError {
    let status = match e.code.as_str() {
        "WEB_DISABLED"
        | "WEB_ALLOWLIST_EMPTY"
        | "WEB_BUSY"
        | "WEB_SEARCH_PACED"
        | "WEB_SEARCH_COOLDOWN"
        | crate::server::state::WINDOW_CHANGED => StatusCode::CONFLICT,
        "WEB_PERMISSION_DENIED" | "WEB_MODE_CONFIRM_REQUIRED" => StatusCode::FORBIDDEN,
        "WEB_RULE_UNKNOWN" => StatusCode::NOT_FOUND,
        "MODEL_SELECTION_REJECTED" => StatusCode::UNPROCESSABLE_ENTITY,
        "WEB_FETCH_FAILED" | "WEB_DNS" | "WEB_CLEAR_FAILED" => StatusCode::BAD_GATEWAY,
        _ => StatusCode::BAD_REQUEST,
    };
    // Fixed public guidance only; upstream exception bodies never reach the UI.
    let message = match e.code.as_str() {
        "WEB_BAD_URL" => "Use an exact HTTP(S) URL for fetch/check, not a wildcard or prose; /web allow takes patterns such as https://www.example.org/* or https://*.example.org/*. Use /web fetch URL --crawl or /web research --url URL <question> for permitted internal links; /help web shows examples.",
        "WEB_HOST_DENIED" => "This exact URL is outside the current shared-home allowlist. Run /web check URL; www and apex hosts are distinct. Only an administrator can grant a matching rule.",
        "WEB_BLOCKED" => "This URL is on the shared-home web block list. /web status lists blocked rules; an administrator can remove one with /web unblock.",
        "WEB_MODE_CONFIRM_REQUIRED" => "Switching to the block list lets every public URL that is not blocked be read. Confirm it in the console dialog, or pass --confirm to cgagentharness web mode denylist.",
        "WEB_BLOCKLIST_FULL" => "The block list is at its rule cap (web.max_rules, restart-only); nothing was added. Remove entries with /web unblock, or raise web.max_rules in config.yaml and restart.",
        "WEB_ALLOWLIST_EMPTY" => "No content URLs are permitted. An administrator must add an exact URL or explicit wildcard using /web allow, e.g. /web allow https://www.example.org/*. For /web search, a SerpAPI key in API Keys needs no URL rule; public Google needs https://www.google.com/*.",
        "WEB_GROUP_UNKNOWN" => "Unknown source group. /web status lists groups; add rules with /web allow PATTERN --group NAME.",
        "WEB_POLICY_INVALID" => "Invalid URL rule, group or seed, or a saved policy over web.max_rules. Each seed must be inside a requested rule; the entire grant was refused. After lowering web.max_rules, raise it again and restart, then /web deny extra rules.",
        "WEB_ALLOWLIST_FULL" => "The shared allowlist is at its rule cap (web.max_rules, restart-only); the entire grant was refused. Remove unused rules with /web deny, or raise web.max_rules in config.yaml and restart.",
        "WEB_DISABLED" => "Web access is disabled for this home. An administrator can enable it with /web on; URL permission is still required.",
        "WEB_GOOGLE_PERMISSION" => "Configure a Google results (SerpAPI) key in API Keys; it needs no URL grant. Otherwise an administrator can allow https://www.google.com/* for public Google, which may answer with a challenge. Exact homepage grants do not permit search URLs.",
        "WEB_GOOGLE_BLOCKED" => "Public Google refused or redirected the request. Save a Tavily or SerpAPI key in API Keys, then retry.",
        "WEB_GOOGLE_CHALLENGE" => "Public Google requires JavaScript or CAPTCHA. Save a Tavily or Google results (SerpAPI) key in API Keys, then retry.",
        "WEB_GOOGLE_UNREADABLE" => "Google returned no recognizable result listing. Save a Tavily or SerpAPI key in API Keys, then retry.",
        "WEB_SEARCH_KEY_REJECTED" => "The configured SerpAPI or Tavily key was rejected. Check API Keys; no fallback was attempted.",
        "WEB_SEARCH_QUOTA" => "SerpAPI or Tavily quota or rate limit reached, and no enabled fallback hop answered.",
        "WEB_SEARCH_PROVIDER" => "The search provider could not be reached or returned an error, and no enabled fallback hop answered (web.search.fallback_*). Nothing was fabricated; retry in a moment.",
        "WEB_SEARCH_PACED" => "Public Google searches stay at least 3 seconds apart, and the next one could not start within web.request_seconds. Wait a moment, then retry.",
        "WEB_SEARCH_KEY_MISSING" => "web.search.provider names a keyed provider (serpapi or tavily) whose key is not active. Add it in API Keys, or set web.search.provider to auto and restart.",
        // Harness-built text naming the provider and the local end time; never upstream content.
        "WEB_SEARCH_COOLDOWN" => &e.message,
        "WEB_RULE_UNKNOWN" => "No rule has that ID or pattern, so nothing was removed. /web status lists rule IDs and patterns; www and apex hosts, and patterns with and without /*, are different rules.",
        "WEB_NO_LAST" => "No saved extract yet. Run /web fetch URL or /web pages <query> first, then /web inject.",
        "WEB_DNS" => "The host did not resolve to a usable public address. Check the exact host (www and apex are distinct).",
        "WEB_FETCH_FAILED" => "The page could not be fetched (connection, TLS or HTTP error). Nothing was saved.",
        "WEB_TIMEOUT" => "The read did not finish within web.request_seconds. Nothing was saved; retry or pick a smaller page.",
        "WEB_REDIRECT_REFUSED" => "The site redirected, and redirects are refused. Allow and fetch the final URL directly.",
        "WEB_POLICY_UNAVAILABLE" => "The shared URL policy file is missing or unreadable, so web reads are refused. An administrator's /web allow creates it; an unreadable file is never reset.",
        "WEB_POLICY_CHANGED" => "The URL policy changed during this request; nothing was delivered. Retry.",
        "WEB_BAD_QUERY" => "The query, result count or --group was refused. Use 1–200 characters with searchable words (research: 1–2,000), --count 1–10 and a group /web status lists.",
        "MODEL_SELECTION_REJECTED" => "The selected model is an Ollama cloud model (a :cloud or -cloud tag), which sends the prompt to ollama.com. Web research needs a local model: choose one with /model use.",
        crate::server::state::WINDOW_CHANGED => "The local model's loaded window changed or could not be read; nothing was sent. Check that Ollama is running, then run the research again.",
        _ => &e.code,
    };
    ApiError::new(status, &e.code, message)
}

fn web_enabled(state: &AppState) -> bool {
    state.settings.lock().unwrap_or_else(|p| p.into_inner()).web_enabled
}

/// Run one web policy read or edit on the blocking pool. Each takes the policy
/// lock, reads or fsyncs the policy file, and `status` may ask the OS
/// credential store for a saved key, which can block on a locked store.
async fn web_blocking<T: Send + 'static>(
    state: &Arc<AppState>,
    work: impl FnOnce(&crate::server::web_search::WebTool, bool) -> T + Send + 'static,
) -> T {
    let (web, enabled) = (state.web_snapshot(), web_enabled(state));
    super::structured_memory::off_worker(move || work(&web, enabled)).await
}

pub async fn web_status(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
) -> ApiResult<Json<Value>> {
    // Every other caller of `status` is administrator-only; this read is not.
    let result_reads = super::auth::may_read_search_results(&state, user.as_deref());
    let owner = super::auth::context_owner(user);
    let mut status = web_blocking(&state, move |web, enabled| web.status(enabled, &owner))
        .await
        .map_err(|e| web_err(&e))?;
    if !result_reads {
        status["search_result_reads"] = json!(false);
    }
    Ok(Json(status))
}

/// Change one console setting the way `model_select` does: mutate a copy under
/// the settings lock, save it, and publish it only after the save succeeded, so
/// a failed save leaves the live value unchanged and concurrent toggles cannot
/// save out of order. The fsync'd write runs off the async workers.
pub(crate) async fn commit_setting(
    state: &Arc<AppState>,
    change: impl FnOnce(&mut crate::common::home::HarnessSettings) + Send + 'static,
) -> ApiResult<crate::common::home::HarnessSettings> {
    let state = state.clone();
    super::structured_memory::off_worker(move || {
        let mut s = state.settings.lock().unwrap_or_else(|p| p.into_inner());
        let mut snapshot = s.clone();
        change(&mut snapshot);
        snapshot
            .save(&state.home)
            .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
        *s = snapshot.clone();
        Ok(snapshot)
    })
    .await
}

pub async fn web_toggle(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<ToggleRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let snapshot = commit_setting(&state, move |s| s.web_enabled = req.enabled).await?;
    state
        .audit
        .log(json!({"event": "web.enabled_set", "owner": owner, "enabled": snapshot.web_enabled}));
    let (web, enabled) = (state.web_snapshot(), snapshot.web_enabled);
    super::structured_memory::off_worker(move || web.status(enabled, &owner))
        .await
        .map(Json)
        .map_err(|e| web_err(&e))
}

/// Who changed the shared URL policy, how, and by how many rules. Counts only:
/// the patterns stay in the policy file.
///
/// Counted, not fail-closed: a refused change must not add a policy line, and
/// this handler cannot roll the policy file back in one step. A dropped line
/// is in `audit_gap`.
fn audit_web_rules(state: &AppState, owner: &str, action: &str, rules: usize, changed: bool) {
    if changed {
        state
            .audit
            .log(json!({"event": "web.policy_rules", "owner": owner, "action": action, "rules": rules}));
    }
}

pub async fn web_allow(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<WebRuleRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let patterns = req.patterns();
    let (rules, status_owner) = (patterns.len(), owner.clone());
    let (changed, status) = web_blocking(&state, move |web, enabled| {
        let changed = web.allow_rules(&patterns, &req.group, &req.seeds, enabled);
        (
            changed.is_ok(),
            changed.and_then(|_| web.status(enabled, &status_owner)),
        )
    })
    .await;
    audit_web_rules(&state, &owner, "allow", rules, changed);
    status.map(Json).map_err(|e| web_err(&e))
}

pub async fn web_deny(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<WebUrlRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let status_owner = owner.clone();
    let (changed, status) = web_blocking(&state, move |web, enabled| {
        let changed = web.deny(&req.url, enabled);
        (
            changed.is_ok(),
            changed.and_then(|_| web.status(enabled, &status_owner)),
        )
    })
    .await;
    audit_web_rules(&state, &owner, "deny", 1, changed);
    status.map(Json).map_err(|e| web_err(&e))
}

pub async fn web_mode(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<WebModeRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let status_owner = owner.clone();
    let (mode, confirm) = (req.mode, req.confirm);
    let result = web_blocking(&state, move |web, enabled| {
        web.set_mode(mode, confirm, enabled)
            .and_then(|_| web.status(enabled, &status_owner))
    })
    .await
    .map(Json)
    .map_err(|e| web_err(&e));
    if result.is_ok() {
        // Counted for the same reason as `audit_web_rules`: the change is
        // already saved, and a refused attempt must not grow this event.
        state
            .audit
            .log(json!({"event": "web.policy_mode", "owner": owner, "mode": req.mode, "confirmed": req.confirm}));
    }
    result
}

pub async fn web_block(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<WebBlockRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let (rules, status_owner) = (req.urls.len(), owner.clone());
    let (changed, status) = web_blocking(&state, move |web, enabled| {
        let changed = web.block_rules(&req.urls, enabled);
        (
            changed.is_ok(),
            changed.and_then(|_| web.status(enabled, &status_owner)),
        )
    })
    .await;
    audit_web_rules(&state, &owner, "block", rules, changed);
    status.map(Json).map_err(|e| web_err(&e))
}

pub async fn web_unblock(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<WebUrlRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let status_owner = owner.clone();
    let (changed, status) = web_blocking(&state, move |web, enabled| {
        let changed = web.unblock(&req.url, enabled);
        (
            changed.is_ok(),
            changed.and_then(|_| web.status(enabled, &status_owner)),
        )
    })
    .await;
    audit_web_rules(&state, &owner, "unblock", 1, changed);
    status.map(Json).map_err(|e| web_err(&e))
}

pub async fn web_fetch(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<WebFetchRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let enabled = web_enabled(&state);
    let web = state.web_snapshot();
    if req.crawl {
        return web
            .crawl(&req.targets(), req.group.as_deref(), enabled, &state.audit)
            .await
            .map(Json)
            .map_err(|e| web_err(&e));
    }
    if let Some(url) = req.url.as_deref().filter(|_| req.group.is_none()) {
        return web
            .fetch(url, enabled, &state.audit, &owner)
            .await
            .map(Json)
            .map_err(|e| web_err(&e));
    }
    web.fetch_many(&req.targets(), req.group.as_deref(), enabled, &state.audit, &owner)
        .await
        .map(Json)
        .map_err(|e| web_err(&e))
}

pub async fn web_check(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<WebFetchRequest>,
) -> ApiResult<Json<Value>> {
    web_blocking(&state, move |web, enabled| {
        web.check_urls(&req.targets(), req.group.as_deref(), enabled)
    })
    .await
    .map(Json)
    .map_err(|e| web_err(&e))
}

pub async fn web_search(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<WebSearchRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let enabled = web_enabled(&state);
    if req.engine.as_deref() == Some("google") {
        if req.group.is_some() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "WEB_BAD_QUERY",
                "Google search cannot use a local source group",
            ));
        }
        let (query, count) = match crate::server::web_intent::parse_with_count(&req.query, req.count) {
            crate::server::web_intent::WebIntentParse::Rewrite(intent) => (intent.query(), intent.count),
            crate::server::web_intent::WebIntentParse::PassThrough => (req.query.clone(), req.count),
            crate::server::web_intent::WebIntentParse::Invalid(message) => {
                return Err(ApiError::new(StatusCode::BAD_REQUEST, "WEB_BAD_QUERY", message));
            }
        };
        return state
            .web_snapshot()
            .google_search(&query, count, enabled, &state.audit)
            .await
            .map(Json)
            .map_err(|e| web_err(&e));
    }
    state
        .web_snapshot()
        .search_with_count(
            &req.query,
            req.group.as_deref(),
            req.count,
            enabled,
            &state.audit,
            &owner,
        )
        .await
        .map(Json)
        .map_err(|e| web_err(&e))
}

pub async fn web_inject(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    web_blocking(&state, move |web, enabled| web.inject(enabled, &owner))
        .await
        .map(Json)
        .map_err(|e| web_err(&e))
}

pub async fn mcp_status(
    State(state): State<Arc<AppState>>,
    role: Option<axum::Extension<crate::common::authz::Role>>,
) -> Json<Value> {
    // The account gate parses the stored role once and hands it over as an
    // extension. Accounts off: the single local user holds full authority.
    let role = role.map(|r| r.0).unwrap_or(crate::common::authz::Role::Admin);
    let mut status = state.mcp.status(role);
    status["memory_gateway"] = super::super::mcp_server::configuration_status(&state.cfg);
    Json(status)
}

pub async fn mcp_call(
    State(state): State<Arc<AppState>>,
    role: Option<axum::Extension<crate::common::authz::Role>>,
    ValidJson(req): ValidJson<McpCallRequest>,
) -> ApiResult<Json<Value>> {
    // The gate already refused an unparseable stored role; a missing extension
    // means accounts are off and the single local user holds full authority.
    let role = role.map(|r| r.0).unwrap_or(crate::common::authz::Role::Admin);
    let arguments = if req.arguments.is_null() {
        json!({})
    } else {
        req.arguments
    };
    let result = state
        .mcp
        .call(
            role,
            &req.server,
            &req.tool,
            arguments,
            req.confirm,
            &state.mcp_tool_allowlist(),
            &state.home.root,
            &state.audit,
            &state.shim.exe,
        )
        .await
        .map_err(|e| {
            let status = match e.code.as_str() {
                "MCP_CONFIRM_REQUIRED"
                | "TOOL_DENIED"
                | "MCP_UNKNOWN_SERVER"
                | "MCP_UNKNOWN_TOOL"
                | "MCP_DISABLED"
                | "MCP_SSRF_DENIED"
                | "MCP_HOME_REFUSED"
                | "MCP_ROLE_REFUSED"
                | "MCP_CAPABILITY_REFUSED"
                | "MCP_CONTAINMENT_UNAVAILABLE" => StatusCode::FORBIDDEN,
                "VALIDATION_ERROR" | "CONFIG_ERROR" => StatusCode::BAD_REQUEST,
                "HARD_SANDBOX_UNAVAILABLE" => StatusCode::SERVICE_UNAVAILABLE,
                "MCP_TIMEOUT" => StatusCode::GATEWAY_TIMEOUT,
                _ => StatusCode::BAD_GATEWAY,
            };
            ApiError::from_err(status, &e)
        })?;
    Ok(Json(json!({"ok": true, "result": result})))
}

pub async fn web_forget(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    web_blocking(&state, move |web, enabled| web.forget(enabled, &owner))
        .await
        .map(Json)
        .map_err(|e| web_err(&e))
}

pub async fn web_research(State(state): State<Arc<AppState>>, req: Request<Body>) -> ApiResult<Json<Value>> {
    let owner = super::auth::web_owner(&state, &req)?;
    let ValidJson(body) = ValidJson::<WebResearchRequest>::from_request(req, &()).await?;
    if body.engine.as_deref().is_some_and(|v| v != "pages") {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "WEB_BAD_QUERY",
            "Dedicated research uses permitted pages",
        ));
    }
    crate::server::web_research::run_from(state, &owner, &body.query, body.group.as_deref(), &body.urls)
        .await
        .map(Json)
        .map_err(|e| web_err(&e))
}

/// `POST /api/web/research/jobs`: the same research as `/api/web/research`, run
/// detached. Refusals before the run holds its gates come back here; a started
/// run answers 202 with its job id.
pub async fn web_research_job_start(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let owner = super::auth::web_owner(&state, &req)?;
    let ValidJson(body) = ValidJson::<WebResearchRequest>::from_request(req, &()).await?;
    if body.engine.as_deref().is_some_and(|v| v != "pages") {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "WEB_BAD_QUERY",
            "Dedicated research uses permitted pages",
        ));
    }
    crate::server::web_research::start_job(state, owner, body.query, body.group, body.urls)
        .await
        .map(|job| (StatusCode::ACCEPTED, Json(job)))
        .map_err(|e| web_err(&e))
}

pub async fn web_research_jobs(State(state): State<Arc<AppState>>, req: Request<Body>) -> ApiResult<Json<Value>> {
    let owner = super::auth::web_owner(&state, &req)?;
    Ok(Json(json!({"jobs": state.web.research_jobs.list(&owner)})))
}

pub async fn web_research_job(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(job_id): axum::extract::Path<String>,
    req: Request<Body>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::web_owner(&state, &req)?;
    if !crate::common::is_lower_hex(&job_id, 32) {
        return Err(ApiError::bad_request(
            "INVALID_JOB_ID",
            "job_id must be 32 lowercase hex characters",
        ));
    }
    crate::server::web_research::deliver_job(&state, &owner, &job_id)
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "WEB_JOB_NOT_FOUND", "no such research job"))
}

pub async fn web_cancel(State(state): State<Arc<AppState>>, req: Request<Body>) -> ApiResult<Json<Value>> {
    let owner = super::auth::web_owner(&state, &req)?;
    state
        .web_snapshot()
        .research
        .cancel(&owner)
        .map(|cancelled| Json(json!({"cancelled":cancelled})))
        .map_err(|e| web_err(&e))
}

// ---------------------------------------------------------------- memory

fn memory_payload(state: &AppState) -> ApiResult<Value> {
    let enabled = state.settings.lock().unwrap_or_else(|p| p.into_inner()).memory_enabled;
    let mut payload = state
        .notes
        .status(enabled)
        .map_err(|e| ApiError::from_err(StatusCode::BAD_REQUEST, &e))?;
    payload["rag"] = rag_flags();
    payload["structured_memory"] = json!({
        "separate": true,
        "enabled": state.cfg.flag_is_true("structured_memory.enabled"),
        "episode_capture": crate::server::structured_memory::capture_available(
            &state.cfg,
            state.cfg.flag_is_true("structured_memory.enabled"),
            &crate::server::structured_memory::current_gates(state),
        ),
        "explicit_recall": crate::server::structured_memory::recall_available(
            &state.cfg,
            state.cfg.flag_is_true("structured_memory.enabled"),
            &crate::server::structured_memory::current_gates(state),
        ),
        "retrieval": crate::server::structured_memory::retrieval_available(
            &state.cfg,
            state.cfg.flag_is_true("structured_memory.enabled"),
            &crate::server::structured_memory::current_gates(state),
        ),
        "auto_retrieval": crate::server::structured_memory::auto_retrieval_available(
            &state.cfg,
            state.cfg.flag_is_true("structured_memory.enabled"),
            &crate::server::structured_memory::current_gates(state),
        ),
        "consolidation": crate::server::structured_memory::consolidation_available(
            &state.cfg,
            state.cfg.flag_is_true("structured_memory.enabled"),
            &crate::server::structured_memory::current_gates(state),
        ),
        "auto_consolidation": crate::server::structured_memory::auto_consolidation_available(
            &state.cfg,
            state.cfg.flag_is_true("structured_memory.enabled"),
            &crate::server::structured_memory::current_gates(state),
        ),
        "auto_suggest_chat": crate::server::structured_memory_suggest::available(
            state,
            crate::server::structured_memory_suggest::Source::Chat,
        ),
        "auto_suggest_coding": crate::server::structured_memory_suggest::available(
            state,
            crate::server::structured_memory_suggest::Source::Coding,
        ),
        "status_path": "/api/structured-memory",
        "prompt_toggle": "harness.json.memory_enabled remains pinned-note inclusion only",
        "session_clear": "does not delete derived episodes unless delete_derived_episodes is confirmed",
    });
    Ok(payload)
}

// `memory_payload` reads the note file: every caller runs it off the worker.
pub async fn memory_status(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    super::structured_memory::off_worker(move || memory_payload(&state).map(Json)).await
}

pub async fn memory_toggle(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<ToggleRequest>,
) -> ApiResult<Json<Value>> {
    commit_setting(&state, move |s| s.memory_enabled = req.enabled).await?;
    super::structured_memory::off_worker(move || memory_payload(&state).map(Json)).await
}

pub async fn memory_add(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<MemoryNoteRequest>,
) -> ApiResult<Json<Value>> {
    // The note file write is fsync'd: keep it and the status read off the worker.
    super::structured_memory::off_worker(move || {
        let added = state
            .notes
            .add(&req.text)
            .map_err(|e| ApiError::from_err(StatusCode::BAD_REQUEST, &e))?;
        let mut payload = memory_payload(&state)?;
        payload["added"] = added;
        Ok(Json(payload))
    })
    .await
}

pub async fn memory_forget(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<MemoryForgetRequest>,
) -> ApiResult<Json<Value>> {
    super::structured_memory::off_worker(move || {
        let forgotten = state
            .notes
            .forget(&req.id)
            .map_err(|e| ApiError::from_err(StatusCode::BAD_REQUEST, &e))?;
        let mut payload = memory_payload(&state)?;
        merge(&mut payload, forgotten);
        Ok(Json(payload))
    })
    .await
}

pub async fn memory_clear(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    super::structured_memory::off_worker(move || {
        let cleared = state
            .notes
            .clear()
            .map_err(|e| ApiError::from_err(StatusCode::BAD_REQUEST, &e))?;
        let mut payload = memory_payload(&state)?;
        merge(&mut payload, cleared);
        Ok(Json(payload))
    })
    .await
}

fn merge(target: &mut Value, extra: Value) {
    if let (Some(t), Some(e)) = (target.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            t.insert(k.clone(), v.clone());
        }
    }
}

// ---------------------------------------------------------------- keys

fn plaintext_keys(state: &AppState) -> bool {
    state
        .cfg
        .flag_is_true(crate::common::credential_store::PLAINTEXT_KEY_FILE)
}

fn with_key_store<T>(
    state: &AppState,
    body: impl FnOnce(&dyn crate::common::credential_store::CredentialStore) -> T,
) -> T {
    if let Some(store) = state.credential_store_test_override() {
        body(store)
    } else {
        let store = crate::common::credential_store::OsCredentialStore::for_home(&state.home.root);
        body(&store)
    }
}

fn key_status(state: &AppState) -> Result<Value, crate::common::errors::HarnessError> {
    let path = state.home.env_path();
    let plaintext = plaintext_keys(state);
    let (stored, warnings) = if plaintext {
        (
            env_keys::read_startup_keys(&path).map_err(|error| {
                crate::common::errors::HarnessError::new(env_keys::ENV_KEY_ERROR, error.to_string())
            })?,
            state.credential_warnings.clone(),
        )
    } else {
        let (stored, store_warning) = with_key_store(state, env_keys::saved_from_store)?;
        let mut warnings = state.credential_warnings.clone();
        if let Some(warning) = store_warning {
            warnings.push(warning);
        }
        if let Ok(names) = env_keys::legacy_managed_names(&path) {
            for name in names {
                if !warnings.iter().any(|warning| warning.contains(&name)) {
                    warnings.push(crate::common::credential_store::migration_warning(
                        &name,
                        crate::common::credential_store::StoreError::Unavailable,
                    ));
                }
            }
        }
        (stored, warnings)
    };
    let keys = if plaintext {
        env_keys::read_status(&path, &state.key_file_sources)?
    } else {
        env_keys::status_rows(&stored, &state.key_file_sources, true)
    };
    Ok(json!({
        "keys": keys,
        "env_file": path.display().to_string(),
        "store": if plaintext { "plaintext-file" } else { crate::common::credential_store::OS_STORE_LABEL },
        "credential_warnings": warnings,
    }))
}

pub async fn api_keys_status(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let body = super::structured_memory::off_worker(move || key_status(&state))
        .await
        .map_err(|e| ApiError::from_err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(body).into_response())
}

pub async fn api_keys_set(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<ApiKeysRequest>,
) -> ApiResult<Json<Value>> {
    // Keychain / Secret Service calls are synchronous and may block on the OS.
    // The audit append shares the blocking task with the write, so a dropped
    // handler cannot leave a stored key without its audit record.
    let worker = Arc::clone(&state);
    let written = super::structured_memory::off_worker(move || {
        let path = worker.home.env_path();
        // Fail closed: names only, never values. The store write runs only
        // after the record is queued, so a full queue leaves the keys untouched.
        let mut names: Vec<String> = req.keys.keys().cloned().chain(req.clear.iter().cloned()).collect();
        names.sort();
        names.dedup();
        if worker
            .audit
            .log_required(json!({"event": "harness_api_keys_updated", "keys": names}))
            .is_err()
        {
            return Err(HarnessError::new(
                "AUDIT_NOT_RECORDED",
                "the action was not committed because its audit record could not be queued",
            ));
        }
        let written = if plaintext_keys(&worker) {
            env_keys::update_keys(&path, &req.keys, &req.clear)
        } else {
            with_key_store(&worker, |store| env_keys::update_os_keys(store, &req.keys, &req.clear))
        }?;
        Ok::<_, crate::common::errors::HarnessError>(written)
    })
    .await
    .map_err(|e| {
        if e.code == "AUDIT_NOT_RECORDED" {
            ApiError::audit_not_recorded()
        } else if e.code == env_keys::ENV_KEY_ERROR {
            ApiError::new(StatusCode::BAD_REQUEST, "ENV_KEY_REJECTED", e.message.clone())
        } else {
            tracing::warn!("saving a managed key failed");
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ENV_KEY_WRITE_FAILED",
                "could not store the managed key",
            )
        }
    })?;
    let mut out = written;
    let status = super::structured_memory::off_worker(move || key_status(&state))
        .await
        .map_err(|e| ApiError::from_err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    out["keys"] = status["keys"].clone();
    out["credential_warnings"] = status["credential_warnings"].clone();
    out["store"] = status["store"].clone();
    Ok(Json(out))
}

// ---------------------------------------------------------------- harness runs

/// Lists and stats every accepted run record: off the async workers.
pub async fn harness_runs(State(state): State<Arc<AppState>>) -> Json<Value> {
    super::structured_memory::off_worker(move || harness_runs_report(&state)).await
}

fn harness_runs_report(state: &AppState) -> Json<Value> {
    let accepted = state.home.optimizer_runs_dir().join("accepted");
    let mut files: Vec<(std::time::SystemTime, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&accepted) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            // One lstat per entry; a symlink is not an accepted run record.
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            files.push((mtime, p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()));
        }
    }
    files.sort_by_key(|a| std::cmp::Reverse(a.0));
    // run_id only: the absolute path embeds the operator's home directory.
    let runs: Vec<Value> = files
        .into_iter()
        .take(MAX_RUNS)
        .map(|(_, stem)| json!({"run_id": stem}))
        .collect();
    Json(json!({"count": runs.len(), "runs": runs}))
}
