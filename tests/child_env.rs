use cgagentharness::{agentic::gh_client::gh_env, common::process, shim};
use std::{process::Command, time::Duration};

const PROVIDER_KEYS: [&str; 4] = [
    "ANTHROPIC_API_KEY",
    "GROK_API_KEY",
    "DEEPAGENT_API_KEY",
    "SERPAPI_API_KEY",
];

fn fixture_argv(test: &str) -> Vec<String> {
    vec![
        std::env::current_exe().unwrap().display().to_string(),
        "--exact".into(),
        test.into(),
        "--ignored".into(),
        "--nocapture".into(),
    ]
}

fn isolated_parent(test: &str) {
    let argv = fixture_argv(test);
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    for key in PROVIDER_KEYS {
        command.env(key, "fake-child-env-regression-key");
    }
    command
        .env("PATH", "child-env-fixture-path")
        .env("GH_TOKEN", "fake-gh-token")
        .env("GITHUB_TOKEN", "fake-github-token")
        .env("GH_CONFIG_DIR", "fixture-gh-config")
        .env("XDG_CONFIG_HOME", "fixture-xdg-config")
        .env("GH_HOST", "github.example.invalid")
        .env("SSH_AUTH_SOCK", "fixture-ssh-agent")
        .env("APPDATA", "fixture-appdata")
        .env("LOCALAPPDATA", "fixture-localappdata")
        .env("GH_TELEMETRY", "true")
        .env("GH_NO_UPDATE_NOTIFIER", "0")
        .env("GIT_TERMINAL_PROMPT", "1")
        .env("DO_NOT_TRACK", "0")
        .env("LD_PRELOAD", "")
        .env("DYLD_INSERT_LIBRARIES", "")
        .env("GIT_SSH_COMMAND", "fixture-disallowed-command")
        .env("CGAGENTHARNESS_AGENT_COMMIT_NAME", "Fixture Author")
        .env("CGAGENTHARNESS_AGENT_COMMIT_EMAIL", "fixture@example.invalid")
        .env("CGAGENTHARNESS_AGENT_BRANCH_PREFIX", "fixture/")
        .env("CGAGENTHARNESS_AGENTIC_WRITE_DISABLE", "1");
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "isolated fixture {test} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_no_provider_keys() {
    for key in PROVIDER_KEYS {
        assert!(
            std::env::var_os(key).is_none(),
            "provider credential {key} reached the child"
        );
    }
    for key in ["LD_PRELOAD", "DYLD_INSERT_LIBRARIES", "GIT_SSH_COMMAND"] {
        assert!(
            std::env::var_os(key).is_none(),
            "execution override {key} reached the child"
        );
    }
    assert_eq!(std::env::var("PATH").unwrap(), "child-env-fixture-path");
    assert_eq!(std::env::var("GH_TOKEN").unwrap(), "fake-gh-token");
    assert_eq!(std::env::var("GITHUB_TOKEN").unwrap(), "fake-github-token");
    assert_eq!(std::env::var("GH_CONFIG_DIR").unwrap(), "fixture-gh-config");
    assert_eq!(std::env::var("XDG_CONFIG_HOME").unwrap(), "fixture-xdg-config");
    assert_eq!(std::env::var("GH_HOST").unwrap(), "github.example.invalid");
    assert_eq!(std::env::var("SSH_AUTH_SOCK").unwrap(), "fixture-ssh-agent");
    assert_eq!(std::env::var("APPDATA").unwrap(), "fixture-appdata");
    assert_eq!(std::env::var("LOCALAPPDATA").unwrap(), "fixture-localappdata");
}

#[test]
fn gh_child_excludes_inherited_provider_credentials() {
    isolated_parent("gh_parent_fixture");
}

#[test]
fn shim_child_excludes_inherited_provider_credentials() {
    isolated_parent("shim_parent_fixture");
}

#[test]
#[ignore = "subprocess fixture"]
fn gh_parent_fixture() {
    assert!(std::env::var_os("ANTHROPIC_API_KEY").is_some());
    let argv = fixture_argv("gh_child_fixture");
    let mut env = gh_env();
    env.insert("GH_PROMPT_DISABLED".into(), "1".into());
    let output = process::run(process::RunSpec {
        argv: &argv,
        cwd: None,
        env: Some(&env),
        timeout: Duration::from_secs(10),
        stdin: None,
    })
    .unwrap();
    assert_eq!(output.status, Some(0), "{}\n{}", output.stdout, output.stderr);
}

#[test]
#[ignore = "subprocess fixture"]
fn gh_child_fixture() {
    assert_no_provider_keys();
    for (key, expected) in [
        ("GH_TELEMETRY", "false"),
        ("GH_NO_UPDATE_NOTIFIER", "1"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("DO_NOT_TRACK", "1"),
        ("GH_PROMPT_DISABLED", "1"),
    ] {
        assert_eq!(std::env::var(key).unwrap(), expected, "{key}");
    }
}

#[tokio::test]
#[ignore = "subprocess fixture"]
async fn shim_parent_fixture() {
    assert!(std::env::var_os("ANTHROPIC_API_KEY").is_some());
    let argv = fixture_argv("shim_child_fixture");
    let (code, stdout, stderr) = shim::run_argv(&argv, &std::env::current_dir().unwrap(), Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(code, 0, "{stdout}\n{stderr}");
}

#[test]
#[ignore = "subprocess fixture"]
fn shim_child_fixture() {
    assert_no_provider_keys();
    for (key, expected) in [
        ("CGAGENTHARNESS_AGENT_COMMIT_NAME", "Fixture Author"),
        ("CGAGENTHARNESS_AGENT_COMMIT_EMAIL", "fixture@example.invalid"),
        ("CGAGENTHARNESS_AGENT_BRANCH_PREFIX", "fixture/"),
        ("CGAGENTHARNESS_AGENTIC_WRITE_DISABLE", "1"),
    ] {
        assert_eq!(std::env::var(key).unwrap(), expected, "{key}");
    }
}

#[test]
fn provider_credentials_are_action_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("probe.rs");
    let probe = dir
        .path()
        .join(format!("child_env_probe{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &source,
        r#"fn main() {
    for key in ["ANTHROPIC_API_KEY", "GROK_API_KEY", "SERPAPI_API_KEY", "CGAH_CHILD_ENV_PROBE"] {
        assert!(std::env::var_os(key).is_none(), "unexpected child variable {key}");
    }
    let action = std::env::args().nth(4).unwrap();
    let key = std::env::var_os("DEEPAGENT_API_KEY");
    assert_eq!(key.is_some(), action == "real-repo-run");
    println!("{{\"present\":{},\"empty\":{}}}", key.is_some(), key.is_some_and(|v| v.is_empty()));
}
"#,
    )
    .unwrap();
    let build = Command::new("rustc")
        .args(["--edition=2021", "--crate-name", "child_env_probe"])
        .arg(&source)
        .arg("-o")
        .arg(&probe)
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    for deepagent in ["fake-deepagent-key", ""] {
        let argv = fixture_argv("action_parent_fixture");
        let output = Command::new(&argv[0])
            .args(&argv[1..])
            .env("CGAH_CHILD_ENV_PROBE", &probe)
            .env("DEEPAGENT_API_KEY", deepagent)
            .env("ANTHROPIC_API_KEY", "fake-anthropic-key")
            .env("GROK_API_KEY", "fake-grok-key")
            .env("SERPAPI_API_KEY", "fake-serpapi-key")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[tokio::test]
#[ignore = "subprocess fixture"]
async fn action_parent_fixture() {
    let inherited_empty = std::env::var_os("DEEPAGENT_API_KEY").unwrap().is_empty();
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.yaml");
    std::fs::write(&config, "agentic:\n  enabled: false\n").unwrap();
    let mut ctx = shim::ShimContext::new(&config, dir.path(), &dir.path().join("tmp"), 720).unwrap();
    ctx.exe = std::env::var_os("CGAH_CHILD_ENV_PROBE").unwrap().into();
    for action in shim::ACTIONS {
        let req = shim::OpsRequest {
            action: action.into(),
            name: Some("fixture".into()),
            desc: Some("Fixture skill".into()),
            body: Some("Reviewed fixture body".into()),
            reason: Some("Verify child environment".into()),
            confirm: true,
            instruction: Some("Fixture instruction".into()),
            checks: Some(vec![serde_json::json!("cargo-test")]),
            branch: Some("agent/fixture".into()),
            commit_message: Some("Fixture commit".into()),
            run_id: Some("a".repeat(32)),
            decision: Some("approve".into()),
            ..Default::default()
        };
        let result = shim::run_agentic_op(&ctx, &req).await.unwrap();
        assert_eq!(result.exit_code, 0, "{action}: {}\n{}", result.stdout, result.stderr);
        assert!(result.ok, "{action}");
        let flags: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();
        assert_eq!(flags["present"], action == "real-repo-run", "{action}");
        assert_eq!(flags["empty"], action == "real-repo-run" && inherited_empty, "{action}");
    }
}
