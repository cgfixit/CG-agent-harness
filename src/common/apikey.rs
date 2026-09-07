//! Fail-closed Bearer API-key verification, port of `utils/auth.py`.
//!
//! An unset or empty key REFUSES the endpoint (never opens it). Comparison is
//! constant-time on the raw header bytes, so a pasted non-ASCII character is a
//! 401, never a 500.

use subtle::ConstantTimeEq;

pub const API_KEY_ENV: &str = "CGAGENTHARNESS_API_KEY";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyFailure {
    KeyNotConfigured,
    BadCredentials,
}

impl ApiKeyFailure {
    pub fn reason(self) -> &'static str {
        match self {
            ApiKeyFailure::KeyNotConfigured => "key_not_configured",
            ApiKeyFailure::BadCredentials => "bad_credentials",
        }
    }

    pub fn message(self) -> String {
        match self {
            ApiKeyFailure::KeyNotConfigured => format!("Operator endpoint disabled: {API_KEY_ENV} not set"),
            ApiKeyFailure::BadCredentials => "Invalid or missing API key".to_string(),
        }
    }
}

/// Extract the token from an `Authorization: Bearer <token>` header value.
pub fn bearer_token(header: Option<&[u8]>) -> Option<&[u8]> {
    let raw = header?;
    let (scheme, rest) = split_first_space(raw)?;
    if !scheme.eq_ignore_ascii_case(b"bearer") {
        return None;
    }
    let token = trim_ascii(rest);
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn split_first_space(raw: &[u8]) -> Option<(&[u8], &[u8])> {
    let trimmed = trim_ascii(raw);
    let idx = trimmed.iter().position(|b| *b == b' ' || *b == b'\t')?;
    Some((&trimmed[..idx], &trimmed[idx..]))
}

fn trim_ascii(raw: &[u8]) -> &[u8] {
    let start = raw.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(raw.len());
    let end = raw
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(start);
    &raw[start..end.max(start)]
}

/// Verify a presented Authorization header against the expected key.
pub fn verify(header: Option<&[u8]>, expected: Option<&str>) -> Result<(), ApiKeyFailure> {
    let expected = match expected {
        Some(k) if !k.is_empty() => k.as_bytes(),
        _ => return Err(ApiKeyFailure::KeyNotConfigured),
    };
    let presented = bearer_token(header).ok_or(ApiKeyFailure::BadCredentials)?;
    if presented.len() != expected.len() {
        // Length mismatch: still run a constant-time compare on equal-length
        // buffers so timing does not scale with the matched prefix.
        let _ = expected.ct_eq(expected);
        return Err(ApiKeyFailure::BadCredentials);
    }
    if presented.ct_eq(expected).into() {
        Ok(())
    } else {
        Err(ApiKeyFailure::BadCredentials)
    }
}

/// Verify against the process environment.
pub fn verify_env(header: Option<&[u8]>) -> Result<(), ApiKeyFailure> {
    let key = std::env::var(API_KEY_ENV).ok();
    verify(header, key.as_deref())
}
