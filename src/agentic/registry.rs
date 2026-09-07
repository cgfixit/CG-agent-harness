//! Governed skills registry, port of `agentic/registry.py`: propose never
//! writes; apply enforces the injection gate, a human reason, an atomic write,
//! a cross-process lock directory with PID-aware stale reclaim, and history.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::common::atomic::write_json_atomic;
use crate::common::errors::{HarnessError, Result};
use crate::common::process::pid_alive;

use super::ctx::AgenticCtx;

const LOCK_STALE_SEC: f64 = 60.0;

fn name_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\A[A-Za-z0-9][A-Za-z0-9_.-]*\z").expect("static regex"))
}

fn rerr(message: impl Into<String>) -> HarnessError {
    HarnessError::registry(message)
}

#[derive(Debug, Clone)]
pub struct SkillSpec {
    pub name: String,
    pub description: String,
    pub body: String,
}

impl SkillSpec {
    fn canonical(&self) -> String {
        format!("{}\n{}\n{}", self.name, self.description, self.body)
    }

    fn validate(&self) -> Result<()> {
        for (k, v) in [
            ("name", &self.name),
            ("description", &self.description),
            ("body", &self.body),
        ] {
            if v.trim().is_empty() {
                return Err(rerr(format!("skill spec field '{k}' must be a non-empty string")).detail("field", k));
            }
        }
        if !name_re().is_match(&self.name) {
            return Err(
                rerr("skill name must match ^[A-Za-z0-9][A-Za-z0-9_.-]*$ (must start with a letter or digit)")
                    .detail("name", self.name.clone()),
            );
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- lock dir

fn lock_token(lock_dir: &Path) -> PathBuf {
    lock_dir.join("owner.json")
}

fn write_lock_token(lock_dir: &Path) {
    let _ = std::fs::write(
        lock_token(lock_dir),
        json!({"pid": std::process::id(), "started_at": crate::common::now_ts()}).to_string(),
    );
}

fn read_lock_token(lock_dir: &Path) -> Option<Value> {
    std::fs::read_to_string(lock_token(lock_dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

fn can_reclaim(lock_dir: &Path, age: f64) -> bool {
    if age <= LOCK_STALE_SEC {
        return false;
    }
    match read_lock_token(lock_dir) {
        None => true,
        Some(tok) => match tok.get("pid").and_then(|p| p.as_u64()) {
            Some(pid) => !pid_alive(pid as u32),
            None => false,
        },
    }
}

fn is_lock_owner(lock_dir: &Path) -> bool {
    match read_lock_token(lock_dir) {
        None => true,
        Some(tok) => tok.get("pid").and_then(|p| p.as_u64()) == Some(std::process::id() as u64),
    }
}

fn reclaim_lock(lock_dir: &Path) -> Result<bool> {
    let guard = lock_dir.with_file_name(format!(
        "{}.reclaim.d",
        lock_dir.file_name().unwrap_or_default().to_string_lossy()
    ));
    match std::fs::create_dir(&guard) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => {
            return Err(
                rerr("could not create skills-registry reclaim guard").detail("errno", e.raw_os_error().unwrap_or(0))
            )
        }
    }
    let result = (|| {
        let present = match std::fs::metadata(lock_dir) {
            Ok(m) => {
                let mtime = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                let age = crate::common::now_ts() - mtime;
                if !can_reclaim(lock_dir, age) {
                    return false;
                }
                let _ = std::fs::remove_dir_all(lock_dir);
                true
            }
            Err(_) => false,
        };
        let _ = present;
        match std::fs::create_dir(lock_dir) {
            Ok(()) => {
                write_lock_token(lock_dir);
                true
            }
            Err(_) => false,
        }
    })();
    let _ = std::fs::remove_dir(&guard);
    Ok(result)
}

pub fn acquire_registry_lock(lock_dir: &Path) -> Result<()> {
    match std::fs::create_dir(lock_dir) {
        Ok(()) => {
            write_lock_token(lock_dir);
            return Ok(());
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(rerr(format!("could not create skills-registry lock: {e}"))),
    }
    if reclaim_lock(lock_dir)? {
        return Ok(());
    }
    Err(rerr("another skills-registry apply is in progress")
        .detail("lock_dir", lock_dir.display().to_string())
        .detail("hint", "Retry shortly, or remove the lock and adjacent reclaim guard after confirming no registry apply is running."))
}

pub fn release_registry_lock(lock_dir: &Path) {
    if !is_lock_owner(lock_dir) {
        return;
    }
    let _ = std::fs::remove_file(lock_token(lock_dir));
    let _ = std::fs::remove_dir(lock_dir);
}

// ---------------------------------------------------------------- registry

pub struct SkillRegistry<'a> {
    ctx: &'a AgenticCtx,
    path: PathBuf,
    data: Value,
}

fn empty() -> Value {
    json!({"version": 0, "updated": null, "skills": {}, "history": []})
}

fn unified_diff(old: &str, new: &str, name: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a == b {
        return String::new();
    }
    let mut prefix = 0;
    while prefix < a.len() && prefix < b.len() && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < a.len() - prefix && suffix < b.len() - prefix && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix] {
        suffix += 1;
    }
    let mut out = format!("--- {name} (current)\n+++ {name} (proposed)\n");
    for line in &a[prefix..a.len() - suffix] {
        out.push_str(&format!("-{line}\n"));
    }
    for line in &b[prefix..b.len() - suffix] {
        out.push_str(&format!("+{line}\n"));
    }
    out
}

impl<'a> SkillRegistry<'a> {
    pub fn open(ctx: &'a AgenticCtx) -> Result<Self> {
        if ctx.scanner.is_empty() {
            return Err(rerr("injection pattern set is empty; refusing to operate with a defeated skill-injection gate (fail-closed)"));
        }
        let path = ctx.acfg.registry_path.clone();
        let data = Self::load(&path)?;
        Ok(Self { ctx, path, data })
    }

    fn load(path: &Path) -> Result<Value> {
        if !path.exists() {
            return Ok(empty());
        }
        let text = std::fs::read_to_string(path).map_err(|e| rerr(format!("Could not read skills registry: {e}")))?;
        let data: Value =
            serde_json::from_str(&text).map_err(|e| rerr(format!("Could not read skills registry: {e}")))?;
        if !data.is_object() || data.get("skills").is_none() {
            return Err(
                rerr("Skills registry is malformed (missing 'skills')").detail("path", path.display().to_string())
            );
        }
        Ok(data)
    }

    pub fn version(&self) -> i64 {
        self.data.get("version").and_then(|v| v.as_i64()).unwrap_or(0)
    }

    pub fn list_skills(&self) -> Vec<String> {
        self.data
            .get("skills")
            .and_then(|s| s.as_object())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn get_skill(&self, name: &str) -> Option<Value> {
        self.data.get("skills").and_then(|s| s.get(name)).cloned()
    }

    fn scan(&self, text: &str) -> Vec<String> {
        self.ctx.scanner.scan(text)
    }

    fn score_spec(&self, spec: &SkillSpec) -> i64 {
        let flags = self.scan(&spec.canonical());
        let penalty = ((flags.len() as i64) * 25).min(80);
        let mut score = 100 - penalty;
        if !flags.is_empty() {
            return score.clamp(0, 20);
        }
        if spec.description.len() > 30 {
            score += 8;
        }
        if spec.body.len() > 100 {
            score += 5;
        }
        score.clamp(0, 100)
    }

    /// Preview a skill add/update. NEVER writes; flags are advisory.
    pub fn propose_skill(&self, spec: &SkillSpec, reason: &str) -> Result<Value> {
        spec.validate()?;
        let canonical = spec.canonical();
        let flags = self.scan(&canonical);
        let existing = self.get_skill(&spec.name);
        let old_body = existing
            .as_ref()
            .and_then(|e| e.get("body"))
            .and_then(|b| b.as_str())
            .unwrap_or("");
        Ok(json!({
            "status": "proposed",
            "name": spec.name,
            "diff": unified_diff(old_body, &spec.body, &spec.name),
            "injection_flags": flags,
            "injection_flag_count": flags.len(),
            "safe_to_apply": flags.is_empty(),
            "reason": reason,
            "proposed_sha": crate::common::sha256_hex(&canonical),
            "is_update": existing.is_some(),
            "governance_score": self.score_spec(spec),
        }))
    }

    /// Atomically add/update a skill, enforcing every gate.
    pub fn apply_skill(&mut self, spec: &SkillSpec, reason: &str) -> Result<Value> {
        spec.validate()?;
        if reason.trim().is_empty() {
            return Err(rerr("apply_skill requires a non-empty human reason").detail("name", spec.name.clone()));
        }
        if !self.ctx.acfg.enabled {
            return Err(rerr("apply_skill requires agentic.enabled=true")
                .detail("name", spec.name.clone())
                .detail("failed_gate", "enabled"));
        }
        if !(self.ctx.acfg.is_write_mode() && self.ctx.acfg.writes_enabled) {
            return Err(
                rerr("apply_skill requires agentic.mode='write' and agentic.writes_enabled=true")
                    .detail("name", spec.name.clone())
                    .detail("mode", self.ctx.acfg.mode.clone())
                    .detail("writes_enabled", self.ctx.acfg.writes_enabled),
            );
        }
        let canonical = spec.canonical();
        let flags = self.scan(&canonical);
        if !flags.is_empty() {
            self.ctx.audit.log(json!({"event": "agentic_skill_injection_blocked", "name": spec.name, "reason": reason, "injection_flag_count": flags.len()}));
            return Err(
                HarnessError::injection("Proposed skill contains injection patterns; refusing to apply")
                    .detail("injection_flags", json!(flags))
                    .detail("name", spec.name.clone()),
            );
        }
        let new_sha = crate::common::sha256_hex(&canonical);
        let ts = crate::common::iso_now();
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock_dir = PathBuf::from(format!("{}.lock.d", self.path.display()));
        acquire_registry_lock(&lock_dir)?;
        let outcome = (|| {
            // Rebase on the LATEST committed state, not the in-memory snapshot.
            let mut data = Self::load(&self.path)?;
            let new_version = data.get("version").and_then(|v| v.as_i64()).unwrap_or(0) + 1;
            data["skills"][&spec.name] = json!({
                "name": spec.name, "description": spec.description, "body": spec.body,
                "sha256": new_sha, "reason": reason, "updated": ts,
            });
            let mut history = data
                .get("history")
                .and_then(|h| h.as_array())
                .cloned()
                .unwrap_or_default();
            history.push(json!({"version": new_version, "name": spec.name, "sha256": new_sha, "reason": reason, "timestamp": ts}));
            data["history"] = json!(history);
            data["version"] = json!(new_version);
            data["updated"] = json!(ts);
            write_json_atomic(&self.path, &data)
                .map_err(|e| rerr(format!("Failed to write skills registry: {}", e.message)))?;
            self.data = data;
            Ok::<i64, HarnessError>(new_version)
        })();
        release_registry_lock(&lock_dir);
        let new_version = outcome?;
        self.ctx.audit.log(json!({"event": "agentic_skill_applied", "name": spec.name, "reason": reason, "version": new_version, "sha256": new_sha}));
        Ok(
            json!({"status": "applied", "name": spec.name, "version": new_version, "sha256": new_sha, "governance_score": self.score_spec(spec)}),
        )
    }
}
