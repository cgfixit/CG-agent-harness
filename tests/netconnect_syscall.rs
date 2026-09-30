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
    assert!(
        stdout.contains("sandbox setup failure exits non-zero"),
        "a failed unshare or strace must not be a pass\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("enabled passive traces are required"),
        "dropping the enabled passive traces must fail self-check\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("enabled status opens no sockets and a gated empty trace does not pass"),
        "passive status must open no socket, and a gated empty trace must not pass\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("devices opens exactly one AF_NETLINK socket"),
        "devices must open exactly one netlink socket\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("pid-prefixed netlink-labeled sendto passes"),
        "a pid-prefixed netlink-labeled sendto must pass\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("tcp and udp labeled sendto fails"),
        "a TCP or UDP labeled sendto must fail\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("bare socket inode sendto fails"),
        "a bare socket:[inode] sendto must fail\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("unlabeled sendto fails"),
        "a sendto with no descriptor label must fail\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("inet socket or connect with a netlink or unix label fails"),
        "an inet socket or connect must fail even when -yy labels it NETLINK or UNIX\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("strace -yy runs inside unshare"),
        "strace -yy must run inside unshare\nstdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn proof_refuses_to_skip_when_unshare_or_strace_is_missing() {
    let text = std::fs::read_to_string(script()).unwrap();
    assert!(text.contains("unshare -rn"));
    assert!(text.contains(
        "\"$unshare_bin\" -rn \"$strace_bin\" -f -yy -e trace=socket,connect,sendto,sendmsg,sendmmsg,setsockopt"
    ));
    assert!(text.contains("trace=socket,connect,sendto,sendmsg,sendmmsg,setsockopt"));
    assert!(text.contains("<TCP:"));
    assert!(text.contains("<UDP:"));
    assert!(text.contains("socket:[12345]"));
    assert!(text.contains("sendto(3, ...)"));
    assert!(text.contains("socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP) = 3<NETLINK:[ROUTE]>"));
    assert!(text.contains("AF_NETLINK"));
    assert!(text.contains("AF_UNIX"));
    assert!(text.contains("IP_ADD_MEMBERSHIP"));
    assert!(text.contains("IPV6_JOIN_GROUP"));
    assert!(text.contains("127.0.0.0/16"));
    assert!(text.contains("enabled: true"));
    assert!(text.contains("assert_enabled_status"));
    assert!(text.contains("exactly_one_netlink"));
    assert!(text.contains("enabled status opened no sockets"));
    assert!(text.contains("ifi_family=AF_UNSPEC"));
    assert!(text.contains("ifa_family=AF_UNSPEC"));
    assert!(!text.contains("disallowed socket call"));
    assert!(text.contains("refusing to skip"));
    assert!(text.contains("apparmor_restrict_unprivileged_userns"));
    assert!(!text.contains("|| true"));
    assert!(!text.contains("continue-on-error"));
}
