//! Fail-closed LAN observation.
//!
//! Gates ship closed. A tier runs only when the master flag and that tier's
//! flag are both the literal boolean `true`, and only inside an operator
//! `allowed_cidrs` scope. Scope is never taken from local interfaces.
//! Passive collection sends no packets. `passive_listen` is configuration
//! only: joining a multicast group is not this mode.
//!
//! Later collectors should implement [`NeighborSource`], [`RouteSource`], and
//! [`InterfaceSource`], then pass rows through [`collect_passive`]. Connect
//! paths must call [`Scope::check_target`] on the final [`std::net::Ipv4Addr`]
//! after resolution. [`sanitize_untrusted`] is the only treatment for a
//! device-supplied name; the result is not an argv element, a command, or a path.

pub mod cli;
mod collect;
mod config;
mod parse;
mod sanitize;
mod scope;
mod sources;

pub use collect::{
    collect_passive, FixtureInterfaces, FixtureNeighbors, FixtureRoutes, InterfaceSource, NeighborSource,
    PassiveReport, RouteSource,
};
pub use config::{NetconnectConfig, Tier, THROUGHPUT_WARNING};
pub use parse::{parse_bsd_arp, parse_bsd_netstat, parse_interface_lines, parse_proc_net_arp, parse_proc_net_route};
pub use sanitize::{sanitize_untrusted, UntrustedString};
pub use scope::Scope;
