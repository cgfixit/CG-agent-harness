//! Per-client sliding-window rate limiter (in-memory), port of `utils/ratelimit.py`.

use std::collections::HashMap;
use std::sync::Mutex;

type Clock = Box<dyn Fn() -> f64 + Send + Sync>;

pub struct RateLimiter {
    max_requests: usize,
    window_seconds: f64,
    clock: Clock,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    hits: HashMap<String, Vec<f64>>,
    last_sweep: f64,
}

impl RateLimiter {
    pub fn new(max_requests: usize, window_seconds: f64) -> Self {
        Self::with_clock(max_requests, window_seconds, Box::new(super::now_ts))
    }

    pub fn with_clock(max_requests: usize, window_seconds: f64, clock: Clock) -> Self {
        assert!(window_seconds > 0.0, "RateLimiter window_seconds must be positive");
        Self {
            max_requests,
            window_seconds,
            clock,
            state: Mutex::new(State::default()),
        }
    }

    pub fn max_requests(&self) -> usize {
        self.max_requests
    }

    pub fn window_seconds(&self) -> f64 {
        self.window_seconds
    }

    fn sweep(&self, st: &mut State, now: f64, window_seconds: f64) {
        if now - st.last_sweep < window_seconds {
            return;
        }
        st.last_sweep = now;
        st.hits.retain(|_, hits| hits.iter().any(|t| now - t < window_seconds));
    }

    /// Record a hit for `client` and report whether it is within the limit.
    pub fn allow(&self, client: &str) -> bool {
        self.allow_with_limits(client, self.max_requests, self.window_seconds)
    }

    /// Apply a validated request snapshot without resetting retained hits.
    /// Increasing a window cannot recover hits already expired under an old one.
    pub fn allow_with_limits(&self, client: &str, max_requests: usize, window_seconds: f64) -> bool {
        let now = (self.clock)();
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.sweep(&mut st, now, window_seconds);
        // Every `/api/*` request passes here: a known client is one `&str`
        // lookup with no allocation; only a first-seen client pays for an
        // owned key.
        match st.hits.get_mut(client) {
            Some(hits) => Self::record(hits, now, max_requests, window_seconds),
            None => Self::record(
                st.hits.entry(client.to_string()).or_default(),
                now,
                max_requests,
                window_seconds,
            ),
        }
    }

    fn record(hits: &mut Vec<f64>, now: f64, max_requests: usize, window_seconds: f64) -> bool {
        hits.retain(|t| now - *t < window_seconds);
        if hits.len() >= max_requests {
            return false;
        }
        hits.push(now);
        true
    }

    /// Seconds until enough retained hits expire; 0 when under the limit.
    pub fn retry_after_sec(&self, client: &str) -> f64 {
        self.retry_after_with_limits(client, self.max_requests, self.window_seconds)
    }

    pub fn retry_after_with_limits(&self, client: &str, max_requests: usize, window_seconds: f64) -> f64 {
        let now = (self.clock)();
        let st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(hits) = st.hits.get(client) else {
            return 0.0;
        };
        // The usual full window needs only its oldest hit, without allocation.
        let mut in_window = 0usize;
        let mut oldest = f64::INFINITY;
        for &t in hits {
            if now - t < window_seconds {
                in_window += 1;
                oldest = oldest.min(t);
            }
        }
        if in_window < max_requests {
            return 0.0;
        }
        if max_requests > 0 && in_window > max_requests {
            // A tightened limit may need multiple hits to expire. Select that
            // threshold without sorting or assuming the wall clock never moved.
            let mut retained: Vec<_> = hits.iter().copied().filter(|t| now - t < window_seconds).collect();
            oldest = *retained
                .select_nth_unstable_by(in_window - max_requests, f64::total_cmp)
                .1;
        }
        (window_seconds - (now - oldest)).max(0.0)
    }

    pub fn tracked_clients(&self) -> usize {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).hits.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn tightened_limit_waits_for_enough_retained_hits_to_expire() {
        let now = Arc::new(Mutex::new(0.0));
        let clock = now.clone();
        let limiter = RateLimiter::with_clock(3, 60.0, Box::new(move || *clock.lock().unwrap()));
        for timestamp in [0.0, 10.0, 20.0] {
            *now.lock().unwrap() = timestamp;
            assert!(limiter.allow("client"));
        }
        *now.lock().unwrap() = 30.0;
        assert!(!limiter.allow_with_limits("client", 1, 60.0));
        assert_eq!(limiter.retry_after_with_limits("client", 1, 60.0), 50.0);
        assert_eq!(limiter.retry_after_with_limits("client", 2, 60.0), 40.0);
        *now.lock().unwrap() = 79.0;
        assert!(!limiter.allow_with_limits("client", 1, 60.0));
        *now.lock().unwrap() = 80.0;
        assert!(limiter.allow_with_limits("client", 1, 60.0));
    }
}
