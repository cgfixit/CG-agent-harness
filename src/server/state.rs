//! Shared application state for the console.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use crate::common::audit::Audit;
use crate::common::auth_store::AuthManager;
use crate::common::config::SharedConfig;
use crate::common::home::{HarnessSettings, Home};
use crate::common::ratelimit::RateLimiter;
use crate::llm::backend::ResolvedLocalBackend;
use crate::llm::openai_chat::ChatClient;

use super::generation_gate::GenerationGate;
use super::memory_notes::MemoryNotes;
use super::sessions::SessionStore;
use super::web_search::WebTool;

pub const HARNESS_LOOP_TOOL: &str = "harness_loop";
pub const AGENT_RUN_TOOL: &str = "agent_run";
/// A 27b turn can sit in the model server for minutes; the in-flight lock must outlast it.
pub const LOOP_INFLIGHT_TTL_SEC: f64 = 900.0;

pub struct AppState {
    pub home: Home,
    pub cfg: SharedConfig,
    pub settings: Mutex<HarnessSettings>,
    pub store: SessionStore,
    pub backend: ResolvedLocalBackend,
    pub chat: ChatClient,
    pub audit: Audit,
    pub rate_limiter: RateLimiter,
    pub loop_rate_limiter: RateLimiter,
    pub loop_max_tokens: u64,
    pub loop_inflight: Mutex<HashMap<String, f64>>,
    pub generation_gate: GenerationGate,
    pub agent_run_gate: GenerationGate,
    pub csrf_token: String,
    pub console_html: String,
    pub api_key_optional: bool,
    /// Snapshot of `CGAGENTHARNESS_API_KEY` at build time (tests inject it).
    pub api_key: Option<String>,
    pub key_file_sources: BTreeSet<String>,
    pub auth: Option<AuthManager>,
    pub web: WebTool,
    pub notes: MemoryNotes,
    /// Test hook: when set, replaces the closed tool allowlists (empty = deny all).
    pub tool_allowlist_override: Option<BTreeSet<String>>,
    /// The only server -> agentic edge (a child process).
    pub shim: crate::shim::ShimContext,
    /// Detached real-repo runs (`/api/agent/jobs`).
    pub jobs: crate::server::agent_jobs::JobStore,
    /// `logging.request_log`: one structured tracing line per request.
    pub request_log: bool,
}

impl AppState {
    pub fn current_model(&self) -> String {
        let selected = self
            .settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .selected_model
            .clone();
        if selected.trim().is_empty() {
            self.backend.model.clone()
        } else {
            selected
        }
    }

    pub fn loop_tool_allowlist(&self) -> BTreeSet<String> {
        self.tool_allowlist_override
            .clone()
            .unwrap_or_else(|| [HARNESS_LOOP_TOOL.to_string()].into_iter().collect())
    }

    pub fn agent_run_tool_allowlist(&self) -> BTreeSet<String> {
        self.tool_allowlist_override
            .clone()
            .unwrap_or_else(|| [AGENT_RUN_TOOL.to_string()].into_iter().collect())
    }

    /// 409 `LOOP_IN_FLIGHT` when a turn for this session is still running.
    pub fn claim_loop_inflight(&self, session_id: &str) -> bool {
        let now = crate::common::now_ts();
        let mut map = self.loop_inflight.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(started) = map.get(session_id) {
            if now - started < LOOP_INFLIGHT_TTL_SEC {
                return false;
            }
        }
        map.insert(session_id.to_string(), now);
        true
    }

    pub fn release_loop_inflight(&self, session_id: &str) {
        self.loop_inflight
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(session_id);
    }
}
