//! Required native Windows acceptance. A named Job Object is not proof: observe
//! live detached descendants and retain process handles through every cleanup.
#[test]
fn required_platform() {
    if std::env::var_os("CGAH_REQUIRE_WINDOWS_MCP").is_some() {
        assert!(cfg!(windows));
    }
}

#[cfg(windows)]
mod windows {
    use std::collections::BTreeMap;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use cgagentharness::common::mcp::StdioClient;
    use cgagentharness::common::mcp_policy::{
        Containment, FilesystemPolicy, NetworkPolicy, ResourceLimits, StdioCapabilities,
    };
    use serde_json::{json, Value};
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};

    async fn client() -> StdioClient {
        let python = std::env::var("CGAH_WINDOWS_PYTHON").expect("native Python executable required");
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/mcp-windows-fixture.py");
        let policy = StdioCapabilities {
            version: 1,
            filesystem: FilesystemPolicy::Unrestricted,
            read_roots: vec![],
            write_roots: vec![],
            network: NetworkPolicy::Unrestricted,
            containment: Containment::JobObject,
            limits: Some(ResourceLimits {
                processes: 8,
                memory_mb: 256,
            }),
        };
        let mut client = StdioClient::spawn(
            &[python, fixture.display().to_string(), "stdio".into()],
            None,
            &BTreeMap::new(),
            65536,
            None,
            &policy,
            Path::new(env!("CARGO_BIN_EXE_cgagentharness")),
            Duration::from_secs(10),
        )
        .await
        .expect("required atomic Job Object spawn");
        assert_eq!(client.backend, "windows-job-object-unrestricted");
        client.initialize().await.unwrap();
        client
    }

    fn payload(result: Value) -> Value {
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn runner_fixture() {
        let Ok(directory) = std::env::var("CGAH_WINDOWS_FIXTURE_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory);
        let mode = std::env::var("CGAH_WINDOWS_FIXTURE_MODE").unwrap();
        let mut client = client().await;
        let result = payload(
            client
                .call_tool("windows_start", json!({"directory": directory}))
                .await
                .unwrap(),
        );
        assert_eq!(result["breakaway_denied"], true);
        assert_eq!(result["processes"], 8);
        assert_eq!(result["memory"], 256 * 1024 * 1024);
        let flags = result["flags"].as_u64().unwrap();
        assert_eq!(flags & (0x2000 | 0x8 | 0x200), 0x2208); // kill on close, active process, job memory
        assert_eq!(flags & (0x800 | 0x1000), 0); // neither breakaway flag
        std::fs::write(directory.join("ready.tmp"), serde_json::to_vec(&result).unwrap()).unwrap();
        std::fs::rename(directory.join("ready.tmp"), directory.join("ready")).unwrap();
        while !directory.join("proceed").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        match mode.as_str() {
            "drop" => drop(client),
            "cancel" => {
                let task = tokio::spawn(async move {
                    let _owned = client;
                    std::future::pending::<()>().await;
                });
                tokio::task::yield_now().await;
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            }
            "timeout" => {
                assert!(tokio::time::timeout(Duration::from_millis(200), async move {
                    client.call_tool("windows_hang", json!({})).await
                })
                .await
                .is_err());
            }
            "protocol" | "child_crash" => {
                let tool = if mode == "protocol" { "header_flood" } else { "crash" };
                assert!(client.call_tool(tool, json!({})).await.is_err());
                drop(client);
            }
            "process_limit" | "memory_limit" => {
                let tool = format!("windows_{mode}");
                let result = payload(client.call_tool(&tool, json!({"directory": directory})).await.unwrap());
                assert_eq!(result["limit_enforced"], true);
                if mode == "memory_limit" {
                    assert_eq!(result["positive"], 1);
                }
                drop(client);
            }
            "runner_exit" => std::process::exit(0), // no destructors; OS must close the job
            "runner_kill" => {
                tokio::time::sleep(Duration::from_secs(60)).await;
                drop(client);
            }
            _ => panic!("unknown mode"),
        }
    }

    struct Runner(Child);
    impl Drop for Runner {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn until(what: &str, mut predicate: impl FnMut() -> bool) {
        let start = Instant::now();
        while !predicate() {
            assert!(start.elapsed() < Duration::from_secs(15), "timed out: {what}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    fn process(pid: u32) -> OwnedHandle {
        // SAFETY: only observing a fixture PID, no inheritance or mutation.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!handle.is_null(), "fixture process must be alive");
        unsafe { OwnedHandle::from_raw_handle(handle) }
    }
    fn state(handle: &OwnedHandle) -> u32 {
        // SAFETY: retained process handle, nonblocking observation.
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) }
    }

    #[test]
    fn job_reclaims_detached_descendants_on_every_exit_path() {
        if std::env::var_os("CGAH_REQUIRE_WINDOWS_MCP").is_none() {
            eprintln!("native Windows acceptance belongs to the required runner");
            return;
        }
        for mode in [
            "drop",
            "cancel",
            "timeout",
            "protocol",
            "child_crash",
            "runner_exit",
            "runner_kill",
            "process_limit",
            "memory_limit",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut runner = Runner(
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "windows::runner_fixture", "--nocapture"])
                    .env("CGAH_WINDOWS_FIXTURE_DIR", directory.path())
                    .env("CGAH_WINDOWS_FIXTURE_MODE", mode)
                    .stdin(Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            until("fixture ready", || {
                assert!(
                    runner.0.try_wait().unwrap().is_none(),
                    "fixture exited before ready: {mode}"
                );
                directory.path().join("ready").exists()
            });
            let ready: Value = serde_json::from_slice(&std::fs::read(directory.path().join("ready")).unwrap()).unwrap();
            let processes: Vec<_> = ready["pids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pid| process(pid.as_u64().unwrap() as u32))
                .collect();
            assert_eq!(processes.len(), 3);
            assert!(processes.iter().all(|p| state(p) == WAIT_TIMEOUT));
            let heartbeat = directory.path().join("heartbeat");
            until("first heartbeat", || heartbeat.exists());
            let initial = std::fs::read(&heartbeat).unwrap();
            until("active detached grandchild", || {
                std::fs::read(&heartbeat).is_ok_and(|b| b != initial)
            });
            std::fs::write(directory.path().join("proceed"), b"go").unwrap();
            if mode == "runner_kill" {
                runner.0.kill().unwrap();
            }
            until("all descendants stopped", || {
                processes.iter().all(|p| state(p) == WAIT_OBJECT_0)
            });
            until("runner exit", || runner.0.try_wait().unwrap().is_some());
            if mode != "runner_kill" {
                assert!(runner.0.wait().unwrap().success(), "runner failed: {mode}");
            }
            let stopped = std::fs::read(&heartbeat).unwrap();
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(std::fs::read(&heartbeat).unwrap(), stopped);
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                let mut fresh = client().await;
                assert_eq!(
                    payload(fresh.call_tool("echo", json!({"recovered": true})).await.unwrap())["echo"]["recovered"],
                    true
                );
            });
            eprintln!("Windows Job Object acceptance passed: {mode}");
        }
    }
}
