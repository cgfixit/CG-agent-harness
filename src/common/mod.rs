//! Shared primitives used by both the server and the agentic child process.
//! Nothing here may reference `crate::agentic` or `crate::server`.

pub mod apikey;
pub mod atomic;
pub mod audit;
pub mod auth_store;
pub mod authn;
pub mod bounded_log;
pub mod config;
pub mod errors;
pub mod file_lease;
pub mod home;
pub mod home_lock;
pub mod identity;
pub mod injection;
pub mod local_tls;
pub mod mcp;
pub mod mcp_policy;
pub mod mcp_worker;
pub mod process;
pub mod ratelimit;
pub mod repo_paths;
pub mod sandbox_wrap;
pub mod tool_broker;

/// Current Unix time as f64 seconds (the shape CyClaw stores in its JSON files).
pub fn now_ts() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// ISO-8601 UTC timestamp with microsecond precision, e.g. `2026-09-07T12:34:56.123456+00:00`.
pub fn iso_now() -> String {
    let now = time::OffsetDateTime::now_utc();
    let fmt =
        time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]+00:00");
    now.format(&fmt)
        .unwrap_or_else(|_| "1970-01-01T00:00:00.000000+00:00".to_string())
}

/// Lowercase hex of `n` random bytes.
pub fn random_hex(n: usize) -> String {
    use rand::RngCore;
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// URL-safe base64 (no padding) of `n` random bytes, the shape of
/// Python's `secrets.token_urlsafe(n)`.
pub fn random_urlsafe(n: usize) -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// SHA-256 hex digest of a UTF-8 string.
pub fn sha256_hex(text: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(text.as_bytes()))
}

/// SHA-256 hex digest of raw bytes.
pub fn sha256_bytes_hex(data: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(data))
}

/// Truncate a string to at most `max` characters (not bytes).
pub fn clip_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Fixed byte ceiling for an explicitly reviewed pull-request description.
pub const MAX_PR_BODY_BYTES: usize = 65_536;

#[cfg(test)]
mod tests {
    /// Timestamps are `f64` seconds and travel through JSON on every route.
    /// The value below is one of the 17-significant-digit shapes that
    /// serde_json's default fast float parser rounds to the neighbouring
    /// double; the `float_roundtrip` feature makes the parse exact, so a
    /// value written by the server compares equal after a client re-parse.
    #[test]
    fn f64_timestamps_survive_a_json_round_trip_exactly() {
        for text in ["1789875807.3327327", "1789875807.332733", "1758337152.1234567"] {
            let parsed: f64 = serde_json::from_str(text).unwrap();
            assert_eq!(
                serde_json::to_string(&parsed).unwrap(),
                text,
                "shortest repr must round-trip"
            );
            let value: serde_json::Value = serde_json::from_str(text).unwrap();
            assert_eq!(value.as_f64(), Some(parsed));
        }
        let now = super::now_ts();
        let encoded = serde_json::to_string(&now).unwrap();
        let back: f64 = serde_json::from_str(&encoded).unwrap();
        assert_eq!(back.to_bits(), now.to_bits(), "{encoded}");
    }
}
