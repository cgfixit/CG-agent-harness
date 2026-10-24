//! `GET /`: the verbatim CyClaw console with the two serve-time placeholders
//! substituted (CSRF token per process, CSP nonce per response). `/static/{name}`
//! serves only the script asset that page loads; the markup itself is never
//! handed out unsubstituted, so the root route is the single place the nonce
//! and token contracts are applied.

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
    // The page is pre-split around the nonce placeholder at startup (see
    // `console_html_segments`), so each request joins segments with the fresh
    // nonce instead of rescanning and reallocating the whole ~130KB page.
    let segments = &state.console_html_segments;
    let mut html = String::with_capacity(
        segments.iter().map(String::len).sum::<usize>() + nonce.len() * segments.len().saturating_sub(1),
    );
    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            html.push_str(&nonce);
        }
        html.push_str(segment);
    }
    let csp = format!(
        "default-src 'none'; script-src 'self' 'nonce-{nonce}'; style-src 'nonce-{nonce}'; \
         connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; \
         frame-src 'self'"
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

/// CSP for `GET /preview/html`. Inline style and script only. No network,
/// and no framing of the console (`frame-src 'none'`).
pub const PREVIEW_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; connect-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'; frame-src 'none'";

/// Empty shell. The operator's HTML arrives by `postMessage` and replaces this
/// document, so it keeps this response's policy instead of the console nonce.
pub const PREVIEW_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>HTML preview</title></head>
<body>
<p>Run a page to preview it here.</p>
<script>
(function () {
  function loopback(origin) {
    try {
      var url = new URL(origin);
      if (url.protocol !== "http:" && url.protocol !== "https:") return false;
      var host = url.hostname;
      return host === "127.0.0.1" || host === "localhost" || host === "::1";
    } catch (e) {
      return false;
    }
  }
  window.addEventListener("message", function (event) {
    if (event.source !== window.parent) return;
    if (!loopback(event.origin)) return;
    if (typeof event.data !== "string" || event.data.length > 200000) return;
    // document.write keeps this response's policy on the page the operator wrote.
    document.open();
    document.write(event.data);
    document.close();
  });
})();
</script>
</body>
</html>
"#;

pub async fn preview_html() -> Response {
    let mut resp = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        PREVIEW_HTML,
    )
        .into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(PREVIEW_CSP));
    // SAMEORIGIN lets the console frame this shell. The middleware default is
    // DENY and only fills a missing value, so the shell has to set this itself.
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
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
        _ => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}
