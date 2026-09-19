//! Linux bubblewrap acceptance. Set CGAH_REQUIRE_LINUX_BWRAP (any value) to
//! require execution: unavailable confinement or a non-Linux host then fails.
//! Ordinary Linux CI still diagnoses runner limitations without claiming proof.
//! SIGKILL of a live grandchild is not covered.

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

#[cfg(not(target_os = "linux"))]
#[test]
fn linux_bwrap_suite_is_linux_only() {
    assert!(!required(), "required linux-bwrap acceptance must run on Linux");
    assert!(std::any::type_name::<LinuxBubblewrapSandbox>().contains("LinuxBubblewrapSandbox"));
}
