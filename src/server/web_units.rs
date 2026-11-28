//! Durable daily provider-unit ceilings for the paid web providers (#772 D10).
//!
//! Every outbound Tavily search, SerpAPI search and Firecrawl read first
//! reserves the provider units it may cost in a private SQLite ledger under
//! the home's tools directory (`web_units.sqlite`). The check and the insert
//! are one `BEGIN IMMEDIATE` transaction, so concurrent turns, `/loop` runs,
//! `/web research` and a CLI `cgagentharness web` process on the same home
//! cannot together pass a ceiling, and a restart keeps what was spent.
//!
//! A reservation is released only when the request provably never left the
//! machine (a refusal before the send). Once the send starts it stays
//! reserved, whatever happens next: a timeout, a dropped connection or a
//! crash does not prove the provider did not count it. A ledger that cannot
//! be opened, read or written refuses the paid hop (`WEB_UNITS_UNAVAILABLE`),
//! and so does an established ledger whose file has gone missing: a deleted
//! file never reads as "nothing spent".
//!
//! Days are UTC days. The ceilings are restart-only config keys, shipped at
//! the operator's approved values (#772 D10/O1: Tavily 30 credits, SerpAPI 8
//! searches, Firecrawl 30 credits). They count only this home's requests; they
//! do not know a provider dashboard's balance, reset date or other clients
//! sharing the key.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{params, Connection, TransactionBehavior};
use serde_json::json;

use super::web_google::unix_now;
use super::web_policy::error;
use super::web_search::WebTool;
use crate::common::audit::Audit;
use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};
use crate::common::private_sqlite;

/// The search-hop refusal when a provider's daily units are spent. It falls
/// through to the next enabled hop (`web_provider::falls_through`), like a
/// provider's own quota refusal, but starts no cooldown.
pub const SEARCH_DAILY_CAP: &str = "WEB_SEARCH_DAILY_CAP";
/// Firecrawl's daily refusal reuses its hourly cap code; `details.limit` says which.
pub const FALLBACK_CAP: &str = "WEB_FALLBACK_CAP";
/// The ledger could not be opened, read or written: the paid hop is refused.
pub const UNITS_UNAVAILABLE: &str = "WEB_UNITS_UNAVAILABLE";

const LEDGER_FILE: &str = "web_units.sqlite";
/// Written once the ledger first holds its schema: a later missing ledger file
/// then refuses instead of starting again from zero.
const MARKER_FILE: &str = "web_units.established";
/// Open limit. With two days kept and SQLite reusing freed pages, even the
/// highest ceilings (100,000 units a day each) stay well under it.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// UTC days of rows kept besides today; older rows are pruned on each
/// reservation. Only today's rows are ever read.
const KEEP_DAYS: i64 = 1;
const SECONDS_PER_DAY: u64 = 86_400;
/// Bounds every `*_units_per_day` key accepts.
const CEILING_RANGE: std::ops::RangeInclusive<u64> = 1..=100_000;

/// A provider whose requests cost units the operator pays for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paid {
    Tavily,
    Serpapi,
    Firecrawl,
}

impl Paid {
    pub fn name(self) -> &'static str {
        match self {
            Self::Tavily => "tavily",
            Self::Serpapi => "serpapi",
            Self::Firecrawl => "firecrawl",
        }
    }

    /// The config key holding this provider's daily ceiling.
    pub fn ceiling_key(self) -> &'static str {
        match self {
            Self::Tavily => "web.search.tavily_units_per_day",
            Self::Serpapi => "web.search.serpapi_units_per_day",
            Self::Firecrawl => "web.firecrawl.units_per_day",
        }
    }

    fn cap_code(self) -> &'static str {
        match self {
            Self::Tavily | Self::Serpapi => SEARCH_DAILY_CAP,
            Self::Firecrawl => FALLBACK_CAP,
        }
    }
}

/// Daily ceilings, restart-only. A missing key takes the shipped value: an
/// older home gets the same protection as a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ceilings {
    pub tavily: u64,
    pub serpapi: u64,
    pub firecrawl: u64,
}

impl Ceilings {
    pub fn load(cfg: &AppConfig) -> Result<Self> {
        let ceilings = Self {
            // Literal keys: the invariant guard checks every numeric read by name.
            tavily: cfg.u64_in("web.search.tavily_units_per_day", 30, CEILING_RANGE)?,
            serpapi: cfg.u64_in("web.search.serpapi_units_per_day", 8, CEILING_RANGE)?,
            firecrawl: cfg.u64_in("web.firecrawl.units_per_day", 30, CEILING_RANGE)?,
        };
        // Every Firecrawl read reserves the PDF cap, so a smaller ceiling admits
        // none. Refused only while enabled (operator decision, 2026-11-25).
        let per_read = super::web_firecrawl::PDF_MAX_PAGES;
        if cfg.flag_is_true("web.firecrawl.enabled") && ceilings.firecrawl < per_read {
            return Err(HarnessError::config(format!(
                "web.firecrawl.units_per_day must be at least {per_read} while web.firecrawl.enabled is true: \
                 every read reserves the {per_read}-page PDF cap"
            )));
        }
        Ok(ceilings)
    }

    pub fn of(&self, provider: Paid) -> u64 {
        match provider {
            Paid::Tavily => self.tavily,
            Paid::Serpapi => self.serpapi,
            Paid::Firecrawl => self.firecrawl,
        }
    }
}

/// Units held for one outbound request; [`Ledger::release`] gives them back.
#[derive(Debug)]
pub struct Reservation {
    id: i64,
}

/// The durable ledger. Holds only paths and ceilings: every call opens its own
/// connection, so it is safe to share across snapshots and threads. Every
/// method blocks (SQLite and fsync); async callers go through
/// [`WebTool::reserve_units`] and [`WebTool::release_unsent`].
#[derive(Debug)]
pub struct Ledger {
    dir: PathBuf,
    ceilings: Ceilings,
}

fn unavailable(message: impl Into<String>) -> HarnessError {
    HarnessError::new(UNITS_UNAVAILABLE, message)
}

fn sql(_: rusqlite::Error) -> HarnessError {
    // The SQLite text can carry a path; the refusal names the store only.
    unavailable("the provider-unit ledger refused the operation; no paid request was sent")
}

impl Ledger {
    pub fn new(tools_dir: &Path, ceilings: Ceilings) -> Self {
        Self {
            dir: tools_dir.to_path_buf(),
            ceilings,
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.join(LEDGER_FILE)
    }

    fn marker(&self) -> PathBuf {
        self.dir.join(MARKER_FILE)
    }

    fn open(&self) -> Result<Connection> {
        let (path, marker) = (self.path(), self.marker());
        let refused = |_: std::io::Error| unavailable("the provider-unit ledger could not be opened");
        if !private_sqlite::present(&path).map_err(|_| unavailable("the provider-unit ledger could not be read"))? {
            if private_sqlite::present(&marker)
                .map_err(|_| unavailable("the provider-unit ledger could not be read"))?
            {
                return Err(unavailable(
                    "the provider-unit ledger is missing although one was established; paid web providers stay \
                     refused until it is restored or its web_units.established marker is removed",
                ));
            }
            std::fs::create_dir_all(&self.dir).map_err(refused)?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(_) => {}
                // Another process or thread created it first: use that one.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(refused(e)),
            }
        }
        // Every failure to open, including a generic I/O error from the private-file
        // checks, is the ledger's own refusal, so a search still falls through.
        let conn =
            private_sqlite::connect(&path, MAX_FILE_BYTES, "provider-unit ledger", unavailable).map_err(|e| {
                if e.code == UNITS_UNAVAILABLE {
                    e
                } else {
                    unavailable("the provider-unit ledger could not be opened")
                }
            })?;
        let established =
            private_sqlite::present(&marker).map_err(|_| unavailable("the provider-unit ledger could not be read"))?;
        if established {
            // An established ledger emptied or truncated has no table: SQLite would
            // accept it as a new database, so creating the schema now would reset
            // the day's spending to zero.
            let tables: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'units'",
                    [],
                    |row| row.get(0),
                )
                .map_err(sql)?;
            if tables != 1 {
                return Err(unavailable(
                    "the established provider-unit ledger has lost its records; paid web providers stay refused \
                     until it is restored or its web_units.established marker is removed",
                ));
            }
            return Ok(conn);
        }
        // One transaction: the table and its index cost one commit's fsyncs on
        // a new home, not two, and never exist one without the other.
        conn.execute_batch(
            "BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS units (
                id INTEGER PRIMARY KEY,
                provider TEXT NOT NULL,
                day INTEGER NOT NULL,
                units INTEGER NOT NULL CHECK (units > 0),
                at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS units_by_day ON units (provider, day);
            COMMIT;",
        )
        .map_err(sql)?;
        crate::common::atomic::write_atomic(&marker, b"provider-unit ledger established\n", Some(0o600))
            .map_err(|_| unavailable("the provider-unit ledger could not be written"))?;
        Ok(conn)
    }

    /// Reserve `units` of `provider` for one request at `now_unix`, or refuse:
    /// the provider's daily cap code when today's units would pass its
    /// ceiling, `WEB_UNITS_UNAVAILABLE` when the ledger cannot answer.
    pub fn reserve(&self, provider: Paid, units: u64, now_unix: u64) -> Result<Reservation> {
        let ceiling = self.ceilings.of(provider);
        let day = (now_unix / SECONDS_PER_DAY) as i64;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let used: i64 = tx
            .query_row(
                "SELECT COALESCE(SUM(units), 0) FROM units WHERE provider = ?1 AND day = ?2",
                params![provider.name(), day],
                |row| row.get(0),
            )
            .map_err(sql)?;
        let used = u64::try_from(used).unwrap_or(u64::MAX);
        if used.saturating_add(units) > ceiling {
            return Err(error(
                provider.cap_code(),
                &format!(
                    "{} is spent for today (UTC); no {} request was sent",
                    provider.ceiling_key(),
                    provider.name()
                ),
            )
            .detail("limit", "daily")
            .detail("provider", provider.name())
            .detail("units_used", used)
            .detail("units_ceiling", ceiling)
            .detail("units_requested", units));
        }
        let units_sql = i64::try_from(units).map_err(|_| unavailable("unit count out of range"))?;
        tx.execute(
            "INSERT INTO units (provider, day, units, at) VALUES (?1, ?2, ?3, ?4)",
            params![provider.name(), day, units_sql, now_unix as i64],
        )
        .map_err(sql)?;
        let id = tx.last_insert_rowid();
        tx.execute("DELETE FROM units WHERE day < ?1", params![day - KEEP_DAYS])
            .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(Reservation { id })
    }

    /// Give back a reservation whose request never left the machine.
    pub fn release(&self, reservation: &Reservation) -> Result<()> {
        let conn = self.open()?;
        conn.execute("DELETE FROM units WHERE id = ?1", params![reservation.id])
            .map_err(sql)?;
        Ok(())
    }

    /// Units `provider` holds for the UTC day of `now_unix`.
    #[cfg(test)]
    pub fn used(&self, provider: Paid, now_unix: u64) -> Result<u64> {
        let conn = self.open()?;
        let used: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(units), 0) FROM units WHERE provider = ?1 AND day = ?2",
                params![provider.name(), (now_unix / SECONDS_PER_DAY) as i64],
                |row| row.get(0),
            )
            .map_err(sql)?;
        Ok(u64::try_from(used).unwrap_or(u64::MAX))
    }
}

/// Units held for one outbound request. Dropped without [`Self::mark_sent`],
/// it gives them back: a refusal before the send, or the request future
/// being dropped by an outer deadline or a cancelled turn before it sent
/// anything, never keeps units. Marked sent, the units stay reserved.
pub struct UnitGuard {
    ledger: Arc<Ledger>,
    reservation: Option<Reservation>,
    sent: std::sync::atomic::AtomicBool,
}

impl UnitGuard {
    /// Set just before the request leaves: from here an outcome is ambiguous.
    pub fn mark_sent(&self) {
        self.sent.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// The flag [`Self::mark_sent`] sets, for code that marks the send itself.
    pub fn sent_flag(&self) -> &std::sync::atomic::AtomicBool {
        &self.sent
    }
}

impl Drop for UnitGuard {
    fn drop(&mut self) {
        if self.sent.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let Some(reservation) = self.reservation.take() else {
            return;
        };
        let ledger = Arc::clone(&self.ledger);
        let release = move || {
            if ledger.release(&reservation).is_err() {
                tracing::warn!(
                    "an unsent paid web request's units could not be released; they stay reserved for today"
                );
            }
        };
        // SQLite and fsync stay off the async worker that drops the guard.
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(release);
            }
            Err(_) => release(),
        }
    }
}

impl WebTool {
    /// Reserve units for one paid request, on the blocking pool. A refusal is
    /// audited with the provider, code and counts, never a query, URL or key.
    /// The reservation crosses a oneshot already wrapped in its [`UnitGuard`]:
    /// if this future is dropped while the blocking task runs, the send fails
    /// and the task gives the units back itself; if it is dropped after the
    /// send but before it is polled again, the guard drops with the channel and
    /// gives them back. A cancelled reservation never leaks either way.
    pub(super) async fn reserve_units(&self, provider: Paid, units: u64, audit: &Audit) -> Result<UnitGuard> {
        let ledger = Arc::clone(&self.units);
        let (handoff, received) = tokio::sync::oneshot::channel();
        tokio::task::spawn_blocking(move || {
            let outcome = ledger
                .reserve(provider, units, unix_now())
                .map(|reservation| UnitGuard {
                    ledger: Arc::clone(&ledger),
                    reservation: Some(reservation),
                    sent: std::sync::atomic::AtomicBool::new(false),
                });
            // Released here, inline, rather than by the guard's spawned release,
            // which a runtime already shutting down might not run.
            if let Err(Ok(mut orphaned)) = handoff.send(outcome) {
                if let Some(reservation) = orphaned.reservation.take() {
                    let _ = ledger.release(&reservation);
                }
            }
        });
        let outcome = received
            .await
            .unwrap_or_else(|_| Err(unavailable("the provider-unit ledger task stopped")));
        if let Err(refusal) = &outcome {
            audit.log(json!({
                "event": "web_units_refused",
                "provider": provider.name(),
                "code": refusal.code,
                "units": units,
                "units_used": refusal.details.get("units_used"),
                "units_ceiling": refusal.details.get("units_ceiling"),
            }));
        }
        outcome
    }
}

#[cfg(test)]
pub(in crate::server) mod tests {
    use super::*;

    const DAY: u64 = SECONDS_PER_DAY;

    fn ledger(dir: &Path, tavily: u64, serpapi: u64, firecrawl: u64) -> Ledger {
        Ledger::new(
            dir,
            Ceilings {
                tavily,
                serpapi,
                firecrawl,
            },
        )
    }

    fn private_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        dir
    }

    #[test]
    fn a_ceiling_holds_per_provider_and_utc_day_and_a_release_gives_units_back() {
        let dir = private_dir();
        let units = ledger(dir.path(), 3, 2, 30);
        let now = 100 * DAY + 10;
        let kept: Vec<Reservation> = (0..3).map(|_| units.reserve(Paid::Tavily, 1, now).unwrap()).collect();
        let refused = units.reserve(Paid::Tavily, 1, now).unwrap_err();
        assert_eq!(refused.code, SEARCH_DAILY_CAP);
        assert_eq!(refused.details["limit"], "daily");
        assert_eq!(refused.details["units_used"], 3);
        assert_eq!(refused.details["units_ceiling"], 3);
        // Another provider has its own ceiling.
        units.reserve(Paid::Serpapi, 1, now).unwrap();
        // A proven-unsent request gives its unit back; a sent one keeps it.
        units.release(&kept[0]).unwrap();
        assert_eq!(units.used(Paid::Tavily, now).unwrap(), 2);
        units.reserve(Paid::Tavily, 1, now).unwrap();
        assert!(units.reserve(Paid::Tavily, 1, now).is_err());
        // The next UTC day starts empty.
        units.reserve(Paid::Tavily, 1, now + DAY).unwrap();
        assert_eq!(units.used(Paid::Tavily, now + DAY).unwrap(), 1);
    }

    #[test]
    fn a_request_larger_than_what_is_left_is_refused_whole() {
        let dir = private_dir();
        let units = ledger(dir.path(), 30, 8, 30);
        let now = 7 * DAY;
        for _ in 0..2 {
            units.reserve(Paid::Firecrawl, 10, now).unwrap();
        }
        units.reserve(Paid::Firecrawl, 1, now).unwrap();
        let refused = units.reserve(Paid::Firecrawl, 10, now).unwrap_err();
        assert_eq!(refused.code, FALLBACK_CAP);
        assert_eq!(refused.details["units_requested"], 10);
        // A one-unit read still fits under the ceiling.
        units.reserve(Paid::Firecrawl, 1, now).unwrap();
        assert_eq!(units.used(Paid::Firecrawl, now).unwrap(), 22);
    }

    #[test]
    fn spent_units_survive_a_restart_and_old_days_are_pruned() {
        let dir = private_dir();
        let now = 500 * DAY;
        ledger(dir.path(), 2, 8, 30)
            .reserve(Paid::Tavily, 1, now - 50 * DAY)
            .unwrap();
        ledger(dir.path(), 2, 8, 30).reserve(Paid::Tavily, 2, now).unwrap();
        let reopened = ledger(dir.path(), 2, 8, 30);
        assert_eq!(
            reopened.reserve(Paid::Tavily, 1, now).unwrap_err().code,
            SEARCH_DAILY_CAP
        );
        assert_eq!(
            reopened.used(Paid::Tavily, now - 50 * DAY).unwrap(),
            0,
            "rows past KEEP_DAYS were pruned"
        );
    }

    #[test]
    fn concurrent_reservations_never_pass_the_ceiling() {
        let dir = private_dir();
        let units = Arc::new(ledger(dir.path(), 8, 8, 30));
        let now = 9 * DAY;
        // Create the ledger first so every thread races on reservations, not the file.
        assert_eq!(units.used(Paid::Serpapi, now).unwrap(), 0);
        let handles: Vec<_> = (0..24)
            .map(|_| {
                let units = Arc::clone(&units);
                std::thread::spawn(move || units.reserve(Paid::Serpapi, 1, now).is_ok())
            })
            .collect();
        let granted = handles.into_iter().map(|h| h.join().unwrap()).filter(|ok| *ok).count();
        assert_eq!(granted, 8, "exactly the ceiling was granted");
        assert_eq!(units.used(Paid::Serpapi, now).unwrap(), 8);
    }

    #[test]
    fn a_missing_corrupt_or_unwritable_ledger_refuses_the_paid_hop() {
        // Established, then deleted: refused, not restarted from zero.
        let dir = private_dir();
        let units = ledger(dir.path(), 30, 8, 30);
        units.reserve(Paid::Tavily, 1, DAY).unwrap();
        std::fs::remove_file(dir.path().join(LEDGER_FILE)).unwrap();
        assert_eq!(units.reserve(Paid::Tavily, 1, DAY).unwrap_err().code, UNITS_UNAVAILABLE);
        assert!(!dir.path().join(LEDGER_FILE).exists(), "nothing was recreated");

        // Not a database.
        let dir = private_dir();
        let path = dir.path().join(LEDGER_FILE);
        std::fs::write(&path, b"not a sqlite file, just text long enough to have a header").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(
            ledger(dir.path(), 30, 8, 30)
                .reserve(Paid::Tavily, 1, DAY)
                .unwrap_err()
                .code,
            UNITS_UNAVAILABLE
        );

        // A ledger others may read or write is refused before it is used.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = private_dir();
            let units = ledger(dir.path(), 30, 8, 30);
            units.reserve(Paid::Tavily, 1, DAY).unwrap();
            let path = dir.path().join(LEDGER_FILE);
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(units.reserve(Paid::Tavily, 1, DAY).unwrap_err().code, UNITS_UNAVAILABLE);
        }
    }

    /// Codex review on #1077: an established ledger truncated to zero bytes is
    /// a valid empty SQLite file, and recreating its table would reset the
    /// day's spending. It is refused instead, and a ledger path that cannot be
    /// opened at all (here a symlink, refused by O_NOFOLLOW) reports the
    /// ledger's own code, so a search still falls through.
    #[test]
    fn a_truncated_or_unopenable_established_ledger_is_refused_not_reset() {
        let dir = private_dir();
        let units = ledger(dir.path(), 2, 8, 30);
        units.reserve(Paid::Tavily, 2, DAY).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(dir.path().join(LEDGER_FILE))
            .unwrap();
        assert_eq!(units.reserve(Paid::Tavily, 1, DAY).unwrap_err().code, UNITS_UNAVAILABLE);
        assert_eq!(units.used(Paid::Tavily, DAY).unwrap_err().code, UNITS_UNAVAILABLE);

        #[cfg(unix)]
        {
            let dir = private_dir();
            let target = dir.path().join("elsewhere.sqlite");
            std::fs::write(&target, b"").unwrap();
            std::os::unix::fs::symlink(&target, dir.path().join(LEDGER_FILE)).unwrap();
            let refused = ledger(dir.path(), 2, 8, 30).reserve(Paid::Tavily, 1, DAY).unwrap_err();
            assert_eq!(refused.code, UNITS_UNAVAILABLE, "{}", refused.message);
        }
    }

    /// An unsent guard releases on the blocking pool after it is dropped, so a
    /// test waits (up to 2 s) for the count instead of reading it once.
    pub(in crate::server) async fn wait_for_used(units: &Ledger, provider: Paid, now: u64, want: u64) {
        for _ in 0..200 {
            if units.used(provider, now).unwrap() == want {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(units.used(provider, now).unwrap(), want);
    }

    /// Codex review on #1077: units must come back when the request future is
    /// dropped before it sends, by an outer hop deadline or a cancelled turn.
    /// A guard marked sent keeps its units.
    #[tokio::test]
    async fn a_dropped_or_abandoned_unsent_reservation_gives_its_units_back() {
        let dir = private_dir();
        let cfg = AppConfig::from_str("{}", Path::new("config.yaml")).unwrap();
        let web = WebTool::new(dir.path(), &cfg).unwrap();
        let audit = Audit::new(dir.path().join("audit.jsonl"), &cfg);
        let now = unix_now();

        let unsent = web.reserve_units(Paid::Tavily, 1, &audit).await.unwrap();
        assert_eq!(web.units.used(Paid::Tavily, now).unwrap(), 1);
        drop(unsent);
        wait_for_used(&web.units, Paid::Tavily, now, 0).await;

        let sent = web.reserve_units(Paid::Tavily, 1, &audit).await.unwrap();
        sent.mark_sent();
        drop(sent);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            web.units.used(Paid::Tavily, now).unwrap(),
            1,
            "a sent request keeps its units"
        );
    }

    /// The awaiting future is dropped while the blocking task is still
    /// reserving: the task hands nothing over and gives the units back. A
    /// second connection holds the write lock, so the reservation is in flight
    /// when the timeout drops it on every machine, and dropping the runtime
    /// waits for the blocking task, so the check reads its final state.
    #[test]
    fn a_reservation_abandoned_in_flight_gives_its_units_back() {
        let dir = private_dir();
        let cfg = AppConfig::from_str("{}", Path::new("config.yaml")).unwrap();
        let web = WebTool::new(dir.path(), &cfg).unwrap();
        let audit = Audit::new(dir.path().join("audit.jsonl"), &cfg);
        let now = unix_now();
        let first = web.units.reserve(Paid::Tavily, 1, now).unwrap();
        web.units.release(&first).unwrap();

        let holder = rusqlite::Connection::open(dir.path().join(LEDGER_FILE)).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let abandoned = runtime.block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_millis(200),
                web.reserve_units(Paid::Serpapi, 1, &audit),
            )
            .await
        });
        assert!(abandoned.is_err(), "the reservation was still in flight");
        holder.execute_batch("COMMIT").unwrap();
        drop(runtime);
        assert_eq!(
            web.units.used(Paid::Serpapi, now).unwrap(),
            0,
            "the abandoned reservation was given back"
        );
    }

    /// The blocking task commits and hands the reservation over, and the
    /// awaiting future is dropped before it is polled again (an outer deadline
    /// firing at that moment). The units still come back: what crosses the
    /// channel is a guard, which releases when it is dropped unsent.
    #[tokio::test]
    async fn a_reservation_handed_to_a_future_dropped_before_its_next_poll_gives_its_units_back() {
        use std::future::Future;
        let dir = private_dir();
        let cfg = AppConfig::from_str("{}", Path::new("config.yaml")).unwrap();
        let web = WebTool::new(dir.path(), &cfg).unwrap();
        let audit = Audit::new(dir.path().join("audit.jsonl"), &cfg);
        let now = unix_now();
        let mut pending = Box::pin(web.reserve_units(Paid::Serpapi, 1, &audit));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(
            pending.as_mut().poll(&mut cx).is_pending(),
            "the reservation runs on the blocking pool"
        );
        // Committed, then handed over a moment later; the future is not polled again.
        wait_for_used(&web.units, Paid::Serpapi, now, 1).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drop(pending);
        wait_for_used(&web.units, Paid::Serpapi, now, 0).await;
    }

    #[test]
    fn ceilings_load_the_shipped_values_and_refuse_out_of_range_numbers() {
        let missing = AppConfig::from_str("{}", Path::new("config.yaml")).unwrap();
        assert_eq!(
            Ceilings::load(&missing).unwrap(),
            Ceilings {
                tavily: 30,
                serpapi: 8,
                firecrawl: 30
            },
            "a home without the keys gets the shipped ceilings"
        );
        let set = AppConfig::from_str(
            "web: {search: {tavily_units_per_day: 5, serpapi_units_per_day: 2}, firecrawl: {units_per_day: 11}}",
            Path::new("config.yaml"),
        )
        .unwrap();
        assert_eq!(
            Ceilings::load(&set).unwrap(),
            Ceilings {
                tavily: 5,
                serpapi: 2,
                firecrawl: 11
            }
        );
        for bad in ["0", "100001", "\"30\""] {
            let cfg = AppConfig::from_str(
                &format!("web: {{search: {{tavily_units_per_day: {bad}}}}}"),
                Path::new("config.yaml"),
            )
            .unwrap();
            assert!(Ceilings::load(&cfg).is_err(), "{bad}");
        }
    }

    /// Every Firecrawl read reserves 10 units, so 1–9 would refuse every read
    /// as spent. Enabled, that ceiling is refused at load; disabled, it is
    /// accepted (operator decision, 2026-11-25).
    #[test]
    fn an_enabled_firecrawl_ceiling_must_cover_one_read() {
        let load = |enabled: &str, units: u64| {
            let yaml = format!("web: {{firecrawl: {{enabled: {enabled}, units_per_day: {units}}}}}");
            Ceilings::load(&AppConfig::from_str(&yaml, Path::new("config.yaml")).unwrap())
        };
        for small in [1, 9] {
            let refused = load("true", small).unwrap_err().message;
            assert!(
                refused.contains("web.firecrawl.units_per_day must be at least 10"),
                "{small}: {refused}"
            );
            assert_eq!(
                load("false", small).unwrap().firecrawl,
                small,
                "disabled keeps its ceiling"
            );
            assert_eq!(
                load("\"true\"", small).unwrap().firecrawl,
                small,
                "only literal true enables"
            );
        }
        assert_eq!(load("true", 10).unwrap().firecrawl, 10, "one read's reservation fits");
    }
}
