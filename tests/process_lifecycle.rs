//! Harmless process ownership regressions; no real repository or secret access.
#![cfg(unix)]
use cgagentharness::{common::process, shim};
use std::time::{Duration, Instant};

#[test]
fn inherited_pipe_cannot_outlive_the_operation_deadline() {
    let argv = vec!["/bin/sh".into(), "-c".into(), "sleep 2 & exit 0".into()];
    let start = Instant::now();
    let result = process::run(process::RunSpec {
        argv: &argv,
        cwd: None,
        env: None,
        timeout: Duration::from_millis(100),
        stdin: None,
    });
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "pipe drain escaped the deadline"
    );
    assert!(result.is_err(), "incomplete pipe capture cannot be success");
}

#[test]
fn early_leader_exit_does_not_orphan_an_ordinary_background_child() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("pid");
    let argv = vec![
        "/bin/sh".into(),
        "-c".into(),
        "/bin/sleep 0.02; /bin/sleep 5 & echo $! > \"$1\"; exit 0".into(),
        "fixture".into(),
        marker.display().to_string(),
    ];
    let result = process::run(process::RunSpec {
        argv: &argv,
        cwd: None,
        env: None,
        timeout: Duration::from_secs(1),
        stdin: None,
    });
    assert!(result.is_err());
    let pid = std::fs::read_to_string(marker).unwrap().trim().parse::<i32>().unwrap();
    // Group cleanup already ran before `run` returned; poll for the kernel to
    // reflect it instead of trusting one fixed sleep on a loaded runner.
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut state = String::new();
    let mut stopped = false;
    while !stopped && Instant::now() < deadline {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        stopped = state.is_empty() || state.starts_with('Z');
        if !stopped {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    // Independent test-owned cleanup if the regression fails.
    if !stopped {
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    assert!(stopped, "same-group descendant survived: {state}");
}

#[tokio::test]
async fn shim_pipe_drain_is_inside_its_deadline() {
    let argv = vec!["/bin/sh".into(), "-c".into(), "sleep 2 & exit 0".into()];
    let start = Instant::now();
    let result = shim::run_argv(&argv, std::path::Path::new("/tmp"), Duration::from_millis(100)).await;
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "shim pipe drain escaped the deadline"
    );
    assert!(result.is_err());
}

#[test]
fn stdin_backpressure_is_inside_the_deadline() {
    let argv = vec!["/bin/sleep".into(), "2".into()];
    let bytes = vec![b'x'; 2 * 1024 * 1024];
    let start = Instant::now();
    let result = process::run(process::RunSpec {
        argv: &argv,
        cwd: None,
        env: None,
        timeout: Duration::from_millis(100),
        stdin: Some(&bytes),
    });
    assert!(matches!(result, Err(process::ProcessError::Timeout { .. })));
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn output_overflow_is_a_failure_in_both_runners() {
    let argv = vec!["/usr/bin/yes".into(), "bounded-output".into()];
    let sync = process::run(process::RunSpec {
        argv: &argv,
        cwd: None,
        env: None,
        timeout: Duration::from_secs(5),
        stdin: None,
    });
    assert!(matches!(sync, Err(process::ProcessError::Capture(ref e)) if e.contains("output exceeded")));
    let asynchronous = shim::run_argv(&argv, std::path::Path::new("/tmp"), Duration::from_secs(5)).await;
    assert!(matches!(asynchronous, Err(shim::ShimError::Io(ref e)) if e.contains("output exceeded")));
    let control = vec![
        "/bin/sh".into(),
        "-c".into(),
        "printf '{\"ok\":true}'; printf diagnostic >&2; exit 7".into(),
    ];
    let (code, stdout, stderr) = shim::run_argv(&control, std::path::Path::new("/tmp"), Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(
        (code, stdout.as_str(), stderr.as_str()),
        (7, "{\"ok\":true}", "diagnostic")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn native_nested_process_fixture() {
    let Ok(marker) = std::env::var("CGAH_LIFECYCLE_FIXTURE") else {
        return;
    };
    let root = std::path::Path::new(&marker).parent().unwrap();
    std::fs::write(root.join("wrapper-pid"), std::process::id().to_string()).unwrap();
    let sandbox = cgagentharness::agentic::executor::sandbox::production_sandbox().expect("native sandbox required");
    let env = std::collections::BTreeMap::from([
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("HOME".into(), root.display().to_string()),
        ("TMPDIR".into(), root.display().to_string()),
    ]);
    let argv = vec![
        "/bin/sh".into(),
        "-c".into(),
        "echo $$ > \"$1\"; /bin/sleep 30".into(),
        "fixture".into(),
        marker.clone(),
    ];
    let candidate = root.parent().unwrap().join("candidate");
    let outcome = sandbox.run_prepared(&argv, &candidate, &env, 30, root, &[]);
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
}

#[cfg(target_os = "macos")]
fn running(pid: i32) -> bool {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    // SAFETY: correctly sized writable process-information buffer.
    let count = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            std::mem::size_of::<libc::proc_bsdinfo>() as i32,
        )
    };
    if count != std::mem::size_of::<libc::proc_bsdinfo>() as i32 {
        return false;
    }
    // SAFETY: the API returned a complete structure; SZOMB=5 has no running work.
    unsafe { info.assume_init().pbi_status != 5 }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn job_cancel_stops_observed_nested_sandbox_groups_and_preserves_a_sibling() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("scratch")).unwrap();
    std::fs::create_dir(tmp.path().join("candidate")).unwrap();
    let marker = tmp.path().join("scratch/check-pid");
    let exe = std::env::current_exe().unwrap();
    let argv = vec![
        "/usr/bin/env".into(),
        format!("CGAH_LIFECYCLE_FIXTURE={}", marker.display()),
        exe.display().to_string(),
        "--exact".into(),
        "native_nested_process_fixture".into(),
        "--nocapture".into(),
    ];
    let mut sibling = std::process::Command::new("/bin/sleep").arg("10").spawn().unwrap();
    let sibling_pid = sibling.id() as i32;
    let task = tokio::spawn(async move {
        let _ = shim::run_argv(&argv, std::path::Path::new("/tmp"), Duration::from_secs(30)).await;
    });
    let store = cgagentharness::server::agent_jobs::JobStore::new();
    store.insert_running("local", "fixture", "real-repo-run", task);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let check = std::fs::read_to_string(&marker)
        .expect("actual native sandbox check must start")
        .trim()
        .parse::<i32>()
        .unwrap();
    let wrapper = std::fs::read_to_string(tmp.path().join("scratch/wrapper-pid"))
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    // SAFETY: process-group queries only, for test-owned processes.
    assert_ne!(unsafe { libc::getpgid(check) }, unsafe { libc::getpgid(wrapper) });
    assert!(running(check) && running(wrapper));
    assert_eq!(store.cancel("local", "fixture").unwrap()["status"], "cancelled");
    let deadline = Instant::now() + Duration::from_secs(2);
    while (running(check) || running(wrapper)) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let stopped = !running(check) && !running(wrapper);
    let sibling_survived = running(sibling_pid);
    // Independent cleanup for a failing regression, using only fixture-owned PIDs.
    unsafe {
        libc::kill(check, libc::SIGKILL);
        libc::kill(wrapper, libc::SIGKILL);
    }
    let _ = sibling.kill();
    let _ = sibling.wait();
    assert!(stopped, "nested sandbox group survived job cancellation");
    assert!(sibling_survived, "cleanup must not target a sibling process");
    assert_eq!(store.get("local", "fixture").unwrap()["status"], "cancelled");
}

// #304 E1 contract: every unix runner child leads its own session (setsid), so
// a controlling terminal is never inherited and pgid == sid == pid keeps
// whole-group timeout and cancel cleanup working.

const SESSION_PROBE: &str = "import os; print(os.getpid(), os.getpgid(0), os.getsid(0))";

fn session_triple(stdout: &str) -> (i32, i32, i32) {
    let v: Vec<i32> = stdout.split_whitespace().map(|n| n.parse().unwrap()).collect();
    assert_eq!(v.len(), 3, "probe output: {stdout:?}");
    (v[0], v[1], v[2])
}

fn own_sid() -> i32 {
    // SAFETY: session query for the calling process only.
    unsafe { libc::getsid(0) }
}

/// Poll until `pid` is gone or a zombie; SIGKILL it ourselves if it survives.
fn gone_or_reap(pid: i32) -> (bool, String) {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut state = String::new();
    while Instant::now() < deadline {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if state.is_empty() || state.starts_with('Z') {
            return (true, state);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // SAFETY: test-owned fixture pid; cleanup only when the regression fails.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    (false, state)
}

#[test]
fn run_child_leads_its_own_session() {
    let argv = vec!["python3".into(), "-c".into(), SESSION_PROBE.into()];
    let out = process::run(process::RunSpec {
        argv: &argv,
        cwd: None,
        env: None,
        timeout: Duration::from_secs(10),
        stdin: None,
    })
    .expect("session probe runs");
    assert_eq!(out.status, Some(0), "{out:?}");
    let (pid, pgid, sid) = session_triple(&out.stdout);
    assert_eq!(sid, pid, "child must be a session leader (setsid), not a group in our session");
    assert_eq!(pgid, pid, "group cleanup relies on pgid == pid");
    assert_ne!(sid, own_sid(), "child must not share the harness session");
}

#[tokio::test]
async fn shim_child_leads_its_own_session() {
    let argv = vec!["python3".into(), "-c".into(), SESSION_PROBE.into()];
    let (code, stdout, stderr) = shim::run_argv(&argv, std::path::Path::new("/tmp"), Duration::from_secs(10))
        .await
        .expect("session probe runs");
    assert_eq!(code, 0, "{stderr}");
    let (pid, pgid, sid) = session_triple(&stdout);
    assert_eq!((sid, pgid), (pid, pid), "shim child must lead its own session and group");
    assert_ne!(sid, own_sid());
}

const GRANDCHILD: &str = "import os, sys, time\n\
with open(sys.argv[1] + '.tmp', 'w') as f:\n    f.write(f'{os.getpid()} {os.getsid(0)}')\n\
os.rename(sys.argv[1] + '.tmp', sys.argv[1])\n\
time.sleep(30)\n";

fn grandchild_argv(marker: &std::path::Path, leader: &std::path::Path) -> Vec<String> {
    vec![
        "/bin/sh".into(),
        "-c".into(),
        "echo $$ > \"$2\"; python3 -c \"$3\" \"$1\" & wait".into(),
        "fixture".into(),
        marker.display().to_string(),
        leader.display().to_string(),
        GRANDCHILD.into(),
    ]
}

fn read_pair(path: &std::path::Path) -> (i32, i32) {
    let text = std::fs::read_to_string(path).unwrap();
    let (a, b) = text.trim().split_once(' ').expect("pid sid");
    (a.parse().unwrap(), b.parse().unwrap())
}

#[test]
fn setsid_child_timeout_still_kills_a_grandchild_in_its_session() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("grandchild");
    let leader = tmp.path().join("leader");
    let argv = grandchild_argv(&marker, &leader);
    let result = process::run(process::RunSpec {
        argv: &argv,
        cwd: None,
        env: None,
        timeout: Duration::from_millis(1500),
        stdin: None,
    });
    assert!(
        matches!(result, Err(process::ProcessError::Timeout { .. })),
        "{result:?}"
    );
    let leader_pid: i32 = std::fs::read_to_string(&leader).unwrap().trim().parse().unwrap();
    let (grandchild, grandchild_sid) = read_pair(&marker);
    let (stopped, state) = gone_or_reap(grandchild);
    assert_eq!(grandchild_sid, leader_pid, "grandchild must inherit the child's own session");
    assert!(stopped, "grandchild survived timeout: {state}");
}

#[test]
fn setsid_child_cancel_still_kills_a_grandchild_in_its_session() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("grandchild");
    let leader = tmp.path().join("leader");
    let argv = grandchild_argv(&marker, &leader);
    let cancelled = std::sync::Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let watched = marker.clone();
    let canceller = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !watched.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        flag.store(true, Ordering::Release);
    });
    let start = Instant::now();
    let result = process::run_cancellable(
        process::RunSpec {
            argv: &argv,
            cwd: None,
            env: None,
            timeout: Duration::from_secs(20),
            stdin: None,
        },
        &cancelled,
    );
    canceller.join().unwrap();
    assert!(result.is_err(), "cancelled run cannot be success: {result:?}");
    assert!(start.elapsed() < Duration::from_secs(15), "cancel must not wait for the deadline");
    let leader_pid: i32 = std::fs::read_to_string(&leader).unwrap().trim().parse().unwrap();
    let (grandchild, grandchild_sid) = read_pair(&marker);
    let (stopped, state) = gone_or_reap(grandchild);
    assert_eq!(grandchild_sid, leader_pid, "grandchild must inherit the child's own session");
    assert!(stopped, "grandchild survived cancel: {state}");
}

#[tokio::test]
async fn mcp_stdio_child_leads_its_own_session() {
    use cgagentharness::common::mcp_policy::{Containment, NetworkPolicy, StdioCapabilities};
    let python = std::process::Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .expect("python3");
    let python = dunce::canonicalize(String::from_utf8(python.stdout).unwrap().trim()).unwrap();
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/mcp-fixture.py");
    let mut roots = vec![script.clone()];
    for prefix in ["/opt/homebrew", "/usr/local", "/Library/Frameworks/Python.framework"] {
        if python.starts_with(prefix) {
            roots.push(prefix.into());
        }
    }
    let capabilities = StdioCapabilities {
        version: 1,
        filesystem: Default::default(),
        read_roots: roots,
        write_roots: vec![],
        network: NetworkPolicy::Deny,
        containment: Containment::ProcessGroup,
        limits: None,
    };
    let argv = vec![
        python.display().to_string(),
        script.display().to_string(),
        "--stdio".into(),
    ];
    let spawned = cgagentharness::common::mcp::StdioClient::spawn(
        &argv,
        None,
        &std::collections::BTreeMap::new(),
        65536,
        None,
        &capabilities,
        std::path::Path::new(env!("CARGO_BIN_EXE_cgagentharness")),
        Duration::from_secs(15),
    )
    .await;
    let client = match spawned {
        Ok(client) => client,
        // Runners without namespace permission must refuse, never run unconfined;
        // the required bwrap job executes this path.
        Err(e) if e.code == "HARD_SANDBOX_UNAVAILABLE" && std::env::var_os("CGAH_REQUIRE_LINUX_BWRAP").is_none() => {
            eprintln!("SKIP mcp stdio session: {}", e.code);
            return;
        }
        Err(e) => panic!("stdio spawn failed: {}", e.code),
    };
    let pid = client.child_pid().expect("child pid") as i32;
    // SAFETY: session and group queries for our own direct child.
    let (sid, pgid) = unsafe { (libc::getsid(pid), libc::getpgid(pid)) };
    drop(client);
    assert_eq!(sid, pid, "MCP stdio child must lead its own session (setsid)");
    assert_eq!(pgid, pid, "drop cleanup relies on pgid == pid");
    assert_ne!(sid, own_sid());
}
