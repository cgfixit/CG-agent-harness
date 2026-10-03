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
        .env("GH_TELEMETRY", "true")
        .env("GH_NO_UPDATE_NOTIFIER", "0")
        .env("GIT_TERMINAL_PROMPT", "1")
        .env("DO_NOT_TRACK", "0")
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
    assert_eq!(std::env::var("PATH").unwrap(), "child-env-fixture-path");
    assert_eq!(std::env::var("GH_TOKEN").unwrap(), "fake-gh-token");
    assert_eq!(std::env::var("GITHUB_TOKEN").unwrap(), "fake-github-token");
    assert_eq!(std::env::var("GH_CONFIG_DIR").unwrap(), "fixture-gh-config");
    assert_eq!(std::env::var("XDG_CONFIG_HOME").unwrap(), "fixture-xdg-config");
    assert_eq!(std::env::var("GH_HOST").unwrap(), "github.example.invalid");
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
