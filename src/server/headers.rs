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

/// One authority parser for HTTP/1 Host and HTTP/2 :authority. Conflicting
/// representations are refused; forwarding headers never supply authority.
pub fn request_authority(req: &Request<Body>) -> Option<axum::http::uri::Authority> {
    if req.headers().get_all(header::HOST).iter().count() > 1 {
        return None;
    }
    let host = req.headers().get(header::HOST).map(|h| h.to_str()).transpose().ok()?;
    let authority = req.uri().authority().map(|a| a.as_str());
    if host.zip(authority).is_some_and(|(h, a)| !h.eq_ignore_ascii_case(a)) {
        return None;
    }
    let raw = authority.or(host)?;
    if raw.contains('@') || raw.chars().any(char::is_whitespace) {
        return None;
    }
    let parsed: axum::http::uri::Authority = raw.parse().ok()?;
    if parsed.port().is_some() && parsed.port_u16().is_none() {
        return None;
    }
    Some(parsed)
}

/// Reject any request whose authority is not a loopback name (DNS rebinding defense).
pub async fn trusted_host(req: Request<Body>, next: Next) -> Response {
    let host = request_authority(&req)
        .map(|a| a.host().trim_matches(['[', ']']).to_lowercase())
        .unwrap_or_default();
    if !LOOPBACK_HOSTS.contains(&host.to_lowercase().as_str()) {
        return (StatusCode::BAD_REQUEST, "Invalid host header").into_response();
    }
    next.run(req).await
}
