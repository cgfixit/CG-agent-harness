//! Operator confirmation for a model-composed chat `web_fetch` (#901 step 4).
//!
//! A web chat turn that wants a URL no search listed, no exact allow rule
//! names and the operator did not write pauses: it registers here, streams a
//! `confirm_fetch` event to the console, and waits for that account's answer
//! on `POST /api/chat/fetch-confirm`. Nothing here reads or authorizes a URL;
//! the caller reloads policy after the wait.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::oneshot;

/// How long a composed read waits for the operator before it is refused.
pub const CONFIRM_WAIT_SECS: u64 = 120;
/// Pending confirmations across all accounts; past this a new one is refused.
const MAX_PENDING: usize = 16;

#[derive(Default)]
pub struct FetchConfirms {
    pending: Mutex<HashMap<String, (String, oneshot::Sender<bool>)>>,
}

/// One open question. Dropping it (answered, timed out or cancelled with the
/// turn) removes it, so a late answer finds nothing.
pub struct Pending<'a> {
    confirms: &'a FetchConfirms,
    pub id: String,
    pub answer: oneshot::Receiver<bool>,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.confirms
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.id);
    }
}

impl FetchConfirms {
    /// Open a question for `owner`, or `None` when too many are open.
    pub fn open(&self, owner: &str) -> Option<Pending<'_>> {
        let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        if pending.len() >= MAX_PENDING {
            return None;
        }
        let id = crate::common::random_urlsafe(18);
        let (sender, answer) = oneshot::channel();
        pending.insert(id.clone(), (owner.to_string(), sender));
        Some(Pending {
            confirms: self,
            id,
            answer,
        })
    }

    /// Deliver `approve` to `owner`'s open question `id`. False when there is
    /// none: unknown, already answered, expired, or another account's.
    pub fn resolve(&self, owner: &str, id: &str, approve: bool) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        if pending.get(id).is_none_or(|(asker, _)| asker != owner) {
            return false;
        }
        pending
            .remove(id)
            .is_some_and(|(_, sender)| sender.send(approve).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_the_asking_account_answers_once_and_a_dropped_question_is_gone() {
        let confirms = FetchConfirms::default();
        let mut pending = confirms.open("alice").unwrap();
        let id = pending.id.clone();
        assert!(!confirms.resolve("bob", &id, true), "another account cannot answer");
        assert!(!confirms.resolve("alice", "guess", true));
        assert!(confirms.resolve("alice", &id, true));
        assert!(!confirms.resolve("alice", &id, false), "answered once");
        assert!((&mut pending.answer).await.unwrap());

        let dropped = confirms.open("alice").unwrap();
        let id = dropped.id.clone();
        drop(dropped);
        assert!(
            !confirms.resolve("alice", &id, true),
            "a cancelled turn's question is gone"
        );

        let held: Vec<_> = (0..MAX_PENDING).map(|_| confirms.open("alice").unwrap()).collect();
        assert!(confirms.open("alice").is_none(), "bounded");
        drop(held);
        assert!(confirms.open("alice").is_some());
    }
}
