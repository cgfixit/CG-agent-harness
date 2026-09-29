//! Lock the netconnect workflow's hardened shape.
//!
//! Top-level `permissions` is an empty mapping. Actions use the same full
//! commit SHAs as the other workflows. There is no `pull_request_target`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_yaml_ng::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn workflow() -> String {
    std::fs::read_to_string(repo_root().join(".github/workflows/netconnect.yml")).expect("netconnect workflow")
}

fn pinned_shas() -> BTreeSet<String> {
    let mut shas = BTreeSet::new();
    let dir = repo_root().join(".github/workflows");
    for entry in std::fs::read_dir(&dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "netconnect.yml" {
            continue;
        }
        let text = std::fs::read_to_string(entry.path()).unwrap();
        for line in text.lines() {
            let Some(at) = line.find('@') else { continue };
            let rest = &line[at + 1..];
            let sha: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
            if sha.len() == 40 {
                shas.insert(sha);
            }
        }
    }
    shas
}

#[test]
fn top_level_permissions_are_empty_and_the_trigger_is_not_pull_request_target() {
    let text = workflow();
    assert!(
        !text.contains("pull_request_target"),
        "the target event must not appear, including in comments"
    );
    let doc: Value = serde_yaml_ng::from_str(&text).expect("workflow yaml");
    let triggers = doc
        .get("on")
        .or_else(|| doc.get(Value::Bool(true)))
        .expect("workflow triggers");
    assert!(triggers.get("pull_request_target").is_none());
    assert!(triggers.get("pull_request").is_some());
    assert!(triggers.get("push").is_some());
    assert!(triggers.get("workflow_dispatch").is_some());
    // Same push scope as ci.yml: a PR-branch push must not start a second run.
    let push_branches = triggers
        .get("push")
        .and_then(|push| push.get("branches"))
        .and_then(Value::as_sequence)
        .expect("push branches");
    let push_branch_names: Vec<&str> = push_branches.iter().filter_map(Value::as_str).collect();
    assert_eq!(push_branch_names, vec!["main"]);
    assert!(
        triggers
            .get("pull_request")
            .and_then(|event| event.get("branches"))
            .is_none(),
        "pull_request stays unscoped by branch"
    );
    let permissions = doc.get("permissions").expect("top-level permissions");
    assert!(
        permissions.as_mapping().is_some_and(|map| map.is_empty()),
        "top-level permissions must be {{}}"
    );

    let jobs = doc.get("jobs").expect("jobs");
    assert_eq!(jobs.as_mapping().map(serde_yaml_ng::Mapping::len), Some(2));
    let flags = jobs.get("flags-and-scope").expect("flags-and-scope");
    let syscall = jobs.get("syscall-proof").expect("syscall-proof");
    for job in [flags, syscall] {
        assert!(job.get("timeout-minutes").and_then(Value::as_i64).is_some(), "{job:?}");
        assert!(job.get("concurrency").and_then(Value::as_mapping).is_some(), "{job:?}");
        let perms = job.get("permissions").expect("job permissions");
        assert_eq!(perms.as_mapping().map(serde_yaml_ng::Mapping::len), Some(1));
        assert_eq!(perms.get("contents").and_then(Value::as_str), Some("read"));
    }

    let os = flags
        .get("strategy")
        .and_then(|value| value.get("matrix"))
        .and_then(|value| value.get("os"))
        .and_then(Value::as_sequence)
        .expect("matrix os");
    let names: Vec<&str> = os.iter().filter_map(Value::as_str).collect();
    assert_eq!(names, vec!["ubuntu-latest", "macos-latest"]);
    assert_eq!(syscall.get("runs-on").and_then(Value::as_str), Some("ubuntu-latest"));
}

#[test]
fn path_filters_cover_both_events_and_actions_are_pinned_shas() {
    let text = workflow();
    for needle in [
        "src/netconnect/**",
        "tests/netconnect*",
        "tests/netconnect_support/**",
        ".github/workflows/netconnect.yml",
        "scripts/netconnect-syscall-proof.sh",
    ] {
        assert!(
            text.matches(needle).count() >= 2,
            "{needle} must be on both pull_request and push"
        );
    }
    assert!(text.contains("workflow_dispatch:"));
    assert_eq!(
        text.matches("persist-credentials: false").count(),
        text.matches("actions/checkout@").count()
    );
    assert!(text.contains("bash scripts/netconnect-syscall-proof.sh --binary target/debug/cgagentharness"));
    assert!(!text.contains("continue-on-error"));

    let known = pinned_shas();
    assert!(!known.is_empty());
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("uses:") {
            let uses = rest.trim();
            let sha = uses
                .split('@')
                .nth(1)
                .and_then(|value| value.split_whitespace().next())
                .unwrap_or("");
            assert_eq!(sha.len(), 40, "{uses}");
            assert!(sha.chars().all(|c| c.is_ascii_hexdigit()), "{uses}");
            assert!(known.contains(sha), "{sha} is not already used in this repo");
        }
    }
}
