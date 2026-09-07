//! Provider-neutral tool name gate, port of `utils/tool_broker.py`.
//! Empty allowlist is deny. Audit logs the tool name and an argv digest only.

use std::collections::BTreeSet;

use serde_json::json;

use super::audit::Audit;
use super::errors::{HarnessError, Result};

#[derive(Debug, Clone)]
pub struct ToolVerdict {
    pub allowed: bool,
    pub reason: String,
    pub tool: String,
    pub argv_digest: String,
}

/// SHA-256 of the compact JSON encoding of the argv list (ASCII-escaped like
/// Python's `json.dumps(..., ensure_ascii=True)`).
pub fn argv_digest(argv: &[String]) -> String {
    let payload = compact_ascii_json(argv);
    super::sha256_hex(&payload)
}

fn compact_ascii_json(argv: &[String]) -> String {
    let mut out = String::from("[");
    for (i, arg) in argv.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        for ch in arg.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c if c.is_ascii() => out.push(c),
                c => {
                    let mut buf = [0u16; 2];
                    for unit in c.encode_utf16(&mut buf) {
                        out.push_str(&format!("\\u{unit:04x}"));
                    }
                }
            }
        }
        out.push('"');
    }
    out.push(']');
    out
}

pub fn decide(name: &str, argv: &[String], allowlist: &BTreeSet<String>) -> ToolVerdict {
    let digest = argv_digest(argv);
    let tool = name.trim().to_string();
    if tool.is_empty() {
        return ToolVerdict {
            allowed: false,
            reason: "empty tool name".into(),
            tool,
            argv_digest: digest,
        };
    }
    if !allowlist.contains(&tool) {
        return ToolVerdict {
            allowed: false,
            reason: "unknown tool".into(),
            tool,
            argv_digest: digest,
        };
    }
    ToolVerdict {
        allowed: true,
        reason: "allowlisted".into(),
        tool,
        argv_digest: digest,
    }
}

/// `decide` then audit; `TOOL_DENIED` on deny.
pub fn assert_allowed(name: &str, argv: &[String], allowlist: &BTreeSet<String>, audit: &Audit) -> Result<ToolVerdict> {
    let verdict = decide(name, argv, allowlist);
    audit.log(json!({
        "event": "tool_broker_decision",
        "tool": verdict.tool,
        "argv_digest": verdict.argv_digest,
        "allowed": verdict.allowed,
        "reason": verdict.reason,
    }));
    if !verdict.allowed {
        return Err(
            HarnessError::tool_denied(format!("tool '{}' denied ({})", verdict.tool, verdict.reason))
                .with_details(json!({"tool": verdict.tool, "argv_digest": verdict.argv_digest})),
        );
    }
    Ok(verdict)
}
