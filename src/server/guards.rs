//! Loopback, rate, origin, account authorization and CSRF boundaries.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::json;
use subtle::ConstantTimeEq;

use super::errors::ApiError;
use super::state::AppState;
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

pub fn retry_after_error(
    limiter: &RateLimiter,
    limits: &super::config_reload::RateBudget,
    client: &str,
    code: &str,
    label: &str,
) -> ApiError {
    let wait = limiter
        .retry_after_with_limits(client, limits.max_requests, limits.window_seconds)
        .ceil()
        .max(1.0) as u64;
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        code,
        format!(
            "{label} ({} req / {}s); retry in {wait}s",
            limits.max_requests, limits.window_seconds as u64
        ),
    )
    .details(json!({"retry_after_sec": wait}))
    .header("Retry-After", &wait.to_string())
}

pub fn enforce_rate_limit(state: &AppState, req: &Request<Body>) -> Result<(), ApiError> {
    let ip = client_ip(req);
    let live = state.runtime_limits();
    if !state
        .rate_limiter
        .allow_with_limits(&ip, live.api.max_requests, live.api.window_seconds)
    {
        return Err(retry_after_error(
            &state.rate_limiter,
            &live.api,
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

pub fn request_scheme(req: &Request<Body>) -> &'static str {
    req.extensions()
        .get::<super::transport::ListenerScheme>()
        .map(|s| s.0)
        .unwrap_or("http")
}

/// (hostname, port) from the validated HTTP authority, shared with the Host guard.
fn request_host_port(req: &Request<Body>) -> (String, Option<u16>) {
    super::headers::request_authority(req)
        .map(|a| (a.host().trim_matches(['[', ']']).to_lowercase(), a.port_u16()))
        .unwrap_or_default()
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
    let origin_host = parsed
        .as_ref()
        .and_then(|u| u.host_str().map(|h| h.trim_matches(['[', ']']).to_lowercase()));
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
            && canonical_port(parsed.port(), &origin_scheme) == canonical_port(req_port, req_scheme)
    });
    if !same_origin {
        return Err(blocked());
    }
    Ok(())
}

/// The peer must be a direct loopback connection with no forwarding headers.
pub fn enforce_direct_loopback(req: &Request<Body>) -> Result<(), ApiError> {
    if is_loopback_peer(req) && !looks_proxied(req.headers()) {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "LOOPBACK_REQUIRED",
        "a direct loopback connection is required",
    ))
}

/// One account boundary covers every API route, including reads. Static login
/// assets and the minimal status endpoint carry no operational authority.
/// The `/api/` paths the gate serves without an account. `routes::ROUTE_CAPS`
/// marks the same paths `Capability::Public`; a routes test keeps the two
/// equal, so a path added here cannot skip a capability the table assigns.
pub(crate) const PUBLIC_API_PATHS: [&str; 3] = ["/api/status", "/api/auth/setup-status", "/api/auth/login"];

pub async fn account_gate(State(state): State<Arc<AppState>>, mut req: Request<Body>, next: Next) -> Response {
    use axum::response::IntoResponse;
    if !req.uri().path().starts_with("/api/") {
        return next.run(req).await;
    }
    let path = req.uri().path().to_string();
    // Refuse foreign requests before they spend the one loopback rate bucket.
    if let Err(e) = enforce_same_origin(&req)
        .and_then(|_| enforce_direct_loopback(&req))
        .and_then(|_| enforce_rate_limit(&state, &req))
    {
        return e.into_response();
    }
    let public = PUBLIC_API_PATHS.contains(&path.as_str());
    let mut actor_name = "local".to_string();
    if state.auth.is_some() {
        let account = super::routes::auth::session_account(&state, &req).await;
        if !public {
            let account = match account {
                Ok(account) => account,
                Err(e) => return e.into_response(),
            };
            // The capability table decides: the request's matched route
            // template and method resolve to exactly one capability, and the
            // stored role parses once. HEAD rides the GET entry, matching
            // axum's automatic HEAD handling. A registered template with no
            // exact entry is a method mismatch: it keeps the router's 405
            // only where `wrong_method_passthrough` says the legacy gate did.
            // Anything else — an unlisted route, an unknown stored role, an
            // unmatched path — fails closed, so a new route is never
            // operator-accessible by default (#965).
            let template = req
                .extensions()
                .get::<axum::extract::MatchedPath>()
                .map(|p| p.as_str().to_string());
            let method = if req.method() == axum::http::Method::HEAD {
                &axum::http::Method::GET
            } else {
                req.method()
            };
            let capability = template
                .as_deref()
                .and_then(|t| super::routes::route_capability(method, t));
            let own = template.as_deref().is_some_and(super::routes::template_is_own_account);
            if account.must_change_password && !own {
                return ApiError::new(
                    StatusCode::FORBIDDEN,
                    "AUTH_PASSWORD_CHANGE_REQUIRED",
                    "replace the bootstrap password before using the harness",
                )
                .into_response();
            }
            let role = crate::common::authz::Role::parse(&account.role);
            let granted = match role {
                Some(role) => match capability {
                    Some(cap) => crate::common::authz::grants(role, cap),
                    None => template
                        .as_deref()
                        .is_some_and(|t| super::routes::wrong_method_passthrough(role, t)),
                },
                None => false,
            };
            if !granted {
                return ApiError::new(StatusCode::FORBIDDEN, "AUTH_PERMISSION_DENIED", "denied").into_response();
            }
            actor_name = account.username.clone();
            // A body that arrives after this check rereads the session and
            // role before it is used (`auth::SessionRecheck`).
            if let Some(recheck) = super::routes::auth::SessionRecheck::new(&state, &req, &account, capability) {
                req.extensions_mut().insert(recheck);
            }
            // Hand the parsed role to handlers that authorize below the route
            // table (per-server MCP authority): the gate stays the single
            // parse site.
            if let Some(role) = role {
                req.extensions_mut().insert(role);
            }
            req.extensions_mut().insert(account);
        } else if let Ok(account) = account {
            actor_name = account.username.clone();
            // Public routes authorize below the table too (/api/status shows
            // a restricted view to auditor accounts): parse the role at the
            // same single site.
            if let Some(role) = crate::common::authz::Role::parse(&account.role) {
                req.extensions_mut().insert(role);
            }
            req.extensions_mut().insert(account);
        } else {
            actor_name = "anonymous".into();
        }
    }
    let method = req.method().to_string();
    let route = req
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|p| p.as_str())
        .unwrap_or("unknown")
        .to_string();
    let response = next.run(req).await;
    state.audit.log(json!({"event":"portal.request","actor":actor_name,"method":method,"route":route,"status":response.status().as_u16()}));
    response
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

/// Mutations and the legacy guarded GET routes require the process CSRF token.
pub async fn guarded(State(state): State<Arc<AppState>>, req: Request<Body>, next: Next) -> Response {
    if let Err(e) = enforce_csrf(&state, &req) {
        return axum::response::IntoResponse::into_response(e);
    }
    next.run(req).await
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

    /// The #965 PR 2 narrowing, asserted at the table: operator loses
    /// shared-state, model, audit/log and replay authority, and keeps
    /// proposing persona edits, per-session style, owner-scoped corpus
    /// deletion and the MCP gate capability.
    #[test]
    fn route_matrix_matches_the_narrowed_target_table() {
        use crate::common::authz::{grants, Role};
        use axum::http::Method;
        let denied = [
            (Method::POST, "/api/model"),
            (Method::POST, "/api/memory"),
            (Method::POST, "/api/memory/add"),
            (Method::POST, "/api/memory/forget"),
            (Method::POST, "/api/memory/clear"),
            (Method::POST, "/api/soul"),
            (Method::POST, "/api/soul/document"),
            (Method::GET, "/api/soul/proposals/{id}"),
            (Method::POST, "/api/soul/proposals/{id}"),
            (Method::POST, "/api/ollama/pull"),
            (Method::POST, "/api/ollama/pull/cancel"),
            (Method::GET, "/api/ollama/inventory"),
            (Method::GET, "/api/ollama/profile"),
            (Method::GET, "/api/audit"),
            (Method::GET, "/api/diag"),
            (Method::POST, "/api/notifications/{delivery_id}/replay"),
            (Method::GET, "/api/sessions/legacy"),
            (Method::POST, "/api/sessions/{session_id}/adopt"),
        ];
        for (method, path) in denied {
            let cap = super::super::routes::route_capability(&method, path)
                .unwrap_or_else(|| panic!("{method} {path} has no capability row"));
            assert!(!grants(Role::Operator, cap), "operator must not hold {method} {path}");
            assert!(grants(Role::Admin, cap), "admin keeps {method} {path}");
        }
        let kept = [
            (Method::POST, "/api/soul/proposals"),
            (Method::GET, "/api/soul/document"),
            (Method::GET, "/api/style"),
            (Method::POST, "/api/style"),
            (Method::POST, "/api/mcp/call"),
            (Method::DELETE, "/api/notes-corpus/{id}"),
        ];
        for (method, path) in kept {
            let cap = super::super::routes::route_capability(&method, path)
                .unwrap_or_else(|| panic!("{method} {path} has no capability row"));
            assert!(grants(Role::Operator, cap), "operator keeps {method} {path}");
        }
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
    fn http2_authority_and_http1_host_use_the_same_origin_boundary() {
        let mut request = HttpRequest::builder()
            .version(axum::http::Version::HTTP_2)
            .uri("https://127.0.0.1:8790/api/chat")
            .header(header::ORIGIN, "https://127.0.0.1:8790")
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(super::super::transport::ListenerScheme("https"));
        assert_eq!(request_host_port(&request), ("127.0.0.1".into(), Some(8790)));
        assert!(enforce_same_origin(&request).is_ok());
        request
            .headers_mut()
            .insert(header::HOST, "evil.example".parse().unwrap());
        assert!(super::super::headers::request_authority(&request).is_none());
        assert!(enforce_same_origin(&request).is_err());
        request
            .headers_mut()
            .insert(header::HOST, "127.0.0.1:8790".parse().unwrap());
        assert!(enforce_same_origin(&request).is_ok());
        request
            .headers_mut()
            .append(header::HOST, "127.0.0.1:8790".parse().unwrap());
        assert!(super::super::headers::request_authority(&request).is_none());
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
