//! `/api/auth/*`: per-user sessions and RBAC. Port of `harness/auth_routes.py`.
//! Every route returns 503 `AUTH_DISABLED` unless `auth.enabled` is the literal true.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::{json, Value};

use crate::common::auth_store::{AuthManager, UserSummary, BOOTSTRAP_USERNAME};
use crate::common::authn::validate_role;
use crate::common::errors::HarnessError;
use crate::server::errors::{ApiError, ApiResult};
use crate::server::guards::{is_loopback_peer, looks_proxied};
use crate::server::schemas::*;
use crate::server::state::AppState;

pub const SESSION_COOKIE: &str = "cgagentharness_session";
const ROLE_ADMIN: &str = "admin";
const ROLE_OPERATOR: &str = "operator";
const PERM_DENIED: &str = "AUTH_PERMISSION_DENIED";

fn manager(state: &AppState) -> ApiResult<&AuthManager> {
    state.auth.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "AUTH_DISABLED",
            "authentication is not enabled",
        )
    })
}

fn cookie_value(req: &Request<Body>) -> Option<String> {
    let raw = req.headers().get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let (k, v) = part.trim().split_once('=')?;
        if k.trim() == SESSION_COOKIE {
            return Some(v.trim().to_string());
        }
    }
    None
}

fn set_cookie(resp: &mut Response, value: &str, secure: bool) {
    let suffix = if secure { "; Secure" } else { "" };
    let v = format!("{SESSION_COOKIE}={value}; HttpOnly; SameSite=Strict; Path=/{suffix}");
    if let Ok(hv) = HeaderValue::from_str(&v) {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
}

fn clear_cookie(resp: &mut Response, secure: bool) {
    let suffix = if secure { "; Secure" } else { "" };
    let v = format!("{SESSION_COOKIE}=; Max-Age=0; HttpOnly; SameSite=Strict; Path=/{suffix}");
    if let Ok(hv) = HeaderValue::from_str(&v) {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
}

fn map_auth_error(e: &HarnessError) -> ApiError {
    match e.code.as_str() {
        "PASSWORD_POLICY" => ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "AUTH_POLICY", e.message.clone()),
        "AUTH_USER_NOT_FOUND" => ApiError::new(StatusCode::NOT_FOUND, &e.code, e.message.clone()),
        "AUTH_LAST_ADMIN" => ApiError::new(StatusCode::FORBIDDEN, &e.code, e.message.clone()),
        "AUTH_USER_EXISTS" => ApiError::new(StatusCode::CONFLICT, &e.code, e.message.clone()),
        "AUTH_BOOTSTRAP_COMPLETE" => ApiError::new(StatusCode::CONFLICT, &e.code, e.message.clone()),
        "AUTH_ACCOUNT_LOCKED" => ApiError::new(StatusCode::LOCKED, &e.code, e.message.clone()),
        "AUTH_LOGIN_FAILED" => ApiError::new(StatusCode::UNAUTHORIZED, &e.code, e.message.clone()),
        _ => ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "AUTH_ERROR", e.message.clone()),
    }
}

pub(crate) fn actor(state: &AppState, req: &Request<Body>) -> ApiResult<UserSummary> {
    if let Some(user) = req.extensions().get::<UserSummary>() {
        return Ok(user.clone());
    }
    let mgr = manager(state)?;
    let token = cookie_value(req).unwrap_or_default();
    let unauthorized = || ApiError::new(StatusCode::UNAUTHORIZED, "AUTH_REQUIRED", "authentication required");
    let info = mgr.validate_session(&token).ok_or_else(unauthorized)?;
    mgr.get_user(&info.username).ok_or_else(unauthorized)
}

/// Request-owned web state uses the authenticated account; explicitly disabled
/// authentication has one documented local operator namespace.
pub(super) fn web_owner(state: &AppState, req: &Request<Body>) -> ApiResult<String> {
    if state.auth.is_some() {
        actor(state, req).map(|u| u.user_id)
    } else {
        Ok("local".into())
    }
}

pub(super) fn context_owner(user: Option<Extension<UserSummary>>) -> String {
    user.map(|u| u.user_id.clone()).unwrap_or_else(|| "local".into())
}

fn denied() -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, PERM_DENIED, "denied")
}

fn require_user_admin(account: &UserSummary) -> ApiResult<()> {
    if account.role != ROLE_ADMIN || account.must_change_password {
        return Err(denied());
    }
    Ok(())
}

fn user_payload(u: &UserSummary) -> Value {
    json!({
        "username": u.username,
        "role": u.role,
        "disabled": u.disabled,
        "created_ts": u.created_ts,
        "last_login_ts": u.last_login_ts,
        "locked": u.locked_until_ts.is_some(),
        "must_change_password": u.must_change_password,
    })
}

pub async fn setup_status(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    let mgr = manager(&state)?;
    let pending = mgr.needs_password_setup();
    Ok(Json(json!({
        "enabled": true,
        "needs_password": pending,
        "username": if pending { Some(BOOTSTRAP_USERNAME) } else { None },
    })))
}

pub async fn bootstrap_password(State(state): State<Arc<AppState>>, req: Request<Body>) -> ApiResult<Response> {
    let secure = crate::server::guards::request_scheme(&req) == "https";
    manager(&state)?;
    if !is_loopback_peer(&req) || looks_proxied(req.headers()) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "AUTH_LOOPBACK_ONLY",
            "first password must be set from this machine without reverse-proxy forwarding headers",
        ));
    }
    let ValidJson(body) = ValidJson::<AuthSetPasswordRequest>::from_request(req, &()).await?;
    let st = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        manager(&st).and_then(|m| m.bootstrap_set_password(&body.password).map_err(|e| map_auth_error(&e)))
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "AUTH_ERROR", "auth task failed"))??;
    let mut resp =
        Json(json!({"username": result.username, "role": ROLE_ADMIN, "csrf_token": result.csrf_token})).into_response();
    set_cookie(&mut resp, &result.session_id, secure);
    Ok(resp)
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    scheme: Option<Extension<crate::server::transport::ListenerScheme>>,
    ValidJson(req): ValidJson<AuthLoginRequest>,
) -> ApiResult<Response> {
    manager(&state)?;
    let st = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        manager(&st).and_then(|m| m.login(&req.username, &req.password).map_err(|e| map_auth_error(&e)))
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "AUTH_ERROR", "auth task failed"))??;
    let role = manager(&state)?
        .get_user(&result.username)
        .map(|u| u.role)
        .unwrap_or_else(|| ROLE_OPERATOR.to_string());
    let must_change_password = manager(&state)?
        .get_user(&result.username)
        .is_some_and(|u| u.must_change_password);
    state
        .audit
        .log(json!({"event":"account.login", "actor":result.username,"outcome":"success"}));
    let mut resp = Json(json!({"username": result.username, "role": role, "csrf_token": result.csrf_token,"must_change_password":must_change_password})).into_response();
    set_cookie(&mut resp, &result.session_id, scheme.is_some_and(|s| s.0 .0 == "https"));
    Ok(resp)
}

pub async fn logout(State(state): State<Arc<AppState>>, req: Request<Body>) -> ApiResult<Response> {
    let mgr = manager(&state)?;
    if let Some(token) = cookie_value(&req) {
        mgr.logout(&token).map_err(|e| map_auth_error(&e))?;
    }
    let mut resp = Json(json!({"ok": true})).into_response();
    clear_cookie(&mut resp, crate::server::guards::request_scheme(&req) == "https");
    Ok(resp)
}

pub async fn whoami(State(state): State<Arc<AppState>>, req: Request<Body>) -> ApiResult<Json<Value>> {
    let account = actor(&state, &req)?;
    Ok(Json(
        json!({"username": account.username, "role": account.role,"must_change_password":account.must_change_password}),
    ))
}

pub async fn change_password(State(state): State<Arc<AppState>>, req: Request<Body>) -> ApiResult<Response> {
    let account = actor(&state, &req)?;
    let secure = crate::server::guards::request_scheme(&req) == "https";
    let ValidJson(body) = ValidJson::<AuthChangePasswordRequest>::from_request(req, &()).await?;
    let st = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        manager(&st)?
            .change_password(&account.username, &body.current_password, &body.password)
            .map_err(|e| map_auth_error(&e))
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "AUTH_ERROR", "auth task failed"))??;
    let mut response = Json(json!({"ok":true,"must_change_password":false})).into_response();
    set_cookie(&mut response, &result.session_id, secure);
    Ok(response)
}

pub async fn set_disabled(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    req: Request<Body>,
) -> ApiResult<Json<Value>> {
    require_user_admin(&actor(&state, &req)?)?;
    let ValidJson(body) = ValidJson::<AuthDisabledRequest>::from_request(req, &()).await?;
    let mgr = manager(&state)?;
    (if body.disabled {
        mgr.disable_user(&username)
    } else {
        mgr.enable_user(&username)
    })
    .map_err(|e| map_auth_error(&e))?;
    Ok(Json(json!({"ok":true})))
}

/// Audit accounts see only this fixed projection of request events. Model
/// content, paths in the filesystem, account records and keys are never read.
pub async fn audit_events(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = match options.open(state.audit.path()) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Json(json!({"events":[]}))),
        Err(_) => {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "AUDIT_UNAVAILABLE",
                "audit unavailable",
            ))
        }
    };
    let size = file
        .metadata()
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "AUDIT_UNAVAILABLE",
                "audit unavailable",
            )
        })?
        .len();
    file.seek(SeekFrom::Start(size.saturating_sub(65536))).map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "AUDIT_UNAVAILABLE",
            "audit unavailable",
        )
    })?;
    let mut text = String::new();
    file.take(65536).read_to_string(&mut text).map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "AUDIT_UNAVAILABLE",
            "audit unavailable",
        )
    })?;
    let events:Vec<Value>=text.lines().rev().filter_map(|line|serde_json::from_str::<Value>(line).ok()).filter(|v|v["event"]=="portal.request").take(100).map(|v|json!({"timestamp":v["timestamp"],"actor":v["actor"],"method":v["method"],"route":v["route"],"status":v["status"]})).collect();
    Ok(Json(json!({"events":events,"bounded":true})))
}

pub async fn list_users(State(state): State<Arc<AppState>>, req: Request<Body>) -> ApiResult<Json<Value>> {
    let account = actor(&state, &req)?;
    require_user_admin(&account)?;
    let rows: Vec<Value> = manager(&state)?.list_users().iter().map(user_payload).collect();
    Ok(Json(Value::Array(rows)))
}

pub async fn create_user(State(state): State<Arc<AppState>>, req: Request<Body>) -> ApiResult<Json<Value>> {
    let account = actor(&state, &req)?;
    require_user_admin(&account)?;
    let ValidJson(body) = ValidJson::<AuthCreateUserRequest>::from_request(req, &()).await?;
    // Canonicalize BEFORE the operator/admin comparison.
    let role = validate_role(body.role.as_deref().unwrap_or(crate::common::authn::DEFAULT_ROLE))
        .map_err(|e| map_auth_error(&e))?;
    if account.role == ROLE_OPERATOR && role == ROLE_ADMIN {
        return Err(denied());
    }
    let st = state.clone();
    let created = tokio::task::spawn_blocking(move || {
        manager(&st).and_then(|m| {
            m.create_user(&body.username, &body.password, &role)
                .map_err(|e| map_auth_error(&e))
        })
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "AUTH_ERROR", "auth task failed"))??;
    let user = manager(&state)?
        .get_user(&created)
        .ok_or_else(|| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "AUTH_ERROR", "created user missing"))?;
    Ok(Json(user_payload(&user)))
}

pub async fn set_password(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    req: Request<Body>,
) -> ApiResult<Json<Value>> {
    let account = actor(&state, &req)?;
    let mgr = manager(&state)?;
    let target = mgr
        .get_user(&username)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "AUTH_USER_NOT_FOUND", "unknown user"))?;
    if account.role == ROLE_OPERATOR && target.role == ROLE_ADMIN {
        return Err(denied());
    }
    require_user_admin(&account)?;
    let ValidJson(body) = ValidJson::<AuthSetPasswordRequest>::from_request(req, &()).await?;
    let st = state.clone();
    tokio::task::spawn_blocking(move || {
        manager(&st).and_then(|m| {
            m.set_password(&username, &body.password)
                .map_err(|e| map_auth_error(&e))
        })
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "AUTH_ERROR", "auth task failed"))??;
    Ok(Json(json!({"ok": true})))
}

pub async fn set_role(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    req: Request<Body>,
) -> ApiResult<Json<Value>> {
    let account = actor(&state, &req)?;
    if account.role != ROLE_ADMIN {
        return Err(denied());
    }
    let ValidJson(body) = ValidJson::<AuthSetRoleRequest>::from_request(req, &()).await?;
    manager(&state)?
        .set_role(&username, &body.role)
        .map_err(|e| map_auth_error(&e))?;
    Ok(Json(json!({"ok": true})))
}

pub async fn delete_user(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    req: Request<Body>,
) -> ApiResult<Json<Value>> {
    let account = actor(&state, &req)?;
    if account.role != ROLE_ADMIN {
        return Err(denied());
    }
    manager(&state)?
        .delete_user(&username)
        .map_err(|e| map_auth_error(&e))?;
    Ok(Json(json!({"ok": true})))
}

use axum::extract::FromRequest;
