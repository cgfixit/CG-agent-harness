//! IPv4 scope. Empty means every armed tier stays refused.
//!
//! Accepted entries are canonical CIDRs that sit fully inside RFC1918
//! (10/8, 172.16/12, 192.168/16) or loopback 127/8, with prefix length 16
//! through 32. Wider prefixes are rejected even when they are inside those
//! blocks. Nothing here reads interface addresses.

use std::net::Ipv4Addr;

use crate::common::errors::{HarnessError, Result};

const MIN_PREFIX: u8 = 16;
pub(crate) const MAX_ENTRIES: usize = 64;

/// Containers a candidate must lie fully inside. Prefix length is separate:
/// a candidate still needs prefix >= [`MIN_PREFIX`].
const PRIVATE_BLOCKS: &[(Ipv4Addr, u8)] = &[
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    (Ipv4Addr::new(192, 168, 0, 0), 16),
    (Ipv4Addr::new(127, 0, 0, 0), 8),
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Cidr {
    network: Ipv4Addr,
    prefix: u8,
}

impl Cidr {
    fn contains(&self, addr: Ipv4Addr) -> bool {
        let mask = prefix_mask(self.prefix);
        u32::from(addr) & mask == u32::from(self.network)
    }

    /// True when the whole `prefix` block at `network` is inside this CIDR.
    fn covers(&self, network: Ipv4Addr, prefix: u8) -> bool {
        if !(self.prefix..=32).contains(&prefix) {
            return false;
        }
        if network_address(network, prefix) != network {
            return false;
        }
        let mask = prefix_mask(self.prefix);
        u32::from(network) & mask == u32::from(self.network)
    }

    fn display(&self) -> String {
        format!("{}/{}", self.network, self.prefix)
    }
}

/// Operator-configured IPv4 ranges. An empty scope contains nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    cidrs: Vec<Cidr>,
}

impl Scope {
    pub fn is_empty(&self) -> bool {
        self.cidrs.is_empty()
    }

    pub fn entries(&self) -> Vec<String> {
        self.cidrs.iter().map(Cidr::display).collect()
    }

    /// Parse `allowed_cidrs`. One rejected entry rejects the whole list.
    pub fn parse(entries: &[String]) -> Result<Self> {
        if entries.len() > MAX_ENTRIES {
            return Err(HarnessError::config(format!(
                "netconnect.allowed_cidrs has more than {MAX_ENTRIES} entries"
            )));
        }
        let mut cidrs = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let cidr = parse_cidr(entry).map_err(|reason| {
                HarnessError::config(format!(
                    "netconnect.allowed_cidrs[{index}] {}: {reason}",
                    preview(entry)
                ))
            })?;
            if cidrs.iter().any(|have: &Cidr| have == &cidr) {
                return Err(HarnessError::config(format!(
                    "netconnect.allowed_cidrs[{index}] {} is duplicated",
                    cidr.display()
                )));
            }
            cidrs.push(cidr);
        }
        Ok(Self { cidrs })
    }

    pub fn contains(&self, addr: Ipv4Addr) -> bool {
        self.cidrs.iter().any(|cidr| cidr.contains(addr))
    }

    /// True when every address of `network/prefix` is inside some allowed CIDR.
    pub fn covers_network(&self, network: Ipv4Addr, prefix: u8) -> bool {
        self.cidrs.iter().any(|cidr| cidr.covers(network, prefix))
    }

    /// Recheck a resolved address immediately before a connect.
    ///
    /// Callers pass the final [`Ipv4Addr`] only. A hostname is not a target,
    /// and an empty scope refuses every address.
    pub fn check_target(&self, addr: Ipv4Addr) -> Result<()> {
        if self.contains(addr) {
            Ok(())
        } else {
            Err(HarnessError::new(
                "NETCONNECT_REFUSED",
                format!("address {addr} is outside netconnect scope"),
            ))
        }
    }
}

fn preview(entry: &str) -> String {
    let clean: String = entry.chars().filter(|c| !c.is_control()).take(64).collect();
    if clean.is_empty() {
        "<empty>".to_string()
    } else {
        clean
    }
}

fn parse_cidr(entry: &str) -> std::result::Result<Cidr, &'static str> {
    let entry = entry.trim();
    if entry.contains(':') {
        return Err("IPv6 is not permitted");
    }
    let Some((addr_text, prefix_text)) = entry.split_once('/') else {
        return Err("not a valid IPv4 CIDR");
    };
    let addr: Ipv4Addr = addr_text.trim().parse().map_err(|_| "not a valid IPv4 CIDR")?;
    let prefix_text = prefix_text.trim();
    let prefix: u8 = prefix_text.parse().map_err(|_| "not a valid IPv4 CIDR")?;
    if prefix_text != prefix.to_string() || prefix > 32 {
        return Err("not a valid IPv4 CIDR");
    }
    if prefix < MIN_PREFIX {
        return Err("prefix wider than /16");
    }
    let network = network_address(addr, prefix);
    if network != addr {
        return Err("host bits must be zero");
    }
    let inside = PRIVATE_BLOCKS.iter().any(|(block, block_prefix)| {
        let mask = prefix_mask(*block_prefix);
        u32::from(network) & mask == u32::from(*block)
    });
    if !inside {
        return Err("outside RFC1918 and loopback");
    }
    Ok(Cidr { network, prefix })
}

fn prefix_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

fn network_address(addr: Ipv4Addr, prefix: u8) -> Ipv4Addr {
    Ipv4Addr::from(u32::from(addr) & prefix_mask(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reject(entry: &str) -> String {
        Scope::parse(&[entry.to_string()]).unwrap_err().message
    }

    #[test]
    fn rejects_every_out_of_scope_form() {
        let cases = [
            ("8.8.8.8/32", "outside RFC1918 and loopback"),
            ("1.0.0.0/16", "outside RFC1918 and loopback"),
            ("9.255.0.0/16", "outside RFC1918 and loopback"),
            ("11.0.0.0/16", "outside RFC1918 and loopback"),
            ("0.0.0.0/8", "prefix wider than /16"),
            ("0.0.0.0/16", "outside RFC1918 and loopback"),
            ("169.254.0.0/16", "outside RFC1918 and loopback"),
            ("169.254.1.0/24", "outside RFC1918 and loopback"),
            ("100.64.0.0/10", "prefix wider than /16"),
            ("100.64.0.0/16", "outside RFC1918 and loopback"),
            ("100.127.0.0/16", "outside RFC1918 and loopback"),
            ("224.0.0.0/4", "prefix wider than /16"),
            ("224.0.0.0/16", "outside RFC1918 and loopback"),
            ("239.255.0.0/16", "outside RFC1918 and loopback"),
            ("240.0.0.0/4", "prefix wider than /16"),
            ("240.0.0.0/16", "outside RFC1918 and loopback"),
            ("255.255.255.255/32", "outside RFC1918 and loopback"),
            ("255.255.0.0/16", "outside RFC1918 and loopback"),
            ("2001:db8::/32", "IPv6 is not permitted"),
            ("::1", "IPv6 is not permitted"),
            ("fe80::1/64", "IPv6 is not permitted"),
            ("::ffff:192.168.1.1/128", "IPv6 is not permitted"),
            ("10.0.0.0/8", "prefix wider than /16"),
            ("172.16.0.0/12", "prefix wider than /16"),
            ("192.168.0.0/15", "prefix wider than /16"),
            ("127.0.0.0/8", "prefix wider than /16"),
            ("172.15.0.0/16", "outside RFC1918 and loopback"),
            ("172.32.0.0/16", "outside RFC1918 and loopback"),
            ("192.167.0.0/16", "outside RFC1918 and loopback"),
            ("192.169.0.0/16", "outside RFC1918 and loopback"),
            ("192.168.1.5/24", "host bits must be zero"),
            ("192.168.1.1", "not a valid IPv4 CIDR"),
            ("192.168.0.0/33", "not a valid IPv4 CIDR"),
            ("192.168.0.0/016", "not a valid IPv4 CIDR"),
            ("", "not a valid IPv4 CIDR"),
        ];
        for (entry, reason) in cases {
            let message = reject(entry);
            assert!(message.contains(reason), "{entry} -> {message}");
            assert!(!message.contains('\n') && !message.contains('\r'));
        }
    }

    #[test]
    fn accepts_private_and_loopback_prefixes_from_16_to_32() {
        for entry in [
            "10.0.0.0/16",
            "10.255.0.0/16",
            "10.1.2.0/24",
            "172.16.0.0/16",
            "172.31.255.0/24",
            "192.168.0.0/16",
            "192.168.1.0/24",
            "127.0.0.0/16",
            "127.0.0.1/32",
            " 192.168.50.0/24 ",
        ] {
            let scope = Scope::parse(&[entry.to_string()]).unwrap();
            assert_eq!(scope.entries().len(), 1, "{entry}");
        }
    }

    #[test]
    fn empty_scope_contains_nothing_and_check_target_rechecks() {
        let empty = Scope::parse(&[]).unwrap();
        assert!(empty.is_empty());
        assert!(!empty.contains(Ipv4Addr::new(192, 168, 1, 1)));
        assert!(empty
            .check_target(Ipv4Addr::LOCALHOST)
            .unwrap_err()
            .is("NETCONNECT_REFUSED"));

        let scope = Scope::parse(&["192.168.0.0/16".to_string(), "127.0.0.0/16".to_string()]).unwrap();
        assert!(scope.contains(Ipv4Addr::new(192, 168, 1, 20)));
        assert!(scope.contains(Ipv4Addr::new(127, 0, 1, 2)));
        assert!(!scope.contains(Ipv4Addr::new(10, 0, 0, 1)));
        assert!(!scope.contains(Ipv4Addr::new(8, 8, 8, 8)));
        assert!(!scope.contains(Ipv4Addr::new(169, 254, 1, 1)));
        scope.check_target(Ipv4Addr::new(192, 168, 1, 20)).unwrap();
        assert!(scope
            .check_target(Ipv4Addr::new(172, 16, 0, 1))
            .unwrap_err()
            .is("NETCONNECT_REFUSED"));
        assert!(scope.covers_network(Ipv4Addr::new(192, 168, 1, 0), 24));
        assert!(!scope.covers_network(Ipv4Addr::new(192, 168, 0, 0), 15));
        assert!(!scope.covers_network(Ipv4Addr::new(10, 1, 0, 0), 16));
    }

    #[test]
    fn one_bad_entry_rejects_the_whole_list() {
        let err = Scope::parse(&["192.168.0.0/16".to_string(), "8.8.8.8/32".to_string()]).unwrap_err();
        assert!(err.is("CONFIG_ERROR"));
        assert!(err.message.contains("outside RFC1918 and loopback"));
    }

    #[test]
    fn control_characters_in_a_rejected_entry_stay_out_of_the_message() {
        let err = Scope::parse(&["8.8.8.8/32\n".to_string()]).unwrap_err();
        assert!(!err.message.contains('\n') && !err.message.contains('\r'));
        assert!(err.message.contains("outside RFC1918 and loopback"));
        let injected = Scope::parse(&["8.8.8.8/32\nINJECT".to_string()]).unwrap_err();
        assert!(!injected.message.contains('\n'));
    }
}
