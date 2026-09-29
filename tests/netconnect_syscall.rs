//! The syscall judge is a script, and this test runs its self-check.
//!
//! A missing `unshare` or `strace` on the proof path must fail the script.
//! That failure is asserted from the script text and from `--self-check`,
//! which does not need a network namespace.

use std::path::PathBuf;
use std::process::Command;

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/netconnect-syscall-proof.sh")
}

#[test]
fn self_check_rejects_packet_sockets_and_multicast_membership() {
    let output = Command::new("bash")
        .arg(script())
        .arg("--self-check")
        .env("GROK_API_KEY", "")
        .env("ANTHROPIC_API_KEY", "")
        .env("DEEPAGENT_API_KEY", "")
        .output()
        .expect("run syscall self-check");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "self-check failed\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(stdout.contains("self-check: ok"), "{stdout}");
}

#[test]
fn proof_refuses_to_skip_when_unshare_or_strace_is_missing() {
    let text = std::fs::read_to_string(script()).unwrap();
    assert!(text.contains("unshare -rn"));
    assert!(text.contains("trace=socket,connect,sendto,setsockopt"));
    assert!(text.contains("AF_NETLINK"));
    assert!(text.contains("AF_UNIX"));
    assert!(text.contains("IP_ADD_MEMBERSHIP"));
    assert!(text.contains("IPV6_JOIN_GROUP"));
    assert!(text.contains("refusing to skip"));
    assert!(text.contains("apparmor_restrict_unprivileged_userns"));
    assert!(!text.contains("|| true"));
    assert!(!text.contains("continue-on-error"));
}
