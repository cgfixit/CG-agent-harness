//! #772 D4: how many web tool calls one run may make (`web.run_limits`).
//!
//! An attempt is counted before it runs, whatever it returns: a failed,
//! refused or repeated call is not free, so a run whose calls keep failing
//! cannot outlast the limit. A server-run `/loop` counts across its turns on
//! its live loop entry (`LoopManager::charge_web_attempt`), so a turn that
//! errors keeps what it spent; any other turn counts on its own. Both limits
//! ship 0 (off) as a trial profile, not a shipped default.
use serde::Serialize;

use super::web_policy::error;
use crate::common::errors::Result;

/// The two kinds of attempt a run's limits count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attempt {
    Search,
    Read,
}

impl Attempt {
    /// `web_search` is a search; every other web tool call (`web_fetch`) a read.
    pub fn of(tool: &str) -> Self {
        if tool == "web_search" {
            Self::Search
        } else {
            Self::Read
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Search => "searches",
            Self::Read => "reads",
        }
    }
}

/// `web.run_limits`: 0 is off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RunLimits {
    pub searches: u64,
    pub reads: u64,
}

impl RunLimits {
    pub fn of(self, attempt: Attempt) -> u64 {
        match attempt {
            Attempt::Search => self.searches,
            Attempt::Read => self.reads,
        }
    }
}

/// What one run has spent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RunSpend {
    pub searches: u64,
    pub reads: u64,
}

impl RunSpend {
    /// Count one `attempt`, or refuse it with `WEB_RUN_BUDGET` when its limit
    /// is spent. A refused attempt is not counted. An off limit counts nothing.
    pub fn charge(&mut self, attempt: Attempt, limits: RunLimits) -> Result<()> {
        let limit = limits.of(attempt);
        if limit == 0 {
            return Ok(());
        }
        let spent = match attempt {
            Attempt::Search => &mut self.searches,
            Attempt::Read => &mut self.reads,
        };
        if *spent >= limit {
            let tool = match attempt {
                Attempt::Search => "web_search",
                Attempt::Read => "web_fetch",
            };
            return Err(error(
                "WEB_RUN_BUDGET",
                &format!(
                    "this run has made its {limit} {tool} calls (web.run_limits.{}); do not call {tool} again this run. \
                     Answer from what was already read, or say the evidence is insufficient",
                    attempt.label()
                ),
            ));
        }
        *spent += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_spends_each_limit_separately_and_a_refused_attempt_is_not_counted() {
        let limits = RunLimits { searches: 2, reads: 1 };
        let mut spend = RunSpend::default();
        assert!(spend.charge(Attempt::Search, limits).is_ok());
        assert!(spend.charge(Attempt::Read, limits).is_ok());
        assert!(spend.charge(Attempt::Search, limits).is_ok());
        for attempt in [Attempt::Search, Attempt::Read] {
            let refused = spend.charge(attempt, limits).expect_err("spent");
            assert_eq!(refused.code, "WEB_RUN_BUDGET");
            assert!(refused.message.contains(attempt.label()), "{}", refused.message);
        }
        assert_eq!(spend, RunSpend { searches: 2, reads: 1 }, "refusals are not counted");
    }

    #[test]
    fn an_off_limit_counts_nothing() {
        let mut spend = RunSpend::default();
        for _ in 0..1000 {
            spend.charge(Attempt::Search, RunLimits::default()).unwrap();
            spend
                .charge(Attempt::Read, RunLimits { searches: 0, reads: 0 })
                .unwrap();
        }
        assert_eq!(spend, RunSpend::default());
    }

    #[test]
    fn only_web_search_is_a_search() {
        assert_eq!(Attempt::of("web_search"), Attempt::Search);
        assert_eq!(Attempt::of("web_fetch"), Attempt::Read);
    }
}
