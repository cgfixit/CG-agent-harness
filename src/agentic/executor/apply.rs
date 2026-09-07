//! Disposable-copy proof before finalize: copy the candidate tree to a
//! throwaway directory, re-verify the acceptance digest there with user/system
//! git config disabled and `core.hooksPath` pinned to an empty dir (passed as
//! env to the git CHILD, never set on this process), then destroy the copy.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use crate::common::errors::{HarnessError, Result};
use crate::common::process::{self, RunSpec};

use super::manifest::build_manifest;

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(src).map_err(std::io::Error::other)?;
        let target = dst.join(rel);
        let ft = entry.file_type();
        if ft.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if ft.is_symlink() {
            let link = std::fs::read_link(entry.path())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&link, &target)?;
            #[cfg(windows)]
            {
                let _ = link;
                // Symlinks need elevation on Windows; copy the content if resolvable, else skip.
                if let Ok(bytes) = std::fs::read(entry.path()) {
                    std::fs::write(&target, bytes)?;
                }
            }
        } else if ft.is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn git_head_scrubbed(worktree: &Path, hooks_path: &Path) -> Result<String> {
    let git = process::which("git").ok_or_else(|| HarnessError::agentic("git executable not found on PATH"))?;
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    env.insert("GIT_CONFIG_GLOBAL".into(), null.into());
    env.insert("GIT_CONFIG_SYSTEM".into(), null.into());
    env.insert("GIT_CONFIG_COUNT".into(), "1".into());
    env.insert("GIT_CONFIG_KEY_0".into(), "core.hooksPath".into());
    env.insert("GIT_CONFIG_VALUE_0".into(), hooks_path.display().to_string());
    let argv = vec![git.display().to_string(), "rev-parse".into(), "HEAD".into()];
    let out = process::run(RunSpec {
        argv: &argv,
        cwd: Some(worktree),
        env: Some(&env),
        timeout: Duration::from_secs(30),
        stdin: None,
    })
    .map_err(|e| HarnessError::agentic(format!("could not read disposable HEAD: {e}")))?;
    let sha = out.stdout.trim().to_string();
    if out.status != Some(0) || sha.len() < 7 {
        return Err(HarnessError::agentic("disposable-copy proof: could not read HEAD"));
    }
    Ok(sha)
}

/// Copy, re-verify the digest in the copy, destroy. Never commits.
pub fn prove_disposable_copy(
    worktree: &Path,
    paths: &[String],
    run_id: &str,
    base_head: &str,
    expected_digest: &str,
) -> Result<String> {
    if !worktree.is_dir() {
        return Err(
            HarnessError::agentic("disposable-copy proof: worktree is not a directory")
                .detail("worktree", worktree.display().to_string()),
        );
    }
    let tmp = tempfile::Builder::new().prefix("cgah-disposable-apply-").tempdir()?;
    let dest = tmp.path().join("tree");
    let hooks = tmp.path().join("empty-hooks");
    std::fs::create_dir_all(&hooks)?;
    copy_tree(worktree, &dest).map_err(|e| {
        HarnessError::agentic("disposable-copy proof: failed to copy candidate tree")
            .detail("error", crate::common::clip_chars(&e.to_string(), 200))
    })?;
    if expected_digest.len() != 64 {
        return Err(
            HarnessError::agentic("run record is missing a valid acceptance_digest; refusing approve")
                .detail("run_id", run_id),
        );
    }
    let live = git_head_scrubbed(&dest, &hooks)?;
    if live != base_head {
        return Err(
            HarnessError::agentic("worktree HEAD drifted from the accepted base; refusing approve")
                .detail("run_id", run_id),
        );
    }
    let (_, digest) = build_manifest(&dest, paths, run_id, base_head)?;
    if digest != expected_digest {
        return Err(
            HarnessError::agentic("acceptance manifest digest mismatch; refusing approve").detail("run_id", run_id),
        );
    }
    Ok(digest)
}
