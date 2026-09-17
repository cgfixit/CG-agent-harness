//! Linux bubblewrap acceptance. Darwin stays a compile-only stub. On Linux a
//! missing or unprobeable `bwrap` skips locally, and fails the job when `CI=true`.
//! SIGKILL of the runner with a grandchild still alive is not covered here.
//! That leftover matches the Seatbelt process-group residual.

use cgagentharness::agentic::executor::sandbox::LinuxBubblewrapSandbox;

#[cfg(target_os = "linux")]
use std::collections::BTreeMap;

#[cfg(target_os = "linux")]
use cgagentharness::agentic::executor::HardSandbox;

#[cfg(target_os = "linux")]
fn env() -> BTreeMap<String, String> {
    BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())])
}

#[cfg(target_os = "linux")]
fn try_bwrap() -> Option<LinuxBubblewrapSandbox> {
    match LinuxBubblewrapSandbox::new() {
        Ok(sb) => Some(sb),
        Err(e) => {
            if std::env::var_os("CI").as_deref() == Some(std::ffi::OsStr::new("true")) {
                panic!("linux-bwrap required on Linux CI: {}", e.message);
            }
            eprintln!("SKIP linux-bwrap: {}", e.message);
            None
        }
    }
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
    let out = sb.run_prepared(
        &["/bin/sh".into(), "-c".into(), format!("cat '{}'", secret.display())],
        &candidate,
        &env(),
        10,
        &scratch,
        &[],
    );
    assert_ne!(out.exit_code, 0, "host secret must be unreadable: {out:?}");
    assert!(!out.stdout.contains("host-secret"), "{out:?}");
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
    let denied = sb.run_prepared(
        &[
            "/bin/sh".into(),
            "-c".into(),
            format!("echo x > '{}'", candidate.join("nope.txt").display()),
        ],
        &candidate,
        &env(),
        10,
        &scratch,
        &[],
    );
    assert_ne!(denied.exit_code, 0, "{denied:?}");
    assert!(!candidate.join("nope.txt").exists());
    let allowed = sb.run_prepared(
        &[
            "/bin/sh".into(),
            "-c".into(),
            format!("echo x > '{}'", scratch.join("ok.txt").display()),
        ],
        &candidate,
        &env(),
        10,
        &scratch,
        &[],
    );
    assert_eq!(allowed.exit_code, 0, "{allowed:?}");
    assert!(scratch.join("ok.txt").exists());
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
    std::os::unix::fs::symlink(&secret, scratch.join("escape")).unwrap();
    let out = sb.run_prepared(
        &[
            "/bin/sh".into(),
            "-c".into(),
            format!("cat '{}'", scratch.join("escape").display()),
        ],
        &candidate,
        &env(),
        10,
        &scratch,
        &[],
    );
    assert!(!out.stdout.contains("host-secret"), "{out:?}");
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
    let out = sb.run_prepared(
        &[
            "/bin/sh".into(),
            "-c".into(),
            "python3 -c 'import socket; s=socket.socket(); s.settimeout(1); s.connect((\"1.1.1.1\", 53))'".into(),
        ],
        &candidate,
        &env(),
        10,
        &scratch,
        &[],
    );
    assert_ne!(out.exit_code, 0, "egress must fail: {out:?}");
}

#[cfg(not(target_os = "linux"))]
#[test]
fn linux_bwrap_suite_is_linux_only() {
    assert!(std::any::type_name::<LinuxBubblewrapSandbox>().contains("LinuxBubblewrapSandbox"));
}
