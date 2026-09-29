//! Scope load refusals and the config-error exit.
//!
//! Invalid `allowed_cidrs` fail at config load with exit 3, before `devices`
//! can read a neighbor table or call `getifaddrs`. The JSON from that refusal
//! has no collected interfaces, routes, or neighbors. The Linux syscall proof
//! in `scripts/netconnect-syscall-proof.sh` traces the same refusal.

use std::net::Ipv4Addr;
use std::path::Path;
use std::process::Command;

use cgagentharness::common::config::AppConfig;
use cgagentharness::netconnect::cli::{self, EXIT_ENV};
use cgagentharness::netconnect::{NetconnectConfig, Scope};
use serde_json::Value;

fn app(yaml: &str) -> AppConfig {
    AppConfig::from_str(yaml, Path::new("fixture.yaml")).expect("yaml")
}

fn invoke(yaml: &str, action: &str) -> (i32, Value, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    std::fs::write(&path, yaml).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_cgagentharness"))
        .args(["netconnect", "--config", path.to_str().expect("utf-8"), action])
        .env("GROK_API_KEY", "")
        .env("ANTHROPIC_API_KEY", "")
        .env("DEEPAGENT_API_KEY", "")
        .output()
        .expect("spawn netconnect");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let body: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|err| panic!("{action} did not print JSON ({err}); stdout={stdout} stderr={stderr}"));
    (out.status.code().unwrap_or(-1), body, stderr)
}

fn assert_config_refusal(cidr: &str, reason: &str) {
    let yaml = format!("netconnect:\n  enabled: true\n  discovery: true\n  allowed_cidrs: ['{cidr}']\n");
    let err = NetconnectConfig::from_config(&app(&yaml)).expect_err(cidr);
    assert!(err.is("CONFIG_ERROR"), "{cidr} -> {err}");
    assert!(err.message.contains(reason), "{cidr} -> {}", err.message);
    assert!(!err.message.contains('\n') && !err.message.contains('\r'), "{cidr}");
    assert_eq!(cli::exit_code_for(&err), EXIT_ENV);

    for action in ["status", "devices"] {
        let (code, body, stderr) = invoke(&yaml, action);
        assert_eq!(code, i32::from(EXIT_ENV), "{cidr} {action} {body} {stderr}");
        assert_eq!(body["ok"], false);
        assert_eq!(body["code"], "CONFIG_ERROR");
        let message = body["message"].as_str().unwrap();
        assert!(message.contains(reason), "{cidr} {action} {message}");
        assert!(!message.contains('\n') && !message.contains('\r'));
        assert!(body.get("interfaces").is_none(), "{action} collected after a bad scope");
        assert!(body.get("routes").is_none(), "{action}");
        assert!(body.get("neighbors").is_none(), "{action}");
        assert!(body.get("packets_sent").is_none(), "{action}");
        assert!(!stderr.contains('\n') || stderr.lines().all(|line| !line.contains('\r')));
    }
}

#[test]
fn config_load_rejects_the_out_of_scope_forms() {
    let cases = [
        ("8.8.8.8/32", "outside RFC1918 and loopback"),
        ("1.1.1.1/32", "outside RFC1918 and loopback"),
        ("203.0.113.0/24", "outside RFC1918 and loopback"),
        ("0.0.0.0/8", "prefix wider than /16"),
        ("0.0.0.0/16", "outside RFC1918 and loopback"),
        ("169.254.0.0/16", "outside RFC1918 and loopback"),
        ("169.254.1.0/24", "outside RFC1918 and loopback"),
        ("100.64.0.0/10", "prefix wider than /16"),
        ("100.64.0.0/16", "outside RFC1918 and loopback"),
        ("224.0.0.0/4", "prefix wider than /16"),
        ("224.0.0.0/16", "outside RFC1918 and loopback"),
        ("239.255.255.255/32", "outside RFC1918 and loopback"),
        ("255.255.255.255/32", "outside RFC1918 and loopback"),
        ("240.0.0.0/4", "prefix wider than /16"),
        ("10.0.0.0/8", "prefix wider than /16"),
        ("10.0.0.0/15", "prefix wider than /16"),
        ("172.16.0.0/12", "prefix wider than /16"),
        ("192.168.0.0/12", "prefix wider than /16"),
        ("192.168.0.0/15", "prefix wider than /16"),
        ("127.0.0.0/8", "prefix wider than /16"),
        ("2001:db8::/32", "IPv6 is not permitted"),
        ("::1", "IPv6 is not permitted"),
        ("fe80::1/64", "IPv6 is not permitted"),
    ];
    for (cidr, reason) in cases {
        assert_config_refusal(cidr, reason);
    }
}

#[test]
fn a_control_character_in_a_rejected_cidr_does_not_break_the_message() {
    let yaml = "netconnect:\n  enabled: true\n  allowed_cidrs: [\"8.8.8.8/32\\nINJECT\"]\n";
    let err = NetconnectConfig::from_config(&app(yaml)).unwrap_err();
    assert!(err.is("CONFIG_ERROR"));
    assert!(!err.message.contains('\n') && !err.message.contains('\r'));
    let (code, body, stderr) = invoke(yaml, "status");
    assert_eq!(code, i32::from(EXIT_ENV));
    let message = body["message"].as_str().unwrap();
    assert!(!message.contains('\n') && !message.contains('\r'));
    assert!(!stderr.contains("\nINJECT"));
}

#[test]
fn empty_allowed_cidrs_is_the_default_and_contains_nothing() {
    let shipped = NetconnectConfig::from_config(&app(AppConfig::embedded_default())).unwrap();
    assert!(shipped.scope.is_empty());
    assert!(shipped.scope.entries().is_empty());
    assert!(shipped
        .scope
        .check_target(Ipv4Addr::new(192, 168, 1, 1))
        .unwrap_err()
        .is("NETCONNECT_REFUSED"));
    assert!(shipped
        .scope
        .check_target(Ipv4Addr::LOCALHOST)
        .unwrap_err()
        .is("NETCONNECT_REFUSED"));

    for yaml in [
        "netconnect:\n  enabled: false\n  allowed_cidrs: []\n",
        "netconnect:\n  enabled: true\n",
        "app:\n  name: test\n",
    ] {
        let cfg = NetconnectConfig::from_config(&app(yaml)).unwrap();
        assert!(cfg.scope.is_empty(), "{yaml}");
        assert!(Scope::parse(&[]).unwrap().is_empty());
    }
}

#[test]
fn shipped_default_status_exits_config_gate_before_a_collector_payload() {
    let (code, body, _) = invoke(AppConfig::embedded_default(), "status");
    assert_eq!(code, i32::from(cli::EXIT_REFUSED), "{body}");
    assert_eq!(body["code"], "NETCONNECT_DISABLED");
    assert_eq!(body["scope_empty"], true);
    assert!(body.get("interfaces").is_none());
    assert!(body.get("packets_sent").is_none());

    let (code, body, _) = invoke(AppConfig::embedded_default(), "devices");
    assert_eq!(code, i32::from(cli::EXIT_REFUSED), "{body}");
    assert_eq!(body["code"], "NETCONNECT_DISABLED");
    assert!(body.get("interfaces").is_none());
    assert!(body.get("neighbors").is_none());
    assert!(body.get("packets_sent").is_none());
}

#[test]
fn one_public_entry_rejects_an_otherwise_private_list() {
    let yaml = "netconnect:\n  allowed_cidrs: ['192.168.0.0/16', '8.8.8.8/32']\n";
    let err = NetconnectConfig::from_config(&app(yaml)).unwrap_err();
    assert!(err.is("CONFIG_ERROR"));
    assert!(err.message.contains("outside RFC1918 and loopback"));
    let (code, _, _) = invoke(yaml, "status");
    assert_eq!(code, i32::from(EXIT_ENV));
}
