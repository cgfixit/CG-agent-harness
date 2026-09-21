//! Persisted recurring agent jobs. At-most-once per occurrence across restart.
//!
//! A schedule stores the same validated `AgentRunRequest` body as
//! `POST /api/agent/jobs`. Fire re-runs `prepare_run` so unreviewed/unbound
//! goals fail closed and write authorization cannot widen. `confirm` is never
//! defaulted.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::common::errors::{HarnessError, Result};
use crate::server::schemas::AgentRunRequest;

pub const ACTIVE: &str = "active";
pub const CANCELLED: &str = "cancelled";
use super::schedule_time::Occurrence;
pub use super::schedule_time::{ScheduleSpec, MAX_INTERVAL_SECS, MIN_INTERVAL_SECS};

#[derive(Debug, Clone)]
pub struct Settings {
    pub poll_interval_ms: u64,
    pub dispatch_grace_secs: u64,
    pub preview_ttl_secs: u64,
    pub max_previews: usize,
    pub max_schedules: usize,
}
impl Settings {
    pub fn from_config(cfg: &crate::common::config::AppConfig) -> Result<Self> {
        if cfg.get("scheduling").is_some_and(|v| !v.is_mapping()) {
            return Err(HarnessError::harness_config("scheduling must be a mapping"));
        }
        let defaults = crate::common::config::AppConfig::from_str(
            crate::common::config::AppConfig::embedded_default(),
            Path::new("defaults"),
        )?;
        let get = |name: &str, min: u64, max: u64| -> Result<u64> {
            let key = format!("scheduling.{name}");
            cfg.get(&key)
                .or_else(|| defaults.get(&key))
                .and_then(|v| v.as_u64())
                .filter(|v| (min..=max).contains(v))
                .ok_or_else(|| HarnessError::harness_config(format!("{key} must be an integer from {min} to {max}")))
        };
        Ok(Self {
            poll_interval_ms: get("poll_interval_ms", 250, 5000)?,
            dispatch_grace_secs: get("dispatch_grace_secs", 5, 30)?,
            preview_ttl_secs: get("preview_ttl_secs", 30, 900)?,
            max_previews: get("max_previews", 1, 128)? as usize,
            max_schedules: get("max_schedules", 1, 128)? as usize,
        })
    }
}
impl Default for Settings {
    fn default() -> Self {
        Self::from_config(
            &crate::common::config::AppConfig::from_str("{}", Path::new("defaults")).expect("empty configuration"),
        )
        .expect("valid shipped scheduling defaults")
    }
}

#[derive(Clone)]
struct Preview {
    owner: String,
    spec: ScheduleSpec,
    request: Value,
    anchor: f64,
    expires: f64,
    occurrences: Vec<Occurrence>,
}
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub schedule_id: String,
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub schedule: Option<ScheduleSpec>,
    #[serde(default)]
    pub next_occurrence_id: String,
    #[serde(default)]
    pub last_occurrence_id: Option<String>,
    #[serde(default)]
    pub last_dispatch: Option<String>,
    #[serde(default)]
    pub interval_secs: u64,
    pub request: Value,
    pub next_fire_at: f64,
    pub last_fired_at: Option<f64>,
    pub created_at: f64,
    pub status: String,
    #[serde(default)]
    pub owner: String,
}

impl Schedule {
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("finite validated schedule")
    }
}

pub struct ScheduleStore {
    inner: Mutex<BTreeMap<String, Schedule>>,
    path: Option<PathBuf>,
    pub settings: Settings,
    previews: Mutex<BTreeMap<String, Preview>>,
}

impl Default for ScheduleStore {
    fn default() -> Self {
        Self {
            inner: Mutex::new(BTreeMap::new()),
            path: None,
            settings: Settings::default(),
            previews: Mutex::new(BTreeMap::new()),
        }
    }
}

impl ScheduleStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(path: &Path) -> Result<Self> {
        Self::open_at(path, crate::common::now_ts(), Settings::default())
    }

    pub fn open_at(path: &Path, now: f64, settings: Settings) -> Result<Self> {
        use std::io::Read;
        let mut rows = BTreeMap::new();
        if path.exists() {
            let mut text = String::new();
            std::fs::File::open(path)?
                .take(MAX_FILE_BYTES as u64 + 1)
                .read_to_string(&mut text)?;
            if text.len() > MAX_FILE_BYTES {
                return Err(HarnessError::harness_config("schedule file exceeds its bound"));
            }
            let parsed: Vec<Schedule> = serde_json::from_str(&text)
                .map_err(|_| HarnessError::harness_config("schedule file is invalid; preserve it for inspection"))?;
            if parsed.len() > settings.max_schedules {
                return Err(HarnessError::harness_config("schedule inventory exceeds its bound"));
            }
            for mut row in parsed {
                if row.schedule_id.len() != 32
                    || !row
                        .schedule_id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(HarnessError::harness_config("invalid recovered schedule identifier"));
                }
                if ![ACTIVE, CANCELLED, "exhausted"].contains(&row.status.as_str()) {
                    return Err(HarnessError::harness_config("invalid recovered schedule status"));
                }
                if !row.next_fire_at.is_finite()
                    || !row.created_at.is_finite()
                    || row.created_at < 0.0
                    || row.next_fire_at <= row.created_at
                    || row.last_fired_at.is_some_and(|v| !v.is_finite() || v < 0.0)
                    || (!row.owner.is_empty() && !crate::server::structured_memory::valid_owner(&row.owner))
                {
                    return Err(HarnessError::harness_config("invalid recovered schedule metadata"));
                }
                let spec = match (row.schema_version, &row.schedule) {
                    (0, None) => ScheduleSpec::Interval {
                        seconds: row.interval_secs,
                    },
                    (1, Some(spec)) => spec.clone(),
                    _ => {
                        return Err(HarnessError::harness_config(
                            "unsupported schedule schema; preserve it for inspection",
                        ))
                    }
                };
                spec.validate()?;
                if row.schema_version == 0 {
                    row.next_occurrence_id = format!(
                        "interval:{:.0}",
                        (row.next_fire_at - row.created_at) / row.interval_secs as f64
                    );
                    row.schedule = Some(spec.clone());
                    row.schema_version = 1;
                }
                if row.next_occurrence_id.is_empty() || row.next_occurrence_id.len() > 128 {
                    return Err(HarnessError::harness_config("invalid recovered occurrence identity"));
                }
                // Recovery never dispatches elapsed work, even within grace.
                if row.status == ACTIVE && row.next_fire_at <= now {
                    row.last_fired_at = Some(row.next_fire_at);
                    row.last_occurrence_id = Some(row.next_occurrence_id.clone());
                    row.last_dispatch = Some("skipped_downtime".into());
                    match spec.next(now, row.created_at) {
                        Ok(next) => {
                            row.next_fire_at = next.at;
                            row.next_occurrence_id = next.identity;
                        }
                        Err(_) => row.status = "exhausted".into(),
                    }
                }
                if rows.contains_key(&row.schedule_id) {
                    return Err(HarnessError::harness_config("duplicate recovered schedule identifier"));
                }
                rows.insert(row.schedule_id.clone(), row);
            }
        }
        let store = Self {
            inner: Mutex::new(rows),
            path: Some(path.to_path_buf()),
            settings,
            previews: Mutex::new(BTreeMap::new()),
        };
        store.persist(&store.inner.lock().unwrap_or_else(|p| p.into_inner()))?;
        Ok(store)
    }

    fn persist(&self, rows: &BTreeMap<String, Schedule>) -> Result<()> {
        if let Some(path) = &self.path {
            let list: Vec<&Schedule> = rows.values().collect();
            let bytes = serde_json::to_vec(&list)?;
            if bytes.len() > MAX_FILE_BYTES {
                return Err(HarnessError::harness_config(
                    "schedule evidence exceeds its durable bound",
                ));
            }
            crate::common::atomic::write_atomic(path, &bytes, Some(0o600))?;
        }
        Ok(())
    }

    /// Internal interval constructor; HTTP activation always requires a preview.
    pub fn create(&self, interval_secs: u64, request: Value, owner: &str, now: f64) -> Result<Schedule> {
        self.create_spec(
            ScheduleSpec::Interval { seconds: interval_secs },
            request,
            owner,
            now,
            now,
        )
    }

    fn create_spec(&self, spec: ScheduleSpec, request: Value, owner: &str, anchor: f64, now: f64) -> Result<Schedule> {
        if !crate::server::structured_memory::valid_owner(owner) {
            return Err(HarnessError::harness_config("invalid schedule owner"));
        }
        let next = spec.next(now, anchor)?;
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let previous = g.clone();
        if g.len() >= self.settings.max_schedules {
            g.retain(|_, s| s.status == ACTIVE);
        }
        if g.len() >= self.settings.max_schedules {
            return Err(HarnessError::new("SCHEDULE_LIMIT", "too many active schedules"));
        }
        let schedule = Schedule {
            schedule_id: crate::common::random_hex(16),
            schema_version: 1,
            interval_secs: match spec {
                ScheduleSpec::Interval { seconds } => seconds,
                _ => 0,
            },
            schedule: Some(spec),
            next_fire_at: next.at,
            next_occurrence_id: next.identity,
            last_occurrence_id: None,
            last_dispatch: None,
            request,
            last_fired_at: None,
            created_at: anchor,
            status: ACTIVE.into(),
            owner: owner.to_string(),
        };
        g.insert(schedule.schedule_id.clone(), schedule.clone());
        if let Err(e) = self.persist(&g) {
            *g = previous;
            return Err(e);
        }
        Ok(schedule)
    }

    pub fn preview(&self, spec: ScheduleSpec, request: Value, owner: &str, now: f64) -> Result<Value> {
        if !crate::server::structured_memory::valid_owner(owner) {
            return Err(HarnessError::harness_config("invalid schedule owner"));
        }
        let mut occurrences = Vec::new();
        let mut after = now;
        for _ in 0..5 {
            let occurrence = spec.next(after, now)?;
            after = occurrence.at;
            occurrences.push(occurrence);
        }
        let expires = (now + self.settings.preview_ttl_secs as f64).min(occurrences[0].at);
        let mut previews = self.previews.lock().unwrap_or_else(|p| p.into_inner());
        previews.retain(|_, p| p.expires > now);
        if previews.len() >= self.settings.max_previews {
            return Err(HarnessError::new(
                "SCHEDULE_PREVIEW_LIMIT",
                "too many unexpired schedule previews",
            ));
        }
        let token = crate::common::random_hex(16);
        let result = json!({"preview_id":token,"schema_version":1,"schedule":spec,"expires_at":expires,"occurrences":occurrences});
        previews.insert(
            token,
            Preview {
                owner: owner.into(),
                spec,
                request,
                anchor: now,
                expires,
                occurrences,
            },
        );
        Ok(result)
    }

    pub fn activate(&self, id: &str, spec: ScheduleSpec, request: Value, owner: &str, now: f64) -> Result<Schedule> {
        let mut previews = self.previews.lock().unwrap_or_else(|p| p.into_inner());
        let p = previews
            .get(id)
            .filter(|p| {
                p.owner == owner && p.expires > now && now >= p.anchor && p.spec == spec && p.request == request
            })
            .ok_or_else(|| {
                HarnessError::new(
                    "SCHEDULE_PREVIEW_REQUIRED",
                    "Preview this exact request again before activation",
                )
            })?;
        if p.occurrences[0].at <= now {
            return Err(HarnessError::new("SCHEDULE_PREVIEW_REQUIRED", "Preview expired"));
        }
        let created = self.create_spec(spec, request, owner, p.anchor, now)?;
        previews.remove(id);
        Ok(created)
    }

    pub fn get(&self, owner: &str, id: &str) -> Option<Schedule> {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .filter(|s| !owner.is_empty() && s.owner == owner)
            .cloned()
    }

    pub fn list(&self, owner: &str) -> Vec<Schedule> {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut v: Vec<Schedule> = g
            .values()
            .filter(|s| !owner.is_empty() && s.owner == owner)
            .cloned()
            .collect();
        v.sort_by(|a, b| {
            b.created_at
                .partial_cmp(&a.created_at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    pub fn cancel(&self, owner: &str, id: &str) -> Result<Option<Schedule>> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let (previous, out) = match g.get_mut(id).filter(|s| !owner.is_empty() && s.owner == owner) {
            Some(s) => {
                let previous = s.status.clone();
                s.status = CANCELLED.into();
                (previous, s.clone())
            }
            None => return Ok(None),
        };
        if let Err(e) = self.persist(&g) {
            if let Some(s) = g.get_mut(id) {
                s.status = previous;
            }
            return Err(e);
        }
        Ok(Some(out))
    }

    /// Active schedules whose next occurrence is due at `now`.
    pub fn due(&self, now: f64) -> Vec<Schedule> {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.values()
            .filter(|s| s.status == ACTIVE && now + f64::EPSILON >= s.next_fire_at)
            .cloned()
            .collect()
    }

    /// Consumption is durable before `dispatch`. Holding this lock through
    /// synchronous job registration linearizes cancellation against dispatch.
    /// A stale poll cannot consume a subsequent occurrence.
    pub fn consume_and_dispatch<T>(&self, due: &Schedule, now: f64, dispatch: impl FnOnce() -> T) -> Option<T> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let row = g.get_mut(&due.schedule_id)?;
        if row.status != ACTIVE
            || row.next_fire_at != due.next_fire_at
            || row.next_occurrence_id != due.next_occurrence_id
            || now < row.next_fire_at
            || !now.is_finite()
        {
            return None;
        }
        let previous = row.clone();
        let timely = now - row.next_fire_at <= self.settings.dispatch_grace_secs as f64;
        row.last_fired_at = Some(row.next_fire_at);
        row.last_occurrence_id = Some(row.next_occurrence_id.clone());
        row.last_dispatch = Some(if timely { "attempted" } else { "skipped_late" }.into());
        match row.schedule.as_ref()?.next(now, row.created_at) {
            Ok(next) => {
                row.next_fire_at = next.at;
                row.next_occurrence_id = next.identity;
            }
            Err(_) => row.status = "exhausted".into(),
        }
        if self.persist(&g).is_err() {
            g.insert(due.schedule_id.clone(), previous);
            return None;
        }
        if timely {
            Some(dispatch())
        } else {
            None
        }
    }

    #[cfg(test)]
    fn mark_attempted(&self, id: &str, now: f64) -> Option<()> {
        let due = self.get("local", id)?;
        self.consume_and_dispatch(&due, now, || ())
    }
}

pub fn parse_request(value: &Value) -> crate::common::errors::Result<AgentRunRequest> {
    serde_json::from_value(value.clone()).map_err(|_| {
        HarnessError::new(
            "INVALID_SCHEDULE_REQUEST",
            "stored schedule request is not a valid agent job",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> Value {
        json!({
            "instruction": "Review the arithmetic implementation",
            "branch": "grok/sched-fixture",
            "commit_message": "Review the arithmetic implementation",
            "reason": "scheduled",
            "confirm": true,
            "goal_stage": {"session_id": "aaaaaaaaaaaa", "stage_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"} // DevSkim: ignore DS173237 because this is a 32-hex stage_id fixture, not a credential.
        })
    }

    #[test]
    fn one_minute_fixture_does_not_double_fire_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schedules.json");
        let store = ScheduleStore::open(&path).unwrap();
        let t0 = 1_000_000.0;
        let created = store.create(60, req(), "local", t0).unwrap();
        assert_eq!(created.next_fire_at, t0 + 60.0);
        assert!(store.due(t0 + 30.0).is_empty(), "mid-window must not fire");
        drop(store);
        let recovered = ScheduleStore::open_at(&path, t0 + 30.0, Settings::default()).unwrap();
        assert!(recovered.due(t0 + 30.0).is_empty(), "restart mid-window must not fire");
        let due = recovered.due(t0 + 60.0);
        assert_eq!(due.len(), 1);
        recovered.mark_attempted(&created.schedule_id, t0 + 60.0);
        assert!(
            recovered.due(t0 + 60.0).is_empty(),
            "same occurrence must not fire twice"
        );
        assert!(recovered.due(t0 + 90.0).is_empty());
        let next = recovered.get("local", &created.schedule_id).unwrap();
        assert_eq!(next.last_fired_at, Some(t0 + 60.0));
        assert_eq!(next.next_fire_at, t0 + 120.0);
        recovered.cancel("local", &created.schedule_id).unwrap();
        assert!(recovered.due(t0 + 120.0).is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn missed_windows_are_skipped_not_caught_up() {
        let store = ScheduleStore::new();
        let t0 = 0.0;
        let created = store.create(60, req(), "local", t0).unwrap();
        // Process was down across three intervals.
        let now = t0 + 60.0 + 180.0;
        let due = store.due(now);
        assert_eq!(due.len(), 1);
        store.mark_attempted(&created.schedule_id, now);
        let next = store.get("local", &created.schedule_id).unwrap();
        assert!(next.next_fire_at > now);
        assert!(store.due(now).is_empty());
    }

    #[test]
    fn interval_below_one_minute_is_rejected() {
        let store = ScheduleStore::new();
        assert!(store.create(59, req(), "local", 0.0).is_err());
    }

    #[test]
    fn recovered_zero_interval_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schedules.json");
        std::fs::write(
            &path,
            r#"[{"schedule_id":"0123456789abcdef0123456789abcdef","interval_secs":0,"request":{},"next_fire_at":1.0,"last_fired_at":null,"created_at":0.0,"status":"active","owner":"local"}]"#, // DevSkim: ignore DS173237 because this is a 32-hex schedule_id fixture, not a credential.
        )
        .unwrap();
        assert!(ScheduleStore::open(&path).is_err());
    }

    #[test]
    fn cancelled_rows_do_not_grow_inventory_past_the_bound() {
        let store = ScheduleStore::new();
        let mut last = String::new();
        for i in 0..store.settings.max_schedules {
            last = store.create(60, req(), "local", i as f64).unwrap().schedule_id;
        }
        assert!(store.create(60, req(), "local", 32.0).is_err());
        store.cancel("local", &last).unwrap();
        assert!(store.create(60, req(), "local", 33.0).is_ok());
        assert!(store.list("local").len() <= store.settings.max_schedules);
        assert_eq!(
            store.list("local").iter().filter(|s| s.status == ACTIVE).count(),
            store.settings.max_schedules
        );
    }

    #[cfg(unix)]
    #[test]
    fn persist_failure_does_not_keep_a_created_or_consumed_row() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schedules.json");
        let store = ScheduleStore::open(&path).unwrap();
        let created = store.create(60, req(), "local", 0.0).unwrap();
        struct Reset<'a>(&'a std::path::Path);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
            }
        }
        let _reset = Reset(dir.path());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        assert!(store.create(60, req(), "local", 1.0).is_err());
        assert_eq!(store.list("local").len(), 1);
        assert!(store.mark_attempted(&created.schedule_id, 60.0).is_none());
        let still = store.get("local", &created.schedule_id).unwrap();
        assert_eq!(still.next_fire_at, 60.0);
        assert!(still.last_fired_at.is_none());
        assert!(store.cancel("local", &created.schedule_id).is_err());
        assert_eq!(store.get("local", &created.schedule_id).unwrap().status, ACTIVE);
    }
    #[test]
    fn preview_is_owner_bound_exact_expiring_and_single_use() {
        let store = ScheduleStore::new();
        let spec = ScheduleSpec::Interval { seconds: 60 };
        let p = store.preview(spec.clone(), req(), "local", 1000.0).unwrap();
        let id = p["preview_id"].as_str().unwrap();
        assert!(store.activate(id, spec.clone(), req(), "user_other", 1001.0).is_err());
        assert!(store.activate(id, spec.clone(), json!({}), "local", 1001.0).is_err());
        assert!(store.activate(id, spec.clone(), req(), "local", 999.0).is_err());
        assert!(store.activate(id, spec.clone(), req(), "local", 1060.0).is_err());
        let made = store.activate(id, spec.clone(), req(), "local", 1001.0).unwrap();
        assert_eq!(made.next_fire_at, 1060.0);
        assert!(store.activate(id, spec, req(), "local", 1001.0).is_err());
    }

    #[test]
    fn stale_poll_clock_shift_cancel_and_restart_cannot_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schedules.json");
        let store = ScheduleStore::open_at(&path, 0.0, Settings::default()).unwrap();
        let row = store.create(60, req(), "local", 1000.0).unwrap();
        assert_eq!(store.consume_and_dispatch(&row, 1060.0, || 1), Some(1));
        assert_eq!(
            store.consume_and_dispatch(&row, 1120.0, || 2),
            None,
            "stale clone cannot consume next"
        );
        assert!(store.due(1040.0).is_empty(), "backward clock cannot replay");
        let due = store.due(1240.0).pop().unwrap();
        assert_eq!(
            store.consume_and_dispatch(&due, 1240.0, || 3),
            None,
            "forward jump skips missed work"
        );
        let advanced = store.get("local", &row.schedule_id).unwrap();
        assert_eq!(advanced.next_fire_at, 1300.0);
        drop(store);
        let store = ScheduleStore::open_at(&path, 1301.0, Settings::default()).unwrap();
        let recovered = store.get("local", &row.schedule_id).unwrap();
        assert_eq!(
            recovered.next_fire_at, 1360.0,
            "restart skips even a recently elapsed occurrence"
        );
        assert_eq!(recovered.last_dispatch.as_deref(), Some("skipped_downtime"));
        store.cancel("local", &row.schedule_id).unwrap();
        assert_eq!(store.consume_and_dispatch(&recovered, 1360.0, || 4), None);
    }

    #[test]
    fn legacy_interval_migrates_without_inventing_an_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schedules.json");
        let raw = json!([{"schedule_id":"0123456789abcdef0123456789abcdef","interval_secs":60,"request":{},"next_fire_at":1060.0,"last_fired_at":null,"created_at":1000.0,"status":"active"}]); // DevSkim: ignore DS173237 because this migration fixture is a public schedule ID, not a credential.
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        let store = ScheduleStore::open_at(&path, 1030.0, Settings::default()).unwrap();
        assert!(store.list("local").is_empty());
        let persisted: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(persisted[0]["schema_version"], 1);
        assert_eq!(persisted[0]["owner"], "");
        assert_eq!(persisted[0]["next_fire_at"], 1060.0);
        assert_eq!(persisted[0]["schedule"]["seconds"], 60);
    }

    #[test]
    fn malformed_tuning_and_preview_pressure_fail_closed() {
        use crate::common::config::AppConfig;
        for yaml in [
            "scheduling: {poll_interval_ms: 0}",
            "scheduling: {max_previews: 129}",
            "scheduling: {dispatch_grace_secs: '5'}",
        ] {
            assert!(Settings::from_config(&AppConfig::from_str(yaml, Path::new("fixture")).unwrap()).is_err());
        }
        let mut store = ScheduleStore::new();
        store.settings.max_previews = 1;
        assert!(store
            .preview(ScheduleSpec::Interval { seconds: 60 }, req(), "local", 1000.0)
            .is_ok());
        assert!(store
            .preview(ScheduleSpec::Interval { seconds: 60 }, req(), "local", 1001.0)
            .is_err());
        assert!(store
            .preview(ScheduleSpec::Interval { seconds: 60 }, req(), "local", 1061.0)
            .is_ok());
    }
}
