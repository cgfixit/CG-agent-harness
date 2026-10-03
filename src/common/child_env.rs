//! Shared child-environment opt-outs: the one table of telemetry and
//! update-check switches every process the harness launches receives, so a
//! spawn site cannot drift from its siblings by hand-rolling the pairs.
//!
//! Reached from the server-side MCP spawn and from the agentic spawn sites
//! alike, so nothing here may reference `crate::agentic` (I6, checked by
//! `tests/invariant_guard.rs`). The independent oracle in
//! `.claude/skills/cgagentharness-otel-hardening/check_otel.py` pins every
//! pair below per child kind and the kind each site asks for: change both in
//! one commit.

use std::collections::BTreeMap;

/// What a child runs decides which switches it reads. Every kind carries
/// `DO_NOT_TRACK=1`, the cross-tool opt-out (consoledonottrack.com) that is
/// the only switch an arbitrary program can be expected to honor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Child {
    /// The GitHub CLI, run directly or by git as its credential helper.
    Gh,
    /// Caller-declared verification checks (pytest, ruff, cargo, ...) over a
    /// repository: whatever that repository's own toolchain reads.
    VerificationCheck,
    /// An operator-declared MCP stdio server.
    McpServer,
}

/// The exact `name=value` pairs a child of that kind receives.
pub fn telemetry_opt_outs(child: Child) -> &'static [(&'static str, &'static str)] {
    match child {
        Child::McpServer => &[("DO_NOT_TRACK", "1")],
        Child::Gh => &[
            ("DO_NOT_TRACK", "1"),
            ("GH_TELEMETRY", "false"),
            ("GH_NO_UPDATE_NOTIFIER", "1"),
            ("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1"),
        ],
        Child::VerificationCheck => &[
            ("DO_NOT_TRACK", "1"),
            ("GH_TELEMETRY", "false"),
            ("HF_HUB_DISABLE_TELEMETRY", "1"),
            ("ANONYMIZED_TELEMETRY", "false"),
        ],
    }
}

/// Insert the child's opt-outs into `env`, replacing any existing value. Call
/// it last: after the inherited allowlist, after caller extras and after an
/// operator-declared `env`, so none of them can switch telemetry back on.
pub fn apply(env: &mut BTreeMap<String, String>, child: Child) {
    for (key, value) in telemetry_opt_outs(child) {
        env.insert((*key).into(), (*value).into());
    }
}

#[cfg(test)]
mod tests {
    use super::{apply, telemetry_opt_outs, Child};
    use std::collections::BTreeMap;

    const KINDS: [Child; 3] = [Child::Gh, Child::VerificationCheck, Child::McpServer];

    #[test]
    fn every_kind_carries_do_not_track_and_no_name_has_two_values() {
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        for kind in KINDS {
            let pairs = telemetry_opt_outs(kind);
            assert!(pairs.contains(&("DO_NOT_TRACK", "1")), "{kind:?}");
            for (key, value) in pairs {
                let previous = seen.insert(key, value);
                assert!(previous.is_none_or(|p| p == *value), "{key} differs across kinds");
            }
        }
    }

    #[test]
    fn each_kind_delivers_exactly_what_its_binary_reads() {
        assert_eq!(telemetry_opt_outs(Child::McpServer), &[("DO_NOT_TRACK", "1")]);
        let gh: BTreeMap<&str, &str> = telemetry_opt_outs(Child::Gh).iter().copied().collect();
        assert_eq!(gh.get("GH_TELEMETRY"), Some(&"false"));
        assert_eq!(gh.get("GH_NO_UPDATE_NOTIFIER"), Some(&"1"));
        assert_eq!(gh.get("GH_NO_EXTENSION_UPDATE_NOTIFIER"), Some(&"1"));
        assert!(!gh.contains_key("HF_HUB_DISABLE_TELEMETRY"));
        let check: BTreeMap<&str, &str> = telemetry_opt_outs(Child::VerificationCheck).iter().copied().collect();
        assert_eq!(check.get("GH_TELEMETRY"), Some(&"false"));
        assert_eq!(check.get("HF_HUB_DISABLE_TELEMETRY"), Some(&"1"));
        assert_eq!(check.get("ANONYMIZED_TELEMETRY"), Some(&"false"));
        assert!(!check.contains_key("GH_NO_UPDATE_NOTIFIER"));
    }

    #[test]
    fn apply_overrides_an_earlier_value_and_leaves_the_rest() {
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        env.insert("GH_TELEMETRY".into(), "true".into());
        env.insert("DO_NOT_TRACK".into(), "0".into());
        env.insert("KEEP".into(), "me".into());
        apply(&mut env, Child::Gh);
        assert_eq!(env.get("GH_TELEMETRY").map(String::as_str), Some("false"));
        assert_eq!(env.get("DO_NOT_TRACK").map(String::as_str), Some("1"));
        assert_eq!(env.get("KEEP").map(String::as_str), Some("me"));
        // KEEP plus the four gh pairs: the two overridden keys were not duplicated.
        assert_eq!(env.len(), 1 + telemetry_opt_outs(Child::Gh).len());
    }
}
