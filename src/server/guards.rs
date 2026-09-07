//! The guard chain: rate limit -> same-origin -> API key (or loopback bypass)
//! -> CSRF. Port of `harness/server.py`'s `guarded` dependency list. Order is
//! load-bearing: a wrong key against a spent budget must be 429, not 401, and
//! CSRF runs last so a missing key still reports 401.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, Request, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::json;
use subtle::ConstantTimeEq;

use super::errors::ApiError;
use super::state::AppState;
use crate::common::apikey;
use crate::common::ratelimit::RateLimiter;
use crate::llm::backend::LOOPBACK_HOSTS;

pub const CSRF_HEADER: &str = "x-cyclaw-csrf";
const FORWARDING_HEADERS: [&str; 5] = [
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-real-ip",
    "forwarded",
];

pub fn client_ip(req: &Request<Body>) -> String {
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// True when any reverse-proxy forwarding header is present (value never parsed).
pub fn looks_proxied(headers: &HeaderMap) -> bool {
    FORWARDING_HEADERS.iter().any(|h| headers.contains_key(*h))
}

/// Keyed on the socket peer, never the Host header. A missing peer reads as
/// NOT loopback (this backs a bypass, so unknown fails closed).
pub fn is_loopback_peer(req: &Request<Body>) -> bool {
    match req.extensions().get::<ConnectInfo<SocketAddr>>() {
        Some(c) => c.0.ip().is_loopback(),
        None => false,
    }
}

pub fn retry_after_error(limiter: &RateLimiter, client: &str, code: &str, label: &str) -> ApiError {
    let wait = limiter.retry_after_sec(client).ceil().max(1.0) as u64;
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        code,
        format!(
            "{label} ({} req / {}s); retry in {wait}s",
            limiter.max_requests(),
            limiter.window_seconds() as u64
        ),
    )
    .details(json!({"retry_after_sec": wait}))
    .header("Retry-After", &wait.to_string())
}

pub fn enforce_rate_limit(state: &AppState, req: &Request<Body>) -> Result<(), ApiError> {
    let ip = client_ip(req);
    if !state.rate_limiter.allow(&ip) {
        return Err(retry_after_error(
            &state.rate_limiter,
            &ip,
            "RATE_LIMIT",
            "Rate limit exceeded",
        ));
    }
    Ok(())
}

fn canonical_port(port: Option<u16>, scheme: &str) -> Option<u16> {
    port.or(match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    })
}

fn request_scheme(req: &Request<Body>) -> String {
    req.uri().scheme_str().unwrap_or("http").to_lowercase()
}

/// (hostname, port) of the request's own URL, from the Host header.
fn request_host_port(req: &Request<Body>) -> (String, Option<u16>) {
    let raw = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let uri: Uri = match format!("http://{raw}/").parse() {
        Ok(u) => u,
        Err(_) => return (String::new(), None),
    };
    (uri.host().unwrap_or("").to_lowercase(), uri.port_u16())
}

/// Reject browser-initiated cross-site requests. Absent headers are ALLOWED on
/// purpose (curl / PowerShell are not CSRF vectors).
pub fn enforce_same_origin(req: &Request<Body>) -> Result<(), ApiError> {
    if let Some(site) = req.headers().get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        if site != "same-origin" && site != "none" {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "CROSS_SITE_BLOCKED",
                "Cross-site request rejected",
            )
            .details(json!({"sec_fetch_site": site})));
        }
    }
    let Some(origin) = req.headers().get(header::ORIGIN) else {
        return Ok(());
    };
    let origin = origin.to_str().unwrap_or("");
    let parsed = url::Url::parse(origin).ok();
    let origin_host = parsed.as_ref().and_then(|u| u.host_str().map(|h| h.to_lowercase()));
    let blocked = || {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "CROSS_ORIGIN_BLOCKED",
            "Cross-origin request rejected",
        )
        .details(json!({"origin_host": origin_host}))
    };
    let Some(parsed) = parsed else {
        return Err(blocked());
    };
    let (req_host, req_port) = request_host_port(req);
    let req_scheme = request_scheme(req);
    let origin_scheme = parsed.scheme().to_lowercase();
    let same_origin = origin_host.as_deref().is_some_and(|h| {
        LOOPBACK_HOSTS.contains(&h)
            && h == req_host
            && origin_scheme == req_scheme
            && canonical_port(parsed.port(), &origin_scheme) == canonical_port(req_port, &req_scheme)
    });
    if !same_origin {
        return Err(blocked());
    }
    Ok(())
}

pub fn enforce_api_key_or_optional(state: &AppState, req: &Request<Body>) -> Result<(), ApiError> {
    if state.api_key_optional && is_loopback_peer(req) && !looks_proxied(req.headers()) {
        return Ok(());
    }
    let header = req.headers().get(header::AUTHORIZATION).map(|v| v.as_bytes());
    match apikey::verify(header, state.api_key.as_deref()) {
        Ok(()) => Ok(()),
        Err(failure) => Err(
            ApiError::new(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", failure.message())
                .details(json!({"reason": failure.reason()})),
        ),
    }
}

pub fn enforce_csrf(state: &AppState, req: &Request<Body>) -> Result<(), ApiError> {
    let supplied = req.headers().get(CSRF_HEADER).map(|v| v.as_bytes()).unwrap_or(b"");
    let expected = state.csrf_token.as_bytes();
    let ok = supplied.len() == expected.len() && bool::from(supplied.ct_eq(expected));
    if !ok {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "CSRF_TOKEN_INVALID",
            "Missing or invalid CSRF token",
        ));
    }
    Ok(())
}

/// Full guard chain for operator routes.
pub async fn guarded(State(state): State<Arc<AppState>>, req: Request<Body>, next: Next) -> Response {
    if let Err(e) = enforce_rate_limit(&state, &req)
        .and_then(|_| enforce_same_origin(&req))
        .and_then(|_| enforce_api_key_or_optional(&state, &req))
        .and_then(|_| enforce_csrf(&state, &req))
    {
        return axum::response::IntoResponse::into_response(e);
    }
    next.run(req).await
}

/// Rate limit + same-origin only (the `/api/auth/*` open routes).
pub async fn auth_open(State(state): State<Arc<AppState>>, req: Request<Body>, next: Next) -> Response {
    if let Err(e) = enforce_rate_limit(&state, &req).and_then(|_| enforce_same_origin(&req)) {
        return axum::response::IntoResponse::into_response(e);
    }
    next.run(req).await
}

/// Rate limit + same-origin + CSRF (the `/api/auth/*` session routes).
pub async fn auth_sess(State(state): State<Arc<AppState>>, req: Request<Body>, next: Next) -> Response {
    if let Err(e) = enforce_rate_limit(&state, &req)
        .and_then(|_| enforce_same_origin(&req))
        .and_then(|_| enforce_csrf(&state, &req))
    {
        return axum::response::IntoResponse::into_response(e);
    }
    next.run(req).await
}

/// Peer IP as an `IpAddr` when known.
pub fn peer_ip(req: &Request<Body>) -> Option<IpAddr> {
    req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;

    fn req(host: &str, origin: Option<&str>, sec_fetch_site: Option<&str>) -> HttpRequest<Body> {
        let mut b = HttpRequest::builder().uri("/api/chat").header(header::HOST, host);
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        if let Some(s) = sec_fetch_site {
            b = b.header("sec-fetch-site", s);
        }
        b.body(Body::empty()).unwrap()
    }

    #[test]
    fn no_origin_header_is_allowed_curl_is_not_a_csrf_vector() {
        assert!(enforce_same_origin(&req("127.0.0.1:8790", None, None)).is_ok());
    }

    #[test]
    fn matching_scheme_host_and_canonical_port_is_same_origin() {
        assert!(enforce_same_origin(&req("127.0.0.1:8790", Some("http://127.0.0.1:8790"), None)).is_ok());
        // Default HTTP port on both sides still matches when neither names it.
        assert!(enforce_same_origin(&req("127.0.0.1", Some("http://127.0.0.1"), None)).is_ok());
    }

    #[test]
    fn a_different_origin_host_is_blocked() {
        assert!(enforce_same_origin(&req("127.0.0.1:8790", Some("http://evil.example"), None)).is_err());
    }

    #[test]
    fn a_mismatched_port_is_blocked_even_on_the_same_host() {
        assert!(enforce_same_origin(&req("127.0.0.1:8790", Some("http://127.0.0.1:9999"), None)).is_err());
    }

    #[test]
    fn cross_site_sec_fetch_site_is_blocked_even_without_an_origin_header() {
        assert!(enforce_same_origin(&req("127.0.0.1:8790", None, Some("cross-site"))).is_err());
        assert!(enforce_same_origin(&req("127.0.0.1:8790", None, Some("same-origin"))).is_ok());
        assert!(enforce_same_origin(&req("127.0.0.1:8790", None, Some("none"))).is_ok());
    }

    #[test]
    fn an_unparseable_origin_is_blocked_not_ignored() {
        assert!(enforce_same_origin(&req("127.0.0.1:8790", Some("not a url"), None)).is_err());
    }
}
