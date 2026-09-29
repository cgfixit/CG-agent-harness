//! Device-supplied names are length-capped, stripped of controls, and labeled
//! untrusted. They are display data: a path-like or over-long interface name
//! does not leave the collector.

use std::path::Path;

use cgagentharness::common::config::AppConfig;
use cgagentharness::netconnect::{
    collect_passive, sanitize_untrusted, FixtureInterfaces, FixtureNeighbors, FixtureRoutes, NetconnectConfig, Scope,
};

fn scope() -> Scope {
    Scope::parse(&["192.168.0.0/16".to_string()]).unwrap()
}

fn controls_gone(value: &str) {
    assert!(value.chars().all(|c| !c.is_control()), "{value:?}");
}

#[test]
fn sanitize_caps_strips_controls_and_marks_untrusted() {
    let marked = sanitize_untrusted("dev\n\t\u{0007}\rice\u{0000}\u{001b}[31mname-EXTRA", 8);
    assert_eq!(marked.value(), "device[3");
    assert!(marked.is_untrusted());
    controls_gone(marked.value());
    let json = serde_json::to_value(&marked).unwrap();
    assert_eq!(json["untrusted"], true);
    assert_eq!(json["value"], "device[3");

    let long = "a".repeat(200);
    assert_eq!(sanitize_untrusted(&long, 64).value().chars().count(), 64);
    assert_eq!(sanitize_untrusted("café\u{0001}!", 3).value(), "caf");
    let empty = sanitize_untrusted("\n\r\u{0000}", 32);
    assert_eq!(empty.value(), "");
    assert!(empty.is_untrusted());
}

#[test]
fn shipped_cap_is_what_sanitize_receives() {
    let cfg = NetconnectConfig::from_config(
        &AppConfig::from_str(AppConfig::embedded_default(), Path::new("defaults")).unwrap(),
    )
    .unwrap();
    assert_eq!(cfg.untrusted_string_max_chars, 64);
    let marked = sanitize_untrusted(&"b".repeat(500), cfg.untrusted_string_max_chars);
    assert_eq!(marked.value().chars().count(), 64);
    assert!(marked.is_untrusted());
}

#[test]
fn collector_names_are_capped_stripped_and_untrusted() {
    let host = format!("bad\u{0001}name{}", "x".repeat(80));
    let arp = format!("{host} (192.168.1.50) at ab:cd:ef:01:23:45 on en0 ifscope [ethernet]\n");
    let long_iface = format!("{} 192.168.1.40", "n".repeat(40));
    let interfaces = format!(
        "eth0\u{0007} 192.168.1.20\n\
         {long_iface}\n\
         ../etc/passwd 192.168.1.30\n\
         \r 192.168.1.31\n"
    );
    let report = collect_passive(
        &scope(),
        &FixtureNeighbors::bsd_arp(&arp),
        &FixtureRoutes::bsd_netstat(""),
        &FixtureInterfaces::from_lines(&interfaces),
        8,
    )
    .unwrap();

    let neighbor = &report.neighbors[0];
    let hostname = neighbor.hostname.as_ref().expect("hostname");
    assert_eq!(hostname.value(), "badnamex");
    assert!(hostname.is_untrusted());
    controls_gone(hostname.value());
    assert!(neighbor.interface.is_untrusted());
    controls_gone(neighbor.interface.value());
    assert!(neighbor.interface.value().chars().count() <= 8);

    assert!(report.interfaces.iter().any(|iface| iface.name.value() == "eth0"));
    for iface in &report.interfaces {
        assert!(iface.name.is_untrusted());
        controls_gone(iface.name.value());
        assert!(iface.name.value().chars().count() <= 8);
        assert!(!iface.name.value().contains('/'));
        assert!(!iface.name.value().contains(".."));
    }
    let rendered = serde_json::to_string(&report).unwrap();
    assert!(rendered.contains("\"untrusted\":true"));
    assert!(!rendered.contains("../"));
    assert!(!rendered.contains("192.168.1.30"));
    assert!(!rendered.contains('\u{0001}'));
    assert!(!rendered.contains(&"n".repeat(40)));
}
