//! Password hashing, token minting, and lockout arithmetic, port of `utils/authn.py`.
//!
//! Records are byte-compatible with CyClaw: `scrypt$131072$8$1$<salt_b64>$<hash_b64>`
//! (n=2^17, r=8, p=1, dklen=32). A bootstrap placeholder is `pending$` + a
//! record of a discarded random secret; verification always fails it after
//! paying the scrypt cost.

use base64::Engine;
use subtle::ConstantTimeEq;

use super::errors::{HarnessError, Result};

pub const SCRYPT_LOG_N: u8 = 17;
pub const SCRYPT_N: u64 = 1 << SCRYPT_LOG_N;
pub const SCRYPT_R: u32 = 8;
pub const SCRYPT_P: u32 = 1;
pub const SCRYPT_DKLEN: usize = 32;
const SALT_BYTES: usize = 16;
const ALGO: &str = "scrypt";
pub const PENDING_HASH_PREFIX: &str = "pending$";

pub const LOCKOUT_THRESHOLD: u32 = 5;
pub const LOCKOUT_BASE_SEC: f64 = 2.0;
pub const LOCKOUT_CEILING_SEC: f64 = 900.0;

pub const ROLES: [&str; 3] = ["admin", "operator", "audit"];
pub const DEFAULT_ROLE: &str = "operator";
pub const MIN_PASSWORD_LEN: usize = 12;
pub const MAX_PASSWORD_LEN: usize = 1024;

fn username_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\A[a-z0-9][a-z0-9_.-]{0,31}\z").expect("static regex"))
}

pub fn validate_role(role: &str) -> Result<String> {
    let canonical = role.trim().to_lowercase();
    if !ROLES.contains(&canonical.as_str()) {
        return Err(HarnessError::password_policy(format!(
            "role must be one of {}",
            ROLES.join(", ")
        )));
    }
    Ok(canonical)
}

/// Lowercased before the pattern check so `Operator` and `operator` cannot become two accounts.
pub fn validate_username(username: &str) -> Result<String> {
    let canonical = username.trim().to_lowercase();
    if !username_re().is_match(&canonical) {
        return Err(HarnessError::password_policy(
            "username must be 1-32 chars, start alphanumeric, and contain only a-z 0-9 . _ -",
        ));
    }
    Ok(canonical)
}

pub fn validate_password(password: &str) -> Result<()> {
    let len = password.chars().count();
    if len < MIN_PASSWORD_LEN {
        return Err(HarnessError::password_policy(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    if len > MAX_PASSWORD_LEN {
        return Err(HarnessError::password_policy(format!(
            "password must be at most {MAX_PASSWORD_LEN} characters"
        )));
    }
    Ok(())
}

fn b64(raw: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(raw)
}

fn unb64(text: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

fn derive(password: &str, salt: &[u8], log_n: u8, r: u32, p: u32) -> Option<[u8; SCRYPT_DKLEN]> {
    let params = scrypt::Params::new(log_n, r, p, SCRYPT_DKLEN).ok()?;
    let mut out = [0u8; SCRYPT_DKLEN];
    scrypt::scrypt(password.as_bytes(), salt, &params, &mut out).ok()?;
    Some(out)
}

/// `scrypt$n$r$p$salt_b64$hash_b64` with a random salt (or a pinned one for tests).
pub fn hash_password_with_salt(password: &str, salt: &[u8]) -> Result<String> {
    validate_password(password)?;
    hash_unchecked(password, salt)
}

fn hash_unchecked(password: &str, salt: &[u8]) -> Result<String> {
    let derived = derive(password, salt, SCRYPT_LOG_N, SCRYPT_R, SCRYPT_P)
        .ok_or_else(|| HarnessError::new("AUTH_ERROR", "scrypt derivation failed"))?;
    Ok(format!(
        "{ALGO}${SCRYPT_N}${SCRYPT_R}${SCRYPT_P}${}${}",
        b64(salt),
        b64(&derived)
    ))
}

fn random_salt() -> [u8; SALT_BYTES] {
    use rand::Rng;
    rand::thread_rng().gen()
}

/// The sole short-password exception: fresh restricted bootstrap credentials.
pub(super) fn hash_bootstrap_password() -> Result<String> {
    hash_unchecked("admin", &random_salt())
}

pub(super) fn valid_password_record(record: &str) -> bool {
    let record = record.strip_prefix(PENDING_HASH_PREFIX).unwrap_or(record);
    let parts: Vec<_> = record.split('$').collect();
    if parts.len() != 6 || parts[0] != ALGO {
        return false;
    }
    matches!((parts[1].parse::<u64>(),parts[2].parse::<u32>(),parts[3].parse::<u32>()),
        (Ok(n),Ok(r),Ok(p)) if n>=2 && n.is_power_of_two() && n<=SCRYPT_N && r>0 && r<=SCRYPT_R && p>0 && p<=SCRYPT_P)
        && unb64(parts[4]).is_some_and(|s| !s.is_empty() && s.len() <= 64)
        && unb64(parts[5]).is_some_and(|h| h.len() == SCRYPT_DKLEN)
}

pub fn hash_password(password: &str) -> Result<String> {
    hash_password_with_salt(password, &random_salt())
}

pub fn is_pending_password_record(record: &str) -> bool {
    record.starts_with(PENDING_HASH_PREFIX)
}

/// Record of a discarded secret, marked unusable until `set_password`.
pub fn hash_pending_placeholder() -> Result<String> {
    Ok(format!(
        "{PENDING_HASH_PREFIX}{}",
        hash_password(&generate_bootstrap_password())?
    ))
}

/// `(ok, needs_rehash)`. Malformed records return `(false, false)` instead of erroring.
pub fn verify_password(password: &str, record: &str) -> (bool, bool) {
    if let Some(inner) = record.strip_prefix(PENDING_HASH_PREFIX) {
        // Pay the inner cost so a pending admin is not a timing oracle, then fail closed.
        let _ = verify_password(password, inner);
        return (false, false);
    }
    let parts: Vec<&str> = record.split('$').collect();
    if parts.len() != 6 || parts[0] != ALGO {
        return (false, false);
    }
    let (n, r, p) = match (
        parts[1].parse::<u64>(),
        parts[2].parse::<u32>(),
        parts[3].parse::<u32>(),
    ) {
        (Ok(n), Ok(r), Ok(p)) => (n, r, p),
        _ => return (false, false),
    };
    let (salt, expected) = match (unb64(parts[4]), unb64(parts[5])) {
        (Some(s), Some(e)) => (s, e),
        _ => return (false, false),
    };
    // A record claiming stronger-than-policy parameters or a different dklen is
    // forged or corrupt, never a legitimate legacy row.
    if n > SCRYPT_N || r > SCRYPT_R || p > SCRYPT_P || expected.len() != SCRYPT_DKLEN {
        return (false, false);
    }
    if n < 2 || !n.is_power_of_two() || r == 0 || p == 0 {
        return (false, false);
    }
    let log_n = n.trailing_zeros() as u8;
    let derived = match derive(password, &salt, log_n, r, p) {
        Some(d) => d,
        None => return (false, false),
    };
    let ok: bool = derived.ct_eq(expected.as_slice()).into();
    let needs_rehash = ok && (n < SCRYPT_N || r < SCRYPT_R || p < SCRYPT_P);
    (ok, needs_rehash)
}

/// Seconds an account must wait after `failed_count` consecutive failures.
pub fn lockout_delay_sec(failed_count: u32) -> f64 {
    if failed_count < LOCKOUT_THRESHOLD {
        return 0.0;
    }
    let over = (failed_count - LOCKOUT_THRESHOLD).min(32);
    (LOCKOUT_BASE_SEC * 2f64.powi(over as i32)).min(LOCKOUT_CEILING_SEC)
}

pub fn is_locked(locked_until_ts: Option<f64>, now: f64) -> bool {
    match locked_until_ts {
        Some(t) if t > 0.0 => now < t,
        _ => false,
    }
}

pub fn next_lock_until(failed_count: u32, now: f64) -> f64 {
    now + lockout_delay_sec(failed_count)
}

pub fn new_session_id() -> String {
    super::random_urlsafe(32)
}

pub fn new_csrf_token() -> String {
    super::random_urlsafe(32)
}

pub fn hash_token(token: &str) -> String {
    super::sha256_hex(token)
}

/// 18 raw bytes -> 24 base64url chars: comfortably above `MIN_PASSWORD_LEN`.
pub fn generate_bootstrap_password() -> String {
    super::random_urlsafe(18)
}
