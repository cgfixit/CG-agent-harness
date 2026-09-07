//! Security headers (outermost) and the loopback-only Host check.
//! Port of `_SecurityHeadersMiddleware` + `TrustedHostMiddleware` in
//! `harness/server.py`.

use axum::body::Body;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::llm::backend::LOOPBACK_HOSTS;

pub const NO_STORE: &str = "no-store, no-cache, must-revalidate, max-age=0";
pub const DEFAULT_CSP: &str = "default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

fn set_default(resp: &mut Response, name: header::HeaderName, value: &str) {
    if !resp.headers().contains_key(&name) {
        if let Ok(v) = HeaderValue::from_str(value) {
            resp.headers_mut().insert(name, v);
        }
    }
}

/// Stamp the hardening headers on every response (setdefault semantics so the
/// console's own nonce CSP survives). `/static/*` also gets no-store.
pub async fn security_headers(req: Request<Body>, next: Next) -> Response {
    let is_static = req.uri().path().starts_with("/static/");
    let mut resp = next.run(req).await;
    set_default(&mut resp, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set_default(&mut resp, header::X_FRAME_OPTIONS, "DENY");
    set_default(&mut resp, header::REFERRER_POLICY, "strict-origin-when-cross-origin");
    set_default(
        &mut resp,
        header::HeaderName::from_static("permissions-policy"),
        "camera=(), microphone=(), geolocation=()",
    );
    set_default(
        &mut resp,
        header::HeaderName::from_static("x-permitted-cross-domain-policies"),
        "none",
    );
    set_default(&mut resp, header::CONTENT_SECURITY_POLICY, DEFAULT_CSP);
    if is_static {
        set_default(&mut resp, header::CACHE_CONTROL, NO_STORE);
    }
    resp
}

/// Host header hostname (port stripped, IPv6 brackets kept intact).
pub fn host_name(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return rest[..end].to_string();
        }
    }
    raw.rsplit_once(':').map(|(h, _)| h).unwrap_or(raw).to_string()
}

/// Reject any request whose Host is not a loopback name (DNS-rebinding defense).
pub async fn trusted_host(req: Request<Body>, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(host_name)
        .unwrap_or_default();
    if !LOOPBACK_HOSTS.contains(&host.to_lowercase().as_str()) {
        return (StatusCode::BAD_REQUEST, "Invalid host header").into_response();
    }
    next.run(req).await
}
