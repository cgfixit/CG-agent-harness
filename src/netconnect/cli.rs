//! `cgagentharness netconnect status|devices`.
//!
//! Exit codes match the harness CLI API: 0 ok, 2 failed, 3 config, 4 gate
//! refused. A false `netconnect.enabled` is a refusal, not a success.
//! `devices` reads local tables and sends no packets.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{json, Value};

use crate::common::config::AppConfig;
use crate::common::errors::HarnessError;
use crate::common::home::Home;

use super::collect::collect_passive;
use super::config::{NetconnectConfig, Tier};
use super::sources::{LiveInterfaces, LiveNeighbors, LiveRoutes};
use super::tools::tier_rows;

pub const EXIT_OK: u8 = 0;
pub const EXIT_FAIL: u8 = 2;
pub const EXIT_ENV: u8 = 3;
pub const EXIT_REFUSED: u8 = 4;

#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Show gates, scope, and whether each tier may run. Sends no packets.
    Status,
    /// List in-scope interfaces, routes, and neighbors. Sends no packets.
    Devices,
}

struct Outcome {
    code: u8,
    warnings: Vec<String>,
    body: Value,
}

pub fn exit_code_for(err: &HarnessError) -> u8 {
    match err.code.as_str() {
        "CONFIG_ERROR" | "HARNESS_CONFIG_ERROR" => EXIT_ENV,
        "NETCONNECT_DISABLED" | "NETCONNECT_REFUSED" => EXIT_REFUSED,
        _ => EXIT_FAIL,
    }
}

pub fn run(config: Option<PathBuf>, action: Action) -> u8 {
    match std::panic::catch_unwind(AssertUnwindSafe(|| execute(config, action))) {
        Ok(outcome) => emit(outcome),
        Err(_) => {
            eprintln!("netconnect: internal error");
            EXIT_FAIL
        }
    }
}

fn emit(outcome: Outcome) -> u8 {
    for warning in &outcome.warnings {
        eprintln!("netconnect: warning: {warning}");
    }
    match serde_json::to_string_pretty(&outcome.body) {
        Ok(text) => println!("{text}"),
        Err(_) => {
            eprintln!("netconnect: internal error");
            return EXIT_FAIL;
        }
    }
    if outcome.code != EXIT_OK {
        if let Some(message) = outcome.body.get("message").and_then(|value| value.as_str()) {
            eprintln!("netconnect: {message}");
        }
    }
    outcome.code
}

fn execute(config: Option<PathBuf>, action: Action) -> Outcome {
    let path = config.unwrap_or_else(|| Home::default_root().join("config.yaml"));
    let app = match AppConfig::load(&path) {
        Ok(cfg) => cfg,
        Err(err) => return error_outcome(&err, Vec::new()),
    };
    let cfg = match NetconnectConfig::from_config(&app) {
        Ok(cfg) => cfg,
        Err(err) => return error_outcome(&err, Vec::new()),
    };
    let warnings = cfg.warnings.clone();
    if !cfg.enabled {
        return Outcome {
            code: EXIT_REFUSED,
            warnings,
            body: json!({
                "ok": false,
                "code": "NETCONNECT_DISABLED",
                "message": "netconnect.enabled is false",
                "enabled": false,
                "scope_empty": cfg.scope.is_empty(),
                "tiers": tier_rows(&cfg),
            }),
        };
    }
    let body = match action {
        Action::Status => Ok(status_body(&cfg)),
        Action::Devices => devices_body(&cfg),
    };
    match body {
        Ok(body) => Outcome {
            code: EXIT_OK,
            warnings,
            body,
        },
        Err(err) => error_outcome(&err, warnings),
    }
}

fn error_outcome(err: &HarnessError, warnings: Vec<String>) -> Outcome {
    Outcome {
        code: exit_code_for(err),
        warnings,
        body: json!({
            "ok": false,
            "code": err.code,
            "message": err.message,
        }),
    }
}

fn status_body(cfg: &NetconnectConfig) -> Value {
    json!({
        "ok": true,
        "enabled": true,
        "scope_empty": cfg.scope.is_empty(),
        "allowed_cidrs": cfg.scope.entries(),
        "active_tiers_refused": Tier::ALL.into_iter().any(|tier| cfg.tier_enabled(tier) && !cfg.tier_may_run(tier)),
        "tiers": tier_rows(cfg),
        "port_allowlist": cfg.port_allowlist,
        "host_cap": cfg.host_cap,
        "rate_limit_per_min": cfg.rate_limit_per_min,
        "connect_timeout_ms": cfg.connect_timeout_ms,
        "read_timeout_ms": cfg.read_timeout_ms,
        "diagnostics_targets": cfg.diagnostics_targets.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "throughput_endpoint": cfg.throughput_endpoint,
        "ha_endpoint": cfg.ha_endpoint,
        "mqtt_endpoint": cfg.mqtt_endpoint,
        "warnings": cfg.warnings,
    })
}

fn devices_body(cfg: &NetconnectConfig) -> Result<Value, HarnessError> {
    let report = collect_passive(
        &cfg.scope,
        &LiveNeighbors,
        &LiveRoutes,
        &LiveInterfaces,
        cfg.untrusted_string_max_chars,
    )?;
    Ok(json!({
        "ok": true,
        "packets_sent": 0,
        "scope_empty": cfg.scope.is_empty(),
        "active_tiers_refused": Tier::ALL.into_iter().any(|tier| cfg.tier_enabled(tier) && !cfg.tier_may_run(tier)),
        "interfaces": report.interfaces,
        "routes": report.routes,
        "default_gateways": report.default_gateways,
        "neighbors": report.neighbors,
    }))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn write_config(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn disabled_status_and_devices_refuse() {
        let (_dir, path) = write_config("netconnect:\n  enabled: false\n");
        for action in [Action::Status, Action::Devices] {
            let outcome = execute(Some(path.clone()), action);
            assert_eq!(outcome.code, EXIT_REFUSED);
            assert_eq!(outcome.body["code"], "NETCONNECT_DISABLED");
            assert_eq!(run(Some(path.clone()), action), EXIT_REFUSED);
        }
    }

    #[test]
    fn bad_scope_is_config_exit() {
        let (_dir, path) = write_config("netconnect:\n  enabled: true\n  allowed_cidrs: ['8.8.8.8/32']\n");
        for action in [Action::Status, Action::Devices] {
            let outcome = execute(Some(path.clone()), action);
            assert_eq!(outcome.code, EXIT_ENV);
            assert_eq!(outcome.body["code"], "CONFIG_ERROR");
            assert!(!outcome.body["message"].as_str().unwrap().contains('\n'));
            assert_eq!(run(Some(path.clone()), action), EXIT_ENV);
        }
    }

    #[test]
    fn enabled_status_reports_empty_scope_refusal() {
        let (_dir, path) = write_config("netconnect:\n  enabled: true\n  discovery: true\n");
        let outcome = execute(Some(path.clone()), Action::Status);
        assert_eq!(outcome.code, EXIT_OK);
        assert_eq!(outcome.body["scope_empty"], true);
        assert_eq!(outcome.body["active_tiers_refused"], true);
        let discovery = outcome.body["tiers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["tier"] == "discovery")
            .unwrap();
        assert_eq!(discovery["tier_enabled"], true);
        assert_eq!(discovery["runnable"], false);
        assert_eq!(run(Some(path), Action::Status), EXIT_OK);
    }

    #[test]
    fn throughput_warning_does_not_echo_the_endpoint() {
        let (_dir, path) = write_config(
            "netconnect:\n  enabled: false\n  throughput: true\n  throughput_endpoint: https://throughput-marker.example/x\n",
        );
        let outcome = execute(Some(path), Action::Status);
        assert_eq!(outcome.code, EXIT_REFUSED);
        assert!(outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("internet egress")));
        assert!(!outcome.warnings.join(" ").contains("throughput-marker.example"));
        assert!(!outcome.body.to_string().contains("throughput-marker.example"));
    }

    #[test]
    fn exit_code_map_matches_the_cli_api() {
        assert_eq!((EXIT_OK, EXIT_FAIL, EXIT_ENV, EXIT_REFUSED), (0, 2, 3, 4));
        assert_eq!(exit_code_for(&HarnessError::config("bad")), EXIT_ENV);
        assert_eq!(
            exit_code_for(&HarnessError::new("NETCONNECT_DISABLED", "off")),
            EXIT_REFUSED
        );
        assert_eq!(
            exit_code_for(&HarnessError::new("NETCONNECT_REFUSED", "out")),
            EXIT_REFUSED
        );
        assert_eq!(exit_code_for(&HarnessError::new("NETCONNECT_IO", "read")), EXIT_FAIL);
    }
}
