//! At most one local-LLM generation at a time (the model is single-stream).
//! Non-blocking claim: a second caller gets 409 instead of queueing.

use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Inner {
    held: bool,
    owner: String,
}

#[derive(Clone, Default)]
pub struct GenerationGate {
    inner: Arc<Mutex<Inner>>,
}

/// Releases the gate on drop, so an early return or a cancelled task never pins it.
pub struct GateGuard {
    inner: Arc<Mutex<Inner>>,
}

impl Drop for GateGuard {
    fn drop(&mut self) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.held = false;
        g.owner.clear();
    }
}

impl GenerationGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn claim(&self, owner: &str) -> Option<GateGuard> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if g.held {
            return None;
        }
        g.held = true;
        g.owner = owner.to_string();
        Some(GateGuard {
            inner: self.inner.clone(),
        })
    }

    pub fn owner(&self) -> String {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).owner.clone()
    }

    pub fn is_held(&self) -> bool {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).held
    }
}
