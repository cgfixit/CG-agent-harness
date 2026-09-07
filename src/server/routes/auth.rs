//! `/api/auth/*`: per-user sessions and RBAC. Port of `harness/auth_routes.py`.
//! Every route returns 503 `AUTH_DISABLED` unless `auth.enabled` is the literal true.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
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

fn set_cookie(resp: &mut Response, value: &str) {
    let v = format!("{SESSION_COOKIE}={value}; HttpOnly; SameSite=Strict; Path=/");
    if let Ok(hv) = HeaderValue::from_str(&v) {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
}

fn clear_cookie(resp: &mut Response) {
    let v = format!("{SESSION_COOKIE}=; Max-Age=0; HttpOnly; SameSite=Strict; Path=/");
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

fn actor(state: &AppState, req: &Request<Body>) -> ApiResult<UserSummary> {
    let mgr = manager(state)?;
    let token = cookie_value(req).unwrap_or_default();
    let unauthorized = || ApiError::new(StatusCode::UNAUTHORIZED, "AUTH_REQUIRED", "authentication required");
    let info = mgr.validate_session(&token).ok_or_else(unauthorized)?;
    mgr.get_user(&info.username).ok_or_else(unauthorized)
}

fn denied() -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, PERM_DENIED, "denied")
}

fn require_user_admin(account: &UserSummary) -> ApiResult<()> {
    if account.role != ROLE_ADMIN && account.role != ROLE_OPERATOR {
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
    set_cookie(&mut resp, &result.session_id);
    Ok(resp)
}

pub async fn login(
    State(state): State<Arc<AppState>>,
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
    let mut resp =
        Json(json!({"username": result.username, "role": role, "csrf_token": result.csrf_token})).into_response();
    set_cookie(&mut resp, &result.session_id);
    Ok(resp)
}

pub async fn logout(State(state): State<Arc<AppState>>, req: Request<Body>) -> ApiResult<Response> {
    let mgr = manager(&state)?;
    if let Some(token) = cookie_value(&req) {
        mgr.logout(&token);
    }
    let mut resp = Json(json!({"ok": true})).into_response();
    clear_cookie(&mut resp);
    Ok(resp)
}

pub async fn whoami(State(state): State<Arc<AppState>>, req: Request<Body>) -> ApiResult<Json<Value>> {
    let account = actor(&state, &req)?;
    Ok(Json(json!({"username": account.username, "role": account.role})))
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
