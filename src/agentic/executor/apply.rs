//! Disposable-copy proof before finalize: copy the candidate tree to a
//! throwaway directory and re-verify its acceptance digest through the same
//! isolated Git boundary as the live clone, then destroy the copy.

use std::path::Path;

use crate::common::errors::{HarnessError, Result};

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
    let live = super::manifest::git_head(&dest)?;
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
