//! Calendar calculations have no dispatch or account authority.
use std::collections::BTreeSet;
use std::str::FromStr;

use chrono::{DateTime, LocalResult, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::common::errors::{HarnessError, Result};

pub const MIN_INTERVAL_SECS: u64 = 60;
pub const MAX_INTERVAL_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduleSpec {
    Interval {
        seconds: u64,
    },
    Cron {
        expression: String,
        #[serde(default = "utc")]
        timezone: String,
    },
}

fn utc() -> String {
    "UTC".into()
}
fn invalid() -> HarnessError {
    HarnessError::new("INVALID_SCHEDULE", "Use an interval of 60–604800 seconds or a five-field cron expression and IANA timezone; day-of-month or weekday must be *")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Occurrence {
    pub at: f64,
    pub identity: String,
    pub local_time: String,
}

// cron uses 1=Sunday. Public expressions use Unix 0/7=Sunday, including ranges,
// lists, names and steps. Expand only this seven-element field before parsing.
fn weekday_field(field: &str) -> Result<String> {
    fn number(s: &str) -> Result<u32> {
        let names = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
        names
            .iter()
            .position(|n| *n == s)
            .map(|n| n as u32)
            .or_else(|| s.parse::<u32>().ok().filter(|n| *n <= 7))
            .ok_or_else(invalid)
    }
    let mut days = BTreeSet::new();
    for part in field.to_ascii_uppercase().split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (
                r,
                s.parse::<usize>()
                    .ok()
                    .filter(|n| (1..=7).contains(n))
                    .ok_or_else(invalid)?,
            ),
            None => (part, 1),
        };
        let (first, last) = if range == "*" {
            (0, 6)
        } else if let Some((a, b)) = range.split_once('-') {
            (number(a)?, number(b)?)
        } else {
            let n = number(range)?;
            (n, if part.contains('/') { 7 } else { n })
        };
        if first > last {
            return Err(invalid());
        }
        for n in (first..=last).step_by(step) {
            days.insert(n % 7 + 1);
        }
    }
    if days.is_empty() {
        return Err(invalid());
    }
    Ok(days.into_iter().map(|n| n.to_string()).collect::<Vec<_>>().join(","))
}

impl ScheduleSpec {
    fn calendar(&self) -> Result<(cron::Schedule, Tz)> {
        let Self::Cron { expression, timezone } = self else {
            return Err(invalid());
        };
        if expression.len() > 256 || timezone.len() > 64 {
            return Err(invalid());
        }
        let fields: Vec<_> = expression.split_whitespace().collect();
        if fields.len() != 5 || (fields[2] != "*" && fields[4] != "*") {
            return Err(invalid());
        }
        let weekday = weekday_field(fields[4])?;
        let normalized = format!("0 {} {} {} {} {weekday} *", fields[0], fields[1], fields[2], fields[3]);
        Ok((
            cron::Schedule::from_str(&normalized).map_err(|_| invalid())?,
            timezone.parse().map_err(|_| invalid())?,
        ))
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Interval { seconds } if (MIN_INTERVAL_SECS..=MAX_INTERVAL_SECS).contains(seconds) => Ok(()),
            Self::Cron { .. } => self.calendar().map(|_| ()),
            _ => Err(invalid()),
        }
    }

    /// Strictly after `after`. Interval cadence remains anchored to creation.
    /// The calendar iterator runs in naive wall time, then maps through the
    /// zone. Choosing the earlier UTC mapping suppresses the repeated fold.
    pub fn next(&self, after: f64, anchor: f64) -> Result<Occurrence> {
        self.validate()?;
        if !after.is_finite() || !anchor.is_finite() || after < 0.0 || anchor < 0.0 {
            return Err(invalid());
        }
        match self {
            Self::Interval { seconds } => {
                let step = *seconds as f64;
                let ordinal = ((after - anchor) / step).floor().max(0.0) + 1.0;
                let at = anchor + ordinal * step;
                if !at.is_finite() || at <= after {
                    return Err(invalid());
                }
                Ok(Occurrence {
                    at,
                    identity: format!("interval:{ordinal:.0}"),
                    local_time: utc_time(at)?,
                })
            }
            Self::Cron { timezone, .. } => {
                let (calendar, zone) = self.calendar()?;
                let after_utc = DateTime::<Utc>::from_timestamp(after.floor() as i64, 0).ok_or_else(invalid)?;
                let wall = after_utc.with_timezone(&zone).naive_local().and_utc();
                // Bounded even for minutely calendars across historical 24h
                // skipped dates. The crate's calendar horizon ends in 2100.
                for candidate in calendar.after(&wall).take(4096) {
                    let mapped = match zone.from_local_datetime(&candidate.naive_utc()) {
                        LocalResult::None => continue,
                        LocalResult::Single(t) => t,
                        LocalResult::Ambiguous(a, b) => a.min(b),
                    };
                    let at = mapped.timestamp() as f64;
                    if at <= after {
                        continue;
                    }
                    return Ok(Occurrence {
                        at,
                        identity: format!("cron:{timezone}:{}", candidate.format("%Y-%m-%dT%H:%M")),
                        local_time: mapped.to_rfc3339(),
                    });
                }
                Err(HarnessError::new(
                    "SCHEDULE_EXHAUSTED",
                    "No future occurrence within the supported calendar horizon (through 2100)",
                ))
            }
        }
    }
}

pub fn utc_time(at: f64) -> Result<String> {
    DateTime::<Utc>::from_timestamp(at.floor() as i64, 0)
        .map(|t| t.to_rfc3339())
        .ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ts(s: &str) -> f64 {
        DateTime::parse_from_rfc3339(s).unwrap().timestamp() as f64
    }
    fn cron(expression: &str, timezone: &str) -> ScheduleSpec {
        ScheduleSpec::Cron {
            expression: expression.into(),
            timezone: timezone.into(),
        }
    }

    #[test]
    fn dst_gaps_skip_and_repeated_wall_time_runs_only_first_mapping() {
        let spring = cron("30 2 * * *", "America/New_York");
        assert_eq!(
            spring.next(ts("2026-03-08T00:00:00Z"), 0.0).unwrap().at,
            ts("2026-03-09T06:30:00Z")
        );
        let fall = cron("30 1 * * *", "America/New_York");
        let first = fall.next(ts("2026-11-01T00:00:00Z"), 0.0).unwrap();
        assert_eq!(first.at, ts("2026-11-01T05:30:00Z"));
        assert_eq!(fall.next(first.at, 0.0).unwrap().at, ts("2026-11-02T06:30:00Z"));
        assert_eq!(
            fall.next(ts("2026-11-01T06:00:00Z"), 0.0).unwrap().at,
            ts("2026-11-02T06:30:00Z")
        );
    }

    #[test]
    fn leap_days_and_unix_weekdays_have_known_utc_results() {
        assert_eq!(
            cron("0 12 29 2 *", "UTC")
                .next(ts("2026-01-01T00:00:00Z"), 0.0)
                .unwrap()
                .at,
            ts("2028-02-29T12:00:00Z")
        );
        for day in ["0", "7", "SUN"] {
            assert_eq!(
                cron(&format!("0 12 * * {day}"), "UTC")
                    .next(ts("2026-09-21T00:00:00Z"), 0.0)
                    .unwrap()
                    .at,
                ts("2026-09-27T12:00:00Z")
            );
        }
        assert_eq!(weekday_field("MON-FRI").unwrap(), "2,3,4,5,6");
        assert_eq!(weekday_field("5-7").unwrap(), "1,6,7");
        assert_eq!(weekday_field("*/2").unwrap(), "1,3,5,7");
    }

    #[test]
    fn invalid_and_ambiguous_syntax_is_refused_without_unbounded_search() {
        for expr in [
            "* * * *",
            "* * * * * *",
            "0 0 1 * MON",
            "60 * * * *",
            "0 0 * * 8",
            "0 0 * * */0",
        ] {
            assert!(cron(expr, "UTC").validate().is_err(), "{expr}");
        }
        assert!(cron("* * * * *", "Not/AZone").validate().is_err());
        assert!(cron("0 0 30 2 *", "UTC").next(ts("2026-01-01T00:00:00Z"), 0.0).is_err());
        assert!(ScheduleSpec::Interval { seconds: 59 }.validate().is_err());
    }
}
