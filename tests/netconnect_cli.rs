//! Process-boundary lock for `cgagentharness netconnect`.
//!
//! Exit codes: 0 ok, 2 failed, 3 config, 4 gate refused.

use std::net::Ipv4Addr;
use std::process::Command;

use cgagentharness::netconnect::Scope;
use serde_json::Value;

fn invoke(config: &std::path::Path, action: &str) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_cgagentharness"))
        .args(["netconnect", "--config", config.to_str().expect("utf-8 path"), action])
        .env("GROK_API_KEY", "")
        .env("ANTHROPIC_API_KEY", "")
        .env("DEEPAGENT_API_KEY", "")
        .output()
        .expect("spawn netconnect");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn addresses(value: &Value) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<Ipv4Addr>, text: &str| {
        if let Some(ip) = text.split('/').next().and_then(|item| item.parse().ok()) {
            out.push(ip);
        }
    };
    for row in value.get("interfaces").and_then(Value::as_array).into_iter().flatten() {
        for address in row.get("addresses").and_then(Value::as_array).into_iter().flatten() {
            if let Some(text) = address.as_str() {
                push(&mut out, text);
            }
        }
    }
    for row in value.get("routes").and_then(Value::as_array).into_iter().flatten() {
        if let Some(text) = row.get("destination").and_then(Value::as_str) {
            push(&mut out, text);
        }
        if let Some(text) = row.get("gateway").and_then(Value::as_str) {
            push(&mut out, text);
        }
    }
    for row in value
        .get("default_gateways")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(text) = row.get("address").and_then(Value::as_str) {
            push(&mut out, text);
        }
    }
    for row in value.get("neighbors").and_then(Value::as_array).into_iter().flatten() {
        if let Some(text) = row.get("address").and_then(Value::as_str) {
            push(&mut out, text);
        }
    }
    out
}

#[test]
fn cli_exit_codes_for_disabled_and_bad_scope() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    std::fs::write(&path, "netconnect:\n  enabled: false\n").unwrap();
    for action in ["status", "devices"] {
        let (code, stdout, stderr) = invoke(&path, action);
        assert_eq!(code, 4, "{action} stderr={stderr}");
        let body: Value = serde_json::from_str(&stdout).expect("json");
        assert_eq!(body["code"], "NETCONNECT_DISABLED");
        assert_eq!(body["ok"], false);
    }

    std::fs::write(&path, "netconnect:\n  enabled: true\n  allowed_cidrs: ['8.8.8.8/32']\n").unwrap();
    for action in ["status", "devices"] {
        let (code, stdout, _) = invoke(&path, action);
        assert_eq!(code, 3, "{action}");
        let body: Value = serde_json::from_str(&stdout).expect("json");
        assert_eq!(body["code"], "CONFIG_ERROR");
        assert!(body["message"]
            .as_str()
            .unwrap()
            .contains("outside RFC1918 and loopback"));
    }

    std::fs::write(
        &path,
        "netconnect:\n  enabled: true\n  discovery: true\n  allowed_cidrs: ['192.168.0.0/16', '10.0.0.0/16', '172.16.0.0/16', '127.0.0.0/16']\n",
    )
    .unwrap();
    let (code, stdout, stderr) = invoke(&path, "status");
    assert_eq!(code, 0, "{stderr}");
    let body: Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(body["ok"], true);
    assert_eq!(body["active_tiers_refused"], false);
    let discovery = body["tiers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["tier"] == "discovery")
        .unwrap();
    assert_eq!(discovery["tier_enabled"], true);
    assert_eq!(discovery["runnable"], true);

    let (code, stdout, stderr) = invoke(&path, "devices");
    let body: Value = serde_json::from_str(&stdout).expect("json");
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        assert_eq!(code, 2, "{stderr}");
        assert_eq!(body["ok"], false);
        assert_eq!(body["code"], "NETCONNECT_UNSUPPORTED");
        return;
    }
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(body["packets_sent"], 0);
    let scope = Scope::parse(&[
        "192.168.0.0/16".to_string(),
        "10.0.0.0/16".to_string(),
        "172.16.0.0/16".to_string(),
        "127.0.0.0/16".to_string(),
    ])
    .unwrap();
    for address in addresses(&body) {
        assert!(scope.contains(address));
    }
}
