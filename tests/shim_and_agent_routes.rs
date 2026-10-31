//! Ports of test_ops_runner.py (agentic half), test_harness_agent_routes.py,
//! and the shim timeout contract. The child is the REAL binary.

mod common;

#[tokio::test]
async fn server_advertises_default_checks_and_honest_job_capabilities() {
    let model = common::start_mock_model().await;
    let server = common::spawn_server(&model.base_url(), common::ServerOptions::default()).await;
    let (status, value) = server.get_json("/api/agent/checks").await;
    assert_eq!(status, 200);
    assert_eq!(value["default_profile"], "cargo-test");
    assert_eq!(value["capabilities"]["jobs"], true);
    assert_eq!(value["capabilities"]["streaming"], false);
    assert_eq!(value["capabilities"]["descendant_stop_guaranteed"], false);
    assert_eq!(value["capabilities"]["job_recovery"], "durable_interrupted");
    assert!(value["poll_interval_ms"].as_u64().unwrap() >= 1000);
}

use std::time::Duration;

use cgagentharness::shim::{self, OpsRequest, ShimContext, ShimError};
use common::*;
use serde_json::json;

fn ctx(dir: &std::path::Path) -> ShimContext {
    let mut c = ShimContext::new(&dir.join("config.yaml"), dir, &dir.join("tmp"), 720).unwrap();
    c.exe = std::path::PathBuf::from(BIN);
    c
}

const HEX32: &str = "0123456789abcdef0123456789abcdef";

// ---------------------------------------------------------------- argv builder

#[test]
fn argv_builder_uses_single_element_form_for_dash_leading_values() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let mut req = OpsRequest::new("real-repo-run");
    req.instruction = Some("--confirm".into());
    req.checks = Some(vec![json!({"name": "cargo-test", "argv": ["cargo", "test"]})]);
    req.branch = Some("claude/x".into());
    req.commit_message = Some("-m evil".into());
    req.reason = Some("-r".into());
    req.plan = Some("plan text".into());
    req.read_files = Some(vec!["src/a.rs".into(), "README.md".into()]);
    req.max_iterations = Some(2);
    req.pr = Some(7);
    let (argv, temps) = shim::build_argv(&c, &req).unwrap();
    assert_eq!(argv[0], BIN);
    assert_eq!(
        &argv[1..5],
        &["agentic", "--config", c.config_path.to_str().unwrap(), "real-repo-run"]
    );
    assert!(argv.contains(&"--instruction=--confirm".to_string()));
    assert!(argv.contains(&"--commit-message=-m evil".to_string()));
    assert!(argv.contains(&"--reason=-r".to_string()));
    assert!(argv.contains(&"--branch=claude/x".to_string()));
    assert!(argv.contains(&"--read-file=src/a.rs".to_string()));
    assert!(argv.windows(2).any(|w| w[0] == "--pr" && w[1] == "7"));
    assert!(argv.windows(2).any(|w| w[0] == "--max-iterations" && w[1] == "2"));
    assert!(
        !argv.contains(&"--confirm".to_string()),
        "confirm was not set, so no flag"
    );
    assert!(
        !argv.iter().any(|a| a.starts_with("--planner-timeout-sec")),
        "untuned runs keep the configured planner timeout"
    );
    assert_eq!(
        shim::timeout_for(&c, &req).as_secs(),
        shim::real_repo_run_timeout_sec(720, Some(2), 1)
    );
    // A speed-derived planner timeout budgets the child and crosses as one element.
    req.planner_timeout_sec = Some(150);
    req.planner_identity = Some("--confirm".into());
    let (tuned, _) = shim::build_argv(&c, &req).unwrap();
    assert!(tuned.contains(&"--planner-timeout-sec=150".to_string()), "{tuned:?}");
    assert!(tuned.contains(&"--planner-identity=--confirm".to_string()), "{tuned:?}");
    assert!(!tuned.contains(&"--confirm".to_string()));
    assert_eq!(
        shim::timeout_for(&c, &req).as_secs(),
        shim::real_repo_run_timeout_sec(150, Some(2), 1)
    );
    // Push and publish outlast the child's own bounds. Publish runs three git
    // calls (origin URL 30 s, rev-parse 30 s, ls-remote DEFAULT_PUSH_TIMEOUT_SEC),
    // each preceded by `git::validate`'s 30 s config read, then the gh version
    // check (2 x 10 s + 1 s) and gh pr create. Killed earlier, the child never
    // records an INDETERMINATE publication.
    let child_bounds = 3 * 30
        + 2 * 30
        + cgagentharness::agentic::workspace::DEFAULT_PUSH_TIMEOUT_SEC
        + 21
        + cgagentharness::agentic::writer::DEFAULT_WRITE_TIMEOUT_SEC;
    for action in ["real-repo-run-push", "real-repo-run-publish"] {
        let budget = shim::timeout_for(&c, &OpsRequest::new(action)).as_secs();
        assert!(
            budget > child_bounds,
            "{action}: {budget}s <= child bounds {child_bounds}s"
        );
    }
    // Approve is the third mutation boundary: it copies the tree for the
    // disposable-copy proof and commits through the same validated git calls,
    // so a decide call shares the write budget rather than the 120 s default.
    assert_eq!(
        shim::timeout_for(&c, &OpsRequest::new("real-repo-run-decide")).as_secs(),
        shim::REAL_REPO_RUN_WRITE_SEC
    );
    let console =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/static/harness.html"))
            .unwrap();
    let wait_ms = (shim::REAL_REPO_RUN_WRITE_SEC + 10) * 1000;
    assert!(
        console.contains(&format!("const AGENT_WRITE_TIMEOUT_MS = {wait_ms};")),
        "the console must wait out the push/publish budget"
    );
    let decision_call = console
        .lines()
        .find(|line| line.contains("'/decision'"))
        .expect("the console posts decisions");
    assert!(
        decision_call.contains("AGENT_WRITE_TIMEOUT_MS"),
        "the console must wait out the approve budget: {decision_call}"
    );
    // Status has its own budget: every run-status GET waits it out.
    let status_ms = (shim::REAL_REPO_RUN_STATUS_SEC + 10) * 1000;
    assert!(
        console.contains(&format!("const AGENT_STATUS_TIMEOUT_MS = {status_ms};")),
        "the console must wait out the status budget"
    );
    let status_calls: Vec<&str> = console
        .lines()
        .filter(|line| line.contains("'/api/agent/runs/' + encodeURIComponent(id), 'GET'"))
        .collect();
    assert!(!status_calls.is_empty(), "the console reads run status");
    for call in status_calls {
        assert!(call.contains("AGENT_STATUS_TIMEOUT_MS"), "{call}");
    }
    assert_eq!(temps.len(), 2, "checks file + plan file");
    let checks_idx = argv.iter().position(|a| a == "--checks-file").unwrap();
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&argv[checks_idx + 1]).unwrap()).unwrap();
    assert_eq!(manifest["checks"][0]["argv"][0], "cargo");
    let paths: Vec<String> = temps.iter().map(|t| t.display().to_string()).collect();
    drop(temps);
    for p in paths {
        assert!(!std::path::Path::new(&p).exists(), "temp files are removed");
    }
}

/// One call through `git::run`: the validating config read plus the operation.
fn validated_git_call_sec(op_sec: u64) -> u64 {
    cgagentharness::agentic::workspace::GIT_VALIDATE_TIMEOUT_SEC + op_sec
}

/// The push/publish sum above has siblings: approve and status also spend their
/// whole budget in validated git calls, and `real-repo-run` spends its fixed
/// overhead before the planner starts. Each shim budget must exceed the sum of
/// the child's own bounds, computed here from the constants the child uses, so a
/// slow operation ends in the child's own error rather than a shim kill (504
/// `AGENTIC_TIMEOUT` with no child record of what happened).
#[test]
fn decide_status_and_setup_budgets_exceed_child_bounds() {
    use cgagentharness::agentic::{config, gh_client, workspace};
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let git_op = validated_git_call_sec(workspace::DEFAULT_GIT_WRITE_TIMEOUT_SEC);

    // Approve (`real_repo_loop::finalize_real_repo_change` ->
    // `executor::apply::prove_disposable_copy` -> `workspace::commit_accepted`):
    // verify_manifest, git_head on the disposable copy, verify_manifest,
    // inspect_index, check-attr, read-tree, write-tree, verify_manifest,
    // commit-tree, update-ref and symbolic-ref, plus hash-object and
    // update-index per accepted file. Copying the clone for the proof is
    // untimed on top. A kill after update-ref strands a committed branch
    // behind a `pending_decision` record. `BUDGETED_APPROVE_FILES` is how many
    // accepted files the write budget covers at the bound (a one-file approve
    // is 780 s); a larger candidate is still approved whenever git answers in
    // ordinary time, the kill guarantee just stops at this size.
    const BUDGETED_APPROVE_FILES: u64 = 4;
    let approve_bounds = |files: u64| 11 * git_op + files * 2 * git_op;
    let mut short = Vec::new();
    let decide = shim::timeout_for(&c, &OpsRequest::new("real-repo-run-decide")).as_secs();
    if decide <= approve_bounds(BUDGETED_APPROVE_FILES) {
        short.push(format!(
            "real-repo-run-decide: {decide}s <= child bounds {}s for {BUDGETED_APPROVE_FILES} file(s) \
             (11 x validated git {git_op}s + {BUDGETED_APPROVE_FILES} x 2 x {git_op}s)",
            approve_bounds(BUDGETED_APPROVE_FILES)
        ));
    }

    // Status renders the pending diff (`commands::render_pending_diff`): `git
    // diff` then `git status`, each through the same validated boundary.
    let status_bounds = 2 * validated_git_call_sec(workspace::DEFAULT_GIT_WRITE_TIMEOUT_SEC);
    let status = shim::timeout_for(&c, &OpsRequest::new("real-repo-run-status")).as_secs();
    if status <= status_bounds {
        short.push(format!(
            "real-repo-run-status: {status}s <= child bounds {status_bounds}s (2 x validated git {git_op}s)"
        ));
    }

    // `real-repo-run` before the planner (`commands::cmd_real_repo_run`), with
    // the shipped gh timeout and retry count: one `gh --version` check, then
    // the PR context (pr_view plus pr_diff, the largest fetched shape; a
    // repo-level run fetches none), the clone, and the origin check; after acceptance, one more
    // `git_head` for the manifest. Every gh read retries on transient errors
    // and timeouts with backoff. The budget formula gives this phase only
    // `REAL_REPO_RUN_OVERHEAD_SEC`; planner and check time are budgeted
    // separately per iteration.
    let acfg = config::load_agentic_config(&config_with(dir.path(), &[]), dir.path()).unwrap();
    let attempts = u64::from(acfg.gh_retries) + 1;
    let backoff = (1..=acfg.gh_retries)
        .map(gh_client::retry_backoff_sec)
        .sum::<f64>()
        .ceil() as u64;
    let gh_read = attempts * acfg.gh_timeout_sec + backoff;
    let clone = attempts * gh_client::DEFAULT_CLONE_TIMEOUT_SEC + backoff;
    let version_check = u64::from(gh_client::VERSION_CHECK_ATTEMPTS) * gh_client::VERSION_CHECK_TIMEOUT_SEC
        + u64::from(gh_client::VERSION_CHECK_ATTEMPTS - 1) * gh_client::VERSION_CHECK_RETRY_DELAY_SEC;
    let context_reads = 2;
    let setup_bounds = version_check + context_reads * gh_read + clone + git_op + git_op;
    if shim::REAL_REPO_RUN_OVERHEAD_SEC <= setup_bounds {
        short.push(format!(
            "REAL_REPO_RUN_OVERHEAD_SEC: {}s <= pre-planner child bounds {setup_bounds}s \
             (gh version check {version_check}s + {context_reads} x gh read {gh_read}s + clone {clone}s \
             + origin and manifest HEAD 2 x {git_op}s)",
            shim::REAL_REPO_RUN_OVERHEAD_SEC
        ));
    }
    assert!(
        short.is_empty(),
        "shim budgets below the child's own bounds:\n{}",
        short.join("\n")
    );
}

#[test]
fn argv_builder_validation_mirrors_ops_runner() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let err = |req: OpsRequest| shim::build_argv(&c, &req).unwrap_err().to_string();
    assert!(err(OpsRequest::new("deepagent-plan")).contains("not allowed"));
    assert!(err(OpsRequest::new("__sleep")).contains("not allowed"));
    let mut r = OpsRequest::new("apply-skill");
    r.name = Some("n".into());
    r.desc = Some("d".into());
    assert!(err(r.clone()).contains("reason"));
    r.reason = Some("  ".into());
    assert!(err(r.clone()).contains("reason"));
    // Invisible text is not a reason: zero-width space, BOM, word joiner, soft hyphen.
    for invisible in ["\u{200B}", "\u{FEFF}", " \u{2060} ", "\u{00AD}\u{200D}"] {
        r.reason = Some(invisible.into());
        assert!(err(r.clone()).contains("reason"), "{invisible:?}");
    }
    let mut r = OpsRequest::new("real-repo-run");
    r.instruction = Some("x".into());
    assert!(err(r.clone()).contains("checks"));
    r.checks = Some(vec![json!({"name": "a", "argv": ["a"]})]);
    assert!(err(r.clone()).contains("branch"));
    r.branch = Some("claude/x".into());
    r.commit_message = Some("m".into());
    assert!(err(r.clone()).contains("reason"));
    r.reason = Some("r".into());
    r.read_files = Some(vec!["../etc/passwd".into()]);
    assert!(err(r.clone()).contains("read_files"));
    r.read_files = None;
    r.plan = Some("   ".into());
    assert!(err(r).contains("plan"));
    let mut r = OpsRequest::new("real-repo-run-decide");
    r.run_id = Some(HEX32.into());
    assert!(err(r.clone()).contains("decision"));
    r.decision = Some("maybe".into());
    assert!(err(r).contains("decision"));
    assert!(err(OpsRequest::new("real-repo-run-status")).contains("run_id"));
    let mut r = OpsRequest::new("real-repo-run-publish");
    r.run_id = Some(HEX32.into());
    assert!(err(r).contains("reason"));
}

#[test]
fn argv_builder_refuses_nul_in_argv_text_fields() {
    // A NUL cannot cross an argv element; refuse it as a request error (400)
    // instead of letting the spawn fail as a shim I/O error (502).
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let mut run = OpsRequest::new("real-repo-run");
    run.instruction = Some("x".into());
    run.checks = Some(vec![json!({"name": "a", "argv": ["a"]})]);
    run.branch = Some("claude/x".into());
    run.commit_message = Some("m".into());
    run.reason = Some("r".into());
    assert!(shim::build_argv(&c, &run).is_ok());
    type Field = fn(&mut OpsRequest) -> &mut Option<String>;
    let fields: [(&str, Field); 4] = [
        ("instruction", |r| &mut r.instruction),
        ("branch", |r| &mut r.branch),
        ("commit_message", |r| &mut r.commit_message),
        ("reason", |r| &mut r.reason),
    ];
    for (label, field) in fields {
        let mut r = run.clone();
        *field(&mut r) = Some("ok\0--confirm".into());
        let err = shim::build_argv(&c, &r).unwrap_err();
        assert!(matches!(err, ShimError::Ops(_)), "{label}: {err}");
        assert!(err.to_string().contains("NUL"), "{label}: {err}");
    }
    let mut skill = OpsRequest::new("propose-skill");
    skill.name = Some("n\0".into());
    skill.desc = Some("d".into());
    assert!(matches!(shim::build_argv(&c, &skill), Err(ShimError::Ops(_))));
    // Temp-file payloads are not argv and keep their bytes.
    run.plan = Some("plan\0bytes".into());
    assert!(shim::build_argv(&c, &run).is_ok());
}

#[test]
fn skill_argv_keeps_dash_values_single_and_confirms_only_apply() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let mut r = OpsRequest::new("propose-skill");
    r.name = Some("--confirm".into());
    r.desc = Some("-d".into());
    r.reason = Some("--confirm".into());
    r.confirm = true;
    let (argv, _) = shim::build_argv(&c, &r).unwrap();
    assert!(argv.contains(&"--name=--confirm".to_string()));
    assert!(argv.contains(&"--desc=-d".to_string()));
    assert!(argv.contains(&"--reason=--confirm".to_string()));
    assert!(
        !argv.contains(&"--confirm".to_string()),
        "propose-skill never forwards confirm"
    );
    r.action = "apply-skill".into();
    let (argv, _) = shim::build_argv(&c, &r).unwrap();
    assert_eq!(argv.iter().filter(|a| *a == "--confirm").count(), 1);
}

#[test]
fn budget_formula_and_labels() {
    assert_eq!(shim::real_repo_run_budget_sec(720, None, 1), 3 * 720 + 3 * 120 + 900);
    assert_eq!(shim::real_repo_run_budget_sec(720, Some(1), 0), 720 + 120 + 900);
    assert_eq!(
        shim::real_repo_run_budget_sec(0, Some(2), 2),
        2 * 720 + 2 * 2 * 120 + 900
    );
    assert_eq!(shim::real_repo_run_timeout_sec(720, Some(10), 8), 3600);
    // An extreme operator planner timeout saturates instead of overflowing
    // (debug panic) or wrapping under the 3600s cap (release).
    assert_eq!(shim::real_repo_run_budget_sec(u64::MAX, Some(3), 1), u64::MAX);
    assert_eq!(shim::real_repo_run_budget_sec(u64::MAX / 2, Some(2), 0), u64::MAX);
    assert_eq!(shim::real_repo_run_timeout_sec(u64::MAX / 3 + 1, Some(3), 1), 3600);
    assert_eq!(shim::label_for(0), (true, "ok"));
    assert_eq!(shim::label_for(2), (false, "failed"));
    assert_eq!(shim::label_for(3), (false, "env_config"));
    assert_eq!(shim::label_for(4), (false, "write_refused"));
    assert_eq!(shim::label_for(1), (false, "unknown"));
    assert_eq!(shim::label_for(101), (false, "unknown"));
}

// ---------------------------------------------------------------- real child

#[tokio::test]
async fn status_against_the_real_child_reports_the_disabled_banner_exit_0() {
    let dir = tempfile::tempdir().unwrap();
    config_with(dir.path(), &[]);
    let c = ctx(dir.path());
    let r = shim::run_agentic_op(&c, &OpsRequest::new("status")).await.unwrap();
    assert_eq!(r.exit_code, 0, "{}", r.stderr);
    assert!(r.ok);
    assert_eq!(r.label, "ok");
    assert!(r.stdout.contains("Agentic layer disabled"));
    assert!(r.is_disabled_layer_success());
    // Enabled: status prints real fields.
    config_with(dir.path(), &[("agentic.enabled", "true")]);
    let r = shim::run_agentic_op(&c, &OpsRequest::new("status")).await.unwrap();
    assert!(r.ok);
    assert!(r.stdout.contains("repo "), "{}", r.stdout);
    assert!(!r.is_disabled_layer_success());
    // Missing config: exit 3 -> env_config.
    let mut c2 = c.clone();
    c2.config_path = dir.path().join("nonexistent.yaml");
    let r = shim::run_agentic_op(&c2, &OpsRequest::new("status")).await.unwrap();
    assert_eq!(r.exit_code, 3);
    assert_eq!(r.label, "env_config");
}

#[tokio::test]
async fn timeout_kills_the_child_and_maps_to_a_typed_error() {
    let dir = tempfile::tempdir().unwrap();
    config_with(dir.path(), &[]);
    let argv = vec![
        BIN.to_string(),
        "agentic".into(),
        "--config".into(),
        dir.path().join("config.yaml").display().to_string(),
        "__sleep".into(),
        "30".into(),
    ];
    let started = std::time::Instant::now();
    let err = shim::run_argv(&argv, dir.path(), Duration::from_millis(500))
        .await
        .unwrap_err();
    assert!(matches!(err, ShimError::Timeout { .. }), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

// ---------------------------------------------------------------- HTTP routes

fn run_body() -> serde_json::Value {
    json!({"instruction": "fix the parser", "branch": "claude/parser-fix", "commit_message": "fix: parser", "reason": "triage"})
}

#[tokio::test]
async fn planner_settings_drift_refuses_sync_and_detached_runs() {
    let model = start_mock_model().await;
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default()
            .with("agentic.enabled", "true")
            .with("agentic.deepagent_github.enabled", "true"),
    )
    .await;
    for (key, value) in [
        ("model", "other-model"),
        ("base_url", "http://127.0.0.1:19001/v1"),
        ("planner_timeout_sec", "1200"),
        ("planner_max_tokens", "8192"),
        ("provider", "openai_compatible"),
    ] {
        config_with(
            &s.home,
            &[
                ("agentic.enabled", "true"),
                ("agentic.deepagent_github.enabled", "true"),
                (&format!("agentic.deepagent_github.{key}"), value),
            ],
        );
        let (status, result) = s.post_json("/api/agent/run", run_body()).await;
        assert_eq!(status, 200, "{key}: {result}");
        assert_eq!(result["exit_code"], 3, "{key}: {result}");
        assert!(
            result["stderr"].as_str().unwrap().contains("restart required"),
            "{result}"
        );
        let (status, job) = s.post_json("/api/agent/jobs", run_body()).await;
        assert_eq!(status, 202, "{key}: {job}");
        let path = format!("/api/agent/jobs/{}", job["job_id"].as_str().unwrap());
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let (_, job) = s.get_json(&path).await;
                if job["result"].is_object() {
                    break job["result"].clone();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(result["exit_code"], 3, "{key}: {result}");
        assert!(!s.state.generation_gate.is_held());
    }
    // An unchanged snapshot proceeds to the independent write gate.
    cgagentharness::common::config::AppConfig::write_private(
        &s.home.join("config.yaml"),
        &serde_yaml_ng::to_string(&s.state.cfg.raw).unwrap(),
    )
    .unwrap();
    let (_, result) = s.post_json("/api/agent/run", run_body()).await;
    assert_eq!(result["exit_code"], 4, "{result}");
    assert!(std::fs::read_dir(s.home.join("data/agentic/workspaces"))
        .unwrap()
        .next()
        .is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn planner_edit_after_preparation_is_checked_by_the_real_child() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("edit-before-exec");
    std::fs::write(
        &exe,
        format!("#!/bin/sh\ncp changed.yaml config.yaml\nexec '{BIN}' \"$@\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let model = start_mock_model().await;
    let mut options = ServerOptions::default()
        .with("agentic.enabled", "true")
        .with("agentic.deepagent_github.enabled", "true");
    options.shim_exe = Some(exe);
    let s = spawn_server(&model.base_url(), options).await;
    let mut changed = s.state.cfg.raw.clone();
    changed["agentic"]["deepagent_github"]["planner_timeout_sec"] = 1200.into();
    cgagentharness::common::config::AppConfig::write_private(
        &s.home.join("changed.yaml"),
        &serde_yaml_ng::to_string(&changed).unwrap(),
    )
    .unwrap();
    let (_, result) = s.post_json("/api/agent/run", run_body()).await;
    assert_eq!(result["exit_code"], 3, "{result}");
    assert!(
        result["stderr"].as_str().unwrap().contains("restart required"),
        "{result}"
    );
    assert!(std::fs::read_dir(s.home.join("data/agentic/workspaces"))
        .unwrap()
        .next()
        .is_none());
}

#[tokio::test]
async fn a_request_can_never_carry_an_argv() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    for hostile in [
        json!({"argv": ["rm", "-rf", "/"]}),
        json!({"checks": [{"name": "pytest", "argv": ["rm", "-rf", "/"]}]}),
        json!({"command": "rm -rf /"}),
        json!({"checks_file": "/etc/passwd"}),
        json!({"planner_identity": "caller-controlled"}),
    ] {
        let mut body = run_body();
        for (k, v) in hostile.as_object().unwrap() {
            body[k] = v.clone();
        }
        let (status, resp) = s.post_json("/api/agent/run", body).await;
        assert_eq!(status, 422, "{hostile}: {resp}");
        assert_eq!(code(&resp), "VALIDATION_ERROR");
        assert!(!resp.to_string().contains("rm"));
    }
    // A non-string check entry is a validation error, not a profile lookup.
    let mut body = run_body();
    body["checks"] = json!([{"name": "pytest"}]);
    let (status, _) = s.post_json("/api/agent/run", body).await;
    assert_eq!(status, 422);
}

#[tokio::test]
async fn a_nul_in_run_text_is_a_request_error_not_a_shim_failure() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let mut body = run_body();
    body["instruction"] = json!("fix\0--confirm");
    let (status, resp) = s.post_json("/api/agent/run", body).await;
    assert_eq!(status, 400, "{resp}");
    assert_eq!(code(&resp), "AGENTIC_ERROR");
}

#[tokio::test]
async fn run_validates_shape_before_any_subprocess() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    // Unknown profile names the valid ones (400, not 422).
    let mut body = run_body();
    body["checks"] = json!(["nope"]);
    let (status, resp) = s.post_json("/api/agent/run", body).await;
    assert_eq!(status, 400, "{resp}");
    assert_eq!(code(&resp), "UNKNOWN_CHECK_PROFILE");
    assert!(message(&resp).contains("cargo-test"));
    // Branch outside the namespace; pr + issue together; iterations bounds; read_files jail.
    for (field, value) in [
        ("branch", json!("main")),
        ("branch", json!("feature/x")),
        ("max_iterations", json!(0)),
        ("max_iterations", json!(11)),
        ("pr", json!(0)),
        ("read_files", json!(["../etc/passwd"])),
        ("read_files", json!(["/abs"])),
        ("plan", json!("   ")),
    ] {
        let mut body = run_body();
        body[field] = value.clone();
        let (status, resp) = s.post_json("/api/agent/run", body).await;
        assert_eq!(status, 422, "{field}={value}: {resp}");
        assert!(
            resp["detail"]["details"]["fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f.as_str().unwrap().contains(field)),
            "{resp}"
        );
    }
    let mut body = run_body();
    body["pr"] = json!(1);
    body["issue"] = json!(2);
    let (status, resp) = s.post_json("/api/agent/run", body).await;
    assert_eq!(status, 422);
    assert!(resp.to_string().contains("pr/issue"));
    // Budget exceeded reports the largest fitting iteration count.
    let mut body = run_body();
    body["max_iterations"] = json!(10);
    body["checks"] = json!(["cargo-test", "cargo-clippy", "cargo-fmt", "pytest", "ruff"]);
    let (status, resp) = s.post_json("/api/agent/run", body).await;
    assert_eq!(status, 422, "{resp}");
    assert_eq!(code(&resp), "AGENTIC_BUDGET_EXCEEDED");
    assert_eq!(resp["detail"]["details"]["cap_sec"], 3600);
    assert_eq!(resp["detail"]["details"]["max_iterations_that_fit"], 2);
    // No audit tool_broker line was written for any of the refusals above.
    let audit = s.audit_log();
    assert!(!audit.contains("tool_broker_decision"));
}

#[tokio::test]
async fn run_reaches_the_child_and_disabled_layer_is_409() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    // Shipped config: agentic.enabled false -> the child prints the banner, exit 0 -> 409.
    let (status, resp) = s.post_json("/api/agent/run", run_body()).await;
    assert_eq!(status, 409, "{resp}");
    assert_eq!(code(&resp), "AGENTIC_DISABLED");
    assert_eq!(resp["detail"]["details"]["action"], "real-repo-run");
    for path in [
        format!("/api/agent/runs/{HEX32}"),
        format!("/api/agent/runs/{HEX32}/push"),
        format!("/api/agent/runs/{HEX32}/discard"),
    ] {
        let (status, resp) = if path.ends_with(HEX32) {
            s.get_json(&path).await
        } else if path.ends_with("/push") {
            s.post_json(&path, json!({"reason": "reviewed fixture", "confirm": true}))
                .await
        } else {
            s.post_json(&path, json!({})).await
        };
        assert_eq!(status, 409, "{path}: {resp}");
        assert_eq!(code(&resp), "AGENTIC_DISABLED");
    }
    // publish checks --confirm before the layer gate (faithful to CyClaw): no confirm is exit 4, not 409.
    let (status, resp) = s
        .post_json(&format!("/api/agent/runs/{HEX32}/publish"), json!({"reason": "r"}))
        .await;
    assert_eq!(status, 200, "{resp}");
    assert_eq!(resp["label"], "write_refused");
    assert_eq!(resp["exit_code"], 4);
    let (status, resp) = s
        .post_json(
            &format!("/api/agent/runs/{HEX32}/publish"),
            json!({"reason": "r", "confirm": true, "body": "Reviewed fixture description"}),
        )
        .await;
    assert_eq!(status, 409, "{resp}");
    let (status, resp) = s
        .post_json(
            &format!("/api/agent/runs/{HEX32}/decision"),
            json!({"decision": "approve", "reason": "reviewed fixture", "confirm": true}),
        )
        .await;
    assert_eq!(status, 409, "{resp}");
    // github status also crosses the shim; disabled banner is returned as ok=true text there.
    let (status, resp) = s.get_json("/api/github/status").await;
    assert_eq!(status, 200, "{resp}");
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["label"], "ok");
    assert!(resp["stdout"].as_str().unwrap().contains("Agentic layer disabled"));
    // The audit log now carries the tool-broker decision for the run.
    let audit = s.audit_log();
    assert!(audit.contains("tool_broker_decision"));
    assert!(audit.contains("\"tool\":\"agent_run\""));
}

#[tokio::test]
async fn malformed_run_ids_never_reach_the_child() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    for bad in [
        "nothex",
        "0123456789ABCDEF0123456789ABCDEF",
        "0123456789abcdef0123456789abcde",
        "0123456789abcdef0123456789abcdef%0A",
        "-0123456789abcdef0123456789abcde",
    ] {
        let (status, resp) = s.get_json(&format!("/api/agent/runs/{bad}")).await;
        assert_eq!(status, 400, "{bad}: {resp}");
        assert_eq!(code(&resp), "INVALID_RUN_ID");
        assert!(resp["detail"]["details"]["run_id"].as_str().unwrap().len() <= 64);
    }
    let huge = "f".repeat(500);
    let (status, resp) = s.get_json(&format!("/api/agent/runs/{huge}")).await;
    assert_eq!(status, 400);
    assert_eq!(resp["detail"]["details"]["run_id"].as_str().unwrap().len(), 64);
    // Traversal-shaped ids never route at all.
    let resp = s
        .req(reqwest::Method::GET, "/api/agent/runs/..%2F..%2Fetc")
        .send()
        .await
        .unwrap();
    assert!(resp.status().as_u16() == 400 || resp.status().as_u16() == 404);
    // Publish requires a reason; confirm is not defaulted (a missing confirm is still forwarded as absent).
    let (status, resp) = s
        .post_json(&format!("/api/agent/runs/{HEX32}/publish"), json!({}))
        .await;
    assert_eq!(status, 422, "{resp}");
    // Whitespace passes the length bound but the shim refuses it before any spawn (400, like OpsError).
    let (status, resp) = s
        .post_json(&format!("/api/agent/runs/{HEX32}/publish"), json!({"reason": "   "}))
        .await;
    assert_eq!(status, 400, "{resp}");
    assert!(message(&resp).contains("non-empty reason"));
}

#[tokio::test]
async fn tool_broker_denial_and_busy_gate() {
    let model = start_mock_model().await;
    let deny = ServerOptions {
        deny_all_tools: true,
        ..Default::default()
    };
    let d = spawn_server(&model.base_url(), deny).await;
    let (status, resp) = d.post_json("/api/agent/run", run_body()).await;
    assert_eq!(status, 403, "{resp}");
    assert_eq!(code(&resp), "TOOL_DENIED");
    // While a chat turn holds the shared backend gate, a run is AGENT_RUN_BUSY.
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default().with(
            "agentic.deepagent_github.base_url",
            &format!("\"{}\"", model.base_url()),
        ),
    )
    .await;
    model.set_delay_ms(3_000);
    let chat = async { s.post_json("/api/chat", json!({"message": "slow"})).await };
    let run = async {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let (status, resp) = s.post_json("/api/agent/run", run_body()).await;
        assert_eq!(status, 409, "{resp}");
        assert_eq!(code(&resp), "AGENT_RUN_BUSY");
        s.post_json("/api/chat/cancel", json!({})).await;
    };
    let _ = tokio::join!(chat, run);
    assert!(!s.state.agent_run_gate.is_held());
    assert!(!s.state.generation_gate.is_held());
}

#[test]
fn cli_surface_hides_agentic_and_refuses_unknown_subcommands() {
    let out = std::process::Command::new(BIN).arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.lines().any(|l| l.trim_start().starts_with("serve")));
    assert!(
        !help.lines().any(|l| l.trim_start().starts_with("agentic")),
        "hidden subcommand must not be advertised: {help}"
    );
    let dir = tempfile::tempdir().unwrap();
    config_with(dir.path(), &[("agentic.enabled", "true")]);
    let (code, _, err) = run_agentic(&dir.path().join("config.yaml"), &["bogus-action"], &[], dir.path());
    assert_eq!(code, 2, "{err}");
    let (code, _, _) = run_agentic(
        std::path::Path::new("/nonexistent/config.yaml"),
        &["status"],
        &[],
        dir.path(),
    );
    assert_eq!(code, 3);
    // Exit 0 iff every check passes; on a CI runner without a hard sandbox
    // (e.g. `unshare --net` refused by a hardened container) the "hard
    // sandbox" check fails and the CLI reports exit 2 -- same environment
    // dependence `real_repo_run_smoke` already tolerates by skipping.
    let (code, out, err) = run_agentic(&dir.path().join("config.yaml"), &["test"], &[], dir.path());
    // Only environment-dependent checks may fail: the hard sandbox, and the gh
    // version where gh is not installed. Exit 2 exactly when one did.
    let failed: Vec<&str> = out
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("[FAIL] "))
        .map(|rest| rest.split(':').next().unwrap_or(rest).trim())
        .collect();
    let gh_missing = cgagentharness::common::process::which("gh").is_none();
    for name in &failed {
        assert!(
            *name == "hard sandbox" || (*name == "gh version" && gh_missing),
            "self-test check {name:?} failed: {out} / {err}"
        );
    }
    if std::env::var_os("CGAH_REQUIRE_CHECK_SANDBOX").is_some() {
        assert!(!failed.contains(&"hard sandbox"), "{out} / {err}");
    }
    assert_eq!(code, if failed.is_empty() { 0 } else { 2 }, "{out} / {err}");
    assert!(out.contains("Self-test:"), "{out}");
    assert!(
        out.contains("passed") || err.contains("passed"),
        "self-test must report a passed/total tally: {out} / {err}"
    );
    // Value starting with --confirm binds to its option, never becomes a flag.
    let (code, _, err) = run_agentic(
        &dir.path().join("config.yaml"),
        &["apply-skill", "--name=x", "--desc=--confirm", "--reason=r"],
        &[],
        dir.path(),
    );
    assert_ne!(code, 0);
    assert!(!err.contains("unexpected argument"), "{err}");
}

#[test]
fn serve_refuses_a_non_loopback_bind() {
    let home = tempfile::tempdir().unwrap();
    for host in ["0.0.0.0", "1.2.3.4", "example.com"] {
        let out = std::process::Command::new(BIN)
            .args(["serve", "--host", host, "--port", "18791"])
            .env("CGAGENTHARNESS_HOME", home.path())
            .output()
            .unwrap();
        assert!(!out.status.success(), "serve --host {host} must be refused");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("loopback"),
            "non-loopback refusal must name the loopback rule: {err}"
        );
    }
}

// ---------------------------------------------------------------- detached jobs

#[tokio::test]
async fn a_detached_job_reaches_the_same_disabled_layer_outcome_as_the_sync_route() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, created) = s.post_json("/api/agent/jobs", run_body()).await;
    assert_eq!(status, 202, "{created}");
    assert_eq!(created["status"], "running");
    let job_id = created["job_id"].as_str().unwrap().to_string();
    assert_eq!(job_id.len(), 32);

    let mut last = serde_json::Value::Null;
    for _ in 0..100 {
        let (status, body) = s.get_json(&format!("/api/agent/jobs/{job_id}")).await;
        assert_eq!(status, 200);
        last = body;
        if last["status"] != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(last["status"], "failed", "{last}");
    // The child prints the disabled banner (exit 0), which the route turns into 409 -- same as the synchronous route.
    assert_eq!(last["error"]["http_status"], 409);
    assert_eq!(last["error"]["detail"]["code"], "AGENTIC_DISABLED");

    let (status, list) = s.get_json("/api/agent/jobs").await;
    assert_eq!(status, 200);
    assert!(list["jobs"].as_array().unwrap().iter().any(|j| j["job_id"] == job_id));
}

#[tokio::test]
async fn cancelling_a_running_job_aborts_it_and_a_finish_after_cancel_does_not_resurrect_it() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, created) = s.post_json("/api/agent/jobs", run_body()).await;
    assert_eq!(status, 202, "{created}");
    let job_id = created["job_id"].as_str().unwrap().to_string();

    let (status, cancelled) = s
        .post_json(&format!("/api/agent/jobs/{job_id}/cancel"), json!({}))
        .await;
    assert_eq!(status, 200, "{cancelled}");
    assert!(matches!(
        cancelled["status"].as_str(),
        Some("cancelled") | Some("failed") | Some("finished")
    ));

    // Give the (possibly already-aborted) task a moment, then confirm cancellation is sticky once observed.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (status, after) = s.get_json(&format!("/api/agent/jobs/{job_id}")).await;
    assert_eq!(status, 200);
    if cancelled["status"] == "cancelled" {
        assert_eq!(after["status"], "cancelled", "{after}");
    }
}

#[tokio::test]
async fn unknown_and_malformed_job_ids_are_rejected() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, resp) = s.get_json(&format!("/api/agent/jobs/{HEX32}")).await;
    assert_eq!(status, 404, "{resp}");
    let (status, resp) = s.get_json("/api/agent/jobs/not-hex").await;
    assert_eq!(status, 400, "{resp}");
    assert_eq!(code(&resp), "INVALID_JOB_ID");
    let (status, resp) = s.post_json(&format!("/api/agent/jobs/{HEX32}/cancel"), json!({})).await;
    assert_eq!(status, 404, "{resp}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_job_holds_the_run_gate_so_a_concurrent_sync_run_is_busy() {
    use std::os::unix::fs::PermissionsExt;
    // A slow child keeps the job in flight; a disabled-layer child would exit at once
    // and let the sync run fail for an unrelated reason.
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("slow-shim");
    std::fs::write(&exe, "#!/bin/sh\nsleep 10\nprintf '{\"status\":\"failed\"}\\n'\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let model = start_mock_model().await;
    let opts = ServerOptions {
        shim_exe: Some(exe),
        ..Default::default()
    };
    let s = spawn_server(&model.base_url(), opts).await;
    let (status, created) = s.post_json("/api/agent/jobs", run_body()).await;
    assert_eq!(status, 202, "{created}");
    // The gate is claimed synchronously inside prepare_run before the task is spawned.
    let (status, resp) = s.post_json("/api/agent/run", run_body()).await;
    assert_eq!(status, 409, "{resp}");
    assert_eq!(code(&resp), "AGENT_RUN_BUSY", "{resp}");
    let job_id = created["job_id"].as_str().unwrap();
    s.post_json(&format!("/api/agent/jobs/{job_id}/cancel"), json!({}))
        .await;
}

#[tokio::test]
async fn jobs_refuse_hostile_argv_and_unknown_profiles_like_the_sync_route() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    for hostile in [
        json!({"argv": ["rm", "-rf", "/"]}),
        json!({"checks": [{"name": "pytest", "argv": ["rm", "-rf", "/"]}]}),
        json!({"command": "rm -rf /"}),
    ] {
        let mut body = run_body();
        for (k, v) in hostile.as_object().unwrap() {
            body[k] = v.clone();
        }
        let (status, resp) = s.post_json("/api/agent/jobs", body).await;
        assert_eq!(status, 422, "{hostile}: {resp}");
        assert_eq!(code(&resp), "VALIDATION_ERROR");
        assert!(!resp.to_string().contains("rm"));
    }
    let mut body = run_body();
    body["checks"] = json!(["nope"]);
    let (status, resp) = s.post_json("/api/agent/jobs", body).await;
    assert_eq!(status, 400, "{resp}");
    assert_eq!(code(&resp), "UNKNOWN_CHECK_PROFILE");
    let mut body = run_body();
    body["branch"] = json!("main");
    let (status, resp) = s.post_json("/api/agent/jobs", body).await;
    assert_eq!(status, 422, "{resp}");
    assert!(
        resp["detail"]["details"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_str().unwrap().contains("branch")),
        "{resp}"
    );
    let deny = ServerOptions {
        deny_all_tools: true,
        ..Default::default()
    };
    let d = spawn_server(&model.base_url(), deny).await;
    let (status, resp) = d.post_json("/api/agent/jobs", run_body()).await;
    assert_eq!(status, 403, "{resp}");
    assert_eq!(code(&resp), "TOOL_DENIED");
}

#[test]
fn publication_body_crosses_the_shim_as_an_exact_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let mut request = OpsRequest::new("real-repo-run-publish");
    request.run_id = Some(HEX32.into());
    request.reason = Some("reviewed template".into());
    request.confirm = true;
    request.body = Some("## Proposed changes\nLiteral `code` and $HOME.\n\n## Evidence\nVerified.\n".into());
    let (argv, temps) = shim::build_argv(&c, &request).unwrap();
    assert_eq!(temps.len(), 1);
    let index = argv
        .iter()
        .position(|a| a == "--body-file")
        .expect("body file argument");
    let path = std::path::PathBuf::from(&argv[index + 1]);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), request.body.unwrap());
    assert!(!argv.iter().any(|a| a.contains("$HOME")));
    drop(temps);
    assert!(!path.exists());
}

#[tokio::test]
async fn publication_requires_bounded_reviewed_text_without_defaulting_approval() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, _) = s
        .post_json(
            &format!("/api/agent/runs/{HEX32}/push"),
            json!({"reason":"r", "confirm":true, "body":"unrelated publication text"}),
        )
        .await;
    assert_eq!(status, 422, "push keeps its original strict schema");
    let path = format!("/api/agent/runs/{HEX32}/publish");
    let (status, _) = s.post_json(&path, json!({"reason":"r", "confirm":true})).await;
    assert_eq!(status, 400);
    for body in [
        " ".to_string(),
        "x".repeat(cgagentharness::common::MAX_PR_BODY_BYTES + 1),
    ] {
        let (status, _) = s
            .post_json(&path, json!({"reason":"r", "confirm":true,"body":body}))
            .await;
        assert_eq!(status, 422);
    }
    let (status, result) = s.post_json(&path, json!({"reason":"r", "body":"Reviewed body"})).await;
    assert_eq!(status, 200);
    assert_eq!(result["exit_code"], 4, "body never supplies approval");
}

#[cfg(unix)]
#[tokio::test]
async fn an_agentic_child_starts_after_the_audit_line_that_authorized_it() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("fixture-shim");
    // The child reports whether the run's broker decision was on disk when it
    // started; it runs in the home, beside logs/audit.jsonl.
    std::fs::write(
        &exe,
        "#!/bin/sh\nif grep -q '\"event\":\"tool_broker_decision\"' logs/audit.jsonl 2>/dev/null; then seen=true; else seen=false; fi\nprintf '{\"status\":\"pending_decision\",\"decision_on_disk\":%s}\\n' \"$seen\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let model = start_mock_model().await;
    // The server's writer may wait 5 s for the lease this test holds.
    let mut opts = ServerOptions::default().with("logging.audit_lease_wait_ms", "5000");
    opts.shim_exe = Some(exe);
    let s = spawn_server(&model.base_url(), opts).await;
    let lease = hold_audit_lease(&s.home).await;
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(lease);
    });
    let mut body = run_body();
    body["confirm"] = json!(true);
    let (status, result) = s.post_json("/api/agent/run", body).await;
    release.await.unwrap();
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["parsed"]["decision_on_disk"], true, "{result}");
}

#[tokio::test]
async fn github_status_spawn_failure_is_502() {
    let model = start_mock_model().await;
    let dir = tempfile::tempdir().unwrap();
    let opts = ServerOptions {
        shim_exe: Some(dir.path().join("missing-shim")),
        ..Default::default()
    };
    let s = spawn_server(&model.base_url(), opts).await;
    let (status, resp) = s.get_json("/api/github/status").await;
    assert_eq!(status, 502, "{resp}");
    assert_eq!(code(&resp), "SHIM_IO_ERROR");
}
