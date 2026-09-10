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
    assert!(err(r).contains("reason"));
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
fn budget_formula_and_labels() {
    assert_eq!(shim::real_repo_run_budget_sec(720, None, 1), 3 * 720 + 3 * 120 + 300);
    assert_eq!(shim::real_repo_run_budget_sec(720, Some(1), 0), 720 + 120 + 300);
    assert_eq!(
        shim::real_repo_run_budget_sec(0, Some(2), 2),
        2 * 720 + 2 * 2 * 120 + 300
    );
    assert_eq!(shim::real_repo_run_timeout_sec(720, Some(10), 8), 3600);
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
async fn a_request_can_never_carry_an_argv() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    for hostile in [
        json!({"argv": ["rm", "-rf", "/"]}),
        json!({"checks": [{"name": "pytest", "argv": ["rm", "-rf", "/"]}]}),
        json!({"command": "rm -rf /"}),
        json!({"checks_file": "/etc/passwd"}),
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
    let audit = std::fs::read_to_string(s.home.join("logs").join("audit.jsonl")).unwrap_or_default();
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
    let audit = std::fs::read_to_string(s.home.join("logs").join("audit.jsonl")).unwrap();
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
    assert!(
        matches!(code, 0 | 2),
        "self-test exit contract is 0 (all checks passed) or 2 (a check failed, including a missing hard sandbox): {out} / {err}"
    );
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

#[tokio::test]
async fn a_job_holds_the_run_gate_so_a_concurrent_sync_run_is_busy() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, created) = s.post_json("/api/agent/jobs", run_body()).await;
    assert_eq!(status, 202, "{created}");
    // The gate is claimed synchronously inside prepare_run before the task is spawned,
    // so a request issued immediately after must already see it held (best-effort race window aside).
    let (status, resp) = s.post_json("/api/agent/run", run_body()).await;
    assert!(
        status == 409 && (code(&resp) == "AGENT_RUN_BUSY" || code(&resp) == "AGENTIC_DISABLED"),
        "{status} {resp}"
    );
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
