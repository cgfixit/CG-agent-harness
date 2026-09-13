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

    fn sweep(&self, st: &mut State, now: f64) {
        if now - st.last_sweep < self.window_seconds {
            return;
        }
        st.last_sweep = now;
        st.hits
            .retain(|_, hits| hits.iter().any(|t| now - t < self.window_seconds));
    }

    /// Record a hit for `client` and report whether it is within the limit.
    pub fn allow(&self, client: &str) -> bool {
        let now = (self.clock)();
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.sweep(&mut st, now);
        // Every `/api/*` request passes here: a known client is one `&str`
        // lookup with no allocation; only a first-seen client pays for an
        // owned key.
        match st.hits.get_mut(client) {
            Some(hits) => self.record(hits, now),
            None => self.record(st.hits.entry(client.to_string()).or_default(), now),
        }
    }

    fn record(&self, hits: &mut Vec<f64>, now: f64) -> bool {
        hits.retain(|t| now - *t < self.window_seconds);
        if hits.len() >= self.max_requests {
            return false;
        }
        hits.push(now);
        true
    }

    /// Seconds until the oldest in-window hit expires; 0 when under the limit.
    pub fn retry_after_sec(&self, client: &str) -> f64 {
        let now = (self.clock)();
        let st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(hits) = st.hits.get(client) else {
            return 0.0;
        };
        // One pass: count the in-window hits and keep the oldest, without
        // materializing a filtered copy just to take its minimum.
        let mut in_window = 0usize;
        let mut oldest = f64::INFINITY;
        for &t in hits {
            if now - t < self.window_seconds {
                in_window += 1;
                oldest = oldest.min(t);
            }
        }
        if in_window < self.max_requests {
            return 0.0;
        }
        (self.window_seconds - (now - oldest)).max(0.0)
    }

    pub fn tracked_clients(&self) -> usize {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).hits.len()
    }
}
