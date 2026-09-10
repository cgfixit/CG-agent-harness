//! Allowlisted dotenv secret store behind the console's `/api` panel.
//! Port of `harness/env_keys.py`: `export KEY='value'` lines, 0600, atomic,
//! unrelated lines preserved verbatim, never returns a value.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::common::atomic::write_atomic;
use crate::common::errors::{HarnessError, Result};

pub const ENV_KEY_ERROR: &str = "ENV_KEY_REJECTED";
const FORBIDDEN_CHARS: [char; 3] = ['\n', '\r', '\0'];
const MAX_VALUE_LEN: usize = 4096;
const MIN_LEN_FOR_TAIL: usize = 12;
const TAIL_LEN: usize = 4;
const MASK: &str = "••••••••";
const QUOTE_ESCAPE: &str = r"'\''";
const HEADER_LINES: [&str; 3] = [
    "# CGagentHarness secrets - chmod 600. Managed by the harness console's /api panel.",
    "# Do not commit. Do not copy into config.yaml. Do not paste into chat logs.",
    "# Unix desktop/serve loads this file as data at startup; other launchers need environment values.",
];

#[derive(Debug, Clone, Copy)]
pub struct KeySpec {
    pub name: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
    pub self_auth: bool,
}

/// Every entry is an env var this binary actually reads.
pub const MANAGED_KEYS: [KeySpec; 4] = [
    KeySpec {
        name: crate::common::apikey::API_KEY_ENV,
        label: "CGagentHarness API key",
        detail: "Bearer secret for this console's guarded routes.",
        self_auth: true,
    },
    KeySpec {
        name: "GROK_API_KEY",
        label: "Grok (xAI)",
        detail: "Optional cloud planner for the agentic real-repo loop (six-gate, per-run --confirm-online).",
        self_auth: false,
    },
    KeySpec {
        name: "ANTHROPIC_API_KEY",
        label: "Claude (Anthropic)",
        detail: "Optional cloud planner for the agentic real-repo loop (same six-gate).",
        self_auth: false,
    },
    KeySpec {
        name: "DEEPAGENT_API_KEY",
        label: "OpenAI-compatible planner key",
        detail: "Bearer for a non-Ollama OpenAI-compatible local planner endpoint.",
        self_auth: false,
    },
];

fn err(message: impl Into<String>) -> HarnessError {
    HarnessError::new(ENV_KEY_ERROR, message)
}

pub fn spec_for(name: &str) -> Result<KeySpec> {
    MANAGED_KEYS
        .iter()
        .copied()
        .find(|s| s.name == name)
        .ok_or_else(|| err(format!("'{name}' is not a settable CGagentHarness key")))
}

pub fn validate_value(name: &str, secret: &str) -> Result<String> {
    spec_for(name)?;
    let cleaned = secret.trim();
    if cleaned.is_empty() {
        return Err(err(format!("{name} value is empty")));
    }
    if cleaned.chars().any(|c| FORBIDDEN_CHARS.contains(&c)) {
        return Err(err(format!("{name} value contains a forbidden control character")));
    }
    if cleaned.chars().count() > MAX_VALUE_LEN {
        return Err(err(format!("{name} value exceeds {MAX_VALUE_LEN} characters")));
    }
    Ok(cleaned.to_string())
}

/// Fixed-width mask plus, when safe, the last 4 chars. Never the value.
pub fn mask(secret: &str) -> String {
    let n = secret.chars().count();
    if n < MIN_LEN_FOR_TAIL {
        return MASK.to_string();
    }
    let tail: String = secret.chars().skip(n - TAIL_LEN).collect();
    format!("{MASK}{tail}")
}

fn shell_single_quote(secret: &str) -> String {
    format!("'{}'", secret.replace('\'', QUOTE_ESCAPE))
}

fn unquote(raw: &str) -> String {
    let token = raw.trim();
    if token.len() >= 2 {
        let first = token.chars().next().unwrap();
        let last = token.chars().last().unwrap();
        if first == last && (first == '\'' || first == '"') {
            let inner = &token[1..token.len() - 1];
            return if first == '\'' {
                inner.replace(QUOTE_ESCAPE, "'")
            } else {
                inner.to_string()
            };
        }
    }
    token.to_string()
}

fn split_assignment(line: &str) -> Option<(String, String)> {
    let mut stripped = line.trim();
    if stripped.is_empty() || stripped.starts_with('#') {
        return None;
    }
    if let Some(rest) = stripped.strip_prefix("export ") {
        stripped = rest.trim_start();
    }
    let (name, raw) = stripped.split_once('=')?;
    Some((name.trim().to_string(), raw.to_string()))
}

/// `{name: value}` for allowlisted keys only; missing file reads as empty.
pub fn read_env_file(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    parse_env_text(&text)
}

fn parse_env_text(text: &str) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    for line in text.lines() {
        if let Some((name, raw)) = split_assignment(line) {
            if MANAGED_KEYS.iter().any(|s| s.name == name) {
                found.insert(name, unquote(&raw));
            }
        }
    }
    found
}

/// Unix desktop/headless startup reads credentials as data through one validated descriptor.
/// Missing is distinct from unreadable/unsafe; no shell expansion is performed.
#[cfg(unix)]
pub fn read_startup_keys(path: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(_) => anyhow::bail!("credential file unreadable; check its owner, access and symlink status"),
    };
    let metadata = file.metadata()?;
    // SAFETY: getuid has no arguments or memory effects.
    if !metadata.is_file() || metadata.uid() != unsafe { libc::getuid() } || metadata.mode() & 0o077 != 0 {
        anyhow::bail!("credential file must be a private regular file owned by this user (0600)");
    }
    let mut text = String::new();
    file.take(65537)
        .read_to_string(&mut text)
        .map_err(|_| anyhow::anyhow!("credential file unreadable or not UTF-8"))?;
    if text.len() > 65536 {
        anyhow::bail!("credential file exceeds 64 KiB");
    }
    let keys = parse_env_text(&text);
    for (name, value) in &keys {
        validate_value(name, value)
            .map_err(|_| anyhow::anyhow!("credential file contains an invalid managed value"))?;
    }
    Ok(keys)
}

/// Presence + masked tail for every managed key. Never returns a value.
pub fn read_status(path: &Path) -> Vec<Value> {
    let stored = read_env_file(path);
    MANAGED_KEYS
        .iter()
        .map(|spec| {
            let live = std::env::var(spec.name).unwrap_or_default().trim().to_string();
            let in_file = stored.get(spec.name).cloned().unwrap_or_default();
            let value_for_mask = if live.is_empty() { in_file.clone() } else { live.clone() };
            let source = if !live.is_empty() {
                "env"
            } else if !in_file.is_empty() {
                "file"
            } else {
                "unset"
            };
            json!({
                "name": spec.name,
                "label": spec.label,
                "detail": spec.detail,
                "self_auth": spec.self_auth,
                "configured": !value_for_mask.is_empty(),
                "masked": if value_for_mask.is_empty() { String::new() } else { mask(&value_for_mask) },
                "source": source,
                "pending_restart": !in_file.is_empty() && in_file != live,
            })
        })
        .collect()
}

fn render_file(existing: &[String], updates: &BTreeMap<String, String>) -> String {
    let mut kept: Vec<String> = Vec::new();
    for line in existing {
        if let Some((name, _)) = split_assignment(line) {
            if updates.contains_key(&name) {
                continue;
            }
        }
        kept.push(line.clone());
    }
    if kept.is_empty() {
        kept = HEADER_LINES.iter().map(|s| s.to_string()).collect();
    }
    for (name, secret) in updates {
        kept.push(format!("export {name}={}", shell_single_quote(secret)));
    }
    format!("{}\n", kept.join("\n"))
}

/// Validate every value BEFORE writing; returns names only.
pub fn write_keys(path: &Path, updates: &BTreeMap<String, String>) -> Result<Value> {
    if updates.is_empty() {
        return Err(err("no keys supplied"));
    }
    let mut cleaned = BTreeMap::new();
    for (name, secret) in updates {
        cleaned.insert(name.clone(), validate_value(name, secret)?);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let existing: Vec<String> = std::fs::read_to_string(path)
        .map(|t| t.lines().map(|l| l.to_string()).collect())
        .unwrap_or_default();
    let rendered = render_file(&existing, &cleaned);
    write_atomic(path, rendered.as_bytes(), Some(0o600))?;
    let written: Vec<&String> = cleaned.keys().collect();
    let self_auth: Vec<&String> = cleaned
        .keys()
        .filter(|n| MANAGED_KEYS.iter().any(|s| s.name == n.as_str() && s.self_auth))
        .collect();
    Ok(json!({
        "written": written,
        "path": path.display().to_string(),
        "restart_required": true,
        "self_auth_written": self_auth,
    }))
}
