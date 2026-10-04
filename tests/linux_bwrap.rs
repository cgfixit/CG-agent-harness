//! Linux bubblewrap acceptance. Set CGAH_REQUIRE_LINUX_BWRAP (any value) to
//! require execution: unavailable confinement or a non-Linux host then fails.
//! Ordinary Linux CI still diagnoses runner limitations without claiming proof.
//! A timed-out run must also take down a live grandchild (no orphans).

use cgagentharness::agentic::executor::sandbox::LinuxBubblewrapSandbox;

#[cfg(target_os = "linux")]
use std::{collections::BTreeMap, path::Path};

#[cfg(target_os = "linux")]
use cgagentharness::agentic::executor::{sandbox::ArgvListSandbox, HardSandbox};

fn required() -> bool {
    std::env::var_os("CGAH_REQUIRE_LINUX_BWRAP").is_some()
}

#[cfg(target_os = "linux")]
fn env() -> BTreeMap<String, String> {
    BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())])
}

#[cfg(target_os = "linux")]
fn ci_linux() -> bool {
    std::env::var_os("CI").as_deref() == Some(std::ffi::OsStr::new("true"))
}

#[cfg(target_os = "linux")]
fn try_bwrap() -> Option<LinuxBubblewrapSandbox> {
    match LinuxBubblewrapSandbox::new() {
        Ok(sb) => Some(sb),
        Err(e) => {
            let kind = cgagentharness::common::sandbox_wrap::classify_probe_err(&e.message);
            if required() || (ci_linux() && kind == "missing_binary") {
                panic!("linux-bwrap required; cannot skip ({kind}): {}", e.message);
            }
            eprintln!("SKIP linux-bwrap ({kind}): {}", e.message);
            None
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn linux_ci_installs_the_bwrap_binary() {
    if !ci_linux() && !required() {
        return;
    }
    assert!(
        std::process::Command::new("bwrap")
            .arg("--version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false),
        "Linux CI must install bubblewrap; a missing binary must not look like a skip"
    );
}

#[cfg(target_os = "linux")]
fn assert_read_denied(sb: &LinuxBubblewrapSandbox, candidate: &Path, scratch: &Path, secret: &Path) {
    let visible = candidate.join("visible");
    std::fs::write(&visible, "candidate-readable").unwrap();
    let out = sb.run_prepared(
        &[
            "/usr/bin/python3".into(),
            "-c".into(),
            r#"import errno, pathlib, sys
assert pathlib.Path(sys.argv[1]).read_text() == 'candidate-readable'
try:
    pathlib.Path(sys.argv[2]).read_text()
except OSError as exc:
    assert exc.errno in (errno.ENOENT, errno.EACCES, errno.EPERM), exc
    print('read-denied')
else:
    raise AssertionError('host secret was readable')
"#
            .into(),
            visible.display().to_string(),
            secret.display().to_string(),
        ],
        candidate,
        &env(),
        10,
        scratch,
        &[],
    );
    assert_eq!(out.exit_code, 0, "child must execute both assertions: {out:?}");
    assert_eq!(out.stdout.trim(), "read-denied", "{out:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_bwrap_name_and_secret_read_denied() {
    let Some(sb) = try_bwrap() else {
        return;
    };
    assert_eq!(sb.name(), "linux-bwrap");
    let tmp = tempfile::tempdir().unwrap();
    let candidate = tmp.path().join("candidate");
    let scratch = tmp.path().join("scratch");
    let secret = tmp.path().join("secret");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::create_dir(&scratch).unwrap();
    std::fs::write(&secret, "host-secret").unwrap();
    assert_read_denied(&sb, &candidate, &scratch, &secret);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_bwrap_scratch_write_ok_candidate_write_denied() {
    let Some(sb) = try_bwrap() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let candidate = tmp.path().join("candidate");
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::create_dir(&scratch).unwrap();
    let out = sb.run_prepared(
        &[
            "/usr/bin/python3".into(),
            "-c".into(),
            r#"import errno, pathlib, sys
try:
    pathlib.Path(sys.argv[1]).write_text('x')
except OSError as exc:
    assert exc.errno in (errno.EROFS, errno.EACCES, errno.EPERM), exc
else:
    raise AssertionError('candidate was writable')
pathlib.Path(sys.argv[2]).write_text('x')
print('candidate-denied scratch-written')
"#
            .into(),
            candidate.join("nope.txt").display().to_string(),
            scratch.join("ok.txt").display().to_string(),
        ],
        &candidate,
        &env(),
        10,
        &scratch,
        &[],
    );
    assert_eq!(out.exit_code, 0, "{out:?}");
    assert_eq!(out.stdout.trim(), "candidate-denied scratch-written", "{out:?}");
    assert!(!candidate.join("nope.txt").exists());
    assert_eq!(std::fs::read_to_string(scratch.join("ok.txt")).unwrap(), "x");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_bwrap_symlink_escape_denied() {
    let Some(sb) = try_bwrap() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let candidate = tmp.path().join("candidate");
    let scratch = tmp.path().join("scratch");
    let secret = tmp.path().join("secret");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::create_dir(&scratch).unwrap();
    std::fs::write(&secret, "host-secret").unwrap();
    let escape = scratch.join("escape");
    std::os::unix::fs::symlink(&secret, &escape).unwrap();
    assert_eq!(std::fs::read_to_string(&escape).unwrap(), "host-secret");
    assert_read_denied(&sb, &candidate, &scratch, &escape);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_bwrap_network_isolated() {
    let Some(sb) = try_bwrap() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let candidate = tmp.path().join("candidate");
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::create_dir(&scratch).unwrap();
    // Keep an owned, reachable endpoint alive throughout both probes. Internet
    // outages, absent Python, or a failed sandbox launch must not prove isolation.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let argv = vec![
        "/usr/bin/python3".into(),
        "-c".into(),
        r#"import errno, os, socket, sys
print(os.readlink('/proc/self/ns/net'))
with socket.socket() as client:
    client.settimeout(1)
    try:
        client.connect(('127.0.0.1', int(sys.argv[1])))
    except OSError as exc:
        assert exc.errno in (errno.ECONNREFUSED, errno.ENETUNREACH, errno.EHOSTUNREACH, errno.EPERM, errno.EACCES), exc
        print('connection-denied')
    else:
        print('connected')
"#
        .into(),
        listener.local_addr().unwrap().port().to_string(),
    ];
    let control = ArgvListSandbox.run(&argv, &candidate, &env(), 10);
    assert_eq!(control.exit_code, 0, "unsandboxed control must execute: {control:?}");
    let parent_netns = std::fs::read_link("/proc/self/ns/net").unwrap();
    assert_eq!(control.stdout.trim(), format!("{}\nconnected", parent_netns.display()));
    let out = sb.run_prepared(&argv, &candidate, &env(), 10, &scratch, &[]);
    assert_eq!(out.exit_code, 0, "sandboxed network probe must execute: {out:?}");
    let (netns, result) = out.stdout.trim().split_once('\n').expect("namespace and result");
    assert_ne!(
        netns,
        parent_netns.to_str().unwrap(),
        "must use a separate network namespace"
    );
    assert_eq!(
        result, "connection-denied",
        "egress to the reachable host listener must fail: {out:?}"
    );
}

// #304 E1 contract, revised on #349: the harness runs the outer bwrap under
// `setsid`, so the sandboxed tree already has no controlling terminal (TIOCSTI
// is closed). bwrap's own `--new-session` would move the command out of the
// process group the timeout and cancel paths kill (bubblewrap #726), so it must
// not be requested.

#[cfg(target_os = "linux")]
fn assert_no_new_session_before_command(argv: &[String]) {
    let split = argv
        .iter()
        .position(|a| a == "--")
        .expect("bwrap argv has a `--` separator");
    assert!(
        !argv[..split].iter().any(|a| a == "--new-session"),
        "bwrap options must not include --new-session (it splits the killed process group): {argv:?}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn bwrap_argv_does_not_request_a_new_session() {
    let tmp = tempfile::tempdir().unwrap();
    let candidate = tmp.path().join("candidate");
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::create_dir(&scratch).unwrap();
    // Pure argv construction: no bwrap binary is executed here.
    let argv = cgagentharness::common::sandbox_wrap::bwrap_argv(
        Path::new("/usr/bin/bwrap"),
        &["/bin/true".into()],
        &candidate,
        &scratch,
        &[],
    )
    .expect("argv");
    assert_no_new_session_before_command(&argv);
    assert_eq!(argv.last().map(String::as_str), Some("/bin/true"));
}

#[cfg(target_os = "linux")]
#[test]
fn mcp_stdio_bwrap_wrap_does_not_request_a_new_session_for_both_network_policies() {
    use cgagentharness::common::mcp_policy::{Containment, NetworkPolicy, StdioCapabilities};
    for network in [NetworkPolicy::Deny, NetworkPolicy::Unrestricted] {
        let capabilities = StdioCapabilities {
            version: 1,
            filesystem: Default::default(),
            read_roots: vec![],
            write_roots: vec![],
            network,
            containment: Containment::ProcessGroup,
            limits: None,
        };
        match cgagentharness::common::sandbox_wrap::wrap_mcp_stdio(&["/bin/true".into()], None, None, &capabilities) {
            Ok(wrapped) => {
                assert!(wrapped.backend.starts_with("linux-bwrap"), "{}", wrapped.backend);
                assert_no_new_session_before_command(&wrapped.argv);
            }
            Err(e) => {
                assert_eq!(e.code, "HARD_SANDBOX_UNAVAILABLE", "{network:?}: {}", e.message);
                if required() {
                    panic!(
                        "linux-bwrap required; MCP wrap unavailable for {network:?}: {}",
                        e.message
                    );
                }
                eprintln!("SKIP mcp bwrap wrap ({network:?}): {}", e.message);
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn linux_bwrap_child_has_no_controlling_terminal() {
    let Some(sb) = try_bwrap() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let candidate = tmp.path().join("candidate");
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::create_dir(&scratch).unwrap();
    // tty_nr (field 7 of /proc/self/stat) is a device number, not a namespaced
    // id, so 0 means no controlling terminal inside the sandbox.
    let out = sb.run_prepared(
        &[
            "/usr/bin/python3".into(),
            "-c".into(),
            "print(open('/proc/self/stat').read().rsplit(')', 1)[1].split()[4])".into(),
        ],
        &candidate,
        &env(),
        10,
        &scratch,
        &[],
    );
    assert_eq!(out.exit_code, 0, "tty probe must execute: {out:?}");
    assert_eq!(
        out.stdout.trim(),
        "0",
        "sandboxed command still has a controlling terminal: {out:?}"
    );
}

#[cfg(target_os = "linux")]
fn grandchild_pids(token: &str) -> Vec<i32> {
    let mut pids = Vec::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else {
            continue;
        };
        let comm = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        let cmdline = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
        if comm.trim() == "sleep" && cmdline.split(|b| *b == 0).any(|arg| arg == token.as_bytes()) {
            pids.push(pid);
        }
    }
    pids
}

#[cfg(target_os = "linux")]
fn pid_gone_or_zombie(pid: i32) -> bool {
    // SAFETY: signal 0 only probes for existence; no signal is delivered.
    if unsafe { libc::kill(pid, 0) } == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        return true;
    }
    // A killed grandchild may sit briefly as a zombie until its new parent reaps it.
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|stat| stat.rsplit(')').next().unwrap_or("").split_whitespace().next() == Some("Z"))
        .unwrap_or(true)
}

#[cfg(target_os = "linux")]
#[test]
fn linux_bwrap_timeout_kills_a_live_grandchild() {
    let Some(sb) = try_bwrap() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let candidate = tmp.path().join("candidate");
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::create_dir(&scratch).unwrap();
    // A unique sleep duration tags the grandchild's argv so the host-side /proc
    // scan cannot match any other process (pids inside the sandbox are namespaced).
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let token = format!("300.{nanos:09}");
    let argv: Vec<String> = vec!["/bin/sh".into(), "-c".into(), format!("sleep {token} & echo $! ; wait")];
    let runner = {
        let (candidate, scratch) = (candidate.clone(), scratch.clone());
        std::thread::spawn(move || sb.run_prepared(&argv, &candidate, &env(), 1, &scratch, &[]))
    };
    let mut seen = Vec::new();
    while seen.is_empty() && !runner.is_finished() {
        seen = grandchild_pids(&token);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = runner.join().expect("runner thread");
    assert!(out.timed_out, "the 1s timeout must fire while sleep 300 runs: {out:?}");
    assert!(
        !seen.is_empty(),
        "grandchild never observed alive; the test would prove nothing: {out:?}"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut alive: Vec<i32> = seen.clone();
    while !alive.is_empty() && std::time::Instant::now() < deadline {
        alive.retain(|pid| !pid_gone_or_zombie(*pid));
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    for pid in &alive {
        // SAFETY: best-effort cleanup of a pid this test observed; avoids leaking it.
        unsafe { libc::kill(*pid, libc::SIGKILL) };
    }
    assert!(
        alive.is_empty(),
        "grandchild {alive:?} outlived the timeout by 2s (orphaned)"
    );
}

#[cfg(not(target_os = "linux"))]
#[test]
fn linux_bwrap_suite_is_linux_only() {
    assert!(!required(), "required linux-bwrap acceptance must run on Linux");
    assert!(std::any::type_name::<LinuxBubblewrapSandbox>().contains("LinuxBubblewrapSandbox"));
}
