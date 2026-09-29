//! Scope filter over collector traits.
//!
//! Sources return unfiltered rows, including addresses the operator did not
//! allow. [`collect_passive`] drops every address outside [`Scope`] and every
//! IPv6 row the parsers already removed. Interface and host strings are
//! sanitized here and never leave as paths.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use serde::Serialize;

use crate::common::errors::Result;

use super::parse::{
    parse_bsd_arp, parse_bsd_netstat, parse_interface_lines, parse_proc_net_arp, parse_proc_net_route, InterfaceRecord,
    NeighborRecord, RouteRecord,
};
use super::sanitize::{sanitize_untrusted, UntrustedString};
use super::scope::Scope;

/// Local IPv4 neighbors. Fixture implementations feed parser text.
pub trait NeighborSource {
    fn load(&self) -> Result<Vec<NeighborRecord>>;
}

/// Local IPv4 routes, including the default route when the table has one.
pub trait RouteSource {
    fn load(&self) -> Result<Vec<RouteRecord>>;
}

/// Local interface addresses. One record per address.
pub trait InterfaceSource {
    fn load(&self) -> Result<Vec<InterfaceRecord>>;
}

#[derive(Debug, Clone)]
pub struct FixtureNeighbors {
    records: Vec<NeighborRecord>,
}

impl FixtureNeighbors {
    pub fn proc_arp(text: &str) -> Self {
        Self {
            records: parse_proc_net_arp(text),
        }
    }

    pub fn bsd_arp(text: &str) -> Self {
        Self {
            records: parse_bsd_arp(text),
        }
    }
}

impl NeighborSource for FixtureNeighbors {
    fn load(&self) -> Result<Vec<NeighborRecord>> {
        Ok(self.records.clone())
    }
}

#[derive(Debug, Clone)]
pub struct FixtureRoutes {
    records: Vec<RouteRecord>,
}

impl FixtureRoutes {
    pub fn proc_route(text: &str) -> Self {
        Self {
            records: parse_proc_net_route(text),
        }
    }

    pub fn bsd_netstat(text: &str) -> Self {
        Self {
            records: parse_bsd_netstat(text),
        }
    }
}

impl RouteSource for FixtureRoutes {
    fn load(&self) -> Result<Vec<RouteRecord>> {
        Ok(self.records.clone())
    }
}

#[derive(Debug, Clone)]
pub struct FixtureInterfaces {
    records: Vec<InterfaceRecord>,
}

impl FixtureInterfaces {
    pub fn from_lines(text: &str) -> Self {
        Self {
            records: parse_interface_lines(text),
        }
    }
}

impl InterfaceSource for FixtureInterfaces {
    fn load(&self) -> Result<Vec<InterfaceRecord>> {
        Ok(self.records.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportedInterface {
    pub name: UntrustedString,
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportedRoute {
    pub interface: UntrustedString,
    pub destination: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportedGateway {
    pub interface: UntrustedString,
    pub address: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportedNeighbor {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    pub interface: UntrustedString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<UntrustedString>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PassiveReport {
    pub interfaces: Vec<ReportedInterface>,
    pub routes: Vec<ReportedRoute>,
    pub default_gateways: Vec<ReportedGateway>,
    pub neighbors: Vec<ReportedNeighbor>,
}

impl PassiveReport {
    pub fn addresses(&self) -> Vec<Ipv4Addr> {
        let mut out = Vec::new();
        for interface in &self.interfaces {
            for address in &interface.addresses {
                if let Ok(ip) = address.parse() {
                    out.push(ip);
                }
            }
        }
        for route in &self.routes {
            if let Some((network, _)) = route.destination.split_once('/') {
                if let Ok(ip) = network.parse() {
                    out.push(ip);
                }
            }
            if let Some(gateway) = &route.gateway {
                if let Ok(ip) = gateway.parse() {
                    out.push(ip);
                }
            }
        }
        for gateway in &self.default_gateways {
            if let Ok(ip) = gateway.address.parse() {
                out.push(ip);
            }
        }
        for neighbor in &self.neighbors {
            if let Ok(ip) = neighbor.address.parse() {
                out.push(ip);
            }
        }
        out
    }
}

pub fn collect_passive(
    scope: &Scope,
    neighbors: &impl NeighborSource,
    routes: &impl RouteSource,
    interfaces: &impl InterfaceSource,
    max_chars: usize,
) -> Result<PassiveReport> {
    let mut by_name: BTreeMap<String, ReportedInterface> = BTreeMap::new();
    for row in interfaces.load()? {
        if !scope.contains(row.address) {
            continue;
        }
        let Some(name) = emit_label(&row.name, max_chars) else {
            continue;
        };
        let entry = by_name
            .entry(name.value().to_string())
            .or_insert_with(|| ReportedInterface {
                name,
                addresses: Vec::new(),
            });
        let address = row.address.to_string();
        if !entry.addresses.contains(&address) {
            entry.addresses.push(address);
        }
    }
    let mut interfaces: Vec<_> = by_name.into_values().collect();
    for interface in &mut interfaces {
        interface.addresses.sort();
    }

    let mut reported_routes = Vec::new();
    let mut default_gateways = Vec::new();
    for row in routes.load()? {
        let Some(interface) = emit_label(&row.interface, max_chars) else {
            continue;
        };
        if let Some(gateway) = row.gateway {
            if row.destination.is_none() && scope.contains(gateway) {
                default_gateways.push(ReportedGateway {
                    interface: interface.clone(),
                    address: gateway.to_string(),
                });
            }
        }
        let Some((network, prefix)) = row.destination else {
            continue;
        };
        if !scope.covers_network(network, prefix) {
            continue;
        }
        let gateway = row
            .gateway
            .filter(|addr| scope.contains(*addr))
            .map(|addr| addr.to_string());
        reported_routes.push(ReportedRoute {
            interface,
            destination: format!("{network}/{prefix}"),
            gateway,
        });
    }
    reported_routes.sort_by(|a, b| (&a.destination, a.interface.value()).cmp(&(&b.destination, b.interface.value())));
    default_gateways.sort_by(|a, b| a.address.cmp(&b.address));

    let mut reported_neighbors = Vec::new();
    for row in neighbors.load()? {
        if !scope.contains(row.address) {
            continue;
        }
        let Some(interface) = emit_label(&row.interface, max_chars) else {
            continue;
        };
        let hostname = row
            .hostname
            .as_deref()
            .map(|host| sanitize_untrusted(host, max_chars))
            .filter(|host| !host.value().is_empty());
        reported_neighbors.push(ReportedNeighbor {
            address: row.address.to_string(),
            mac: row.mac,
            interface,
            hostname,
        });
    }
    reported_neighbors.sort_by(|a, b| (&a.address, a.interface.value()).cmp(&(&b.address, b.interface.value())));

    Ok(PassiveReport {
        interfaces,
        routes: reported_routes,
        default_gateways,
        neighbors: reported_neighbors,
    })
}

fn emit_label(raw: &str, max_chars: usize) -> Option<UntrustedString> {
    let stripped: String = raw.chars().filter(|c| !c.is_control()).collect();
    if stripped.chars().count() > max_chars.min(32) || !is_label(&stripped) {
        None
    } else {
        Some(UntrustedString::new(stripped))
    }
}

fn is_label(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && name.chars().count() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    fn scope(entries: &[&str]) -> Scope {
        Scope::parse(&entries.iter().map(|e| (*e).to_string()).collect::<Vec<_>>()).unwrap()
    }

    fn le_hex(addr: Ipv4Addr) -> String {
        let octets = addr.octets();
        format!("{:02X}{:02X}{:02X}{:02X}", octets[3], octets[2], octets[1], octets[0])
    }

    #[test]
    fn fixtures_drop_out_of_scope_and_ipv6() {
        let arp = "\
IP address       HW type     Flags       HW address            Mask     Device
192.168.1.10     0x1         0x2         aa:bb:cc:dd:ee:ff     *        eth0
8.8.8.8          0x1         0x2         11:22:33:44:55:66     *        eth0
fe80::1          0x1         0x2         aa:bb:cc:dd:ee:ff     *        eth0
10.1.0.9         0x1         0x2         01:02:03:04:05:06     *        eth0
";
        let gateway = Ipv4Addr::new(192, 168, 1, 1);
        let local = Ipv4Addr::new(192, 168, 1, 0);
        let mask = Ipv4Addr::new(255, 255, 255, 0);
        let public_gw = Ipv4Addr::new(8, 8, 8, 8);
        let routes = format!(
            "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\n\
             eth0 {} {} 0003 0 0 0 {} 0 0 0\n\
             eth1 {} {} 0003 0 0 0 {} 0 0 0\n\
             eth0 {} 00000000 0001 0 0 0 {} 0 0 0\n\
             eth0 {} 00000000 0001 0 0 0 {} 0 0 0\n",
            le_hex(Ipv4Addr::UNSPECIFIED),
            le_hex(gateway),
            le_hex(Ipv4Addr::UNSPECIFIED),
            le_hex(Ipv4Addr::UNSPECIFIED),
            le_hex(public_gw),
            le_hex(Ipv4Addr::UNSPECIFIED),
            le_hex(local),
            le_hex(mask),
            le_hex(Ipv4Addr::new(10, 1, 0, 0)),
            le_hex(Ipv4Addr::new(255, 255, 0, 0)),
        );
        let interfaces = "\
eth0 192.168.1.20
eth0 8.8.8.8
eth0 fe80::1
eth1 10.9.9.9
eth0/../../tmp 192.168.1.30
";
        let report = collect_passive(
            &scope(&["192.168.0.0/16"]),
            &FixtureNeighbors::proc_arp(arp),
            &FixtureRoutes::proc_route(&routes),
            &FixtureInterfaces::from_lines(interfaces),
            64,
        )
        .unwrap();
        assert_eq!(report.neighbors.len(), 1);
        assert_eq!(report.neighbors[0].address, "192.168.1.10");
        assert!(report.neighbors[0].interface.is_untrusted());
        assert_eq!(report.default_gateways.len(), 1);
        assert_eq!(report.default_gateways[0].address, "192.168.1.1");
        assert_eq!(report.routes.len(), 1);
        assert_eq!(report.routes[0].destination, "192.168.1.0/24");
        assert_eq!(report.interfaces.len(), 1);
        assert_eq!(report.interfaces[0].addresses, vec!["192.168.1.20".to_string()]);
        let rendered = serde_json::to_string(&report).unwrap();
        assert!(!rendered.contains("8.8.8.8"));
        assert!(!rendered.contains("fe80"));
        assert!(!rendered.contains("10.1.0.9"));
        assert!(!rendered.contains("../"));
        assert!(!rendered.contains("10.9.9.9"));
        for address in report.addresses() {
            assert!(scope(&["192.168.0.0/16"]).contains(address));
        }
    }

    #[test]
    fn empty_scope_emits_no_addresses() {
        let report = collect_passive(
            &scope(&[]),
            &FixtureNeighbors::proc_arp("192.168.1.1 0x1 0x2 aa:bb:cc:dd:ee:ff * eth0\n"),
            &FixtureRoutes::proc_route("eth0 0001A8C0 00000000 0001 0 0 0 00FFFFFF 0 0 0\n"),
            &FixtureInterfaces::from_lines("eth0 192.168.1.1\n"),
            64,
        )
        .unwrap();
        assert!(report.addresses().is_empty());
        assert!(report.interfaces.is_empty());
        assert!(report.neighbors.is_empty());
        assert!(report.routes.is_empty());
    }

    #[test]
    fn hostname_is_capped_marked_untrusted_and_not_a_path() {
        let text = "bad\u{0001}name (192.168.1.50) at ab:cd:ef:01:23:45 on en0 ifscope [ethernet]\n";
        let report = collect_passive(
            &scope(&["192.168.0.0/16"]),
            &FixtureNeighbors::bsd_arp(text),
            &FixtureRoutes::proc_route(""),
            &FixtureInterfaces::from_lines(""),
            4,
        )
        .unwrap();
        let host = report.neighbors[0].hostname.as_ref().unwrap();
        assert_eq!(host.value(), "badn");
        assert!(host.is_untrusted());
        assert!(!host.value().contains('\u{0001}'));
        assert!(!host.value().contains('/'));
    }
}
