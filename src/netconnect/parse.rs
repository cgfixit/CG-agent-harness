//! Text parsers for neighbor, route, and interface tables.
//!
//! Malformed lines are skipped. IPv6 rows are dropped before a record exists.
//! Parsers do not apply scope; [`super::collect_passive`] does that.

use std::net::Ipv4Addr;

use super::scope::network_address;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeighborRecord {
    pub address: Ipv4Addr,
    pub mac: Option<String>,
    pub interface: String,
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRecord {
    pub interface: String,
    /// `None` is the default route. The destination address is not emitted
    /// unless the whole prefix sits inside scope.
    pub destination: Option<(Ipv4Addr, u8)>,
    pub gateway: Option<Ipv4Addr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceRecord {
    pub name: String,
    pub address: Ipv4Addr,
}

pub fn parse_proc_net_arp(text: &str) -> Vec<NeighborRecord> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 6 {
            continue;
        }
        let address = cols[0];
        if address.contains(':') {
            continue;
        }
        let Ok(address) = address.parse::<Ipv4Addr>() else {
            continue;
        };
        rows.push(NeighborRecord {
            address,
            mac: normalize_mac(cols[3]),
            interface: cols[5].to_string(),
            hostname: None,
        });
    }
    rows
}

pub fn parse_proc_net_route(text: &str) -> Vec<RouteRecord> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 8 {
            continue;
        }
        let Some(destination) = parse_proc_ip(cols[1]) else {
            continue;
        };
        let Some(gateway) = parse_proc_ip(cols[2]) else {
            continue;
        };
        let Some(prefix) = parse_proc_mask(cols[7]) else {
            continue;
        };
        let Ok(flags) = u32::from_str_radix(cols[3], 16) else {
            continue;
        };
        let gateway = if flags & 0x2 != 0 && !gateway.is_unspecified() {
            Some(gateway)
        } else {
            None
        };
        let destination = if destination.is_unspecified() {
            None
        } else {
            Some((network_address(destination, prefix), prefix))
        };
        rows.push(RouteRecord {
            interface: cols[0].to_string(),
            destination,
            gateway,
        });
    }
    rows
}

/// Fixture lines: `name address`. IPv6 addresses are dropped.
pub fn parse_interface_lines(text: &str) -> Vec<InterfaceRecord> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let Some(address) = parts.next() else { continue };
        if parts.next().is_some() || address.contains(':') {
            continue;
        }
        let Ok(address) = address.parse::<Ipv4Addr>() else {
            continue;
        };
        rows.push(InterfaceRecord {
            name: name.to_string(),
            address,
        });
    }
    rows
}

pub fn parse_bsd_arp(text: &str) -> Vec<NeighborRecord> {
    let mut rows = Vec::new();
    for line in text.lines() {
        if let Some(row) = parse_bsd_arp_line(line) {
            rows.push(row);
        }
    }
    rows
}

pub fn parse_bsd_netstat(text: &str) -> Vec<RouteRecord> {
    let mut rows = Vec::new();
    let mut in_table = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("Destination") && line.contains("Gateway") {
            in_table = true;
            continue;
        }
        if !in_table || line.ends_with(':') {
            continue;
        }
        if let Some(row) = parse_bsd_route_line(line) {
            rows.push(row);
        }
    }
    rows
}

fn parse_bsd_arp_line(line: &str) -> Option<NeighborRecord> {
    let open = line.find(" (")?;
    let rest = &line[open + 2..];
    let close = rest.find(')')?;
    let address = &rest[..close];
    if address.contains(':') {
        return None;
    }
    let address: Ipv4Addr = address.parse().ok()?;
    let after = rest[close + 1..].trim().strip_prefix("at")?.trim();
    let tokens: Vec<&str> = after.split_whitespace().collect();
    if tokens.len() < 3 || tokens[1] != "on" {
        return None;
    }
    let mac = if tokens[0] == "(incomplete)" {
        None
    } else {
        normalize_mac(tokens[0])
    };
    let host = line[..open].trim();
    let hostname = if host.is_empty() || host == "?" {
        None
    } else {
        Some(host.to_string())
    };
    Some(NeighborRecord {
        address,
        mac,
        interface: tokens[2].to_string(),
        hostname,
    })
}

fn parse_bsd_route_line(line: &str) -> Option<RouteRecord> {
    let cols: Vec<&str> = line.split_whitespace().collect();
    if cols.len() < 4 || cols[0].contains(':') {
        return None;
    }
    let destination = parse_bsd_destination(cols[0])?;
    let parsed_gateway = parse_bsd_gateway(cols[1]);
    let flags = cols[2];
    let interface = cols[3];
    let gateway = if flags.contains('G') {
        Some(parsed_gateway?)
    } else {
        parsed_gateway
    };
    Some(RouteRecord {
        interface: interface.to_string(),
        destination,
        gateway,
    })
}

fn parse_bsd_destination(dest: &str) -> Option<Option<(Ipv4Addr, u8)>> {
    if dest == "default" {
        return Some(None);
    }
    if dest.contains(':') {
        return None;
    }
    if let Some((addr, prefix)) = dest.split_once('/') {
        let addr: Ipv4Addr = addr.parse().ok()?;
        let prefix = parse_prefix(prefix)?;
        return Some(Some((network_address(addr, prefix), prefix)));
    }
    if let Ok(addr) = dest.parse::<Ipv4Addr>() {
        return Some(Some((addr, 32)));
    }
    let parts: Vec<&str> = dest.split('.').collect();
    if !(1..4).contains(&parts.len()) {
        return None;
    }
    let mut octets = [0u8; 4];
    for (index, part) in parts.iter().enumerate() {
        octets[index] = strict_u8(part)?;
    }
    let prefix = u8::try_from(parts.len() * 8).ok()?;
    Some(Some((Ipv4Addr::from(octets), prefix)))
}

fn parse_bsd_gateway(token: &str) -> Option<Ipv4Addr> {
    let addr: Ipv4Addr = token.parse().ok()?;
    if addr.is_unspecified() {
        None
    } else {
        Some(addr)
    }
}

fn parse_prefix(text: &str) -> Option<u8> {
    let prefix: u8 = text.parse().ok()?;
    if text == prefix.to_string() && prefix <= 32 {
        Some(prefix)
    } else {
        None
    }
}

fn strict_u8(part: &str) -> Option<u8> {
    if part.is_empty() || (part.len() > 1 && part.starts_with('0')) {
        return None;
    }
    let value: u8 = part.parse().ok()?;
    Some(value)
}

fn parse_proc_ip(hex: &str) -> Option<Ipv4Addr> {
    if hex.len() != 8 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(Ipv4Addr::from(value.to_le_bytes()))
}

fn parse_proc_mask(hex: &str) -> Option<u8> {
    let addr = parse_proc_ip(hex)?;
    let value = u32::from(addr);
    if value != 0 && (!value) & ((!value).wrapping_add(1)) != 0 {
        return None;
    }
    u8::try_from(value.count_ones()).ok()
}

fn normalize_mac(raw: &str) -> Option<String> {
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() != 6
        || !parts
            .iter()
            .all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return None;
    }
    Some(
        parts
            .iter()
            .map(|part| part.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(":"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le_hex(addr: Ipv4Addr) -> String {
        let octets = addr.octets();
        format!("{:02X}{:02X}{:02X}{:02X}", octets[3], octets[2], octets[1], octets[0])
    }

    #[test]
    fn proc_arp_drops_ipv6_and_keeps_ipv4() {
        let text = "\
IP address       HW type     Flags       HW address            Mask     Device
192.168.1.10     0x1         0x2         AA:BB:CC:DD:EE:FF     *        eth0
8.8.8.8          0x1         0x2         11:22:33:44:55:66     *        eth0
fe80::1          0x1         0x2         aa:bb:cc:dd:ee:ff     *        eth0
not-an-ip        0x1         0x2         aa:bb:cc:dd:ee:ff     *        eth0
";
        let rows = parse_proc_net_arp(text);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].address, Ipv4Addr::new(192, 168, 1, 10));
        assert_eq!(rows[0].mac.as_deref(), Some("aa:bb:cc:dd:ee:ff"));
        assert_eq!(rows[1].address, Ipv4Addr::new(8, 8, 8, 8));
        assert!(rows.iter().all(|row| row.hostname.is_none()));
    }

    #[test]
    fn proc_route_decodes_little_endian_and_default() {
        let gateway = Ipv4Addr::new(192, 168, 1, 1);
        let local = Ipv4Addr::new(192, 168, 1, 0);
        let mask = Ipv4Addr::new(255, 255, 255, 0);
        let text = format!(
            "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\n\
             eth0 {} {} 0003 0 0 100 {} 0 0 0\n\
             eth0 {} 00000000 0001 0 0 100 {} 0 0 0\n\
             eth0 fe80::1 00000000 0001 0 0 0 00000000 0 0 0\n",
            le_hex(Ipv4Addr::UNSPECIFIED),
            le_hex(gateway),
            le_hex(Ipv4Addr::UNSPECIFIED),
            le_hex(local),
            le_hex(mask),
        );
        let rows = parse_proc_net_route(&text);
        assert_eq!(rows.len(), 2, "{text} -> {rows:?}");
        assert_eq!(rows[0].destination, None);
        assert_eq!(rows[0].gateway, Some(gateway));
        assert_eq!(rows[1].destination, Some((local, 24)));
        assert_eq!(rows[1].gateway, None);
    }

    #[test]
    fn bsd_arp_drops_ipv6_and_keeps_hostname_text() {
        let text = "\
? (192.168.1.1) at aa:bb:cc:dd:ee:ff on en0 ifscope [ethernet]
? (8.8.8.8) at 11:22:33:44:55:66 on en0 ifscope [ethernet]
? (fe80::1%en0) at aa:bb:cc:dd:ee:ff on en0 ifscope [ethernet]
livingroom (192.168.1.50) at ab:cd:ef:01:23:45 on en0 ifscope [ethernet]
? (192.168.1.9) at (incomplete) on en0 ifscope [ethernet]
";
        let rows = parse_bsd_arp(text);
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|row| !row.address.to_string().contains(':')));
        assert_eq!(rows[2].hostname.as_deref(), Some("livingroom"));
        assert_eq!(rows[3].mac, None);
    }

    #[test]
    fn bsd_netstat_reads_default_and_shorthand() {
        let text = "\
Routing tables

Internet:
Destination        Gateway            Flags        Netif Expire
default            192.168.4.1        UGSc           en0
192.168.4          link#14            UCS            en0
192.168.4.10       aa:bb:cc:dd:ee:ff  UHLW           en0
fe80::%en0         fe80::1            UGc            en0
";
        let rows = parse_bsd_netstat(text);
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert_eq!(rows[0].destination, None);
        assert_eq!(rows[0].gateway, Some(Ipv4Addr::new(192, 168, 4, 1)));
        assert_eq!(rows[1].destination, Some((Ipv4Addr::new(192, 168, 4, 0), 24)));
        assert_eq!(rows[1].gateway, None);
        assert_eq!(rows[2].destination, Some((Ipv4Addr::new(192, 168, 4, 10), 32)));
        assert_eq!(rows[2].gateway, None);
    }

    #[test]
    fn interface_fixture_drops_ipv6() {
        let rows = parse_interface_lines(
            "\
# comment
eth0 192.168.1.10
eth0 8.8.8.8
eth0 fe80::1
eth1 10.0.0.1
bogus
",
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].address, Ipv4Addr::new(10, 0, 0, 1));
    }
}
