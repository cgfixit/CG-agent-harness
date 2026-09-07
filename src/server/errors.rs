//! HTTP error envelope: `{"detail": {"code", "message", "details"}}` -- the
//! shape `static/harness.html`'s `api()` helper reads (`detail.message`).

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use crate::common::errors::HarnessError;

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
    pub details: Value,
    pub headers: Vec<(String, String)>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.to_string(),
            message: message.into(),
            details: json!({}),
            headers: Vec::new(),
        }
    }

    pub fn from_err(status: StatusCode, err: &HarnessError) -> Self {
        Self {
            status,
            code: err.code.clone(),
            message: err.message.clone(),
            details: err.details.clone(),
            headers: Vec::new(),
        }
    }

    pub fn details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn bad_request(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn body(&self) -> Value {
        json!({"detail": {"code": self.code, "message": self.message, "details": self.details}})
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut headers = HeaderMap::new();
        for (k, v) in &self.headers {
            if let (Ok(name), Ok(val)) = (k.parse::<axum::http::HeaderName>(), HeaderValue::from_str(v)) {
                headers.insert(name, val);
            }
        }
        (self.status, headers, axum::Json(self.body())).into_response()
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;

/// Persist failures are the server's fault (502); anything else about a
/// session id is the operator's (404).
pub fn session_status(err: &HarnessError) -> StatusCode {
    if err.code == crate::server::sessions::PERSIST_ERROR_CODE {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::NOT_FOUND
    }
}
