//! Lock `scripts/start-web.sh`: argument refusals exit 3 before anything
//! starts, and the printed URL follows the home's TLS gate (only a literal
//! `enabled: true` under `tls:`, or no config at all, is HTTPS; a config
//! without that key is HTTP, as `transport.rs` serves it). A fake
//! backend stands in for the real binary, so no server or browser runs.
#![cfg(unix)]

use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The script binds and reaches the console on loopback only.
const LOOPBACK: &str = "127.0.0.1"; // DevSkim: ignore DS162092 because start-web.sh and its fake backend use loopback only.

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/start-web.sh")
}

/// Runs on a port read back from a released `:0` bind. That reservation is
/// advisory (another process may take it before the script binds), so a
/// port-in-use refusal from the script or the fake retries on a fresh port.
fn run_on_free_port(home: &Path, bin: &Path) -> (u16, Output) {
    let mut seen = Vec::new();
    for _ in 0..5 {
        let port = TcpListener::bind((LOOPBACK, 0)).unwrap().local_addr().unwrap().port();
        let out = run(home, bin, &port.to_string());
        if !stderr(&out).contains("already in use") {
            return (port, out);
        }
        seen.push(stderr(&out));
    }
    panic!("no free loopback port after 5 attempts: {seen:?}");
}

fn run(home: &Path, bin: &Path, port: &str) -> Output {
    Command::new("bash")
        .arg(script())
        .arg(port)
        .env("CGAGENTHARNESS_HOME", home)
        .env("CGAH_BIN", bin)
        .env("NO_OPEN", "1")
        .env("CGAH_FAKE_HOST", LOOPBACK)
        .env_remove("PORT")
        .output()
        .unwrap()
}

/// A stand-in for `cgagentharness serve --port N`: listens on loopback,
/// answers the script's two readiness probes, then exits 0.
fn fake_backend(dir: &Path) -> PathBuf {
    let bin = dir.join("fake-backend");
    std::fs::write(
        &bin,
        r#"#!/bin/sh
[ "$1" = serve ] && [ "$2" = --port ] || exit 9
exec python3 -c '
import socket, sys
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind((sys.argv[1], int(sys.argv[2])))
s.listen(4)
s.settimeout(30)
for _ in range(2):
    s.accept()[0].close()
' "$CGAH_FAKE_HOST" "$3"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn refuses_bad_ports_and_missing_binary_with_exit_3() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-binary");
    for (port, needle) in [
        ("abc", "port must be a number"),
        ("-1", "port must be a number"),
        ("80", "port must be 1024-65535"),
        ("65536", "port must be 1024-65535"),
    ] {
        let out = run(dir.path(), &missing, port);
        assert_eq!(out.status.code(), Some(3), "{port}: {}", stderr(&out));
        assert!(stderr(&out).contains(needle), "{port}: {}", stderr(&out));
    }
    let (_, out) = run_on_free_port(dir.path(), &missing);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("CGAH_BIN is not executable"), "{}", stderr(&out));
}

#[test]
fn refuses_a_port_that_is_already_listening() {
    let dir = tempfile::tempdir().unwrap();
    let held = TcpListener::bind((LOOPBACK, 0)).unwrap();
    let port = held.local_addr().unwrap().port().to_string();
    let out = run(dir.path(), &fake_backend(dir.path()), &port);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("already in use"), "{}", stderr(&out));
}

#[test]
fn url_scheme_follows_only_a_literal_tls_true() {
    for (config, scheme) in [
        (None, "https"),
        (Some("tls:\n  enabled: true\n"), "https"),
        (Some("tls:\n  enabled: false\n"), "http"),
        (Some("tls:\n  enabled: \"true\"\n"), "http"),
        // A later top-level section ends the tls block: web's `enabled: true`
        // is not tls's, so a tls block without the key stays HTTP.
        (Some("tls:\n  cert: x\nweb:\n  enabled: true\n"), "http"),
        // A home seeded before TLS has no tls block; the server serves HTTP.
        (Some("web:\n  enabled: true\n"), "http"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        if let Some(config) = config {
            std::fs::write(dir.path().join("config.yaml"), config).unwrap();
        }
        let (port, out) = run_on_free_port(dir.path(), &fake_backend(dir.path()));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(0), "{config:?}: {stdout} {}", stderr(&out));
        assert!(
            stdout.contains(&format!("CGagentHarness console: {scheme}://{LOOPBACK}:{port}/")),
            "{config:?}: {stdout}"
        );
    }
}
