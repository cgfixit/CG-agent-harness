//! `/net` and its exact aliases.
//!
//! An exact alias plus an exact read-only subcommand runs. `status` returns
//! config and scope and does not load collectors. `devices` reads the passive
//! tables. A near-miss prints `did you mean` and does not read tables or
//! connect. `device` is exact-only:
//! fuzzy matching never selects it, and the exact command always refuses.
//! Active subcommands refuse when [`NetconnectConfig::tier_may_run`] is false
//! and still do not connect when it is true.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::common::config::AppConfig;
use crate::common::edit_distance;
use crate::common::errors::HarnessError;
use crate::common::home::Home;

use super::cli::{exit_code_for, EXIT_FAIL, EXIT_OK};
use super::collect::{InterfaceSource, NeighborSource, RouteSource};
use super::config::{NetconnectConfig, Tier};
use super::sources::{LiveInterfaces, LiveNeighbors, LiveRoutes};
use super::tools;

/// Candidate aliases, checked against the slash registry before they are kept.
pub const CANDIDATE_ALIASES: &[&str] = &["net", "netconnect", "lan", "scan", "ports", "speed"];

const EXACT_SUBS: &[&str] = &["status", "devices", "ports", "diag", "watch", "device"];

/// Fuzzy candidates. `device` is omitted on purpose.
const FUZZY_SUBS: &[&str] = &["status", "devices", "ports", "diag", "watch"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Dispatch { canonical: String, sub: &'static str },
    Suggest { notice: String, suggestions: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub code: u8,
    pub executed: bool,
    pub notice: String,
    pub suggestions: Vec<String>,
    pub body: Value,
}

pub fn classify_aliases(reserved: &BTreeSet<&str>) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for alias in CANDIDATE_ALIASES {
        if reserved.contains(*alias) {
            dropped.push(*alias);
        } else {
            kept.push(*alias);
        }
    }
    (kept, dropped)
}

pub fn interpret(cmd: &str, rest: &[&str], reserved: &BTreeSet<&str>) -> Option<Intent> {
    let (kept, _) = classify_aliases(reserved);
    if kept.contains(&cmd) {
        return Some(parse_exact(rest));
    }
    let near = nearby(cmd, &kept);
    if near.is_empty() {
        return None;
    }
    let suggestions = vec!["/net status".to_string(), "/net devices".to_string()];
    Some(suggest(suggestions))
}

fn parse_exact(rest: &[&str]) -> Intent {
    if rest.len() != 1 {
        return suggest(vec!["/net status".to_string(), "/net devices".to_string()]);
    }
    let sub = rest[0].to_ascii_lowercase();
    if let Some(exact) = EXACT_SUBS.iter().copied().find(|name| *name == sub) {
        return Intent::Dispatch {
            canonical: format!("/net {exact}"),
            sub: exact,
        };
    }
    let near = nearby(&sub, FUZZY_SUBS);
    let suggestions = if near.is_empty() {
        vec!["/net status".to_string()]
    } else {
        near.into_iter().map(|name| format!("/net {name}")).collect()
    };
    suggest(suggestions)
}

fn suggest(suggestions: Vec<String>) -> Intent {
    let notice = format!("did you mean {}?", suggestions.join(" or "));
    Intent::Suggest { notice, suggestions }
}

fn nearby<'a>(token: &str, candidates: &[&'a str]) -> Vec<&'a str> {
    let mut scored: Vec<(&str, u8)> = candidates
        .iter()
        .copied()
        .filter_map(|candidate| {
            if candidate == token {
                return None;
            }
            let distance = edit_distance(token, candidate);
            if distance == 1 || (candidate.starts_with(token) && token.len() >= 3) {
                Some((candidate, 70))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    scored.truncate(5);
    scored.into_iter().map(|(name, _)| name).collect()
}

pub fn run(config: Option<PathBuf>, line: &str) -> u8 {
    let outcome = evaluate_path(config, line);
    for line in outcome.notice.lines() {
        if !line.is_empty() {
            eprintln!("netconnect: {line}");
        }
    }
    match serde_json::to_string_pretty(&outcome.body) {
        Ok(text) => println!("{text}"),
        Err(_) => {
            eprintln!("netconnect: internal error");
            return EXIT_FAIL;
        }
    }
    outcome.code
}

pub fn evaluate_path(config: Option<PathBuf>, line: &str) -> Outcome {
    let reserved = crate::server::slash::preexisting_slash_names();
    if let Some(Intent::Suggest { notice, suggestions }) = interpret_line(line, &reserved) {
        return suggest_outcome(notice, suggestions);
    }
    let path = config.unwrap_or_else(|| Home::default_root().join("config.yaml"));
    let app = match AppConfig::load(&path) {
        Ok(cfg) => cfg,
        Err(err) => return error_outcome(&err),
    };
    let cfg = match NetconnectConfig::from_config(&app) {
        Ok(cfg) => cfg,
        Err(err) => return error_outcome(&err),
    };
    evaluate(&cfg, line, &LiveNeighbors, &LiveRoutes, &LiveInterfaces)
}

pub fn evaluate(
    cfg: &NetconnectConfig,
    line: &str,
    neighbors: &impl NeighborSource,
    routes: &impl RouteSource,
    interfaces: &impl InterfaceSource,
) -> Outcome {
    let reserved = crate::server::slash::preexisting_slash_names();
    let Some(intent) = interpret_line(line, &reserved) else {
        return suggest_outcome("did you mean /net status?".to_string(), vec!["/net status".to_string()]);
    };
    match intent {
        Intent::Suggest { notice, suggestions } => suggest_outcome(notice, suggestions),
        Intent::Dispatch { canonical, sub } => dispatch(cfg, &canonical, sub, neighbors, routes, interfaces),
    }
}

fn interpret_line(line: &str, reserved: &BTreeSet<&str>) -> Option<Intent> {
    if line.chars().any(|c| c.is_control()) {
        return Some(suggest(vec!["/net status".to_string()]));
    }
    let trimmed = line.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let body = trimmed.trim_start_matches('/');
    let tokens: Vec<&str> = body.split_whitespace().filter(|token| !token.is_empty()).collect();
    let cmd = tokens.first()?.to_ascii_lowercase();
    interpret(&cmd, &tokens[1..], reserved)
}

fn dispatch(
    cfg: &NetconnectConfig,
    canonical: &str,
    sub: &str,
    neighbors: &impl NeighborSource,
    routes: &impl RouteSource,
    interfaces: &impl InterfaceSource,
) -> Outcome {
    let result = match sub {
        "status" => tools::call_status(cfg),
        "devices" => tools::call_devices(cfg, neighbors, routes, interfaces),
        "ports" => tools::invoke_tier(cfg, Tier::PortScan, None),
        "diag" => tools::invoke_tier(cfg, Tier::Diagnostics, None),
        "watch" => tools::invoke_tier(cfg, Tier::PassiveListen, None),
        "device" => Err(HarnessError::new(
            "NETCONNECT_REFUSED",
            "device control is refused; home_automation is not in this build",
        )),
        _ => Err(HarnessError::new("NETCONNECT_REFUSED", "unknown netconnect subcommand")),
    };
    match result {
        Ok(body) => Outcome {
            code: EXIT_OK,
            executed: true,
            notice: String::new(),
            suggestions: Vec::new(),
            body,
        },
        Err(err) => {
            let mut outcome = error_outcome(&err);
            outcome.body["command"] = json!(canonical);
            outcome
        }
    }
}

fn suggest_outcome(notice: String, suggestions: Vec<String>) -> Outcome {
    Outcome {
        code: EXIT_FAIL,
        executed: false,
        body: json!({
            "ok": false,
            "code": "NETCONNECT_SUGGEST",
            "message": notice,
            "suggestions": suggestions,
        }),
        notice,
        suggestions,
    }
}

fn error_outcome(err: &HarnessError) -> Outcome {
    Outcome {
        code: exit_code_for(err),
        executed: false,
        notice: err.message.clone(),
        suggestions: Vec::new(),
        body: json!({
            "ok": false,
            "code": err.code,
            "message": err.message,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::common::config::AppConfig;
    use crate::netconnect::collect::{FixtureInterfaces, FixtureNeighbors, FixtureRoutes};
    use crate::netconnect::parse::{InterfaceRecord, NeighborRecord, RouteRecord};

    fn reserved_without(names: &[&str]) -> BTreeSet<&'static str> {
        crate::server::slash::preexisting_slash_names()
            .into_iter()
            .filter(|name| !names.contains(name))
            .collect()
    }

    fn cfg(yaml: &str) -> NetconnectConfig {
        NetconnectConfig::from_config(&AppConfig::from_str(yaml, Path::new("fixture.yaml")).unwrap()).unwrap()
    }

    struct FlagSource(Arc<AtomicBool>);

    impl NeighborSource for FlagSource {
        fn load(&self) -> crate::common::errors::Result<Vec<NeighborRecord>> {
            self.0.store(true, Ordering::SeqCst);
            Ok(Vec::new())
        }
    }
    impl RouteSource for FlagSource {
        fn load(&self) -> crate::common::errors::Result<Vec<RouteRecord>> {
            self.0.store(true, Ordering::SeqCst);
            Ok(Vec::new())
        }
    }
    impl InterfaceSource for FlagSource {
        fn load(&self) -> crate::common::errors::Result<Vec<InterfaceRecord>> {
            self.0.store(true, Ordering::SeqCst);
            Ok(Vec::new())
        }
    }

    #[test]
    fn collision_check_drops_reserved_names_and_keeps_free_aliases() {
        let preexisting = crate::server::slash::preexisting_slash_names();
        let (kept, dropped) = classify_aliases(&preexisting);
        assert!(dropped.is_empty(), "dropped aliases: {dropped:?}");
        assert_eq!(kept, CANDIDATE_ALIASES);
        for alias in kept {
            assert!(!preexisting.contains(alias), "{alias} collides");
        }
        let mut reserved = preexisting.clone();
        reserved.insert("scan");
        reserved.insert("status");
        let (kept, dropped) = classify_aliases(&reserved);
        assert!(dropped.contains(&"scan"));
        assert!(!kept.contains(&"scan"));
        assert!(!dropped.contains(&"status"), "status is not a candidate alias");
        assert!(classify_aliases(&reserved_without(&[])).0.contains(&"ports"));
    }

    #[test]
    fn exact_alias_runs_read_only_subcommand() {
        let loaded = cfg("netconnect:\n  enabled: true\n  allowed_cidrs: ['192.168.0.0/16']\n");
        let neighbors =
            FixtureNeighbors::bsd_arp("livingroom (192.168.1.50) at ab:cd:ef:01:23:45 on en0 ifscope [ethernet]\n");
        for line in [
            "/net devices",
            "/netconnect devices",
            "/lan devices",
            "/scan devices",
            "/ports devices",
            "/speed devices",
        ] {
            let outcome = evaluate(
                &loaded,
                line,
                &neighbors,
                &FixtureRoutes::proc_route(""),
                &FixtureInterfaces::from_lines("en0 192.168.1.20\n"),
            );
            assert_eq!(outcome.code, EXIT_OK, "{line}: {}", outcome.notice);
            assert!(outcome.executed, "{line}");
            assert_eq!(outcome.body["neighbors"][0]["address"], "192.168.1.50");
            assert_eq!(outcome.body["neighbors"][0]["hostname"]["untrusted"], true);
            assert_eq!(outcome.body["packets_sent"], 0);
        }
        let status = evaluate(
            &loaded,
            "/net status",
            &neighbors,
            &FixtureRoutes::proc_route(""),
            &FixtureInterfaces::from_lines(""),
        );
        assert_eq!(status.code, EXIT_OK);
        assert!(status.executed);
        assert_eq!(status.body["enabled"], true);
    }

    /// Counting source that panics if `/net status` loads a collector.
    struct Boom(Arc<AtomicUsize>);

    impl NeighborSource for Boom {
        fn load(&self) -> crate::common::errors::Result<Vec<NeighborRecord>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            panic!("status must not call a collector");
        }
    }
    impl RouteSource for Boom {
        fn load(&self) -> crate::common::errors::Result<Vec<RouteRecord>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            panic!("status must not call a collector");
        }
    }
    impl InterfaceSource for Boom {
        fn load(&self) -> crate::common::errors::Result<Vec<InterfaceRecord>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            panic!("status must not call a collector");
        }
    }

    #[test]
    fn enabled_passive_status_skips_collectors_and_devices_show_fixtures() {
        let loaded = cfg(
            "netconnect:\n  enabled: true\n  passive_listen: false\n  discovery: false\n  port_scan: false\n  \
             diagnostics: false\n  throughput: false\n  anomaly_detection: false\n  home_automation: false\n  \
             allowed_cidrs: ['127.0.0.0/16']\n",
        );
        for tier in Tier::ALL {
            assert!(!loaded.tier_flag(tier), "{}", tier.key());
        }
        let loads = Arc::new(AtomicUsize::new(0));
        let boom = Boom(Arc::clone(&loads));
        let status = evaluate(&loaded, "/net status", &boom, &boom, &boom);
        assert_eq!(status.code, EXIT_OK, "{}", status.notice);
        assert!(status.executed);
        assert_eq!(status.body["enabled"], true);
        assert_eq!(status.body["scope_empty"], false);
        assert_eq!(status.body["allowed_cidrs"][0], "127.0.0.0/16");
        assert!(status.body.get("neighbors").is_none());
        assert_eq!(loads.load(Ordering::SeqCst), 0);

        let neighbors = FixtureNeighbors::bsd_arp(
            "lo\u{0001}cal (127.0.0.1) at ab:cd:ef:01:23:45 on lo0 ifscope [ethernet]\n\
             outsider (192.168.1.50) at aa:bb:cc:dd:ee:ff on en0 ifscope [ethernet]\n",
        );
        let routes = FixtureRoutes::bsd_netstat(
            "Destination        Gateway            Flags        Netif\n\
             127.0.0.1          127.0.0.1          UH           lo0\n",
        );
        let interfaces = FixtureInterfaces::from_lines("lo0 127.0.0.1\n");
        let outcome = evaluate(&loaded, "/net devices", &neighbors, &routes, &interfaces);
        assert_eq!(outcome.code, EXIT_OK, "{}", outcome.notice);
        assert!(outcome.executed);
        assert_eq!(outcome.body["packets_sent"], 0);
        assert_eq!(outcome.body["neighbors"][0]["address"], "127.0.0.1");
        assert_eq!(outcome.body["neighbors"][0]["hostname"]["untrusted"], true);
        assert_eq!(outcome.body["neighbors"][0]["hostname"]["value"], "local");
        assert_eq!(outcome.body["interfaces"][0]["addresses"][0], "127.0.0.1");
        assert_eq!(outcome.body["interfaces"][0]["name"]["untrusted"], true);
        let rendered = outcome.body.to_string();
        assert!(!rendered.contains('\u{0001}'));
        assert!(!rendered.contains("192.168"));
    }

    #[test]
    fn fuzzy_alias_suggests_and_does_not_execute() {
        let hit = Arc::new(AtomicBool::new(false));
        let source = FlagSource(Arc::clone(&hit));
        let loaded = cfg("netconnect:\n  enabled: true\n  allowed_cidrs: ['192.168.0.0/16']\n");
        for line in ["/nett devices", "/scann status", "/lanx devices", "/net status please"] {
            let outcome = evaluate(&loaded, line, &source, &source, &source);
            assert!(!outcome.executed, "{line}");
            assert_eq!(outcome.code, EXIT_FAIL, "{line}");
            assert!(outcome.notice.contains("did you mean"), "{line}: {}", outcome.notice);
            assert!(!hit.load(Ordering::SeqCst), "{line}");
        }
    }

    #[test]
    fn fuzzy_never_resolves_to_device() {
        let hit = Arc::new(AtomicBool::new(false));
        let source = FlagSource(Arc::clone(&hit));
        let loaded =
            cfg("netconnect:\n  enabled: true\n  home_automation: true\n  allowed_cidrs: ['192.168.0.0/16']\n");
        for line in ["/net devic", "/net devicee", "/net devixe", "/nett device"] {
            let outcome = evaluate(&loaded, line, &source, &source, &source);
            assert!(!outcome.executed, "{line}");
            assert!(outcome.notice.contains("did you mean"), "{line}: {}", outcome.notice);
            assert!(
                outcome.suggestions.iter().all(|item| item != "/net device"),
                "{line}: {:?}",
                outcome.suggestions
            );
            assert!(!hit.load(Ordering::SeqCst), "{line}");
        }
        let exact = evaluate(&loaded, "/net device", &source, &source, &source);
        assert!(!exact.executed);
        assert_eq!(exact.code, super::super::cli::EXIT_REFUSED);
        assert_eq!(exact.body["code"], "NETCONNECT_REFUSED");
        assert!(!exact.body.to_string().contains("livingroom"));
        assert!(!hit.load(Ordering::SeqCst));
        let armed = evaluate(&loaded, "/speed device printer", &source, &source, &source);
        assert!(!armed.executed);
        assert!(armed.suggestions.iter().all(|item| item != "/net device"));
        assert!(!hit.load(Ordering::SeqCst));
    }

    #[test]
    fn disabled_tier_refuses_with_the_gate_exit_code() {
        let hit = Arc::new(AtomicBool::new(false));
        let source = FlagSource(Arc::clone(&hit));
        let loaded = cfg("netconnect:\n  enabled: true\n  allowed_cidrs: ['192.168.0.0/16']\n");
        for line in ["/net ports", "/lan diag", "/scan watch"] {
            let outcome = evaluate(&loaded, line, &source, &source, &source);
            assert_eq!(
                outcome.code,
                super::super::cli::EXIT_REFUSED,
                "{line}: {}",
                outcome.body
            );
            assert_eq!(outcome.body["code"], "NETCONNECT_REFUSED");
            assert!(!outcome.executed, "{line}");
            assert!(outcome.notice.contains("may not run"), "{line}: {}", outcome.notice);
            assert!(!hit.load(Ordering::SeqCst), "{line}");
        }
        let empty = cfg("netconnect:\n  enabled: true\n  port_scan: true\n");
        let outcome = evaluate(&empty, "/net ports", &source, &source, &source);
        assert_eq!(outcome.code, super::super::cli::EXIT_REFUSED);
        assert!(!outcome.executed);
        let off = cfg("netconnect:\n  enabled: false\n  port_scan: true\n  allowed_cidrs: ['192.168.0.0/16']\n");
        let outcome = evaluate(&off, "/ports ports", &source, &source, &source);
        assert_eq!(outcome.code, super::super::cli::EXIT_REFUSED);
        assert!(!hit.load(Ordering::SeqCst));
    }

    #[test]
    fn run_reports_the_gate_exit_code_and_fuzzy_does_not_need_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            "netconnect:\n  enabled: true\n  allowed_cidrs: ['192.168.0.0/16']\n",
        )
        .unwrap();
        assert_eq!(run(Some(path), "/net ports"), super::super::cli::EXIT_REFUSED);
        assert_eq!(run(None, "/nett devices"), EXIT_FAIL);
    }
}
