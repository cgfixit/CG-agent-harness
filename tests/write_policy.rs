//! Real local Git operations: no model, network remote, or sandbox skip.
mod common;
use cgagentharness::agentic::{
    ctx::AgenticCtx,
    edits::Proposal,
    executor::manifest,
    real_repo_loop::FinalizeParams,
    run_store::{save_run, RealRepoRunRecord},
    workspace::RepoWorkspace,
};
use common::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The production approve call for `target.txt`. The gate runs before any of
/// these values are read, so a refusal names the gate, not a bad parameter.
fn finalize<'a>(reason: &'a str, confirm: bool, changed: &'a [String]) -> FinalizeParams<'a> {
    FinalizeParams {
        reason,
        confirm,
        branch_name: "codex/write-policy",
        commit_message: "denied",
        changed_files: changed,
        decision: "approve",
        protected_write_paths: &[],
        run_id: "fixture",
        acceptance_digest: None,
        acceptance_base_head: None,
    }
}

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
    // The production write path: apply (loop), approve (decide) and push.
    let origin = ws.origin_url().unwrap();
    let changed = vec!["target.txt".to_string()];
    let proposal = Proposal {
        files: BTreeMap::from([("target.txt".to_string(), "denied".to_string())]),
        originals: BTreeMap::from([("target.txt".to_string(), Some("hello\n".to_string()))]),
    };
    let production = |reason: &str, confirm: bool| {
        [
            (
                "apply_proposal",
                ws.apply_proposal(&proposal, &[], reason, confirm).err(),
            ),
            (
                "commit_accepted",
                ws.commit_accepted(&finalize(reason, confirm, &changed)).err(),
            ),
            (
                "push_approved",
                ws.push_approved("codex/write-policy", &head, &origin, reason, confirm)
                    .err(),
            ),
        ]
    };
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
        ("agentic.deepagent_github.scan_code_shape", "false", "policy_scope"),
        // A retargeted repository or workspace root must not reuse this clone.
        ("agentic.repo", "fixture/other", "policy_target"),
        (
            "agentic.deepagent_github.workspace_root",
            "data/agentic/other",
            "policy_target",
        ),
    ] {
        armed(dir.path(), &[(field, value)]); // same already-open workspace, changed policy
        for (op, error) in production("r", true) {
            let error = error.unwrap_or_else(|| panic!("{op} ran under revoked {gate}"));
            assert_eq!(error.code, "AGENTIC_WRITE_REFUSED", "{op}");
            assert_eq!(error.details["failed_gate"], gate, "{op}");
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
        for (op, error) in production(reason, confirm) {
            let error = error.unwrap_or_else(|| panic!("{op} ran without {gate}"));
            assert_eq!(error.details["failed_gate"], gate, "{op}");
        }
    }
    std::fs::write(ctx.config_path.clone(), "agentic: [invalid]").unwrap();
    // The refusal must come from the config reload itself (exit 3), not from a
    // later check such as commit_accepted's missing acceptance digest.
    for (op, error) in production("r", true) {
        let error = error.unwrap_or_else(|| panic!("{op} ran with an invalid config"));
        assert_eq!(error.code, "AGENTIC_CONFIG_INVALID", "{op}: {}", error.message);
    }
    std::fs::remove_file(&ctx.config_path).unwrap();
    for (op, error) in production("r", true) {
        let error = error.unwrap_or_else(|| panic!("{op} ran with no config"));
        assert_eq!(error.code, "CONFIG_ERROR", "{op}: {}", error.message);
    }
    assert_eq!(std::fs::read_to_string(dest.join("target.txt")).unwrap(), "hello\n");
    assert_eq!(git(&["rev-parse", "HEAD"], &dest), head);
    assert!(git(&["branch", "--list", "codex/*"], &bare).is_empty());
    armed(dir.path(), &[]);
    // Past the gate, production approval and push still refuse bad names.
    for (branch, message) in [("main2", "fixture"), ("codex/write-policy", "   ")] {
        let params = FinalizeParams {
            branch_name: branch,
            commit_message: message,
            ..finalize("reviewed fixture", true, &changed)
        };
        let error = ws.commit_accepted(&params).unwrap_err();
        assert!(
            error.message.contains("invalid branch or empty commit message"),
            "{error:?}"
        );
    }
    let error = ws
        .push_approved("main", &head, &origin, "reviewed fixture", true)
        .unwrap_err();
    assert!(error.message.contains("valid approved commit"), "{error:?}");
    // Control: with the policy armed, the same production calls commit and push.
    std::fs::write(dest.join("target.txt"), "approved\n").unwrap();
    let (_, digest) = manifest::build_manifest(&dest, &changed, "fixture", &head).unwrap();
    let commit = ws
        .commit_accepted(&FinalizeParams {
            branch_name: "codex/policy-control",
            commit_message: "fixture change",
            acceptance_digest: Some(&digest),
            acceptance_base_head: Some(&head),
            ..finalize("reviewed fixture", true, &changed)
        })
        .unwrap();
    assert_ne!(commit, head);
    assert!(
        git(&["branch", "--list", "codex/*"], &bare).is_empty(),
        "commit never pushes"
    );
    ws.push_approved("codex/policy-control", &commit, &origin, "separate push", true)
        .unwrap();
    assert_eq!(git(&["rev-parse", "refs/heads/codex/policy-control"], &bare), commit);
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

#[test]
fn decide_refuses_combined_push_or_publish_before_any_gate_or_record_read() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, dest, bare, rec) = fixture(dir.path());
    let fakebin = install_fake_gh(dir.path(), &bare);
    let path = path_with(&fakebin);
    let log = dir.path().join("gh.log");
    let record = ctx.runs_dir().join(format!("{}.json", rec.run_id));
    let before = std::fs::read(&record).unwrap();
    let head = git(&["rev-parse", "HEAD"], &dest);
    let env = [
        ("PATH", path.as_str()),
        ("GROK_API_KEY", ""),
        ("ANTHROPIC_API_KEY", ""),
        ("DEEPAGENT_API_KEY", ""),
    ];
    let idarg = format!("--run-id={}", rec.run_id);
    // Armed first, then with the master gate closed: the refusal must precede
    // every gate, or a disabled layer would answer with its exit-0 banner.
    for (policy, extra) in [
        ("armed", vec![]),
        ("master gate closed", vec![("agentic.enabled", "false")]),
    ] {
        armed(dir.path(), &extra);
        for flag in ["--push", "--publish"] {
            let args = [
                "real-repo-run-decide",
                &idarg,
                "--decision=approve",
                "--reason=reviewed",
                "--confirm",
                flag,
            ];
            let (code, out, err) = run_agentic(&ctx.config_path, &args, &env, dir.path());
            assert_eq!(code, 4, "{policy} {flag}: {out} {err}");
            assert!(
                format!("{out}{err}").contains("approve only commits locally"),
                "{policy} {flag}: {out} {err}"
            );
            assert_eq!(
                std::fs::read(&record).unwrap(),
                before,
                "{policy} {flag}: record changed"
            );
            assert!(!log.exists(), "{policy} {flag}: refusal must precede gh invocation");
            assert_eq!(git(&["rev-parse", "HEAD"], &dest), head);
            assert!(git(&["branch", "--list", "codex/*"], &bare).is_empty());
        }
    }
}

#[test]
fn reject_settles_a_pending_run_whose_clone_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, dest, _, mut rec) = fixture(dir.path());
    rec.status = "pending_decision".into();
    save_run(&ctx.runs_dir(), &mut rec).unwrap();
    std::fs::remove_dir_all(&dest).unwrap();
    let env = [
        ("GROK_API_KEY", ""),
        ("ANTHROPIC_API_KEY", ""),
        ("DEEPAGENT_API_KEY", ""),
    ];
    let idarg = format!("--run-id={}", rec.run_id);
    let record = ctx.runs_dir().join(format!("{}.json", rec.run_id));
    let status = || -> String {
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
        v["status"].as_str().unwrap().to_string()
    };
    // Approve still needs the clone it would commit in.
    let args = [
        "real-repo-run-decide",
        &idarg,
        "--decision=approve",
        "--reason=r",
        "--confirm",
    ];
    let (code, out, err) = run_agentic(&ctx.config_path, &args, &env, dir.path());
    assert_ne!(code, 0, "{out} {err}");
    assert_eq!(status(), "pending_decision");
    // Reject only records the decision, so a missing clone cannot strand the run.
    let args = [
        "real-repo-run-decide",
        &idarg,
        "--decision=reject",
        "--reason=clone gone",
    ];
    let (code, out, err) = run_agentic(&ctx.config_path, &args, &env, dir.path());
    assert_eq!(code, 0, "{out} {err}");
    assert_eq!(status(), "rejected");
    let (code, _, err) = run_agentic(&ctx.config_path, &["real-repo-run-discard", &idarg], &env, dir.path());
    assert_eq!(code, 0, "{err}");
    assert_eq!(status(), "discarded");
}

#[cfg(unix)]
#[test]
fn reject_keeps_a_pending_run_whose_clone_cannot_be_inspected() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, dest, _, mut rec) = fixture(dir.path());
    // Point the record under a regular file: lookup fails with ENOTDIR, not NotFound.
    let blocker = dest.parent().unwrap().join("blocker");
    std::fs::write(&blocker, "").unwrap();
    rec.dest = blocker.join("repo").display().to_string();
    rec.status = "pending_decision".into();
    save_run(&ctx.runs_dir(), &mut rec).unwrap();
    let env = [
        ("GROK_API_KEY", ""),
        ("ANTHROPIC_API_KEY", ""),
        ("DEEPAGENT_API_KEY", ""),
    ];
    let idarg = format!("--run-id={}", rec.run_id);
    let args = ["real-repo-run-decide", &idarg, "--decision=reject", "--reason=r"];
    let (code, out, err) = run_agentic(&ctx.config_path, &args, &env, dir.path());
    assert_ne!(code, 0, "{out} {err}");
    let record = ctx.runs_dir().join(format!("{}.json", rec.run_id));
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(record).unwrap()).unwrap();
    assert_eq!(v["status"], "pending_decision");
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
    // Approve is the third mutation boundary: the browser body never supplies
    // its confirmation either, and the child refuses before touching the clone.
    let mut pending = RealRepoRunRecord::new(&"b".repeat(32), &ctx.acfg.repo, &rec.dest, "pending_decision");
    pending.branch_name = Some("codex/write-policy".into());
    pending.commit_message = Some("fixture".into());
    pending.changed_files = vec!["target.txt".into()];
    save_run(&ctx.runs_dir(), &mut pending).unwrap();
    let clone = Path::new(&rec.dest);
    let head = git(&["rev-parse", "HEAD"], clone);
    let (status, response) = s
        .post_json(
            &format!("/api/agent/runs/{}/decision", pending.run_id),
            json!({"decision":"approve", "reason":"reviewed"}),
        )
        .await;
    assert_eq!(status, 200, "{response}");
    assert_eq!(response["exit_code"], 4, "{response}");
    assert_eq!(
        git(&["rev-parse", "HEAD"], clone),
        head,
        "approve without confirm committed"
    );
    assert_eq!(
        cgagentharness::agentic::run_store::load_run(&ctx.runs_dir(), &pending.run_id)
            .unwrap()
            .status,
        "pending_decision"
    );
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

#[cfg(unix)]
#[test]
fn record_mutations_wait_for_the_run_owner_lease() {
    use cgagentharness::agentic::run_store::acquire_run_lease;
    let dir = tempfile::tempdir().unwrap();
    let (ctx, dest, bare, rec) = fixture(dir.path());
    let record = ctx.runs_dir().join(format!("{}.json", rec.run_id));
    let before = std::fs::read(&record).unwrap();
    let env = [
        ("GROK_API_KEY", ""),
        ("ANTHROPIC_API_KEY", ""),
        ("DEEPAGENT_API_KEY", ""),
    ];
    let idarg = format!("--run-id={}", rec.run_id);
    // Another operation on this run (a push still in flight) owns the lease.
    let lease = acquire_run_lease(&ctx.runs_dir(), &rec.run_id, true).unwrap();
    for args in [
        vec!["real-repo-run-discard", &idarg],
        vec!["real-repo-run-push", &idarg, "--reason=reviewed", "--confirm"],
    ] {
        let (code, out, err) = run_agentic(&ctx.config_path, &args, &env, dir.path());
        assert_eq!(code, 2, "{}: {out} {err}", args[0]);
        assert_eq!(std::fs::read(&record).unwrap(), before, "{}: record changed", args[0]);
        assert!(dest.is_dir(), "{}: clone removed", args[0]);
        assert!(git(&["branch", "--list", "codex/*"], &bare).is_empty());
    }
    drop(lease);
    let (code, _, err) = run_agentic(&ctx.config_path, &["real-repo-run-discard", &idarg], &env, dir.path());
    assert_eq!(code, 0, "{err}");
    assert!(!dest.exists());
}
