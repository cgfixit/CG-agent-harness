//! `GET /` and `/static/*`: the verbatim CyClaw console with the two serve-time
//! placeholders substituted (CSRF token per process, CSP nonce per response).

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use super::headers::NO_STORE;
use super::state::AppState;

pub const CSRF_PLACEHOLDER: &str = "__CYCLAW_CSRF_TOKEN__";
pub const CSP_NONCE_PLACEHOLDER: &str = "__CYCLAW_CSP_NONCE__";
pub const HARNESS_HTML: &str = include_str!("../../assets/static/harness.html");
pub const AUTH_ADMIN_JS: &str = include_str!("../../assets/static/auth_admin.js");

pub async fn console(State(state): State<Arc<AppState>>) -> Response {
    // Minted per request: a nonce reused across responses is replayable by any
    // markup an XSS bug injects.
    let nonce = crate::common::random_urlsafe(16);
    let html = state.console_html.replace(CSP_NONCE_PLACEHOLDER, &nonce);
    let csp = format!(
        "default-src 'none'; script-src 'self' 'nonce-{nonce}'; style-src 'nonce-{nonce}'; \
         connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    );
    let mut resp = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response();
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&csp) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(NO_STORE));
    resp
}

pub async fn static_asset(axum::extract::Path(name): axum::extract::Path<String>) -> Response {
    match name.as_str() {
        "auth_admin.js" => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
            AUTH_ADMIN_JS,
        )
            .into_response(),
        "harness.html" => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            HARNESS_HTML,
        )
            .into_response(),
        _ => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}
