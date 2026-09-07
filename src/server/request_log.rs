//! One structured line per HTTP request: method, path, status, latency,
//! client. Never headers, never bodies, never the query string (the query is
//! the one place a caller could smuggle a secret into a log).
//!
//! Emitted through `tracing` under target `cgagentharness::http`, so the
//! operator selects it with `RUST_LOG=cgagentharness::http=info` and it lands
//! wherever the process's log goes. Distinct from the audit file, which is for
//! security events and is redacted separately.

use std::time::Instant;

use axum::body::Body;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;

pub const TARGET: &str = "cgagentharness::http";

/// Path only: strips the query and clips to keep a log line bounded.
pub fn loggable_path(uri: &axum::http::Uri) -> String {
    let p = uri.path();
    crate::common::clip_chars(p, 200)
}

pub async fn request_log(req: Request<Body>, next: Next) -> Response {
    let started = Instant::now();
    let method = req.method().clone();
    let path = loggable_path(req.uri());
    let client = crate::server::guards::client_ip(&req);
    let resp = next.run(req).await;
    let status = resp.status().as_u16();
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
    tracing::info!(
        target: TARGET,
        method = %method,
        path = %path,
        status,
        latency_ms = format_args!("{latency_ms:.1}"),
        client = %client,
        "request"
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_string_never_reaches_the_log_line() {
        let uri: axum::http::Uri = "/api/keys?token=SECRET&x=1".parse().unwrap();
        let p = loggable_path(&uri);
        assert_eq!(p, "/api/keys");
        assert!(!p.contains("SECRET"));
    }

    #[test]
    fn overlong_paths_are_clipped() {
        let long = format!("/api/{}", "a".repeat(1000));
        let uri: axum::http::Uri = long.parse().unwrap();
        assert!(loggable_path(&uri).chars().count() <= 201);
    }
}
