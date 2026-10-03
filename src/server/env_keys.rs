//! Allowlisted managed secrets behind the console's `/api` panel.
//! Port of `harness/env_keys.py`: presence and a masked tail only.
//! The default store is the OS credential store. The home `.env` is the
//! one-time migration source, or the live store when
//! `security.allow_plaintext_key_file` is literal true.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

// ponytail: one small credential file per owned server; split locks only if contention is measured.
static KEY_MUTATION: Mutex<()> = Mutex::new(());
use std::path::Path;

use serde_json::{json, Value};

use crate::common::credential_store::write_private_text_atomic;
use crate::common::errors::{HarnessError, Result};

pub const ENV_KEY_ERROR: &str = "ENV_KEY_REJECTED";
const FORBIDDEN_CHARS: [char; 3] = ['\n', '\r', '\0'];
const MAX_VALUE_LEN: usize = 4096;
const MIN_LEN_FOR_TAIL: usize = 12;
const TAIL_LEN: usize = 4;
const MASK: &str = "••••••••";
const QUOTE_ESCAPE: &str = r"'\''";
const HEADER_LINES: [&str; 3] = [
    "# CGagentHarness secrets - chmod 600. Used only when security.allow_plaintext_key_file is literal true.",
    "# Do not commit. Do not copy into config.yaml. Do not paste into chat logs.",
    "# Inherited environment values win. The default store is the OS credential store, not this file.",
];

#[derive(Debug, Clone, Copy)]
pub struct KeySpec {
    pub name: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
    pub self_auth: bool,
}

/// Every entry is an env var this binary actually reads.
pub const MANAGED_KEYS: [KeySpec; 7] = [
    KeySpec {
        name: "CGAGENTHARNESS_WEBHOOK_TOKEN",
        label: "Completion webhook bearer token",
        detail: "Optional bearer for the configured completion webhook. Restart after saving. Never sent to a model.",
        self_auth: false,
    },
    KeySpec {
        name: "SERPAPI_API_KEY",
        label: "Google results (SerpAPI)",
        detail: "SerpAPI key (not a Google Cloud key). Save to use Google keyword search immediately; no Google URL permission is needed. Linked pages still need URL permission.",
        self_auth: false,
    },
    KeySpec {
        name: "GH_TOKEN",
        label: "GitHub personal access token",
        detail: "Used by GitHub CLI and its Git credential helper after restart. Leave unset to use existing gh login. Token permissions and repository write approvals still apply.",
        self_auth: false,
    },
    KeySpec {
        name: crate::common::apikey::API_KEY_ENV,
        label: "CGagentHarness API key",
        detail: "Optional compatibility metadata. Never grants account access or provider permissions.",
        self_auth: true,
    },
    KeySpec {
        name: "GROK_API_KEY",
        label: "Grok (xAI)",
        detail: "Optional explicit Grok chat and cloud planner key. Chat sends a message only after selecting Grok; the coding loop retains its six gates.",
        self_auth: false,
    },
    KeySpec {
        name: "ANTHROPIC_API_KEY",
        label: "Claude (Anthropic)",
        detail: "Optional explicit Claude chat and cloud planner key. Chat sends a message only after selecting Claude; the coding loop retains its six gates.",
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

fn is_managed(name: &str) -> bool {
    MANAGED_KEYS.iter().any(|spec| spec.name == name)
}

fn parse_env_text(text: &str) -> BTreeMap<String, String> {
    crate::common::credential_store::parse_managed(text, is_managed)
}

pub fn read_startup_keys(path: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let keys = parse_env_text(&crate::common::credential_store::read_private_text(path)?);
    for (name, value) in &keys {
        validate_value(name, value)
            .map_err(|_| anyhow::anyhow!("credential file contains an invalid managed value"))?;
    }
    Ok(keys)
}

/// Names still present in a legacy file. Values are dropped before the caller sees them.
pub fn legacy_managed_names(path: &Path) -> anyhow::Result<Vec<String>> {
    let text = crate::common::credential_store::read_private_text(path)?;
    Ok(parse_env_text(&text).into_keys().collect())
}

/// Presence + masked tail for every managed key. Never returns a value.
pub fn read_status(path: &Path, loaded_from_file: &BTreeSet<String>) -> Result<Vec<Value>> {
    let _guard = KEY_MUTATION.lock().map_err(|_| err("credential lock unavailable"))?;
    let stored = read_startup_keys(path).map_err(|e| err(e.to_string()))?;
    Ok(status_rows(&stored, loaded_from_file, false))
}

pub fn status_from_saved(
    stored: &BTreeMap<String, String>,
    loaded_from_saved: &BTreeSet<String>,
    from_store: bool,
) -> Vec<Value> {
    status_rows(stored, loaded_from_saved, from_store)
}

fn status_rows(
    stored: &BTreeMap<String, String>,
    loaded_from_saved: &BTreeSet<String>,
    from_store: bool,
) -> Vec<Value> {
    let saved_label = if from_store { "os_store" } else { "file" };
    let hot_saved = if from_store { "saved_store" } else { "saved_file" };
    let startup_saved = if from_store { "startup_store" } else { "startup_file" };
    MANAGED_KEYS
        .iter()
        .map(|spec| {
            let hot_reload = spec.name == "SERPAPI_API_KEY";
            let from_saved = loaded_from_saved.contains(spec.name) || std::env::var_os(spec.name).is_none();
            let live = if hot_reload && from_saved {
                stored.get(spec.name).cloned().unwrap_or_default()
            } else {
                std::env::var(spec.name).unwrap_or_default().trim().to_string()
            };
            let saved = stored.get(spec.name).cloned().unwrap_or_default();
            let value_for_mask = if live.is_empty() { saved.clone() } else { live.clone() };
            let source = if !live.is_empty() {
                "env"
            } else if !saved.is_empty() {
                saved_label
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
                "saved_configured": !saved.is_empty(),
                "saved_masked": if saved.is_empty() { String::new() } else { mask(&saved) },
                "active_configured": !live.is_empty(),
                "active_masked": if live.is_empty() { String::new() } else { mask(&live) },
                "active_source": if hot_reload && from_saved { hot_saved } else if loaded_from_saved.contains(spec.name) { startup_saved } else if std::env::var_os(spec.name).is_some() { "environment" } else { "unset" },
                "environment_override": !loaded_from_saved.contains(spec.name) && std::env::var_os(spec.name).is_some(),
                "pending_restart": !hot_reload && saved != live,
            })
        })
        .collect()
}

fn render_file(existing: &[String], updates: &BTreeMap<String, String>) -> String {
    let mut kept: Vec<String> = Vec::new();
    for line in existing {
        if let Some((name, _)) = crate::common::credential_store::split_assignment(line) {
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
        if !secret.is_empty() {
            kept.push(format!("export {name}={}", shell_single_quote(secret)));
        }
    }
    format!("{}\n", kept.join("\n"))
}

/// Validate every value BEFORE writing; returns names only.
pub fn write_keys(path: &Path, updates: &BTreeMap<String, String>) -> Result<Value> {
    update_keys(path, updates, &[])
}

fn clean_updates(updates: &BTreeMap<String, String>, clear: &[String]) -> Result<BTreeMap<String, String>> {
    if updates.is_empty() && clear.is_empty() {
        return Err(err("no keys supplied"));
    }
    let mut cleaned = BTreeMap::new();
    for (name, secret) in updates {
        cleaned.insert(name.clone(), validate_value(name, secret)?);
    }
    for name in clear {
        spec_for(name)?;
        if cleaned.insert(name.clone(), String::new()).is_some() {
            return Err(err("a key cannot be saved and cleared together or cleared twice"));
        }
    }
    Ok(cleaned)
}

fn result_json(cleaned: &BTreeMap<String, String>, clear: &[String], path: &str) -> Value {
    let written: Vec<&String> = cleaned.keys().collect();
    let self_auth: Vec<&String> = cleaned
        .keys()
        .filter(|name| {
            MANAGED_KEYS
                .iter()
                .any(|spec| spec.name == name.as_str() && spec.self_auth)
        })
        .collect();
    json!({
        "written": written,
        "cleared": clear,
        "path": path,
        "restart_required": cleaned.keys().any(|name| name != "SERPAPI_API_KEY"),
        "self_auth_written": self_auth,
    })
}

pub fn update_keys(path: &Path, updates: &BTreeMap<String, String>, clear: &[String]) -> Result<Value> {
    let cleaned = clean_updates(updates, clear)?;
    let _guard = KEY_MUTATION.lock().map_err(|_| err("credential lock unavailable"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let existing: Vec<String> = crate::common::credential_store::read_private_text(path)
        .map_err(|e| err(e.to_string()))?
        .lines()
        .map(str::to_string)
        .collect();
    let rendered = render_file(&existing, &cleaned);
    if rendered.len() > 65536 {
        return Err(err("credential file exceeds 64 KiB"));
    }
    write_private_text_atomic(path, rendered.as_bytes())?;
    Ok(result_json(&cleaned, clear, &path.display().to_string()))
}

pub struct StartupKeys {
    pub values: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

/// Inherited names are omitted so the caller does not overwrite an explicit environment value.
pub fn without_inherited(saved: BTreeMap<String, String>, inherited: &BTreeSet<String>) -> BTreeMap<String, String> {
    saved
        .into_iter()
        .filter(|(name, _)| !inherited.contains(name))
        .collect()
}

/// Plaintext opt-in reads the file and does not touch `store`. Otherwise migrate, then read the store.
/// Failed migration names are not loaded. Warnings name the key and the reason, never the value.
pub fn load_startup(
    path: &Path,
    allow_plaintext: bool,
    store: &dyn crate::common::credential_store::CredentialStore,
) -> anyhow::Result<StartupKeys> {
    use crate::common::credential_store::{migration_warning, StoreError};
    let text = crate::common::credential_store::read_private_text(path)?;
    let assignments = parse_env_text(&text);
    for (name, value) in &assignments {
        validate_value(name, value)
            .map_err(|_| anyhow::anyhow!("credential file contains an invalid managed value"))?;
    }
    if allow_plaintext {
        return Ok(StartupKeys {
            values: assignments,
            warnings: Vec::new(),
        });
    }
    let report = crate::common::credential_store::migrate_text(&text, &assignments, store);
    let mut warnings: Vec<String> = report
        .failed
        .iter()
        .map(|(name, error)| migration_warning(name, *error))
        .collect();
    if report.text != text && write_private_text_atomic(path, report.text.as_bytes()).is_err() {
        warnings.push(
            "Migrated keys could not be removed from the private .env file. Fix that file's permissions and restart."
                .to_string(),
        );
    }
    let failed: BTreeSet<String> = report.failed.into_iter().map(|(name, _)| name).collect();
    let mut values = BTreeMap::new();
    if !warnings.iter().any(|warning| warning.contains("unavailable")) {
        for spec in MANAGED_KEYS {
            if failed.contains(spec.name) {
                continue;
            }
            match store.get(spec.name) {
                Ok(Some(value)) => match validate_value(spec.name, &value) {
                    Ok(cleaned) => {
                        values.insert(spec.name.to_string(), cleaned);
                    }
                    Err(_) => warnings.push(format!(
                        "{} in the OS credential store is not a valid managed value and was not loaded.",
                        spec.name
                    )),
                },
                Ok(None) => {}
                Err(StoreError::Unavailable) => break,
                Err(_) => warnings.push(format!(
                    "{} could not be read from the OS credential store and was not loaded.",
                    spec.name
                )),
            }
        }
    }
    Ok(StartupKeys { values, warnings })
}

pub fn saved_from_store(
    store: &dyn crate::common::credential_store::CredentialStore,
) -> Result<(BTreeMap<String, String>, Option<String>)> {
    use crate::common::credential_store::{save_refusal, StoreError};
    let _guard = KEY_MUTATION.lock().map_err(|_| err("credential lock unavailable"))?;
    let mut saved = BTreeMap::new();
    for spec in MANAGED_KEYS {
        match store.get(spec.name) {
            Ok(Some(value)) => match validate_value(spec.name, &value) {
                Ok(cleaned) => {
                    saved.insert(spec.name.to_string(), cleaned);
                }
                Err(_) => {
                    return Ok((
                        saved,
                        Some(format!(
                            "{} in the OS credential store is not a valid managed value and was ignored.",
                            spec.name
                        )),
                    ));
                }
            },
            Ok(None) => {}
            Err(StoreError::Unavailable) => {
                return Ok((BTreeMap::new(), Some(save_refusal(StoreError::Unavailable).to_string())))
            }
            Err(_) => {
                return Ok((
                    saved,
                    Some(format!("{} could not be read from the OS credential store.", spec.name)),
                ));
            }
        }
    }
    Ok((saved, None))
}

pub fn update_os_keys(
    store: &dyn crate::common::credential_store::CredentialStore,
    updates: &BTreeMap<String, String>,
    clear: &[String],
) -> Result<Value> {
    use crate::common::credential_store::{save_refusal, secret_eq, StoreError, OS_STORE_LABEL};
    let cleaned = clean_updates(updates, clear)?;
    let _guard = KEY_MUTATION.lock().map_err(|_| err("credential lock unavailable"))?;
    let mut prior = Vec::new();
    for name in cleaned.keys() {
        prior.push((name.clone(), store.get(name).map_err(|error| err(save_refusal(error)))?));
    }
    let mut applied = Vec::new();
    for (name, secret) in &cleaned {
        let result = if secret.is_empty() {
            store.delete(name)
        } else {
            store.set(name, secret).and_then(|_| match store.get(name) {
                Ok(Some(got)) if secret_eq(&got, secret) => Ok(()),
                Ok(_) => {
                    let _ = store.delete(name);
                    Err(StoreError::VerifyMismatch)
                }
                Err(error) => Err(error),
            })
        };
        if let Err(error) = result {
            for (prior_name, prior_value) in prior.iter().rev() {
                if !applied.iter().any(|applied_name| applied_name == prior_name) {
                    continue;
                }
                match prior_value {
                    Some(value) => {
                        let _ = store.set(prior_name, value);
                    }
                    None => {
                        let _ = store.delete(prior_name);
                    }
                }
            }
            return Err(err(save_refusal(error)));
        }
        applied.push(name.clone());
    }
    Ok(result_json(&cleaned, clear, OS_STORE_LABEL))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::credential_store::{CredentialStore, StoreError};
    use std::sync::Mutex;

    struct MemoryStore {
        values: Mutex<BTreeMap<String, String>>,
    }

    impl CredentialStore for MemoryStore {
        fn get(&self, name: &str) -> std::result::Result<Option<String>, StoreError> {
            Ok(self.values.lock().unwrap().get(name).cloned())
        }
        fn set(&self, name: &str, value: &str) -> std::result::Result<(), StoreError> {
            self.values.lock().unwrap().insert(name.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, name: &str) -> std::result::Result<(), StoreError> {
            self.values.lock().unwrap().remove(name);
            Ok(())
        }
    }

    struct PanicStore;

    impl CredentialStore for PanicStore {
        fn get(&self, _: &str) -> std::result::Result<Option<String>, StoreError> {
            panic!("plaintext opt-in must not read the OS store");
        }
        fn set(&self, _: &str, _: &str) -> std::result::Result<(), StoreError> {
            panic!("plaintext opt-in must not write the OS store");
        }
        fn delete(&self, _: &str) -> std::result::Result<(), StoreError> {
            panic!("plaintext opt-in must not delete from the OS store");
        }
    }

    #[test]
    fn inherited_names_are_not_replaced_by_saved_values() {
        let saved = BTreeMap::from([
            ("GROK_API_KEY".into(), "saved-grok-value".into()),
            ("ANTHROPIC_API_KEY".into(), "saved-anthropic-value".into()),
        ]);
        let inherited = BTreeSet::from(["GROK_API_KEY".into()]);
        let apply = without_inherited(saved, &inherited);
        assert!(!apply.contains_key("GROK_API_KEY"));
        assert_eq!(
            apply.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("saved-anthropic-value")
        );
    }

    #[test]
    fn plaintext_opt_in_reads_the_file_and_does_not_touch_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        write_private_text_atomic(&path, b"export DEEPAGENT_API_KEY='dak-plaintext-opt-in-1234'\n").unwrap();
        let loaded = load_startup(&path, true, &PanicStore).unwrap();
        assert_eq!(
            loaded.values.get("DEEPAGENT_API_KEY").map(String::as_str),
            Some("dak-plaintext-opt-in-1234")
        );
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn startup_migration_loads_the_store_and_strips_the_verified_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        write_private_text_atomic(
            &path,
            b"# keep\nexport OTHER='x'\nexport DEEPAGENT_API_KEY='dak-migrate-value-1234'\n",
        )
        .unwrap();
        let store = MemoryStore {
            values: Mutex::new(BTreeMap::new()),
        };
        let loaded = load_startup(&path, false, &store).unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(
            loaded.values.get("DEEPAGENT_API_KEY").map(String::as_str),
            Some("dak-migrate-value-1234")
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep"));
        assert!(text.contains("export OTHER='x'"));
        assert!(!text.contains("dak-migrate-value-1234"));
    }

    struct DownStore;

    impl CredentialStore for DownStore {
        fn get(&self, _: &str) -> std::result::Result<Option<String>, StoreError> {
            Err(StoreError::Unavailable)
        }
        fn set(&self, _: &str, _: &str) -> std::result::Result<(), StoreError> {
            Err(StoreError::Unavailable)
        }
        fn delete(&self, _: &str) -> std::result::Result<(), StoreError> {
            Err(StoreError::Unavailable)
        }
    }

    #[test]
    fn os_save_refuses_when_the_store_is_unavailable_and_does_not_create_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let secret = "dak-must-not-land-1234";
        let updates = BTreeMap::from([("DEEPAGENT_API_KEY".into(), secret.into())]);
        let error = update_os_keys(&DownStore, &updates, &[]).unwrap_err();
        assert!(error.message.contains("allow_plaintext_key_file"));
        assert!(!error.message.contains(secret));
        assert!(!path.exists());
    }
}
