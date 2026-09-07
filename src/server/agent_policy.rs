//! What the console may ask the agentic child to run. Port of
//! `harness/agent_policy.py`.
//!
//! The console never sends an argv: it sends a profile NAME, and this module
//! maps that name to a fixed command. `RUN_ID_RE` is a deliberate duplicate
//! of `agentic::run_store::RUN_ID_RE` (I6 forbids the import);
//! `tests/invariant_guard.rs` asserts they stay identical.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::common::errors::{HarnessError, Result};

pub const RUN_ID_PATTERN: &str = r"\A[0-9a-f]{32}\z";
pub const DEFAULT_CHECK_PROFILE: &str = "cargo-test";

pub fn run_id_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(RUN_ID_PATTERN).expect("static regex"))
}

#[derive(Debug, Clone)]
pub struct CheckProfile {
    pub description: &'static str,
    pub argv: Vec<String>,
}

fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

fn profiles() -> &'static BTreeMap<&'static str, CheckProfile> {
    static TABLE: OnceLock<BTreeMap<&'static str, CheckProfile>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let mut m = BTreeMap::new();
        m.insert(
            "cargo-test",
            CheckProfile {
                description: "run the Rust test suite",
                argv: s(&["cargo", "test", "--quiet"]),
            },
        );
        m.insert(
            "cargo-clippy",
            CheckProfile {
                description: "lint with clippy, warnings denied",
                argv: s(&["cargo", "clippy", "--all-targets", "--", "-D", "warnings"]),
            },
        );
        m.insert(
            "cargo-fmt",
            CheckProfile {
                description: "check rustfmt formatting",
                argv: s(&["cargo", "fmt", "--check"]),
            },
        );
        m.insert(
            "pytest",
            CheckProfile {
                description: "run the Python test suite",
                argv: s(&[python(), "-m", "pytest", "-q", "--tb=short"]),
            },
        );
        m.insert(
            "ruff",
            CheckProfile {
                description: "lint with the repo's ruff selection",
                argv: s(&[python(), "-m", "ruff", "check", "--select", "E,F,I,B,C4,UP,S", "."]),
            },
        );
        m
    })
}

/// `(name, description)` for every selectable profile.
pub fn available_profiles() -> Vec<(String, String)> {
    profiles()
        .iter()
        .map(|(k, v)| (k.to_string(), v.description.to_string()))
        .collect()
}

/// Map profile names to the `[{"name","argv"}]` manifest the child parses.
/// An unknown name is an error, never silently skipped.
pub fn resolve_check_profiles(names: &[String]) -> Result<Vec<Value>> {
    if names.is_empty() {
        return Err(HarnessError::new(
            "UNKNOWN_CHECK_PROFILE",
            "at least one check profile is required",
        ));
    }
    let mut out = Vec::new();
    for name in names {
        match profiles().get(name.as_str()) {
            Some(p) => out.push(json!({"name": name, "argv": p.argv})),
            None => {
                let known: Vec<&str> = profiles().keys().copied().collect();
                return Err(HarnessError::new(
                    "UNKNOWN_CHECK_PROFILE",
                    format!("unknown check profile '{name}'; available: {}", known.join(", ")),
                ));
            }
        }
    }
    Ok(out)
}
