//! At most one local-LLM generation at a time (the model is single-stream).
//! Non-blocking claim: a second caller gets 409 instead of queueing. The one
//! exception is a caller that may preempt a best-effort holder: it asks that
//! holder to stop and waits a bounded time for the release (`claim_preempting`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

#[derive(Default)]
struct Inner {
    held: bool,
    owner: String,
}

#[derive(Default)]
struct Shared {
    state: Mutex<Inner>,
    released: Notify,
}

#[derive(Clone, Default)]
pub struct GenerationGate {
    shared: Arc<Shared>,
}

/// Releases the gate on drop, so an early return or a cancelled task never pins it.
pub struct GateGuard {
    shared: Arc<Shared>,
}

impl Drop for GateGuard {
    fn drop(&mut self) {
        {
            let mut g = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
            g.held = false;
            g.owner.clear();
        }
        self.shared.released.notify_waiters();
    }
}

impl GenerationGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn claim(&self, owner: &str) -> Option<GateGuard> {
        self.claim_or_busy_owner(owner).ok()
    }

    /// Claim, or the owner that already holds the gate, under one lock.
    pub fn claim_or_busy_owner(&self, owner: &str) -> Result<GateGuard, String> {
        let mut g = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if g.held {
            return Err(g.owner.clone());
        }
        g.held = true;
        g.owner = owner.to_string();
        Ok(GateGuard {
            shared: self.shared.clone(),
        })
    }

    /// Claim for `owner`. When `yielding` holds the gate, call `ask` (which must
    /// make that holder let go) and wait at most `wait` for the release. `ask`
    /// runs again each time `yielding` is found holding the gate, so it cannot
    /// win the gate back ahead of this caller. Any other holder is refused at
    /// once: ordinary contenders never queue.
    pub async fn claim_preempting(
        &self,
        owner: &str,
        yielding: &str,
        wait: Duration,
        ask: impl Fn(),
    ) -> Result<GateGuard, String> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            // Register before checking, so a release between the check and the
            // wait still wakes this caller.
            let released = self.shared.released.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            match self.claim_or_busy_owner(owner) {
                Ok(guard) => return Ok(guard),
                Err(busy) if busy == yielding => ask(),
                Err(busy) => return Err(busy),
            }
            if tokio::time::timeout_at(deadline, released).await.is_err() {
                return self.claim_or_busy_owner(owner);
            }
        }
    }

    pub fn owner(&self) -> String {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .owner
            .clone()
    }

    pub fn is_held(&self) -> bool {
        self.shared.state.lock().unwrap_or_else(|p| p.into_inner()).held
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Semaphore;

    const SUGGESTION: &str = "memory-suggestion";

    #[tokio::test]
    async fn claim_preempting_asks_only_the_yielding_holder_and_waits_a_bounded_time() {
        let gate = GenerationGate::new();
        assert!(gate
            .claim_preempting("chat", SUGGESTION, Duration::ZERO, || ())
            .await
            .is_ok());

        // Any other holder is refused at once, without being asked or waited for.
        let other = gate.claim("consolidation").unwrap();
        let started = std::time::Instant::now();
        let refused = gate
            .claim_preempting("chat", SUGGESTION, Duration::from_secs(5), || {
                panic!("only the yielding holder is asked")
            })
            .await;
        assert_eq!(refused.err().as_deref(), Some("consolidation"));
        assert!(started.elapsed() < Duration::from_secs(4));
        drop(other);

        // A yielding holder that ignores the ask keeps the gate past the wait.
        let stuck = gate.claim(SUGGESTION).unwrap();
        let asked = std::cell::Cell::new(0);
        let refused = gate
            .claim_preempting("chat", SUGGESTION, Duration::from_millis(20), || {
                asked.set(asked.get() + 1)
            })
            .await;
        assert_eq!(refused.err().as_deref(), Some(SUGGESTION));
        assert_eq!(asked.get(), 1);
        assert_eq!(gate.owner(), SUGGESTION);
        drop(stuck);
        assert!(!gate.is_held());
    }

    #[tokio::test]
    async fn claim_preempting_asks_again_when_the_holder_wins_the_gate_back() {
        let gate = GenerationGate::new();
        let asked = Arc::new(Semaphore::new(0));
        let first = gate.claim(SUGGESTION).unwrap();
        let holder = tokio::spawn({
            let (gate, asked) = (gate.clone(), asked.clone());
            async move {
                // Let go when asked, then take the gate straight back, as a worker
                // can before the woken caller runs; let go for good when asked again.
                asked.acquire().await.unwrap().forget();
                drop(first);
                let again = gate.claim(SUGGESTION).unwrap();
                asked.acquire().await.unwrap().forget();
                drop(again);
            }
        });
        let started = std::time::Instant::now();
        let guard = gate
            .claim_preempting("chat", SUGGESTION, Duration::from_secs(5), || asked.add_permits(1))
            .await
            .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "woken by the release, not the deadline"
        );
        assert_eq!(gate.owner(), "chat");
        holder.await.unwrap();
        // Asked exactly twice: each ask was consumed by one release.
        assert_eq!(asked.available_permits(), 0);
        drop(guard);
        assert!(!gate.is_held());
    }
}
