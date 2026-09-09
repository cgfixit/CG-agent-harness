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
    std::thread::sleep(Duration::from_millis(30));
    let state = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&state.stdout);
    let stopped = state.trim().is_empty() || state.trim().starts_with('Z');
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
    store.insert_running("fixture", "real-repo-run", task);
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
    assert_eq!(store.cancel("fixture").unwrap()["status"], "cancelled");
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
    assert_eq!(store.get("fixture").unwrap()["status"], "cancelled");
}
