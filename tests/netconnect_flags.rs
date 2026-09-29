//! Table-driven lock for netconnect tier gates.
//!
//! A tier may run only when the master switch and that tier's flag are both
//! the literal boolean `true` and `allowed_cidrs` is non-empty. Anything else
//! is a refusal: a closed master exits 4 with `NETCONNECT_DISABLED` (not exit
//! 0), and an armed tier with an empty scope is reported refused. Scope
//! itself answers `NETCONNECT_REFUSED` from `Scope::check_target`.

#[path = "netconnect_support/flag_table.rs"]
mod flag_table;

use std::path::Path;
use std::process::Command;

use cgagentharness::common::config::AppConfig;
use cgagentharness::netconnect::cli::{self, Action};
use cgagentharness::netconnect::{NetconnectConfig, Scope, Tier};
use flag_table::{flag_cases, quoted_true_yaml, FlagCase};
use serde_json::Value;

fn load(yaml: &str) -> NetconnectConfig {
    let cfg = AppConfig::from_str(yaml, Path::new("fixture.yaml")).expect("yaml");
    NetconnectConfig::from_config(&cfg).expect("netconnect config")
}

fn invoke(yaml: &str, action: &str) -> (i32, Value) {
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
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let body: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|err| panic!("{action} did not print JSON ({err}); stdout={stdout} stderr={stderr}"));
    (out.status.code().unwrap_or(-1), body)
}

fn tier_row(body: &Value, tier: Tier) -> &Value {
    let rows = body["tiers"].as_array().expect("tiers array");
    assert_eq!(
        rows.len(),
        Tier::ALL.len(),
        "every tier is reported; a missing row would hide a gate"
    );
    rows.iter()
        .find(|row| row["tier"] == tier.key())
        .unwrap_or_else(|| panic!("missing tier {}", tier.key()))
}

fn assert_case(case: FlagCase) {
    let expect = case.master && case.tier_on && case.scope_nonempty;
    assert_eq!(case.may_run(), expect);
    let cfg = load(&case.yaml());
    assert_eq!(cfg.tier_enabled(case.tier), case.master && case.tier_on, "{case:?}");
    assert_eq!(cfg.tier_may_run(case.tier), expect, "{case:?}");
    for other in Tier::ALL {
        if other != case.tier {
            assert!(!cfg.tier_flag(other), "{other:?} must stay off in {case:?}");
            assert!(!cfg.tier_may_run(other), "{case:?}");
        }
    }

    if !case.scope_nonempty {
        let err = cfg
            .scope
            .check_target(std::net::Ipv4Addr::new(192, 168, 1, 1))
            .expect_err("empty scope refuses");
        assert!(err.is("NETCONNECT_REFUSED"), "{case:?} -> {err}");
        assert_eq!(cli::exit_code_for(&err), cli::EXIT_REFUSED);
    } else if expect {
        cfg.scope
            .check_target(std::net::Ipv4Addr::new(192, 168, 1, 9))
            .unwrap_or_else(|err| panic!("in-scope target refused: {err}"));
    }

    let (code, body) = invoke(&case.yaml(), "status");
    let row = tier_row(&body, case.tier);
    assert_eq!(row["runnable"].as_bool().unwrap(), expect, "{case:?} {body}");
    assert_eq!(row["flag"].as_bool().unwrap(), case.tier_on, "{case:?}");
    assert_eq!(
        row["tier_enabled"].as_bool().unwrap(),
        case.master && case.tier_on,
        "{case:?}"
    );

    if !case.master {
        assert_eq!(code, i32::from(cli::EXIT_REFUSED), "{case:?} {body}");
        assert_eq!(body["ok"], false);
        assert_eq!(body["code"], "NETCONNECT_DISABLED");
        assert_eq!(body["message"], "netconnect.enabled is false");
        let (devices, devices_body) = invoke(&case.yaml(), "devices");
        assert_eq!(devices, i32::from(cli::EXIT_REFUSED), "{case:?} {devices_body}");
        assert_eq!(devices_body["code"], "NETCONNECT_DISABLED");
        assert!(devices_body.get("interfaces").is_none(), "{devices_body}");
        assert!(devices_body.get("packets_sent").is_none(), "{devices_body}");
        assert!(devices_body.get("neighbors").is_none(), "{devices_body}");
    } else if expect {
        assert_eq!(code, i32::from(cli::EXIT_OK), "{case:?} {body}");
        assert_eq!(body["ok"], true);
        assert_eq!(body["active_tiers_refused"], false);
        assert_eq!(body["scope_empty"], false);
    } else {
        assert_eq!(code, i32::from(cli::EXIT_OK), "{case:?} {body}");
        assert_eq!(body["ok"], true);
        assert_eq!(
            row["runnable"], false,
            "a closed tier must be reported refused, not omitted"
        );
        let armed_without_scope = case.tier_on && !case.scope_nonempty;
        assert_eq!(body["active_tiers_refused"], armed_without_scope, "{case:?} {body}");
        if armed_without_scope {
            assert_eq!(body["scope_empty"], true);
            assert_eq!(row["tier_enabled"], true);
        }
    }
}

#[test]
fn exit_codes_match_the_cli_contract() {
    assert_eq!(
        (cli::EXIT_OK, cli::EXIT_FAIL, cli::EXIT_ENV, cli::EXIT_REFUSED),
        (0, 2, 3, 4)
    );
    let _ = Action::Status;
}

#[test]
fn flag_table_covers_every_tier_once_per_combination() {
    let cases = flag_cases();
    assert_eq!(cases.len(), Tier::ALL.len() * 8);
    let mut seen = std::collections::BTreeSet::new();
    for case in &cases {
        assert!(seen.insert((case.tier.key(), case.master, case.tier_on, case.scope_nonempty)));
    }
    for tier in Tier::ALL {
        let rows = cases.iter().filter(|case| case.tier == tier).count();
        assert_eq!(rows, 8, "{}", tier.key());
        let runnable = cases
            .iter()
            .filter(|case| case.tier == tier && case.master && case.tier_on && case.scope_nonempty)
            .count();
        assert_eq!(runnable, 1, "{}", tier.key());
    }
}

#[test]
fn every_combination_refuses_unless_master_flag_and_scope_all_hold() {
    for case in flag_cases() {
        assert_case(case);
    }
}

#[test]
fn quoted_true_does_not_arm_a_tier_or_exit_zero() {
    for tier in Tier::ALL {
        for (master_quoted, flag_quoted) in [(true, true), (true, false), (false, true)] {
            let yaml = quoted_true_yaml(tier, master_quoted, flag_quoted);
            let cfg = load(&yaml);
            assert!(!cfg.tier_may_run(tier), "{yaml}");
            if master_quoted {
                assert!(!cfg.enabled);
                assert!(!cfg.tier_enabled(tier));
                let (code, body) = invoke(&yaml, "status");
                assert_eq!(code, i32::from(cli::EXIT_REFUSED), "{body}");
                assert_eq!(body["code"], "NETCONNECT_DISABLED");
                assert_eq!(tier_row(&body, tier)["runnable"], false);
                let (devices, devices_body) = invoke(&yaml, "devices");
                assert_eq!(devices, i32::from(cli::EXIT_REFUSED));
                assert!(devices_body.get("packets_sent").is_none());
            } else {
                assert!(cfg.enabled);
                assert!(!cfg.tier_flag(tier));
                assert!(!cfg.tier_enabled(tier));
                let (code, body) = invoke(&yaml, "status");
                assert_eq!(code, i32::from(cli::EXIT_OK), "{body}");
                let row = tier_row(&body, tier);
                assert_eq!(row["flag"], false);
                assert_eq!(row["tier_enabled"], false);
                assert_eq!(row["runnable"], false);
            }
        }
    }
}

#[test]
fn in_scope_check_target_refuses_addresses_outside_the_cidr() {
    let scope = Scope::parse(&["192.168.1.0/24".to_string()]).unwrap();
    scope.check_target(std::net::Ipv4Addr::new(192, 168, 1, 9)).unwrap();
    for addr in [
        std::net::Ipv4Addr::new(8, 8, 8, 8),
        std::net::Ipv4Addr::new(192, 168, 2, 1),
        std::net::Ipv4Addr::UNSPECIFIED,
    ] {
        let err = scope.check_target(addr).unwrap_err();
        assert!(err.is("NETCONNECT_REFUSED"), "{addr} {err}");
        assert_eq!(cli::exit_code_for(&err), cli::EXIT_REFUSED);
    }
}
