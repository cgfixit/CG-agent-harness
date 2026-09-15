//! Phase 6 leftover: default-off idle auto-consolidator.
//!
//! Reuses [`super::structured_memory_consolidate::run_manual`]. Does not fork a
//! summarizer. Claims the single local generation gate only while a bounded job
//! runs; idle wait never holds the gate. Output is pending proposals only.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use crate::server::state::AppState;
use crate::server::structured_memory::{
    auto_consolidation_available, current_gates, ConsolidationRun, StructuredMemoryStore,
};
use crate::server::structured_memory_consolidate::run_manual;

/// Process-local handle. Feature-off never spawns. Disable stops new claims.
#[derive(Debug)]
pub struct AutoConsolidationControl {
    stop_new_claims: AtomicBool,
    spawned: AtomicBool,
}

impl Default for AutoConsolidationControl {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoConsolidationControl {
    pub fn new() -> Self {
        Self {
            stop_new_claims: AtomicBool::new(true),
            spawned: AtomicBool::new(false),
        }
    }

    pub fn is_spawned(&self) -> bool {
        self.spawned.load(Ordering::SeqCst)
    }

    pub fn claims_allowed(&self) -> bool {
        !self.stop_new_claims.load(Ordering::SeqCst)
    }
}

/// Start a worker only when auto-consolidation is available. Disable stops
/// **new** claims; an in-flight `run_manual` is left to finish or fail like
/// the manual path.
pub fn sync_worker(state: &Arc<AppState>) {
    let available = worker_available(state);
    state
        .auto_consolidation
        .stop_new_claims
        .store(!available, Ordering::SeqCst);
    if available {
        start_if_needed(state);
    }
}

fn worker_available(state: &AppState) -> bool {
    auto_consolidation_available(&state.cfg, state.structured_memory.is_some(), &current_gates(state))
}

fn start_if_needed(state: &Arc<AppState>) {
    if state
        .auto_consolidation
        .spawned
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        worker_loop(state).await;
    });
}

async fn worker_loop(state: Arc<AppState>) {
    loop {
        if !state.auto_consolidation.claims_allowed() || !worker_available(&state) {
            break;
        }
        let idle = Duration::from_millis(idle_ms(&state));
        if state.generation_gate.is_held() {
            tokio::time::sleep(idle).await;
            continue;
        }
        let Some(store) = state.structured_memory.as_ref() else {
            break;
        };
        let batch = match store.next_auto_consolidation_batch() {
            Ok(Some(batch)) => batch,
            Ok(None) => {
                tokio::time::sleep(idle).await;
                continue;
            }
            Err(_) => {
                tokio::time::sleep(idle).await;
                continue;
            }
        };
        if !state.auto_consolidation.claims_allowed() || !worker_available(&state) {
            break;
        }
        if state.generation_gate.is_held() {
            tokio::time::sleep(idle).await;
            continue;
        }
        let Some(_gate) = state.generation_gate.claim("consolidation") else {
            tokio::time::sleep(idle).await;
            continue;
        };
        if !state.auto_consolidation.claims_allowed() || !worker_available(&state) {
            drop(_gate);
            break;
        }
        let outcome = run_manual(&state, store, &batch.0, &batch.1).await;
        match outcome {
            Ok(run) => audit_run(&state, &run),
            Err(err) => {
                state.audit.log(json!({
                    "event": "structured_memory_consolidation",
                    "owner_id": batch.0,
                    "trigger": "auto",
                    "error_class": err.code,
                }));
            }
        }
        drop(_gate);
        if !state.auto_consolidation.claims_allowed() || !worker_available(&state) {
            break;
        }
        tokio::time::sleep(idle).await;
    }
    state.auto_consolidation.spawned.store(false, Ordering::SeqCst);
}

fn idle_ms(state: &AppState) -> u64 {
    state
        .structured_memory
        .as_ref()
        .map(|store| store.limits().auto_consolidation_idle_ms)
        .unwrap_or(StructuredMemoryStore::default_auto_idle_ms())
}

fn audit_run(state: &AppState, run: &ConsolidationRun) {
    state.audit.log(json!({
        "event": "structured_memory_consolidation",
        "owner_id": run.owner_id,
        "trigger": "auto",
        "id": run.id,
        "state": run.state,
        "episode_count": run.episode_ids.len(),
        "proposal_count": run.proposal_count,
        "candidate_count": run.candidate_count,
        "rejected_count": run.rejected_count,
        "error_class": run.error_class,
        "summarizer_version": run.summarizer_version,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_starts_with_claims_blocked_and_no_spawn() {
        let control = AutoConsolidationControl::new();
        assert!(!control.is_spawned());
        assert!(!control.claims_allowed());
    }
}
