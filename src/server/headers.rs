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

fn set_default(resp: &mut Response, name: header::HeaderName, value: &'static str) {
    if !resp.headers().contains_key(&name) {
        resp.headers_mut().insert(name, HeaderValue::from_static(value));
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
    // `Authority::port` is `None` both when the port is absent and when the
    // port text is not a u16: `Port::from_str` failure becomes `None`, and
    // `port_u16` is only `port().map(|p| p.as_u16())`. A present port that is
    // not a decimal u16 must fail closed instead of looking like "no port".
    if !authority_port_is_u16(raw) {
        return None;
    }
    Some(parsed)
}

/// True when `raw` names no port, or its port is a decimal `u16` (no `+`,
/// no empty text, no value above 65535). Bracketed hosts keep colons inside
/// `[...]`; the port separator is only the colon after `]`.
fn authority_port_is_u16(raw: &str) -> bool {
    let port = if let Some(rest) = raw.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return false;
        };
        let after = &rest[end + 1..];
        if after.is_empty() {
            return true;
        }
        let Some(port) = after.strip_prefix(':') else {
            return false;
        };
        port
    } else if let Some((_, port)) = raw.split_once(':') {
        port
    } else {
        return true;
    };
    !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok()
}

/// Reject any request whose authority is not a loopback name (DNS rebinding defense).
pub async fn trusted_host(req: Request<Body>, next: Next) -> Response {
    let host = request_authority(&req)
        .map(|a| a.host().trim_matches(['[', ']']).to_lowercase())
        .unwrap_or_default();
    if !LOOPBACK_HOSTS.contains(&host.as_str()) {
        return (StatusCode::BAD_REQUEST, "Invalid host header").into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{header, Request};

    fn with_host(host: &str) -> Request<Body> {
        Request::builder()
            .uri("/api/status")
            .header(header::HOST, host)
            .body(Body::empty())
            .unwrap()
    }

    fn with_uri_authority(authority: &str) -> Request<Body> {
        Request::builder()
            // DevSkim: ignore DS137138 because this fixture only parses an authority and opens no connection.
            .uri(format!("http://{authority}/api/status"))
            .body(Body::empty())
            .unwrap()
    }

    #[test]
    fn decimal_u16_ports_and_absent_ports_stay_accepted() {
        // DevSkim: ignore DS162092 because these fixtures must name loopback hosts to prove valid ports still parse.
        for host in [
            "127.0.0.1",
            "127.0.0.1:8790",
            "127.0.0.1:080",
            "localhost",
            "localhost:0",
            "[::1]",
            "[::1]:8790",
        ] {
            let from_host = request_authority(&with_host(host)).unwrap_or_else(|| panic!("host {host}"));
            let from_uri = request_authority(&with_uri_authority(host)).unwrap_or_else(|| panic!("uri {host}"));
            assert_eq!(from_host.port_u16(), from_uri.port_u16(), "{host}");
        }
        assert_eq!(
            request_authority(&with_host("127.0.0.1:8790")).unwrap().port_u16(),
            Some(8790)
        );
        assert_eq!(
            request_authority(&with_host("127.0.0.1:080")).unwrap().port_u16(),
            Some(80)
        );
        assert_eq!(
            request_authority(&with_host("[::1]:8790")).unwrap().port_u16(),
            Some(8790)
        );
        assert!(request_authority(&with_host("[::1]")).unwrap().port_u16().is_none());
    }

    #[test]
    fn a_port_that_is_not_a_decimal_u16_is_rejected() {
        // DevSkim: ignore DS162092 because these fixtures must name loopback hosts whose bad ports used to be ignored.
        for host in [
            "127.0.0.1:notaport",
            "127.0.0.1:",
            "127.0.0.1:99999",
            "127.0.0.1:65536",
            "127.0.0.1:+80",
            "localhost:80abc",
            "[::1]:notaport",
            "[::1]:",
            "[::1]:99999",
            "[::1]80",
        ] {
            assert!(request_authority(&with_host(host)).is_none(), "host {host}");
            assert!(request_authority(&with_uri_authority(host)).is_none(), "uri {host}");
        }
    }

    #[tokio::test]
    async fn a_non_u16_host_port_is_rejected_on_a_loopback_listener() {
        let app = axum::Router::new()
            .route("/api/status", axum::routing::get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn(trusted_host));
        // DevSkim: ignore DS162092 because this listener must bind only to loopback.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        // DevSkim: ignore DS162092 because the control request uses the listener's own loopback port.
        let ok = http_status(addr, &format!("127.0.0.1:{}", addr.port())).await;
        assert_eq!(ok, 200, "numeric port");
        assert_eq!(http_status(addr, "127.0.0.1").await, 200, "absent port"); // DevSkim: ignore DS162092 because an absent port on loopback must stay accepted.
        assert_eq!(http_status(addr, "[::1]:8790").await, 200, "ipv6 numeric port");

        for host in [
            "127.0.0.1:notaport",
            "127.0.0.1:",
            "127.0.0.1:99999",
            "[::1]:notaport",
            "[::1]:",
        ] {
            // DevSkim: ignore DS162092 because these hosts are the loopback forms whose bad ports must fail closed.
            let (status, body) = http_exchange(addr, host).await;
            assert_eq!(status, 400, "{host}: {body}");
            assert!(body.contains("Invalid host header"), "{host}: {body}");
        }
    }

    async fn http_status(addr: std::net::SocketAddr, host: &str) -> u16 {
        http_exchange(addr, host).await.0
    }

    async fn http_exchange(addr: std::net::SocketAddr, host: &str) -> (u16, String) {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let request = format!("GET /api/status HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes())
            .await
            .unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut response),
        )
        .await
        .expect("listener reply")
        .unwrap();
        let text = String::from_utf8(response).unwrap();
        let status = text
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        (status, text)
    }
}
