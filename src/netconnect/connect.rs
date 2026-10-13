//! Budgeted TCP dialer for active netconnect tiers.
//!
//! This is the only file under `src/netconnect` that opens a socket, and
//! `tests/invariant_guard.rs` holds it to that. A dial takes a final
//! [`Ipv4Addr`] and a port. Nothing here resolves a name: the socket address
//! is built from the IPv4 value, so there is no lookup to bypass.
//!
//! Every dial rechecks [`Scope::check_target`] first. A refused address is
//! reported and never reaches the connector. The [`Budget`] is a hard limit,
//! not a hint: distinct hosts stop at `host_cap`, dials stop at the per-run
//! ceiling (`max_dials`, and never more than `rate_limit_per_min`), the shared
//! [`RateBucket`] spaces dials across runs, and one run never outlives its
//! deadline (`connect_timeout_ms` per allowed dial, capped at 60 seconds).
//! Hitting a limit stops the run and returns what it has, marked truncated.
//!
//! The loop runs inside the caller's future and holds no task of its own, so
//! dropping that future (a server dropping a request) stops it at the next
//! await. The deadline bounds a run that nobody drops.

use std::collections::BTreeSet;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tokio::time::Instant;

use super::config::NetconnectConfig;
use super::scope::Scope;

/// Upper bound on one run, whatever the config says.
pub const MAX_RUN: Duration = Duration::from_secs(60);

/// What one probe found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeResult {
    /// The handshake completed. The connection is closed straight away.
    Open,
    /// The host answered with a reset.
    Closed,
    /// No answer before the connect timeout.
    Timeout,
    /// Any other connect error, such as no route to host.
    Unreachable,
    /// `Scope::check_target` refused the address. No packet was sent.
    RefusedScope,
}

/// Why a run stopped before every pair was probed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Truncation {
    HostCap,
    MaxDials,
    RateLimit,
    Deadline,
}

impl Truncation {
    pub fn message(self) -> &'static str {
        match self {
            Truncation::HostCap => "stopped at netconnect.host_cap distinct hosts",
            Truncation::MaxDials => "stopped at the per-run dial ceiling (max_dials, at most rate_limit_per_min)",
            Truncation::RateLimit => "netconnect.rate_limit_per_min has no dial left before the run deadline",
            Truncation::Deadline => "stopped at the run deadline",
        }
    }
}

/// Opens one TCP connection. Tests swap in a recorder; production uses [`LiveConnector`].
pub trait Connector {
    fn connect(&self, addr: SocketAddrV4, timeout: Duration) -> impl Future<Output = ProbeResult> + Send;
}

/// Real TCP connect. Takes a socket address, never a host name.
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveConnector;

impl Connector for LiveConnector {
    async fn connect(&self, addr: SocketAddrV4, timeout: Duration) -> ProbeResult {
        match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(SocketAddr::V4(addr))).await {
            Err(_) => ProbeResult::Timeout,
            Ok(Ok(stream)) => {
                drop(stream);
                ProbeResult::Open
            }
            Ok(Err(err)) if err.kind() == std::io::ErrorKind::ConnectionRefused => ProbeResult::Closed,
            Ok(Err(_)) => ProbeResult::Unreachable,
        }
    }
}

/// Process-wide token bucket for `rate_limit_per_min`.
///
/// It holds up to `per_min` dials and refills at `per_min` per minute, so no
/// sixty-second window carries more than twice that. The per-run ceiling in
/// [`Budget`] keeps one run to `per_min`.
#[derive(Debug, Default)]
pub struct RateBucket {
    state: Mutex<Option<BucketState>>,
}

#[derive(Debug)]
struct BucketState {
    per_min: u64,
    tokens: f64,
    last: Instant,
}

impl RateBucket {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take one dial, or say how long until one is free.
    fn take(&self, per_min: u64, now: Instant) -> std::result::Result<(), Duration> {
        let per_min = per_min.max(1);
        let mut guard = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let state = guard.get_or_insert(BucketState {
            per_min,
            tokens: per_min as f64,
            last: now,
        });
        if state.per_min != per_min {
            *state = BucketState {
                per_min,
                tokens: per_min as f64,
                last: now,
            };
        }
        let per_sec = per_min as f64 / 60.0;
        let elapsed = now.saturating_duration_since(state.last).as_secs_f64();
        state.tokens = (state.tokens + elapsed * per_sec).min(per_min as f64);
        state.last = now;
        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - state.tokens) / per_sec))
        }
    }
}

/// Hard limits for one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    host_cap: usize,
    per_min: u64,
    connect: Duration,
    dial_ceiling: usize,
    deadline: Duration,
}

impl Budget {
    pub fn from_config(cfg: &NetconnectConfig) -> Self {
        let dial_ceiling = cfg.max_dials.min(cfg.rate_limit_per_min).max(1);
        let connect = Duration::from_millis(cfg.connect_timeout_ms);
        let deadline = connect
            .saturating_mul(u32::try_from(dial_ceiling).unwrap_or(u32::MAX))
            .min(MAX_RUN);
        Self {
            host_cap: usize::try_from(cfg.host_cap).unwrap_or(usize::MAX),
            per_min: cfg.rate_limit_per_min,
            connect,
            dial_ceiling: usize::try_from(dial_ceiling).unwrap_or(usize::MAX),
            deadline,
        }
    }

    pub fn dial_ceiling(&self) -> usize {
        self.dial_ceiling
    }

    pub fn deadline(&self) -> Duration {
        self.deadline
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Probe {
    pub address: Ipv4Addr,
    pub port: u16,
    pub result: ProbeResult,
    /// Time to the connect result. Null when no packet was sent.
    pub rtt_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialReport {
    pub probes: Vec<Probe>,
    /// Connect attempts. Kernel retransmits and the close are not counted separately.
    pub dials: u64,
    pub truncated: Option<Truncation>,
    pub elapsed: Duration,
}

/// The one way an active tier reaches the network.
pub struct Dialer<'a, C> {
    scope: &'a Scope,
    budget: Budget,
    rate: &'a RateBucket,
    connector: C,
}

impl<'a, C: Connector> Dialer<'a, C> {
    pub fn new(scope: &'a Scope, budget: Budget, rate: &'a RateBucket, connector: C) -> Self {
        Self {
            scope,
            budget,
            rate,
            connector,
        }
    }

    /// Probe each pair in order, one at a time, until done or a limit stops the run.
    pub async fn probe_all(&self, pairs: &[(Ipv4Addr, u16)]) -> DialReport {
        let start = Instant::now();
        let deadline = start + self.budget.deadline;
        let mut hosts = BTreeSet::new();
        let mut probes = Vec::with_capacity(pairs.len().min(self.budget.dial_ceiling));
        let mut dials = 0u64;
        let mut truncated = None;
        'pairs: for &(address, port) in pairs {
            if self.scope.check_target(address).is_err() {
                probes.push(Probe {
                    address,
                    port,
                    result: ProbeResult::RefusedScope,
                    rtt_ms: None,
                });
                continue;
            }
            if !hosts.contains(&address) && hosts.len() >= self.budget.host_cap {
                truncated = Some(Truncation::HostCap);
                break;
            }
            if dials >= self.budget.dial_ceiling as u64 {
                truncated = Some(Truncation::MaxDials);
                break;
            }
            loop {
                let now = Instant::now();
                match self.rate.take(self.budget.per_min, now) {
                    Ok(()) => break,
                    Err(wait) if now + wait < deadline => tokio::time::sleep(wait).await,
                    Err(_) => {
                        truncated = Some(Truncation::RateLimit);
                        break 'pairs;
                    }
                }
            }
            let now = Instant::now();
            if now >= deadline {
                truncated = Some(Truncation::Deadline);
                break;
            }
            let timeout = self.budget.connect.min(deadline - now);
            hosts.insert(address);
            dials += 1;
            let sent = Instant::now();
            let result = self.connector.connect(SocketAddrV4::new(address, port), timeout).await;
            probes.push(Probe {
                address,
                port,
                result,
                rtt_ms: Some(u64::try_from(sent.elapsed().as_millis()).unwrap_or(u64::MAX)),
            });
        }
        DialReport {
            probes,
            dials,
            truncated,
            elapsed: start.elapsed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;

    /// Records every address that reached the socket layer.
    #[derive(Clone, Default)]
    struct Recorder {
        seen: Arc<Mutex<Vec<SocketAddrV4>>>,
        delay: Duration,
    }

    impl Connector for Recorder {
        async fn connect(&self, addr: SocketAddrV4, timeout: Duration) -> ProbeResult {
            self.seen.lock().unwrap().push(addr);
            if self.delay > timeout {
                tokio::time::sleep(timeout).await;
                return ProbeResult::Timeout;
            }
            tokio::time::sleep(self.delay).await;
            if addr.port() == 22 {
                ProbeResult::Open
            } else {
                ProbeResult::Closed
            }
        }
    }

    fn scope() -> Scope {
        Scope::parse(&["192.168.1.0/24".to_string()]).unwrap()
    }

    fn budget(host_cap: usize, per_min: u64, dial_ceiling: usize, deadline_ms: u64) -> Budget {
        Budget {
            host_cap,
            per_min,
            connect: Duration::from_millis(100),
            dial_ceiling,
            deadline: Duration::from_millis(deadline_ms),
        }
    }

    fn addr(last: u8) -> Ipv4Addr {
        Ipv4Addr::new(192, 168, 1, last)
    }

    #[tokio::test]
    async fn scope_refused_address_never_reaches_the_socket() {
        let scope = scope();
        let rate = RateBucket::new();
        let recorder = Recorder::default();
        let dialer = Dialer::new(&scope, budget(8, 100, 8, 5_000), &rate, recorder.clone());
        let report = dialer
            .probe_all(&[
                (Ipv4Addr::new(8, 8, 8, 8), 22),
                (addr(5), 22),
                (Ipv4Addr::new(10, 0, 0, 1), 80),
            ])
            .await;
        assert_eq!(*recorder.seen.lock().unwrap(), vec![SocketAddrV4::new(addr(5), 22)]);
        assert_eq!(report.dials, 1);
        assert_eq!(report.truncated, None);
        assert_eq!(report.probes[0].result, ProbeResult::RefusedScope);
        assert_eq!(report.probes[0].rtt_ms, None);
        assert_eq!(report.probes[1].result, ProbeResult::Open);
        assert_eq!(report.probes[2].result, ProbeResult::RefusedScope);
    }

    #[tokio::test]
    async fn dial_ceiling_truncates_and_keeps_partial_results() {
        let scope = scope();
        let rate = RateBucket::new();
        let recorder = Recorder::default();
        let dialer = Dialer::new(&scope, budget(8, 100, 2, 5_000), &rate, recorder.clone());
        let report = dialer.probe_all(&[(addr(1), 22), (addr(1), 80), (addr(1), 443)]).await;
        assert_eq!(report.dials, 2);
        assert_eq!(report.probes.len(), 2);
        assert_eq!(report.truncated, Some(Truncation::MaxDials));
        assert_eq!(recorder.seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn host_cap_counts_distinct_hosts() {
        let scope = scope();
        let rate = RateBucket::new();
        let recorder = Recorder::default();
        let dialer = Dialer::new(&scope, budget(1, 100, 8, 5_000), &rate, recorder.clone());
        let report = dialer.probe_all(&[(addr(1), 22), (addr(1), 80), (addr(2), 22)]).await;
        assert_eq!(report.dials, 2, "a second port on the same host is not a new host");
        assert_eq!(report.truncated, Some(Truncation::HostCap));
        assert!(recorder.seen.lock().unwrap().iter().all(|seen| *seen.ip() == addr(1)));
    }

    #[tokio::test]
    async fn rate_limit_shared_across_runs_refuses_past_the_deadline() {
        let scope = scope();
        let rate = RateBucket::new();
        let recorder = Recorder::default();
        // One dial per minute: the first run spends it, the second cannot wait a minute.
        let first = Dialer::new(&scope, budget(8, 1, 8, 500), &rate, recorder.clone())
            .probe_all(&[(addr(1), 22), (addr(1), 80)])
            .await;
        assert_eq!(first.dials, 1);
        assert_eq!(first.truncated, Some(Truncation::RateLimit));
        let second = Dialer::new(&scope, budget(8, 1, 8, 500), &rate, recorder.clone())
            .probe_all(&[(addr(2), 22)])
            .await;
        assert_eq!(second.dials, 0);
        assert_eq!(second.truncated, Some(Truncation::RateLimit));
        assert_eq!(recorder.seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn deadline_stops_a_slow_run() {
        let scope = scope();
        let rate = RateBucket::new();
        let recorder = Recorder {
            delay: Duration::from_millis(80),
            ..Recorder::default()
        };
        let dialer = Dialer::new(&scope, budget(8, 1_000, 64, 150), &rate, recorder.clone());
        let pairs: Vec<_> = (1..=10).map(|last| (addr(last), 22)).collect();
        let report = dialer.probe_all(&pairs).await;
        assert_eq!(report.truncated, Some(Truncation::Deadline));
        assert!(report.dials < 10);
        assert!(report.elapsed < Duration::from_millis(1_000), "{:?}", report.elapsed);
    }

    #[tokio::test]
    async fn dropping_the_run_stops_dialing() {
        let scope = scope();
        let rate = RateBucket::new();
        let calls = Arc::new(AtomicUsize::new(0));
        struct Hang(Arc<AtomicUsize>);
        impl Connector for Hang {
            async fn connect(&self, _addr: SocketAddrV4, timeout: Duration) -> ProbeResult {
                self.0.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(timeout).await;
                ProbeResult::Timeout
            }
        }
        let mut long = budget(8, 1_000, 64, 5_000);
        long.connect = Duration::from_secs(5);
        let dialer = Dialer::new(&scope, long, &rate, Hang(Arc::clone(&calls)));
        let pairs: Vec<_> = (1..=10).map(|last| (addr(last), 22)).collect();
        let run = dialer.probe_all(&pairs);
        assert!(tokio::time::timeout(Duration::from_millis(50), run).await.is_err());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "a dropped run starts no further dial");
    }

    #[tokio::test]
    async fn live_connector_reports_open_and_closed_on_loopback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this fixture listener is loopback only.
        let open = listener.local_addr().unwrap().port();
        let closed = {
            let spare = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); // DevSkim: ignore DS162092 because this spare port is bound on loopback only to find a closed one.
            spare.local_addr().unwrap().port()
        };
        let scope = Scope::parse(&["127.0.0.0/16".to_string()]).unwrap();
        let rate = RateBucket::new();
        let dialer = Dialer::new(&scope, budget(4, 100, 4, 5_000), &rate, LiveConnector);
        let report = dialer
            .probe_all(&[(Ipv4Addr::LOCALHOST, open), (Ipv4Addr::LOCALHOST, closed)])
            .await;
        assert_eq!(report.probes[0].result, ProbeResult::Open);
        assert_eq!(report.probes[1].result, ProbeResult::Closed);
        assert_eq!(report.dials, 2);
    }

    #[test]
    fn budget_from_config_caps_one_run() {
        let app = crate::common::config::AppConfig::from_str(
            "netconnect:\n  max_dials: 65536\n  rate_limit_per_min: 10000\n  connect_timeout_ms: 60000\n",
            std::path::Path::new("fixture.yaml"),
        )
        .unwrap();
        let cfg = NetconnectConfig::from_config(&app).unwrap();
        let budget = Budget::from_config(&cfg);
        assert_eq!(
            budget.dial_ceiling(),
            10_000,
            "one run never exceeds rate_limit_per_min"
        );
        assert_eq!(budget.deadline(), MAX_RUN);
        let shipped = NetconnectConfig::from_config(
            &crate::common::config::AppConfig::from_str(
                crate::common::config::AppConfig::embedded_default(),
                std::path::Path::new("defaults"),
            )
            .unwrap(),
        )
        .unwrap();
        let budget = Budget::from_config(&shipped);
        assert_eq!(budget.dial_ceiling(), 30);
        assert_eq!(budget.deadline(), Duration::from_secs(30));
    }
}
