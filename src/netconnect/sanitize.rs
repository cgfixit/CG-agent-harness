//! Device-supplied text. The value is display data only.
//!
//! Callers must not place it in argv, a shell command, a filesystem path,
//! or a URL that this process will fetch. The structured form always sets
//! `untrusted` so a later encoder cannot treat it as operator input.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UntrustedString {
    value: String,
    untrusted: bool,
}

impl UntrustedString {
    pub fn new(value: String) -> Self {
        Self { value, untrusted: true }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn is_untrusted(&self) -> bool {
        self.untrusted
    }
}

/// Cap to `max_chars` Unicode scalars and drop control characters.
pub fn sanitize_untrusted(input: &str, max_chars: usize) -> UntrustedString {
    let value: String = input.chars().filter(|c| !c.is_control()).take(max_chars).collect();
    UntrustedString::new(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_length_and_strips_controls() {
        let marked = sanitize_untrusted("hello\n\tworld\u{0007}\r", 8);
        assert_eq!(marked.value(), "hellowor");
        assert!(marked.is_untrusted());
        let json = serde_json::to_value(&marked).unwrap();
        assert_eq!(json["untrusted"], true);
        assert_eq!(json["value"], "hellowor");
        assert!(!json["value"].as_str().unwrap().contains('\n'));

        let long = "a".repeat(100);
        assert_eq!(sanitize_untrusted(&long, 64).value().chars().count(), 64);
        assert_eq!(sanitize_untrusted("café\u{0001}!", 3).value(), "caf");
        assert_eq!(sanitize_untrusted("ok", 0).value(), "");
    }
}
