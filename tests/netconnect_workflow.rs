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
    assert_eq!(syscall.get("runs-on").and_then(Value::as_str), Some("ubuntu-26.04"));
    assert_syscall_proof_cannot_skip(syscall);
}

fn assert_syscall_proof_cannot_skip(syscall: &Value) {
    assert!(
        syscall.get("continue-on-error").is_none(),
        "syscall-proof must fail the job when unshare or strace cannot run"
    );
    assert!(
        syscall.get("if").is_none(),
        "syscall-proof must not skip on runners that restrict user namespaces"
    );
    let steps = syscall
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("syscall-proof steps");
    assert!(!steps.is_empty());
    for step in steps {
        assert!(step.get("continue-on-error").is_none(), "{step:?}");
        assert!(step.get("if").is_none(), "{step:?}");
    }
}

#[test]
fn path_filters_cover_both_events_and_actions_are_pinned_shas() {
    let text = workflow();
    // Read each event's parsed `paths` list: a raw text count also matches a
    // run step or comment that names a file, and hides a dropped filter.
    let doc: Value = serde_yaml_ng::from_str(&text).expect("workflow yaml");
    let triggers = doc
        .get("on")
        .or_else(|| doc.get(Value::Bool(true)))
        .expect("workflow triggers");
    let paths = |event: &str| -> Vec<String> {
        triggers
            .get(event)
            .and_then(|e| e.get("paths"))
            .and_then(Value::as_sequence)
            .unwrap_or_else(|| panic!("{event} paths"))
            .iter()
            .map(|p| p.as_str().expect("path filter string").to_string())
            .collect()
    };
    let (pull_request, push) = (paths("pull_request"), paths("push"));
    assert_eq!(pull_request, push, "pull_request and push must filter the same paths");
    for needle in [
        "src/netconnect/**",
        "tests/netconnect*",
        "tests/netconnect_support/**",
        ".github/workflows/netconnect.yml",
        "scripts/netconnect-syscall-proof.sh",
        "assets/config.default.yaml",
        "src/main.rs",
        "src/lib.rs",
        "src/common/config.rs",
        "src/common/credential_store.rs",
        "src/common/errors.rs",
        "src/common/home.rs",
        "src/common/process.rs",
        "src/common/process/**",
        "Cargo.toml",
        "Cargo.lock",
    ] {
        assert!(
            pull_request.iter().any(|p| p == needle),
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

    // Walk the parsed steps: a scan for lines that start with `uses:` never
    // matched the `- uses:` step form, so it checked nothing and passed.
    let known = pinned_shas();
    assert!(!known.is_empty());
    let mut checked = 0;
    for job in doc.get("jobs").and_then(Value::as_mapping).expect("jobs").values() {
        let steps = job.get("steps").and_then(Value::as_sequence).expect("job steps");
        for uses in steps.iter().filter_map(|step| step.get("uses")) {
            let uses = uses.as_str().expect("uses string");
            let sha = uses.split_once('@').map_or("", |(_, sha)| sha);
            assert_eq!(sha.len(), 40, "{uses}");
            assert!(sha.chars().all(|c| c.is_ascii_hexdigit()), "{uses}");
            assert!(known.contains(sha), "{sha} is not already used in this repo");
            checked += 1;
        }
    }
    assert!(checked > 0, "no step `uses:` was checked");
}
