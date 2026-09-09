//! Required native macOS gate: no capability skip and no model double.
#![cfg(target_os = "macos")]
mod common;
use cgagentharness::{
    agentic::executor::{run_verification, Check},
    common::audit::Audit,
};
use std::{path::Path, process::Command};

fn prepare(repo: &Path, home: &Path) {
    let out = Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/prepare-cargo.py"))
        .args([repo, home])
        .env("CARGO_NET_OFFLINE", "true")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "offline preparation failed; prefetch fixture dependencies before tests: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn cargo_test() -> Check {
    Check::with_timeout("cargo-test", vec!["cargo".into(), "test".into(), "--quiet".into()], 60).unwrap()
}

#[test]
fn macos_cargo_compiles_dependencies_build_script_tests_and_doctests_offline() {
    let dir = tempfile::Builder::new().prefix("cargo sandbox '").tempdir().unwrap();
    let repo = dir.path().join("candidate");
    let source = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cargo-sandbox"));
    for entry in walkdir::WalkDir::new(source) {
        let entry = entry.unwrap();
        let target = repo.join(entry.path().strip_prefix(source).unwrap());
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
    let home = dir.path().join("app-home-préparé");
    std::fs::create_dir(&home).unwrap();
    let cfg = common::config_with(&home, &[]);
    let audit = Audit::new(home.join("audit.jsonl"), &cfg);
    let prepared = home.join("data/agentic/cargo-prepared");
    let before_lock = std::fs::read(repo.join("Cargo.lock")).unwrap();
    let missing = run_verification(&repo, &[cargo_test()], &audit, None, Some(&prepared)).unwrap_err();
    assert!(missing.message.contains("not prepared"));
    prepare(&repo, &home);
    let hash = cgagentharness::common::sha256_bytes_hex(&before_lock);
    let sentinel = prepared.join(&hash).join("vendor-sentinel");
    std::fs::write(&sentinel, "immutable dependency marker").unwrap();
    let secret = dir.path().join("synthetic-sensitive");
    std::fs::write(&secret, "synthetic only; never a real secret").unwrap();
    std::os::unix::fs::symlink(&secret, repo.join("sensitive-alias")).unwrap();
    std::fs::create_dir_all(repo.join(".git/hooks")).unwrap();
    std::fs::create_dir_all(repo.join("nested/.git")).unwrap();
    for path in [".git/config", ".git/index", "nested/.git/config"] {
        std::fs::write(repo.join(path), "original").unwrap();
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    std::fs::write(
        repo.join("probe-paths.txt"),
        format!(
            "{}\n{}\n{}\n{}\n",
            secret.display(),
            sentinel.display(),
            dir.path().join("outside-write").display(),
            listener.local_addr().unwrap()
        ),
    )
    .unwrap();
    let original_source = std::fs::read(repo.join("src/lib.rs")).unwrap();
    for _ in 0..2 {
        let report = run_verification(&repo, &[cargo_test()], &audit, None, Some(&prepared)).unwrap();
        assert!(report.ok, "real native Cargo failed: {report:?}");
        assert!(
            report.results[0].stdout.contains("1 passed"),
            "must run tests, not only compile"
        );
        assert!(
            !report.results[0].stderr.contains("couldn't create cache"),
            "SDK/toolchain preparation must avoid host cache writes"
        );
    }
    assert_eq!(std::fs::read(repo.join("Cargo.lock")).unwrap(), before_lock);
    assert_eq!(std::fs::read(repo.join("src/lib.rs")).unwrap(), original_source);
    assert_eq!(
        std::fs::read_to_string(&sentinel).unwrap(),
        "immutable dependency marker"
    );
    assert_eq!(std::fs::read_to_string(repo.join(".git/index")).unwrap(), "original");
    assert!(!repo.join(".git/hooks/probe").exists());
    assert!(!dir.path().join("outside-write").exists());
    assert!(!repo.join("target").exists());
    assert!(!repo.join("renamed-git").exists());
    assert!(listener.accept().is_err());
    std::fs::remove_file(prepared.join(&hash).join("vendor/serde-1.0.229/Cargo.toml")).unwrap();
    let unavailable = run_verification(&repo, &[cargo_test()], &audit, None, Some(&prepared)).unwrap_err();
    assert!(unavailable.message.contains("not prepared"), "{unavailable:?}");
    // Stale lock refuses before any execution or model retries.
    std::fs::write(repo.join("Cargo.lock"), "changed").unwrap();
    assert!(run_verification(&repo, &[cargo_test()], &audit, None, Some(&prepared))
        .unwrap_err()
        .message
        .contains("no snapshot"));
}

#[test]
fn macos_minimal_cargo_uses_fresh_writable_scratch_and_missing_component_is_actionable() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname='minimal'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(repo.join("src/lib.rs"), "#[test] fn works() { assert_eq!(1+1,2); }\n").unwrap();
    let out = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(out.status.success());
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let cfg = common::config_with(&home, &[]);
    let audit = Audit::new(home.join("audit.jsonl"), &cfg);
    prepare(&repo, &home);
    let prepared = home.join("data/agentic/cargo-prepared");
    let scratch = Check::new(
        "scratch",
        vec![
            "sh".into(),
            "-c".into(),
            "echo \"$HOME\"; echo owned > \"$HOME/check-marker\"".into(),
        ],
    )
    .unwrap();
    let report = run_verification(&repo, &[cargo_test(), scratch], &audit, None, Some(&prepared)).unwrap();
    assert!(report.ok, "{report:?}");
    assert!(
        !Path::new(report.results[1].stdout.trim()).exists(),
        "scratch must be removed after verification"
    );
    let hash = cgagentharness::common::sha256_bytes_hex(&std::fs::read(repo.join("Cargo.lock")).unwrap());
    let metadata = prepared.join(hash).join("prepared.json");
    let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(&metadata).unwrap()).unwrap();
    value["toolchain_root"] = serde_json::json!(dir.path().join("absent-toolchain"));
    std::fs::write(metadata, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(run_verification(&repo, &[cargo_test()], &audit, None, Some(&prepared))
        .unwrap_err()
        .message
        .contains("toolchain disappeared"));
}
