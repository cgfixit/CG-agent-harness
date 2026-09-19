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
pub const MIN_INTERVAL_SECS: u64 = 60;
pub const MAX_INTERVAL_SECS: u64 = 7 * 24 * 60 * 60;
const MAX_SCHEDULES: usize = 32;
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub schedule_id: String,
    pub interval_secs: u64,
    pub request: Value,
    pub next_fire_at: f64,
    pub last_fired_at: Option<f64>,
    pub created_at: f64,
    pub status: String,
    pub owner: String,
}

impl Schedule {
    pub fn to_json(&self) -> Value {
        json!({
            "schedule_id": self.schedule_id,
            "interval_secs": self.interval_secs,
            "request": self.request,
            "next_fire_at": self.next_fire_at,
            "last_fired_at": self.last_fired_at,
            "created_at": self.created_at,
            "status": self.status,
            "owner": self.owner,
        })
    }
}

pub struct ScheduleStore {
    inner: Mutex<BTreeMap<String, Schedule>>,
    path: Option<PathBuf>,
}

impl Default for ScheduleStore {
    fn default() -> Self {
        Self {
            inner: Mutex::new(BTreeMap::new()),
            path: None,
        }
    }
}

impl ScheduleStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(path: &Path) -> Result<Self> {
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
            if parsed.len() > MAX_SCHEDULES {
                return Err(HarnessError::harness_config("schedule inventory exceeds its bound"));
            }
            for row in parsed {
                if row.schedule_id.len() != 32
                    || !row
                        .schedule_id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(HarnessError::harness_config("invalid recovered schedule identifier"));
                }
                if row.status != ACTIVE && row.status != CANCELLED {
                    return Err(HarnessError::harness_config("invalid recovered schedule status"));
                }
                rows.insert(row.schedule_id.clone(), row);
            }
        }
        let store = Self {
            inner: Mutex::new(rows),
            path: Some(path.to_path_buf()),
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

    pub fn create(&self, interval_secs: u64, request: Value, owner: &str, now: f64) -> Result<Schedule> {
        if !(MIN_INTERVAL_SECS..=MAX_INTERVAL_SECS).contains(&interval_secs) {
            return Err(HarnessError::new(
                "INVALID_SCHEDULE_INTERVAL",
                format!("interval_secs must be from {MIN_INTERVAL_SECS} to {MAX_INTERVAL_SECS}"),
            ));
        }
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let active = g.values().filter(|s| s.status == ACTIVE).count();
        if active >= MAX_SCHEDULES {
            return Err(HarnessError::new("SCHEDULE_LIMIT", "too many active schedules"));
        }
        let schedule = Schedule {
            schedule_id: crate::common::random_hex(16),
            interval_secs,
            request,
            next_fire_at: now + interval_secs as f64,
            last_fired_at: None,
            created_at: now,
            status: ACTIVE.into(),
            owner: owner.to_string(),
        };
        g.insert(schedule.schedule_id.clone(), schedule.clone());
        self.persist(&g)?;
        Ok(schedule)
    }

    pub fn get(&self, id: &str) -> Option<Schedule> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).get(id).cloned()
    }

    pub fn list(&self) -> Vec<Schedule> {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut v: Vec<Schedule> = g.values().cloned().collect();
        v.sort_by(|a, b| {
            b.created_at
                .partial_cmp(&a.created_at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    pub fn cancel(&self, id: &str) -> Option<Schedule> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let s = g.get_mut(id)?;
        s.status = CANCELLED.into();
        let out = s.clone();
        let _ = self.persist(&g);
        Some(out)
    }

    /// Active schedules whose next occurrence is due at `now`.
    pub fn due(&self, now: f64) -> Vec<Schedule> {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.values()
            .filter(|s| s.status == ACTIVE && now + f64::EPSILON >= s.next_fire_at)
            .cloned()
            .collect()
    }

    /// Consume the due occurrence (at-most-once) and skip any windows already
    /// in the past so a restart never double-fires or catch-up-storms.
    pub fn mark_attempted(&self, id: &str, now: f64) -> Option<Schedule> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let s = g.get_mut(id)?;
        if s.status != ACTIVE {
            return None;
        }
        let occurrence = s.next_fire_at;
        s.last_fired_at = Some(occurrence);
        let step = s.interval_secs as f64;
        let mut next = occurrence + step;
        while next <= now {
            next += step;
        }
        s.next_fire_at = next;
        let out = s.clone();
        let _ = self.persist(&g);
        Some(out)
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
            "goal_stage": {"session_id": "aaaaaaaaaaaa", "stage_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
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
        let recovered = ScheduleStore::open(&path).unwrap();
        assert!(recovered.due(t0 + 30.0).is_empty(), "restart mid-window must not fire");
        let due = recovered.due(t0 + 60.0);
        assert_eq!(due.len(), 1);
        recovered.mark_attempted(&created.schedule_id, t0 + 60.0);
        assert!(
            recovered.due(t0 + 60.0).is_empty(),
            "same occurrence must not fire twice"
        );
        assert!(recovered.due(t0 + 90.0).is_empty());
        let next = recovered.get(&created.schedule_id).unwrap();
        assert_eq!(next.last_fired_at, Some(t0 + 60.0));
        assert_eq!(next.next_fire_at, t0 + 120.0);
        recovered.cancel(&created.schedule_id);
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
        let next = store.get(&created.schedule_id).unwrap();
        assert!(next.next_fire_at > now);
        assert!(store.due(now).is_empty());
    }

    #[test]
    fn interval_below_one_minute_is_rejected() {
        let store = ScheduleStore::new();
        assert!(store.create(59, req(), "local", 0.0).is_err());
    }
}
