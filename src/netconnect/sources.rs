//! Live passive sources. No raw sockets, no privilege change, no user argv.
//!
//! Linux reads `/proc/net/arp` and `/proc/net/route` and asks libc for
//! interface addresses. macOS uses the same interface call and, for the
//! route and neighbor tables, fixed absolute argv (`/usr/sbin/arp`,
//! `/usr/sbin/netstat`) with no shell. Our code opens no netlink, packet,
//! raw, UDP or TCP socket, and on Linux glibc `getifaddrs` opens an
//! `AF_NETLINK` socket internally.

use crate::common::errors::{HarnessError, Result};

use super::collect::{InterfaceSource, NeighborSource, RouteSource};
#[cfg(target_os = "macos")]
use super::parse::{parse_bsd_arp, parse_bsd_netstat};
#[cfg(target_os = "linux")]
use super::parse::{parse_proc_net_arp, parse_proc_net_route};
use super::parse::{InterfaceRecord, NeighborRecord, RouteRecord};

#[cfg(target_os = "linux")]
const PROC_ARP: &str = "/proc/net/arp";
#[cfg(target_os = "linux")]
const PROC_ROUTE: &str = "/proc/net/route";

#[derive(Debug, Default)]
pub struct LiveNeighbors;

#[derive(Debug, Default)]
pub struct LiveRoutes;

#[derive(Debug, Default)]
pub struct LiveInterfaces;

impl NeighborSource for LiveNeighbors {
    fn load(&self) -> Result<Vec<NeighborRecord>> {
        #[cfg(target_os = "linux")]
        {
            Ok(parse_proc_net_arp(&read_file(PROC_ARP)?))
        }
        #[cfg(target_os = "macos")]
        {
            Ok(parse_bsd_arp(&read_fixed_argv(&["/usr/sbin/arp", "-an"])?))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(unsupported())
        }
    }
}

impl RouteSource for LiveRoutes {
    fn load(&self) -> Result<Vec<RouteRecord>> {
        #[cfg(target_os = "linux")]
        {
            Ok(parse_proc_net_route(&read_file(PROC_ROUTE)?))
        }
        #[cfg(target_os = "macos")]
        {
            Ok(parse_bsd_netstat(&read_fixed_argv(&[
                "/usr/sbin/netstat",
                "-rn",
                "-f",
                "inet",
            ])?))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(unsupported())
        }
    }
}

impl InterfaceSource for LiveInterfaces {
    fn load(&self) -> Result<Vec<InterfaceRecord>> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            interfaces_from_getifaddrs()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(unsupported())
        }
    }
}

#[cfg(target_os = "linux")]
fn read_file(path: &str) -> Result<String> {
    std::fs::read_to_string(path).map_err(|_| HarnessError::new("NETCONNECT_IO", "cannot read a local network table"))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn unsupported() -> HarnessError {
    HarnessError::new(
        "NETCONNECT_UNSUPPORTED",
        "passive collectors are implemented for linux and macos",
    )
}

#[cfg(target_os = "macos")]
fn read_fixed_argv(argv: &[&str]) -> Result<String> {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use crate::common::process::{self, RunSpec};

    let owned: Vec<String> = argv.iter().copied().map(str::to_string).collect();
    let mut env = BTreeMap::new();
    env.insert("PATH".to_string(), "/usr/bin:/bin:/usr/sbin:/sbin".to_string());
    env.insert("LC_ALL".to_string(), "C".to_string());
    let output = process::run(RunSpec {
        argv: &owned,
        cwd: None,
        env: Some(&env),
        timeout: Duration::from_secs(2),
        stdin: None,
    })
    .map_err(|_| HarnessError::new("NETCONNECT_IO", "cannot read a local network table"))?;
    if output.timed_out || output.status != Some(0) {
        return Err(HarnessError::new("NETCONNECT_IO", "cannot read a local network table"));
    }
    Ok(output.stdout)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn interfaces_from_getifaddrs() -> Result<Vec<InterfaceRecord>> {
    use std::net::Ipv4Addr;

    // Uninitialized on purpose: null_mut() is what CodeQL treats as the invalid
    // pointer, and getifaddrs writes the list head through this slot on success.
    let mut slot = std::mem::MaybeUninit::<*mut libc::ifaddrs>::uninit();
    // SAFETY: slot is a writable out-pointer. On success it owns a list freed below.
    let rc = unsafe { libc::getifaddrs(slot.as_mut_ptr()) };
    if rc != 0 {
        return Err(HarnessError::new(
            "NETCONNECT_IO",
            "cannot read local interface addresses",
        ));
    }
    // SAFETY: rc == 0 means getifaddrs stored the list head (null when there are no interfaces).
    let head = unsafe { slot.assume_init() };
    let _guard = Ifaddrs(head);
    let mut rows = Vec::new();
    let mut cursor = head;
    loop {
        if cursor.is_null() {
            break;
        }
        // SAFETY: cursor is non-null and is a live getifaddrs node until Ifaddrs drops.
        let node = unsafe { &*cursor };
        let next = node.ifa_next;
        if !node.ifa_addr.is_null() && !node.ifa_name.is_null() {
            // SAFETY: ifa_addr is non-null and points at a sockaddr allocated by getifaddrs.
            let family = unsafe { (*node.ifa_addr).sa_family };
            if family == libc::AF_INET as libc::sa_family_t {
                // SAFETY: AF_INET addresses from getifaddrs are sockaddr_in.
                let sa = unsafe { &*(node.ifa_addr.cast::<libc::sockaddr_in>()) };
                let address = Ipv4Addr::from(sa.sin_addr.s_addr.to_ne_bytes());
                // SAFETY: ifa_name is a non-null NUL-terminated C string.
                let name = unsafe { std::ffi::CStr::from_ptr(node.ifa_name) }
                    .to_string_lossy()
                    .into_owned();
                rows.push(InterfaceRecord { name, address });
            }
        }
        cursor = next;
    }
    Ok(rows)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
struct Ifaddrs(*mut libc::ifaddrs);

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Drop for Ifaddrs {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from getifaddrs and has not been freed.
            unsafe { libc::freeifaddrs(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn passive_sources_do_not_open_raw_sockets_or_a_shell() {
        let text = include_str!("sources.rs");
        assert!(text.contains("\"/usr/sbin/arp\""));
        assert!(text.contains("\"/usr/sbin/netstat\""));
        // Module docs name the socket glibc opens. Scan the implementation.
        let implementation: String = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("//!"))
            .collect::<Vec<_>>()
            .join("\n");
        for parts in [
            &["SOCK_", "RAW"][..],
            &["AF_", "PACKET"],
            &["AF_", "NETLINK"],
            &["Command", "::", "new"],
            &["/bin/", "sh"],
            &["su", "do"],
            &["set", "cap"],
        ] {
            let needle: String = parts.concat();
            assert!(!implementation.contains(&needle), "{needle}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_linux_read_keeps_only_in_scope_addresses() {
        if !std::path::Path::new(super::PROC_ARP).is_file() {
            return;
        }
        let scope = super::super::Scope::parse(&[
            "10.0.0.0/16".to_string(),
            "172.16.0.0/16".to_string(),
            "192.168.0.0/16".to_string(),
            "127.0.0.0/16".to_string(),
        ])
        .unwrap();
        let report = super::super::collect_passive(
            &scope,
            &super::LiveNeighbors,
            &super::LiveRoutes,
            &super::LiveInterfaces,
            64,
        )
        .unwrap();
        for address in report.addresses() {
            assert!(scope.contains(address));
        }
        let empty = super::super::Scope::parse(&[]).unwrap();
        let dropped = super::super::collect_passive(
            &empty,
            &super::LiveNeighbors,
            &super::LiveRoutes,
            &super::LiveInterfaces,
            64,
        )
        .unwrap();
        assert!(dropped.addresses().is_empty());
    }
}
