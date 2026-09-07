//! Ports of test_ops_runner.py (agentic half), test_harness_agent_routes.py,
//! and the shim timeout contract. The child is the REAL binary.

mod common;

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
            json!({"reason": "r", "confirm": true}),
        )
        .await;
    assert_eq!(status, 409, "{resp}");
    let (status, resp) = s
        .post_json(
            &format!("/api/agent/runs/{HEX32}/decision"),
            json!({"decision": "approve"}),
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
    let (code, out, _) = run_agentic(&dir.path().join("config.yaml"), &["test"], &[], dir.path());
    assert_eq!(code, 0);
    assert!(out.contains("Self-test:"), "{out}");
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
