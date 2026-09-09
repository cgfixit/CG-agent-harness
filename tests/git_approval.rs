//! Actual local Git integrity boundary; no model or network service.
mod common;
use cgagentharness::agentic::{
    ctx::AgenticCtx,
    executor::manifest,
    real_repo_loop::{finalize_real_repo_change, FinalizeParams},
    workspace::RepoWorkspace,
};
use common::*;
use std::path::{Path, PathBuf};

fn fixture(home: &Path) -> (AgenticCtx, PathBuf) {
    let cfg = config_with(
        home,
        &[
            ("agentic.enabled", "true"),
            ("agentic.deepagent_github.enabled", "true"),
            ("agentic.deepagent_github.allow_git_write_tools", "true"),
        ],
    );
    let ctx = AgenticCtx::new(cfg, &home.join("config.yaml")).unwrap();
    let bare = real_bare_repo(home);
    let dest = ctx.acfg.deepagent.workspace_root.join("fixture/repo");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    git(&["clone", "-q", bare.to_str().unwrap(), dest.to_str().unwrap()], home);
    (ctx, dest)
}
fn approve(
    ctx: &AgenticCtx,
    ws: &RepoWorkspace<'_>,
    paths: &[String],
) -> cgagentharness::common::errors::Result<serde_json::Value> {
    let head = manifest::git_head(ws.worktree()).unwrap();
    let (_, digest) = manifest::build_manifest(ws.worktree(), paths, "fixture", &head).unwrap();
    finalize_real_repo_change(
        ctx,
        ws,
        &FinalizeParams {
            reason: "reviewed fixture",
            confirm: true,
            branch_name: "codex/exact-tree",
            commit_message: "test: exact tree",
            changed_files: paths,
            decision: "approve",
            protected_write_paths: &[],
            run_id: "fixture",
            acceptance_digest: Some(&digest),
            acceptance_base_head: Some(&head),
        },
    )
}

#[test]
fn unrelated_staged_changes_cannot_enter_an_approved_commit() {
    for kind in ["addition", "modification", "deletion", "intent-to-add"] {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, dest) = fixture(temp.path());
        std::fs::write(dest.join("existing.txt"), "base\n").unwrap();
        git(&["add", "existing.txt"], &dest);
        git(
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-qm",
                "Tracked control",
            ],
            &dest,
        );
        match kind {
            "modification" => {
                std::fs::write(dest.join("existing.txt"), "not approved\n").unwrap();
                git(&["add", "existing.txt"], &dest);
            }
            "deletion" => {
                git(&["rm", "existing.txt"], &dest);
            }
            _ => {
                std::fs::write(dest.join("unreviewed.txt"), "not approved\n").unwrap();
                let args = if kind == "intent-to-add" {
                    vec!["add", "-N", "unreviewed.txt"]
                } else {
                    vec!["add", "unreviewed.txt"]
                };
                git(&args, &dest);
            }
        }
        let index = std::fs::read(dest.join(".git/index")).unwrap();
        let head = git(&["rev-parse", "HEAD"], &dest);
        std::fs::write(dest.join("target.txt"), "approved\n").unwrap();
        let ws = RepoWorkspace::attach(&ctx, &dest).unwrap();
        assert!(approve(&ctx, &ws, &["target.txt".into()]).is_err(), "{kind}");
        assert_eq!(git(&["rev-parse", "HEAD"], &dest), head, "{kind}");
        assert_eq!(std::fs::read(dest.join(".git/index")).unwrap(), index, "{kind}");
    }
}

#[test]
fn approved_glob_filename_does_not_stage_its_neighbors() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, dest) = fixture(temp.path());
    std::fs::write(dest.join("literal*.txt"), "approved\n").unwrap();
    std::fs::write(dest.join("literal-extra.txt"), "not approved\n").unwrap();
    let ws = RepoWorkspace::attach(&ctx, &dest).unwrap();
    approve(&ctx, &ws, &["literal*.txt".into()]).unwrap();
    assert_eq!(
        git(&["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"], &dest),
        "literal*.txt"
    );
    assert_eq!(git(&["show", "HEAD:literal*.txt"], &dest), "approved");
}

#[cfg(unix)]
#[test]
fn hooks_and_filters_do_not_execute_during_inspection_or_approval() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let (ctx, dest) = fixture(temp.path());
    for hook in [
        "post-checkout",
        "prepare-commit-msg",
        "post-commit",
        "pre-push",
        "reference-transaction",
    ] {
        let path = dest.join(".git/hooks").join(hook);
        std::fs::write(
            &path,
            format!("#!/bin/sh\nprintf executed > '{}'\n", temp.path().join(hook).display()),
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(dest.join("target.txt"), "approved\n").unwrap();
    let ws = RepoWorkspace::attach(&ctx, &dest).unwrap();
    approve(&ctx, &ws, &["target.txt".into()]).unwrap();
    ws.push_branch("codex/exact-tree", "separate fixture push", true)
        .unwrap();
    for hook in [
        "post-checkout",
        "prepare-commit-msg",
        "post-commit",
        "pre-push",
        "reference-transaction",
    ] {
        assert!(!temp.path().join(hook).exists(), "hook {hook} executed");
    }
    let marker = temp.path().join("filter-marker");
    git(
        &[
            "config",
            "filter.fixture.clean",
            &format!("touch '{}'; sed s/approved/TRANSFORMED/", marker.display()),
        ],
        &dest,
    );
    std::fs::write(dest.join(".gitattributes"), "target.txt filter=fixture\n").unwrap();
    std::fs::write(dest.join("target.txt"), "approved next\n").unwrap();
    assert!(
        ws.diff(false).is_err(),
        "unsafe local configuration must refuse inspection"
    );
    assert!(ws.add(&["target.txt".into()], "reviewed fixture", true).is_err());
    assert!(!marker.exists(), "clean filter executed outside sandbox");
}

#[test]
fn branch_and_destination_drift_refuse_separate_push() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, dest) = fixture(temp.path());
    std::fs::write(dest.join("target.txt"), "approved\n").unwrap();
    let ws = RepoWorkspace::attach(&ctx, &dest).unwrap();
    let origin = ws.origin_url().unwrap();
    let approved = approve(&ctx, &ws, &["target.txt".into()]).unwrap();
    let commit = approved["approved_commit"].as_str().unwrap();
    let base = git(&["rev-parse", "HEAD^"], &dest);
    git(&["update-ref", "refs/heads/codex/exact-tree", &base], &dest);
    assert!(ws
        .push_approved("codex/exact-tree", commit, &origin, "separate push", true)
        .is_err());
    assert!(git(&["branch", "--list", "codex/*"], Path::new(&origin)).is_empty());
    git(&["update-ref", "refs/heads/codex/exact-tree", commit], &dest);
    git(&["config", "remote.origin.url", temp.path().to_str().unwrap()], &dest);
    assert!(ws
        .push_approved("codex/exact-tree", commit, &origin, "separate push", true)
        .is_err());
    git(&["config", "remote.origin.url", &origin], &dest);
    ws.push_approved("codex/exact-tree", commit, &origin, "separate push", true)
        .unwrap();
    ws.verify_published_source("codex/exact-tree", commit, &origin).unwrap();
    git(
        &["update-ref", "refs/heads/codex/exact-tree", &base],
        Path::new(&origin),
    );
    assert!(ws.verify_published_source("codex/exact-tree", commit, &origin).is_err());
}

#[test]
fn transformed_paths_and_existing_index_lock_refuse_without_committing() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, dest) = fixture(temp.path());
    std::fs::write(dest.join("target.txt"), "approved\n").unwrap();
    let ws = RepoWorkspace::attach(&ctx, &dest).unwrap();
    let base = git(&["rev-parse", "HEAD"], &dest);
    let index = std::fs::read(dest.join(".git/index")).unwrap();
    for attr in [
        "filter=lfs",
        "text",
        "eol=crlf",
        "working-tree-encoding=UTF-16",
        "ident",
    ] {
        std::fs::write(dest.join(".gitattributes"), format!("target.txt {attr}\n")).unwrap();
        assert!(approve(&ctx, &ws, &["target.txt".into()])
            .unwrap_err()
            .message
            .contains("transformations"));
        assert_eq!(git(&["rev-parse", "HEAD"], &dest), base);
        assert_eq!(std::fs::read(dest.join(".git/index")).unwrap(), index);
    }
    std::fs::remove_file(dest.join(".gitattributes")).unwrap();
    std::fs::write(dest.join(".git/index.lock"), "other operation").unwrap();
    assert!(approve(&ctx, &ws, &["target.txt".into()])
        .unwrap_err()
        .message
        .contains("locked"));
    assert_eq!(
        std::fs::read_to_string(dest.join(".git/index.lock")).unwrap(),
        "other operation"
    );
}

#[cfg(unix)]
#[test]
fn acceptance_binds_executable_mode_and_raw_blob_content() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let (ctx, dest) = fixture(temp.path());
    let source = dest.join("target.txt");
    std::fs::write(&source, "approved\n").unwrap();
    let base = manifest::git_head(&dest).unwrap();
    let paths = ["target.txt".into()];
    let (_, digest) = manifest::build_manifest(&dest, &paths, "fixture", &base).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(manifest::verify_manifest(&dest, &paths, "fixture", &base, &digest).is_err());
    let ws = RepoWorkspace::attach(&ctx, &dest).unwrap();
    approve(&ctx, &ws, &paths).unwrap();
    assert!(git(&["ls-tree", "HEAD", "target.txt"], &dest).starts_with("100755 blob "));
    assert_eq!(git(&["show", "HEAD:target.txt"], &dest), "approved");
    assert_eq!(git(&["status", "--porcelain"], &dest), "");
}

#[test]
fn replacement_refs_cannot_substitute_the_accepted_base_tree() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, dest) = fixture(temp.path());
    let base = git(&["rev-parse", "HEAD"], &dest);
    std::fs::write(dest.join("unreviewed.txt"), "attacker tree\n").unwrap();
    git(&["add", "unreviewed.txt"], &dest);
    git(
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "Replacement tree",
        ],
        &dest,
    );
    let replacement = git(&["rev-parse", "HEAD"], &dest);
    git(&["reset", "--hard", &base], &dest);
    git(&["replace", &base, &replacement], &dest);
    git(&["read-tree", &replacement], &dest);
    std::fs::write(dest.join("target.txt"), "approved\n").unwrap();
    let ws = RepoWorkspace::attach(&ctx, &dest).unwrap();
    let original_index = std::fs::read(dest.join(".git/index")).unwrap();
    assert!(approve(&ctx, &ws, &["target.txt".into()]).is_err());
    assert_eq!(git(&["rev-parse", "HEAD"], &dest), base);
    assert_eq!(std::fs::read(dest.join(".git/index")).unwrap(), original_index);
}
