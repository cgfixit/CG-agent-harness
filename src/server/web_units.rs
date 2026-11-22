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
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// UTC days of rows kept; older rows are pruned on each reservation.
const KEEP_DAYS: i64 = 40;
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
        Ok(Self {
            // Literal keys: the invariant guard checks every numeric read by name.
            tavily: cfg.u64_in("web.search.tavily_units_per_day", 30, CEILING_RANGE)?,
            serpapi: cfg.u64_in("web.search.serpapi_units_per_day", 8, CEILING_RANGE)?,
            firecrawl: cfg.u64_in("web.firecrawl.units_per_day", 30, CEILING_RANGE)?,
        })
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
        let conn = private_sqlite::connect(&path, MAX_FILE_BYTES, "provider-unit ledger", unavailable)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS units (
                id INTEGER PRIMARY KEY,
                provider TEXT NOT NULL,
                day INTEGER NOT NULL,
                units INTEGER NOT NULL CHECK (units > 0),
                at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS units_by_day ON units (provider, day);",
        )
        .map_err(sql)?;
        if !private_sqlite::present(&marker).map_err(|_| unavailable("the provider-unit ledger could not be read"))? {
            crate::common::atomic::write_atomic(&marker, b"provider-unit ledger established\n", Some(0o600))
                .map_err(|_| unavailable("the provider-unit ledger could not be written"))?;
        }
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

impl WebTool {
    /// Reserve units for one paid request, on the blocking pool. A refusal is
    /// audited with the provider, code and counts, never a query, URL or key.
    pub(super) async fn reserve_units(&self, provider: Paid, units: u64, audit: &Audit) -> Result<Reservation> {
        let ledger = Arc::clone(&self.units);
        let outcome = tokio::task::spawn_blocking(move || ledger.reserve(provider, units, unix_now()))
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

    /// Release a reservation when its request was never sent; keep it when
    /// the send started (`sent`). A failed release keeps the units reserved,
    /// which only ever errs toward spending less.
    pub(super) async fn release_unsent(&self, reservation: Reservation, sent: bool) {
        if sent {
            return;
        }
        let ledger = Arc::clone(&self.units);
        let released = tokio::task::spawn_blocking(move || ledger.release(&reservation)).await;
        if !matches!(released, Ok(Ok(()))) {
            tracing::warn!("an unsent paid web request's units could not be released; they stay reserved for today");
        }
    }
}

#[cfg(test)]
mod tests {
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
}
