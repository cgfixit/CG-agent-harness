//! Native systemd/cgroup acceptance. Required CI must execute, never classify a
//! missing containment backend as a successful isolation test.
#[test]
fn required_lifecycle_platform() {
    if std::env::var_os("CGAH_REQUIRE_LINUX_LIFECYCLE").is_some() {
        assert!(cfg!(target_os = "linux"));
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use cgagentharness::common::mcp::StdioClient;
    use cgagentharness::common::mcp_policy::{Containment, NetworkPolicy, ResourceLimits, StdioCapabilities};
    use serde_json::{json, Value};

    const BIN: &str = env!("CARGO_BIN_EXE_cgagentharness");

    async fn client(directory: &Path, seconds: u64) -> StdioClient {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/mcp-fixture.py");
        let argv = vec![
            "/usr/bin/python3".into(),
            fixture.display().to_string(),
            "--stdio".into(),
        ];
        let policy = StdioCapabilities {
            version: 1,
            read_roots: vec![fixture],
            write_roots: vec![directory.into()],
            network: NetworkPolicy::Deny,
            containment: Containment::Strict,
            limits: Some(ResourceLimits {
                processes: 32,
                memory_mb: 256,
            }),
        };
        let mut client = StdioClient::spawn(
            &argv,
            None,
            &BTreeMap::new(),
            65536,
            None,
            &policy,
            Path::new(BIN),
            Duration::from_secs(seconds),
        )
        .await
        .expect("required strict service");
        assert_eq!(client.backend, "linux-systemd-bwrap");
        client.initialize().await.unwrap();
        client
    }

    fn payload(result: Value) -> Value {
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn lifecycle_runner_fixture() {
        let Ok(directory) = std::env::var("CGAH_LIFECYCLE_FIXTURE_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory);
        let mode = std::env::var("CGAH_LIFECYCLE_FIXTURE_MODE").unwrap();
        let mut client = client(&directory, if mode == "timeout" { 4 } else { 20 }).await;
        let result = payload(
            client
                .call_tool("lifecycle_start", json!({"directory": directory}))
                .await
                .unwrap(),
        );
        assert_eq!(result["migration_denied"], true);
        std::fs::write(directory.join("ready.tmp"), serde_json::to_vec(&result).unwrap()).unwrap();
        std::fs::rename(directory.join("ready.tmp"), directory.join("ready")).unwrap();
        // Let the parent observe both a populated cgroup and a changing heartbeat
        // before triggering cleanup. This guards against a vacuous deny test.
        while !directory.join("proceed").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        match mode.as_str() {
            "drop" => drop(client),
            "cancel" => {
                let handle = tokio::spawn(async move {
                    let _owned = client;
                    std::future::pending::<()>().await;
                });
                tokio::task::yield_now().await;
                handle.abort();
                assert!(handle.await.unwrap_err().is_cancelled());
            }
            "protocol" => {
                assert!(client.call_tool("header_flood", json!({})).await.is_err());
                drop(client);
            }
            "crash" => {
                assert!(client.call_tool("crash", json!({})).await.is_err());
                drop(client);
            }
            "process_limit" => {
                let result = payload(client.call_tool("process_limit", json!({})).await.unwrap());
                assert_eq!(result["limit_enforced"], true);
                assert!(result["created"].as_u64().unwrap() > 0);
                drop(client);
            }
            "timeout" | "runner_kill" | "supervisor_kill" => {
                tokio::time::sleep(Duration::from_secs(25)).await;
                drop(client);
            }
            _ => panic!("unknown fixture mode"),
        }
    }

    struct OwnedRunner(Child);
    impl Drop for OwnedRunner {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn wait_until(what: &str, seconds: u64, mut check: impl FnMut() -> bool) {
        let started = Instant::now();
        while !check() {
            assert!(started.elapsed() < Duration::from_secs(seconds), "timed out: {what}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    #[test]
    fn strict_cgroup_reclaims_escaped_descendants_on_every_exit_path() {
        if std::env::var_os("CGAH_REQUIRE_LINUX_LIFECYCLE").is_none() {
            eprintln!("native lifecycle acceptance belongs to the required systemd runner");
            return;
        }
        for mode in [
            "drop",
            "cancel",
            "protocol",
            "crash",
            "timeout",
            "runner_kill",
            "supervisor_kill",
            "process_limit",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let log = std::fs::File::create(directory.path().join("runner.log")).unwrap();
            let mut runner = OwnedRunner(
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "linux::lifecycle_runner_fixture", "--nocapture"])
                    .env("CGAH_LIFECYCLE_FIXTURE_DIR", directory.path())
                    .env("CGAH_LIFECYCLE_FIXTURE_MODE", mode)
                    .stdout(log.try_clone().unwrap())
                    .stderr(log)
                    .stdin(Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            wait_until("fixture started", 12, || {
                if let Some(status) = runner.0.try_wait().unwrap() {
                    panic!(
                        "fixture {mode} exited {status}: {}",
                        std::fs::read_to_string(directory.path().join("runner.log")).unwrap()
                    );
                }
                directory.path().join("ready").exists() && directory.path().join("heartbeat").exists()
            });
            let ready: Value = serde_json::from_slice(&std::fs::read(directory.path().join("ready")).unwrap()).unwrap();
            let group_path = ready["group"].as_str().unwrap();
            assert!(group_path.starts_with('/') && !group_path.contains(".."));
            let group = Path::new("/sys/fs/cgroup").join(group_path.trim_start_matches('/'));
            let unit = group.file_name().unwrap().to_str().unwrap();
            assert!(unit.starts_with("cgah-mcp-") && unit.ends_with(".service"));
            let pids = std::fs::read_to_string(group.join("cgroup.procs")).unwrap();
            assert!(
                pids.lines().count() >= 4,
                "fixture must actually spawn descendants: {pids}"
            );
            assert_eq!(std::fs::read_to_string(group.join("pids.max")).unwrap().trim(), "32");
            assert_eq!(
                std::fs::read_to_string(group.join("memory.max")).unwrap().trim(),
                "268435456"
            );
            let heartbeat = std::fs::read(directory.path().join("heartbeat")).unwrap();
            wait_until("escaped grandchild running", 2, || {
                std::fs::read(directory.path().join("heartbeat")).unwrap_or_default() != heartbeat
            });
            std::fs::write(directory.path().join("proceed"), b"go").unwrap();
            if mode == "runner_kill" {
                runner.0.kill().unwrap();
            }
            if mode == "supervisor_kill" {
                let mut cmd = Command::new("systemctl");
                // SAFETY: read-only UID lookup, matches the product manager selection.
                if unsafe { libc::geteuid() } != 0 {
                    cmd.arg("--user");
                }
                let out = cmd
                    .args(["show", "--property=MainPID", "--value", unit])
                    .output()
                    .unwrap();
                assert!(out.status.success());
                let pid: i32 = String::from_utf8(out.stdout).unwrap().trim().parse().unwrap();
                let pid_text = pid.to_string();
                assert!(pid > 1 && pids.lines().any(|line| line == pid_text));
                // SAFETY: exact main PID of this fixture's independently identified service.
                assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
            }
            wait_until("empty service cgroup", 8, || {
                std::fs::read_to_string(group.join("cgroup.procs")).map_or(true, |s| s.trim().is_empty())
            });
            let final_heartbeat = std::fs::read(directory.path().join("heartbeat")).unwrap();
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(
                std::fs::read(directory.path().join("heartbeat")).unwrap(),
                final_heartbeat
            );
            if !matches!(mode, "timeout" | "runner_kill" | "supervisor_kill") {
                wait_until("runner completed", 3, || runner.0.try_wait().unwrap().is_some());
                assert!(
                    runner.0.wait().unwrap().success(),
                    "{}",
                    std::fs::read_to_string(directory.path().join("runner.log")).unwrap()
                );
            }
            eprintln!("strict lifecycle: {mode}: live escaped descendants reclaimed");
            // A new service must work after each failure, not merely kill the old one.
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let mut next = client(directory.path(), 10).await;
                assert_eq!(
                    payload(next.call_tool("echo", json!({"recovery":mode})).await.unwrap())["echo"]["recovery"],
                    mode
                );
            });
        }
    }
}
