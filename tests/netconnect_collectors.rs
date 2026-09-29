//! Fixture-backed passive collectors.
//!
//! Rows come from in-memory `/proc` and BSD table text. Nothing in this file
//! calls the live neighbor, route, or interface sources, so the test does not
//! read the runner's network.

use cgagentharness::netconnect::{collect_passive, FixtureInterfaces, FixtureNeighbors, FixtureRoutes, Scope};
use std::net::Ipv4Addr;

fn scope(entries: &[&str]) -> Scope {
    Scope::parse(&entries.iter().map(|entry| (*entry).to_string()).collect::<Vec<_>>()).unwrap()
}

fn le_hex(addr: Ipv4Addr) -> String {
    let octets = addr.octets();
    format!("{:02X}{:02X}{:02X}{:02X}", octets[3], octets[2], octets[1], octets[0])
}

fn assert_only_in_scope(report: &cgagentharness::netconnect::PassiveReport, allowed: &Scope) {
    assert!(!report.addresses().is_empty());
    for address in report.addresses() {
        assert!(allowed.contains(address), "{address} leaked");
    }
    let rendered = serde_json::to_string(report).unwrap();
    for banned in [
        "8.8.8.8",
        "fe80",
        "10.1.0.9",
        "10.9.9.9",
        "169.254",
        "100.64",
        "224.0.0.1",
        "255.255.255.255",
        "../",
    ] {
        assert!(!rendered.contains(banned), "{banned} in {rendered}");
    }
}

#[test]
fn proc_fixtures_drop_out_of_scope_and_ipv6() {
    let arp = "\
IP address       HW type     Flags       HW address            Mask     Device
192.168.1.10     0x1         0x2         aa:bb:cc:dd:ee:ff     *        eth0
8.8.8.8          0x1         0x2         11:22:33:44:55:66     *        eth0
fe80::1          0x1         0x2         aa:bb:cc:dd:ee:ff     *        eth0
10.1.0.9         0x1         0x2         01:02:03:04:05:06     *        eth0
169.254.1.1      0x1         0x2         0a:0b:0c:0d:0e:0f     *        eth0
100.64.1.1       0x1         0x2         0a:0b:0c:0d:0e:0f     *        eth0
224.0.0.1        0x1         0x2         01:00:5e:00:00:01     *        eth0
255.255.255.255  0x1         0x2         ff:ff:ff:ff:ff:ff     *        eth0
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
         eth0 {} 00000000 0001 0 0 0 {} 0 0 0\n\
         eth0 fe80::1 00000000 0001 0 0 0 00000000 0 0 0\n",
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
wlan0 169.254.1.1
wlan0 100.64.1.1
eth1 224.0.0.1
eth1 255.255.255.255
eth0/../../tmp 192.168.1.30
";
    let allowed = scope(&["192.168.0.0/16"]);
    let report = collect_passive(
        &allowed,
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
    assert_only_in_scope(&report, &allowed);
}

#[test]
fn bsd_fixtures_drop_out_of_scope_and_ipv6() {
    let arp = "\
? (192.168.1.10) at aa:bb:cc:dd:ee:ff on en0 ifscope [ethernet]
? (8.8.8.8) at 11:22:33:44:55:66 on en0 ifscope [ethernet]
? (fe80::1%en0) at aa:bb:cc:dd:ee:ff on en0 ifscope [ethernet]
? (10.1.0.9) at 01:02:03:04:05:06 on en1 ifscope [ethernet]
? (169.254.1.1) at 0a:0b:0c:0d:0e:0f on en0 ifscope [ethernet]
";
    let routes = "\
Routing tables

Internet:
Destination        Gateway            Flags        Netif Expire
default            192.168.1.1        UGSc           en0
default            8.8.8.8            UGSc           en1
192.168.1          link#14            UCS            en0
10.1               link#15            UCS            en1
8.8.8.8/32         192.168.1.1        UGSc           en0
fe80::%en0         fe80::1            UGc            en0
";
    let interfaces = "\
en0 192.168.1.20
en0 8.8.8.8
en0 fe80::1
en1 10.9.9.9
";
    let allowed = scope(&["192.168.0.0/16"]);
    let report = collect_passive(
        &allowed,
        &FixtureNeighbors::bsd_arp(arp),
        &FixtureRoutes::bsd_netstat(routes),
        &FixtureInterfaces::from_lines(interfaces),
        64,
    )
    .unwrap();
    assert_eq!(report.neighbors.len(), 1);
    assert_eq!(report.neighbors[0].address, "192.168.1.10");
    assert_eq!(report.default_gateways.len(), 1);
    assert_eq!(report.default_gateways[0].address, "192.168.1.1");
    assert!(report.routes.iter().any(|route| route.destination == "192.168.1.0/24"));
    assert!(report
        .routes
        .iter()
        .all(|route| route.destination.starts_with("192.168.")));
    assert_eq!(report.interfaces.len(), 1);
    assert_only_in_scope(&report, &allowed);
}

#[test]
fn empty_scope_keeps_no_fixture_address() {
    let report = collect_passive(
        &scope(&[]),
        &FixtureNeighbors::proc_arp(
            "192.168.1.10 0x1 0x2 aa:bb:cc:dd:ee:ff * eth0\n8.8.8.8 0x1 0x2 11:22:33:44:55:66 * eth0\n",
        ),
        &FixtureRoutes::proc_route("eth0 0001A8C0 00000000 0001 0 0 0 00FFFFFF 0 0 0\n"),
        &FixtureInterfaces::from_lines("eth0 192.168.1.20\neth0 8.8.8.8\n"),
        64,
    )
    .unwrap();
    assert!(report.addresses().is_empty());
    assert!(report.interfaces.is_empty());
    assert!(report.neighbors.is_empty());
    assert!(report.routes.is_empty());
    assert!(report.default_gateways.is_empty());
    assert!(!serde_json::to_string(&report).unwrap().contains("8.8.8.8"));
}
