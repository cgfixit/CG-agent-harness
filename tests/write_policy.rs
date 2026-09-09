//! Real local Git operations: no model, network remote, or sandbox skip.
mod common;
use cgagentharness::agentic::{
    ctx::AgenticCtx,
    run_store::{save_run, RealRepoRunRecord},
    workspace::RepoWorkspace,
};
use common::*;
use serde_json::json;
use std::path::{Path, PathBuf};

fn armed(home: &Path, extra: &[(&str, &str)]) -> AgenticCtx {
    let overrides = vec![
        ("agentic.enabled", "true"),
        ("agentic.mode", "write"),
        ("agentic.writes_enabled", "true"),
        ("agentic.deepagent_github.enabled", "true"),
        ("agentic.deepagent_github.allow_git_write_tools", "true"),
    ];
    let mut cfg = config_with(home, &overrides);
    for (field, value) in extra {
        let mut target = &mut cfg.raw;
        for part in field.split('.') {
            target = &mut target[part];
        }
        *target = serde_yaml_ng::from_str(value).unwrap();
    }
    std::fs::write(&cfg.path, serde_yaml_ng::to_string(&cfg.raw).unwrap()).unwrap();
    AgenticCtx::new(cfg, &home.join("config.yaml")).unwrap()
}

fn fixture(home: &Path) -> (AgenticCtx, PathBuf, PathBuf, RealRepoRunRecord) {
    let ctx = armed(home, &[]);
    let bare = real_bare_repo(home);
    let dest = ctx.acfg.deepagent.workspace_root.join("fixture/repo");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    git(&["clone", "-q", bare.to_str().unwrap(), dest.to_str().unwrap()], home);
    git(&["checkout", "-b", "codex/write-policy"], &dest);
    let mut rec = RealRepoRunRecord::new(&"a".repeat(32), &ctx.acfg.repo, dest.to_str().unwrap(), "approved");
    rec.branch_name = Some("codex/write-policy".into());
    rec.commit_message = Some("fixture".into());
    rec.approved_commit = Some(git(&["rev-parse", "HEAD"], &dest));
    rec.origin_url = Some(bare.display().to_string());
    save_run(&ctx.runs_dir(), &mut rec).unwrap();
    (ctx, dest, bare, rec)
}

#[test]
fn each_mutation_reloads_policy_and_refusal_preserves_files_index_head_and_remote() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, dest, bare, _) = fixture(dir.path());
    let ws = RepoWorkspace::attach(&ctx, &dest).unwrap();
    // Invalid argv is a safety tripwire: a broken gate still cannot invoke gh.
    let publication = json!({"op":"pr_create", "repo":ctx.acfg.repo, "reason":"r", "params":{"head":"codex/write-policy"}, "would_run":[]});
    let head = git(&["rev-parse", "HEAD"], &dest);
    let index = std::fs::read(dest.join(".git/index")).unwrap();
    for (field, value, gate) in [
        ("agentic.enabled", "false", "enabled"),
        ("agentic.mode", "read", "mode"),
        ("agentic.writes_enabled", "false", "writes_enabled"),
        ("agentic.deepagent_github.enabled", "false", "deepagent_enabled"),
        (
            "agentic.deepagent_github.allow_git_write_tools",
            "false",
            "allow_git_write_tools",
        ),
        (
            "agentic.deepagent_github.protected_write_paths",
            "[target.txt]",
            "policy_scope",
        ),
        ("agentic.deepagent_github.max_write_budget_bytes", "1", "policy_scope"),
    ] {
        armed(dir.path(), &[(field, value)]); // same already-open workspace, changed policy
        for result in [
            ws.write_file("target.txt", "denied", "r", true),
            ws.checkout_branch("codex/denied", "r", true),
            ws.add(&["target.txt".into()], "r", true),
            ws.commit("denied", "r", true),
            ws.push_branch("codex/write-policy", "r", true),
        ] {
            let error = result.unwrap_err();
            assert_eq!(error.code, "AGENTIC_WRITE_REFUSED");
            assert_eq!(error.details["failed_gate"], gate);
        }
        assert_eq!(
            cgagentharness::agentic::writer::execute_write(&ctx, &publication, true, 1)
                .unwrap_err()
                .details["failed_gate"],
            gate
        );
        assert_eq!(std::fs::read_to_string(dest.join("target.txt")).unwrap(), "hello\n");
        assert_eq!(git(&["rev-parse", "HEAD"], &dest), head);
        assert_eq!(std::fs::read(dest.join(".git/index")).unwrap(), index);
        assert!(git(&["branch", "--list", "codex/*"], &bare).is_empty());
        assert!(ws.diff(false).unwrap().is_empty());
        assert!(ws.untracked_files().unwrap().is_empty());
        assert_eq!(
            std::fs::read(dest.join(".git/index")).unwrap(),
            index,
            "inspection must not refresh index"
        );
    }
    armed(dir.path(), &[]);
    for (reason, confirm, gate) in [(" ", true, "reason"), ("r", false, "confirm")] {
        for result in [
            ws.write_file("target.txt", "denied", reason, confirm),
            ws.add(&["target.txt".into()], reason, confirm),
            ws.commit("denied", reason, confirm),
            ws.push_branch("codex/write-policy", reason, confirm),
        ] {
            assert_eq!(result.unwrap_err().details["failed_gate"], gate);
        }
    }
    std::fs::write(ctx.config_path.clone(), "agentic: [invalid]").unwrap();
    assert!(ws.write_file("target.txt", "denied", "r", true).is_err());
    std::fs::remove_file(&ctx.config_path).unwrap();
    assert!(ws.add(&["target.txt".into()], "r", true).is_err());
    armed(dir.path(), &[]);
    ws.write_file("target.txt", "approved\n", "reviewed fixture", true)
        .unwrap();
    ws.add(&["target.txt".into()], "reviewed fixture", true).unwrap();
    ws.commit("fixture change", "reviewed fixture", true).unwrap();
    assert_ne!(git(&["rev-parse", "HEAD"], &dest), head);
    assert!(
        git(&["branch", "--list", "codex/*"], &bare).is_empty(),
        "commit never pushes"
    );
    ws.push_branch("codex/write-policy", "separate push", true).unwrap();
    assert_eq!(
        git(&["rev-parse", "refs/heads/codex/write-policy"], &bare),
        git(&["rev-parse", "HEAD"], &dest)
    );
}

#[test]
fn approved_cli_push_and_publish_refuse_revoked_policy_without_side_effects() {
    for (extra, kill, confirm, reason) in [
        (
            vec![("agentic.mode", "read"), ("agentic.writes_enabled", "false")],
            "1",
            true,
            "r",
        ),
        (vec![], "1", true, "r"),
        (vec![("agentic.mode", "read")], "0", true, "r"),
        (vec![("agentic.writes_enabled", "false")], "0", true, "r"),
        (
            vec![("agentic.deepagent_github.allow_git_write_tools", "false")],
            "0",
            true,
            "r",
        ),
        (vec![], "0", false, "r"),
        (vec![], "0", true, " "),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, dest, bare, mut rec) = fixture(dir.path());
        let fakebin = install_fake_gh(dir.path(), &bare);
        let path = path_with(&fakebin);
        let log = dir.path().join("gh.log");
        armed(dir.path(), &extra);
        let env = [
            ("CGAGENTHARNESS_AGENTIC_WRITE_DISABLE", kill),
            ("PATH", path.as_str()),
            ("FAKE_GH_LOG", log.to_str().unwrap()),
            ("GROK_API_KEY", ""),
            ("ANTHROPIC_API_KEY", ""),
            ("DEEPAGENT_API_KEY", ""),
        ];
        for action in ["real-repo-run-push", "real-repo-run-publish"] {
            rec.pushed = action == "real-repo-run-publish";
            save_run(&ctx.runs_dir(), &mut rec).unwrap();
            let before = std::fs::read(ctx.runs_dir().join(format!("{}.json", rec.run_id))).unwrap();
            let idarg = format!("--run-id={}", rec.run_id);
            let reasonarg = format!("--reason={reason}");
            let mut args = vec![action, &idarg, &reasonarg];
            if confirm {
                args.push("--confirm");
            }
            let (code, out, err) = run_agentic(&ctx.config_path, &args, &env, dir.path());
            assert_eq!(code, 4, "{action}: {out} {err}");
            let after: serde_json::Value =
                serde_json::from_slice(&std::fs::read(ctx.runs_dir().join(format!("{}.json", rec.run_id))).unwrap())
                    .unwrap();
            let before: serde_json::Value = serde_json::from_slice(&before).unwrap();
            assert_eq!(after["pushed"], before["pushed"]);
            assert!(after["pr_url"].is_null());
            assert!(!log.exists(), "publication refusal must precede gh invocation");
            assert!(git(&["branch", "--list", "codex/*"], &bare).is_empty());
            assert_eq!(git(&["status", "--porcelain"], &dest), "");
        }
        // Read inspection and cleanup remain useful in read/disabled-write mode.
        let (code, _, err) = run_agentic(
            &ctx.config_path,
            &["real-repo-run-discard", &format!("--run-id={}", rec.run_id)],
            &env,
            dir.path(),
        );
        assert_eq!(code, 0, "{err}");
        assert!(!dest.exists());
    }
}

#[tokio::test]
async fn browser_api_requires_explicit_intent_and_shim_rechecks_disk_policy() {
    let model = start_mock_model().await;
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default()
            .with("agentic.enabled", "true")
            .with("agentic.deepagent_github.enabled", "true")
            .with("agentic.deepagent_github.allow_git_write_tools", "true"),
    )
    .await;
    let (ctx, _, bare, rec) = fixture(&s.home);
    let url = format!("/api/agent/runs/{}/push", rec.run_id);
    assert_eq!(s.post_json(&url, json!({})).await.0, 422);
    let (status, response) = s.post_json(&url, json!({"reason":"r"})).await;
    assert_eq!(status, 200);
    assert_eq!(response["exit_code"], 4, "{response}");
    armed(&s.home, &[("agentic.mode", "read")]);
    let (status, response) = s.post_json(&url, json!({"reason":"r", "confirm":true})).await;
    assert_eq!(status, 200);
    assert_eq!(response["exit_code"], 4, "{response}");
    assert!(git(&["branch", "--list", "codex/*"], &bare).is_empty());
    armed(&s.home, &[]);
    let (status, response) = s
        .post_json(&url, json!({"reason":"reviewed branch", "confirm":true}))
        .await;
    assert_eq!(status, 200);
    assert_eq!(response["exit_code"], 0, "{response}");
    assert!(!git(&["branch", "--list", "codex/*"], &bare).is_empty());
    assert_eq!(
        cgagentharness::agentic::run_store::load_run(&ctx.runs_dir(), &rec.run_id)
            .unwrap()
            .pr_url,
        None
    );
}

#[cfg(unix)]
#[test]
fn denied_write_policy_still_allows_diff_without_executable_git_extensions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (ctx, dest, _, _) = fixture(dir.path());
    let hook = dest.join("tripwire.sh");
    std::fs::write(&hook, "#!/bin/sh\necho invoked > tripwire-marker\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    git(&["config", "diff.external", hook.to_str().unwrap()], &dest);
    git(&["config", "core.fsmonitor", hook.to_str().unwrap()], &dest);
    std::fs::write(dest.join("target.txt"), "review me\n").unwrap();
    armed(dir.path(), &[("agentic.writes_enabled", "false")]);
    let ws = RepoWorkspace::attach(&ctx, &dest).unwrap();
    assert!(ws.diff(false).unwrap().contains("review me"));
    assert!(ws.untracked_files().unwrap().contains(&"tripwire.sh".into()));
    assert!(!dest.join("tripwire-marker").exists());
}
