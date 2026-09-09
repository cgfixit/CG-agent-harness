//! Immutable acceptance digest: `run_id + base HEAD + path->sha256 + mode`, snapshotted
//! at propose time and re-verified at approve. Port of `agentic/executor/manifest.py`.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use crate::common::errors::{HarnessError, Result};

pub fn git_head(worktree: &Path) -> Result<String> {
    let out = super::super::git::run(worktree, &["rev-parse", "HEAD"], Duration::from_secs(30), &[], None)?;
    let sha = out.stdout.trim().to_string();
    if out.status != Some(0) || sha.len() < 7 {
        return Err(
            HarnessError::agentic("could not read worktree HEAD for acceptance manifest")
                .detail("stderr", crate::common::clip_chars(&out.stderr, 200)),
        );
    }
    Ok(sha)
}

pub fn file_mode(path: &Path) -> Result<&'static str> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(HarnessError::write_refused("accepted files must remain ordinary files"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            return Ok("100755");
        }
    }
    Ok("100644")
}

fn jail(worktree: &Path, rel: &str) -> Result<std::path::PathBuf> {
    let root = dunce::canonicalize(worktree)?;
    let candidate = dunce::canonicalize(root.join(rel))
        .map_err(|_| HarnessError::agentic("manifest path missing from worktree").detail("path", rel))?;
    if !candidate.starts_with(&root) {
        return Err(HarnessError::agentic("manifest path escapes worktree").detail("path", rel));
    }
    Ok(candidate)
}

/// `(canonical payload, sha256 of its compact sorted JSON)`.
pub fn build_manifest(worktree: &Path, paths: &[String], run_id: &str, base_head: &str) -> Result<(Value, String)> {
    let mut sorted: Vec<String> = paths.iter().map(|p| p.replace('\\', "/")).collect();
    sorted.sort();
    sorted.dedup();
    let mut files = Vec::new();
    for rel in sorted {
        if rel.is_empty() || rel.starts_with('/') || rel.split('/').any(|s| s == "..") {
            return Err(HarnessError::agentic("refusing unsafe manifest path").detail("path", rel));
        }
        let target = jail(worktree, &rel)?;
        if !target.is_file() {
            return Err(HarnessError::agentic("manifest path missing from worktree").detail("path", rel));
        }
        let bytes = std::fs::read(&target)?;
        files
            .push(json!({"path": rel, "sha256": crate::common::sha256_bytes_hex(&bytes), "mode": file_mode(&target)?}));
    }
    // Canonical form: keys sorted, compact separators (matches Python's json.dumps(sort_keys=True, separators=(",",":"))).
    let payload = json!({"base_head": base_head, "files": files, "run_id": run_id});
    let canonical = canonical_json(&payload);
    Ok((payload, crate::common::sha256_hex(&canonical)))
}

/// Compact JSON with object keys sorted (serde_json::Value objects are BTreeMap-backed
/// without `preserve_order`, so serialization is already sorted; separators are compact).
pub fn canonical_json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// Rebuild the digest and refuse on any drift.
pub fn verify_manifest(
    worktree: &Path,
    paths: &[String],
    run_id: &str,
    base_head: &str,
    expected_digest: &str,
) -> Result<String> {
    if expected_digest.len() != 64 {
        return Err(
            HarnessError::agentic("run record is missing a valid acceptance_digest; refusing approve")
                .detail("run_id", run_id),
        );
    }
    let live = git_head(worktree)?;
    if live != base_head {
        return Err(
            HarnessError::agentic("worktree HEAD drifted from the accepted base; refusing approve")
                .detail("run_id", run_id),
        );
    }
    let (_, digest) = build_manifest(worktree, paths, run_id, base_head)?;
    if digest != expected_digest {
        return Err(
            HarnessError::agentic("acceptance manifest digest mismatch; refusing approve").detail("run_id", run_id),
        );
    }
    Ok(digest)
}
