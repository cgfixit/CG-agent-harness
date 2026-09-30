//! Typed view of the `netconnect` section.
//!
//! Numeric defaults live in `assets/config.default.yaml`. Missing keys on an
//! older home use that shipped file. A present but ill-typed value is a
//! config error. Boolean gates use `flag_is_true`: only an unquoted `true`
//! arms a gate.

use std::net::Ipv4Addr;
use std::path::Path;

use serde_yaml_ng::Value;

use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};

use super::scope::Scope;

/// Internet egress outside the LAN scope. Passive commands do not use it.
pub const THROUGHPUT_WARNING: &str = concat!(
    "netconnect throughput is configured; it is internet egress outside LAN scope ",
    "and passive commands do not use it"
);

const HOST_CAP: (u64, u64) = (1, 4096);
const RATE_PER_MIN: (u64, u64) = (1, 10_000);
const TIMEOUT_MS: (u64, u64) = (100, 60_000);
const NAME_CAP: (u64, u64) = (8, 256);
const MAX_PORTS: usize = 128;
const MAX_TARGETS: usize = 64;
const MAX_ENDPOINT_CHARS: usize = 256;

/// Tiers another crate can table-test. Order is the config-key order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    PassiveListen,
    Discovery,
    PortScan,
    Diagnostics,
    Throughput,
    AnomalyDetection,
    HomeAutomation,
}

impl Tier {
    pub const ALL: [Tier; 7] = [
        Tier::PassiveListen,
        Tier::Discovery,
        Tier::PortScan,
        Tier::Diagnostics,
        Tier::Throughput,
        Tier::AnomalyDetection,
        Tier::HomeAutomation,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Tier::PassiveListen => "passive_listen",
            Tier::Discovery => "discovery",
            Tier::PortScan => "port_scan",
            Tier::Diagnostics => "diagnostics",
            Tier::Throughput => "throughput",
            Tier::AnomalyDetection => "anomaly_detection",
            Tier::HomeAutomation => "home_automation",
        }
    }
}

/// Effective netconnect settings. Tier flags are private so callers use
/// [`NetconnectConfig::tier_enabled`] and [`NetconnectConfig::tier_may_run`].
#[derive(Debug, Clone)]
pub struct NetconnectConfig {
    pub enabled: bool,
    passive_listen: bool,
    discovery: bool,
    port_scan: bool,
    diagnostics: bool,
    throughput: bool,
    anomaly_detection: bool,
    home_automation: bool,
    pub scope: Scope,
    pub port_allowlist: Vec<u16>,
    pub host_cap: u64,
    pub rate_limit_per_min: u64,
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub diagnostics_targets: Vec<Ipv4Addr>,
    pub throughput_endpoint: Option<String>,
    pub ha_endpoint: Option<String>,
    pub mqtt_endpoint: Option<String>,
    pub untrusted_string_max_chars: usize,
    pub warnings: Vec<String>,
}

impl NetconnectConfig {
    pub fn from_config(cfg: &AppConfig) -> Result<Self> {
        if cfg.get("netconnect").is_some_and(|value| !value.is_mapping()) {
            return Err(HarnessError::config("netconnect must be a mapping"));
        }
        let defaults = AppConfig::from_str(AppConfig::embedded_default(), Path::new("defaults"))?;
        let enabled = cfg.flag_is_true("netconnect.enabled");
        let passive_listen = cfg.flag_is_true("netconnect.passive_listen");
        let discovery = cfg.flag_is_true("netconnect.discovery");
        let port_scan = cfg.flag_is_true("netconnect.port_scan");
        let diagnostics = cfg.flag_is_true("netconnect.diagnostics");
        let throughput = cfg.flag_is_true("netconnect.throughput");
        let anomaly_detection = cfg.flag_is_true("netconnect.anomaly_detection");
        let home_automation = cfg.flag_is_true("netconnect.home_automation");

        let scope = Scope::parse(&string_list(
            cfg,
            "netconnect.allowed_cidrs",
            super::scope::MAX_ENTRIES,
        )?)?;
        let port_allowlist = port_list(cfg)?;
        let diagnostics_targets = target_list(cfg, &scope)?;
        let throughput_endpoint = endpoint(cfg, "netconnect.throughput_endpoint")?;
        let ha_endpoint = endpoint(cfg, "netconnect.ha_endpoint")?;
        let mqtt_endpoint = endpoint(cfg, "netconnect.mqtt_endpoint")?;

        let mut warnings = Vec::new();
        if throughput || throughput_endpoint.is_some() {
            warnings.push(THROUGHPUT_WARNING.to_string());
        }

        Ok(Self {
            enabled,
            passive_listen,
            discovery,
            port_scan,
            diagnostics,
            throughput,
            anomaly_detection,
            home_automation,
            scope,
            port_allowlist,
            host_cap: integer(cfg, &defaults, "host_cap", HOST_CAP)?,
            rate_limit_per_min: integer(cfg, &defaults, "rate_limit_per_min", RATE_PER_MIN)?,
            connect_timeout_ms: integer(cfg, &defaults, "connect_timeout_ms", TIMEOUT_MS)?,
            read_timeout_ms: integer(cfg, &defaults, "read_timeout_ms", TIMEOUT_MS)?,
            diagnostics_targets,
            throughput_endpoint,
            ha_endpoint,
            mqtt_endpoint,
            untrusted_string_max_chars: integer(cfg, &defaults, "untrusted_string_max_chars", NAME_CAP)? as usize,
            warnings,
        })
    }

    pub fn tier_flag(&self, tier: Tier) -> bool {
        match tier {
            Tier::PassiveListen => self.passive_listen,
            Tier::Discovery => self.discovery,
            Tier::PortScan => self.port_scan,
            Tier::Diagnostics => self.diagnostics,
            Tier::Throughput => self.throughput,
            Tier::AnomalyDetection => self.anomaly_detection,
            Tier::HomeAutomation => self.home_automation,
        }
    }

    /// Master flag AND the tier flag. Scope is not part of this predicate.
    pub fn tier_enabled(&self, tier: Tier) -> bool {
        self.enabled && self.tier_flag(tier)
    }

    /// Armed tier that is allowed to run. An empty scope refuses every tier.
    pub fn tier_may_run(&self, tier: Tier) -> bool {
        self.tier_enabled(tier) && !self.scope.is_empty()
    }
}

fn integer(cfg: &AppConfig, defaults: &AppConfig, name: &str, bounds: (u64, u64)) -> Result<u64> {
    let key = format!("netconnect.{name}");
    let (min, max) = bounds;
    let value = if cfg.get(&key).is_some() {
        cfg.get(&key)
    } else {
        defaults.get(&key)
    };
    let Some(Value::Number(number)) = value else {
        return Err(HarnessError::config(format!(
            "{key} must be an integer from {min} to {max}"
        )));
    };
    let parsed = number.as_u64().filter(|n| (min..=max).contains(n));
    parsed.ok_or_else(|| HarnessError::config(format!("{key} must be an integer from {min} to {max}")))
}

fn string_list(cfg: &AppConfig, key: &str, max: usize) -> Result<Vec<String>> {
    match cfg.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Sequence(items)) => {
            if items.len() > max {
                return Err(HarnessError::config(format!("{key} has more than {max} entries")));
            }
            let mut out = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let Some(text) = item.as_str() else {
                    return Err(HarnessError::config(format!("{key}[{index}] must be a string")));
                };
                out.push(text.to_string());
            }
            Ok(out)
        }
        Some(_) => Err(HarnessError::config(format!("{key} must be a list"))),
    }
}

fn port_list(cfg: &AppConfig) -> Result<Vec<u16>> {
    let key = "netconnect.port_allowlist";
    match cfg.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Sequence(items)) => {
            if items.len() > MAX_PORTS {
                return Err(HarnessError::config(format!("{key} has more than {MAX_PORTS} entries")));
            }
            let mut ports = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let Some(port) = item.as_u64().filter(|n| (1..=65535).contains(n)) else {
                    return Err(HarnessError::config(format!(
                        "{key}[{index}] must be an integer from 1 to 65535"
                    )));
                };
                let port = port as u16;
                if ports.contains(&port) {
                    return Err(HarnessError::config(format!("{key}[{index}] duplicates {port}")));
                }
                ports.push(port);
            }
            Ok(ports)
        }
        Some(_) => Err(HarnessError::config(format!("{key} must be a list"))),
    }
}

fn target_list(cfg: &AppConfig, scope: &Scope) -> Result<Vec<Ipv4Addr>> {
    let key = "netconnect.diagnostics_targets";
    let texts = string_list(cfg, key, MAX_TARGETS)?;
    let mut targets = Vec::with_capacity(texts.len());
    for (index, text) in texts.iter().enumerate() {
        let trimmed = text.trim();
        if trimmed.contains(':') {
            return Err(HarnessError::config(format!("{key}[{index}] must be an IPv4 address")));
        }
        let Ok(addr) = trimmed.parse::<Ipv4Addr>() else {
            return Err(HarnessError::config(format!("{key}[{index}] must be an IPv4 address")));
        };
        if !scope.contains(addr) {
            return Err(HarnessError::config(format!(
                "{key}[{index}] {addr} is outside allowed_cidrs"
            )));
        }
        if targets.contains(&addr) {
            return Err(HarnessError::config(format!("{key}[{index}] duplicates {addr}")));
        }
        targets.push(addr);
    }
    Ok(targets)
}

fn endpoint(cfg: &AppConfig, key: &str) -> Result<Option<String>> {
    match cfg.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) => {
            let text = raw.trim();
            if text.is_empty()
                || text.chars().any(|c| c.is_control())
                || text.chars().count() > MAX_ENDPOINT_CHARS
                || text.contains('@')
                || text.contains('?')
                || text.contains('#')
            {
                return Err(HarnessError::config(format!(
                    "{key} must be an endpoint without credentials, a query, or a fragment"
                )));
            }
            Ok(Some(text.to_string()))
        }
        Some(_) => Err(HarnessError::config(format!("{key} must be a string or null"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(yaml: &str) -> AppConfig {
        AppConfig::from_str(yaml, Path::new("fixture.yaml")).unwrap()
    }

    fn load(yaml: &str) -> NetconnectConfig {
        NetconnectConfig::from_config(&app(yaml)).unwrap()
    }

    #[test]
    fn shipped_defaults_are_closed_and_unscoped() {
        let cfg = load(AppConfig::embedded_default());
        assert!(!cfg.enabled);
        assert!(cfg.scope.is_empty());
        assert!(cfg.warnings.is_empty());
        assert!(cfg.throughput_endpoint.is_none());
        assert!(cfg.ha_endpoint.is_none());
        assert!(cfg.mqtt_endpoint.is_none());
        assert!(cfg.diagnostics_targets.is_empty());
        assert!(cfg.port_allowlist.is_empty());
        for tier in Tier::ALL {
            assert!(!cfg.tier_flag(tier), "{}", tier.key());
            assert!(!cfg.tier_enabled(tier));
            assert!(!cfg.tier_may_run(tier));
        }
    }

    #[test]
    fn absent_section_stays_closed() {
        let cfg = load("app:\n  name: test\n");
        assert!(!cfg.enabled);
        assert!(cfg.scope.is_empty());
        assert!(!cfg.tier_enabled(Tier::Discovery));
        assert!(!cfg.tier_may_run(Tier::Discovery));
    }

    #[test]
    fn tier_enabled_is_the_and_of_master_and_flag() {
        for tier in Tier::ALL {
            let armed = load(&format!(
                "netconnect:\n  enabled: true\n  {}: true\n  allowed_cidrs: ['192.168.0.0/16']\n",
                tier.key()
            ));
            assert!(armed.tier_enabled(tier), "{}", tier.key());
            assert!(armed.tier_may_run(tier));
            for other in Tier::ALL {
                if other != tier {
                    assert!(!armed.tier_enabled(other), "{}", other.key());
                }
            }

            let master_off = load(&format!(
                "netconnect:\n  enabled: false\n  {}: true\n  allowed_cidrs: ['192.168.0.0/16']\n",
                tier.key()
            ));
            assert!(master_off.tier_flag(tier));
            assert!(!master_off.tier_enabled(tier));
            assert!(!master_off.tier_may_run(tier));
        }

        let mut yaml = String::from("netconnect:\n  enabled: true\n  allowed_cidrs: ['10.1.0.0/16']\n");
        for tier in Tier::ALL {
            yaml.push_str(&format!("  {}: true\n", tier.key()));
        }
        let all = load(&yaml);
        for tier in Tier::ALL {
            assert!(all.tier_enabled(tier));
            assert!(all.tier_may_run(tier));
        }
        let quoted =
            load("netconnect:\n  enabled: \"true\"\n  discovery: \"true\"\n  allowed_cidrs: ['192.168.0.0/16']\n");
        assert!(!quoted.enabled);
        assert!(!quoted.tier_enabled(Tier::Discovery));
        assert!(quoted.warnings.is_empty());
    }

    #[test]
    fn empty_scope_refuses_armed_tiers() {
        let mut yaml = String::from("netconnect:\n  enabled: true\n");
        for tier in Tier::ALL {
            yaml.push_str(&format!("  {}: true\n", tier.key()));
        }
        let cfg = load(&yaml);
        assert!(cfg.scope.is_empty());
        for tier in Tier::ALL {
            assert!(cfg.tier_enabled(tier), "{}", tier.key());
            assert!(!cfg.tier_may_run(tier), "{}", tier.key());
        }
    }

    #[test]
    fn bad_scope_is_a_config_error() {
        let err =
            NetconnectConfig::from_config(&app("netconnect:\n  enabled: true\n  allowed_cidrs: ['8.8.8.8/32']\n"))
                .unwrap_err();
        assert!(err.is("CONFIG_ERROR"));
        assert!(err.message.contains("outside RFC1918 and loopback"));
    }

    #[test]
    fn throughput_warns_when_the_flag_or_endpoint_is_set() {
        let flag = load("netconnect:\n  throughput: true\n");
        assert_eq!(flag.warnings, vec![THROUGHPUT_WARNING.to_string()]);
        assert!(!flag.tier_enabled(Tier::Throughput));
        let endpoint = load("netconnect:\n  throughput_endpoint: https://203.0.113.5/speed\n");
        assert_eq!(endpoint.warnings, vec![THROUGHPUT_WARNING.to_string()]);
        assert_eq!(
            endpoint.throughput_endpoint.as_deref(),
            Some("https://203.0.113.5/speed")
        );
        assert!(!endpoint.warnings[0].contains("203.0.113.5"));
        let quoted = load("netconnect:\n  throughput: \"true\"\n");
        assert!(quoted.warnings.is_empty());
    }

    #[test]
    fn endpoints_reject_credentials_without_echoing_them() {
        let secret = "s3cret-token";
        let err = NetconnectConfig::from_config(&app(&format!(
            "netconnect:\n  mqtt_endpoint: mqtt://user:{secret}@192.168.1.2:1883\n"
        )))
        .unwrap_err();
        assert!(err.is("CONFIG_ERROR"));
        assert!(!err.message.contains(secret));
        assert!(NetconnectConfig::from_config(&app(
            "netconnect:\n  ha_endpoint: https://192.168.1.2:8123/api?token=s3cret-token\n"
        ))
        .unwrap_err()
        .message
        .contains("without credentials"));
    }

    #[test]
    fn diagnostics_targets_must_be_in_scope_ipv4() {
        let ok = load("netconnect:\n  allowed_cidrs: ['192.168.0.0/16']\n  diagnostics_targets: ['192.168.1.9']\n");
        assert_eq!(ok.diagnostics_targets, vec![Ipv4Addr::new(192, 168, 1, 9)]);
        for yaml in [
            "netconnect:\n  allowed_cidrs: ['192.168.0.0/16']\n  diagnostics_targets: ['8.8.8.8']\n",
            "netconnect:\n  diagnostics_targets: ['192.168.1.9']\n",
            "netconnect:\n  allowed_cidrs: ['192.168.0.0/16']\n  diagnostics_targets: ['fe80::1']\n",
            "netconnect:\n  allowed_cidrs: ['192.168.0.0/16']\n  diagnostics_targets: ['printer.local']\n",
        ] {
            assert!(NetconnectConfig::from_config(&app(yaml)).is_err(), "{yaml}");
        }
    }

    #[test]
    fn limits_come_from_config_and_reject_out_of_range() {
        let cfg = load("netconnect:\n  host_cap: 4\n  untrusted_string_max_chars: 12\n  port_allowlist: [80, 443]\n");
        assert_eq!(cfg.host_cap, 4);
        assert_eq!(cfg.untrusted_string_max_chars, 12);
        assert_eq!(cfg.port_allowlist, vec![80, 443]);
        assert!(NetconnectConfig::from_config(&app("netconnect:\n  host_cap: 0\n")).is_err());
        assert!(NetconnectConfig::from_config(&app("netconnect:\n  port_allowlist: [0]\n")).is_err());
        assert!(NetconnectConfig::from_config(&app("netconnect:\n  connect_timeout_ms: 1\n")).is_err());
    }
}
