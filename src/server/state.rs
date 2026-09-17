//! Shared application state for the console.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::common::audit::Audit;
use crate::common::auth_store::AuthManager;
use crate::common::config::{AppConfig, SharedConfig};
use crate::common::errors::{HarnessError, Result};
use crate::common::home::{HarnessSettings, Home};
use crate::common::ratelimit::RateLimiter;
use crate::llm::backend::ResolvedLocalBackend;
use crate::llm::cloud_chat::CloudChat;
use crate::llm::openai_chat::ChatClient;

use super::generation_gate::GenerationGate;
use super::mcp::McpRuntime;
use super::memory_notes::MemoryNotes;
use super::sessions::SessionStore;
use super::structured_memory::StructuredMemoryStore;
use super::web_search::WebTool;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub const HARNESS_LOOP_TOOL: &str = "harness_loop";
pub const AGENT_RUN_TOOL: &str = "agent_run";
/// A 27b turn can sit in the model server for minutes; the in-flight lock must outlast it.
pub const LOOP_INFLIGHT_TTL_SEC: f64 = 900.0;

pub fn auth_operation_concurrency(cfg: &AppConfig) -> Result<usize> {
    match cfg.get("auth.max_concurrent_operations") {
        None => Ok(2),
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=4).contains(value))
            .map(|value| value as usize)
            .ok_or_else(|| HarnessError::config("auth.max_concurrent_operations must be an integer from 1 to 4")),
    }
}

pub struct AppState {
    pub home: Home,
    pub cfg: SharedConfig,
    pub settings: Mutex<HarnessSettings>,
    pub store: SessionStore,
    pub backend: ResolvedLocalBackend,
    pub chat: ChatClient,
    pub cloud_chat: CloudChat,
    pub audit: Audit,
    pub rate_limiter: RateLimiter,
    pub loop_rate_limiter: RateLimiter,
    pub loop_max_tokens: u64,
    pub loop_inflight: Mutex<HashMap<String, f64>>,
    pub generation_gate: GenerationGate,
    pub agent_run_gate: GenerationGate,
    pub csrf_token: String,
    /// `HARNESS_HTML` (CSRF already substituted) split around every
    /// `CSP_NONCE_PLACEHOLDER` occurrence, so each request joins segments with a
    /// fresh nonce instead of re-scanning and reallocating the full ~130KB page.
    pub console_html_segments: Vec<String>,
    pub api_key_optional: bool,
    /// Snapshot of `CGAGENTHARNESS_API_KEY` at build time (tests inject it).
    pub api_key: Option<String>,
    pub key_file_sources: BTreeSet<String>,
    pub auth: Option<AuthManager>,
    /// Limits concurrent memory-hard scrypt derivations for this app instance.
    pub auth_operation_permits: Arc<tokio::sync::Semaphore>,
    pub web: WebTool,
    pub mcp: McpRuntime,
    pub notes: MemoryNotes,
    /// Present only when `structured_memory.enabled` is the literal boolean true.
    pub structured_memory: Option<StructuredMemoryStore>,
    /// Home-local administrator overrides for structured-memory sub-gates.
    pub structured_gates: Mutex<crate::server::structured_memory::OperatorGates>,
    /// Test hook: when set, replaces the closed tool allowlists (empty = deny all).
    pub tool_allowlist_override: Option<BTreeSet<String>>,
    /// Test hook for MCP only. Independent of `tool_allowlist_override`.
    pub mcp_tool_allowlist_override: Option<BTreeSet<String>>,
    /// The only server -> agentic edge (a child process).
    pub shim: crate::shim::ShimContext,
    /// Detached real-repo runs (`/api/agent/jobs`).
    pub jobs: crate::server::agent_jobs::JobStore,
    /// `logging.request_log`: one structured tracing line per request.
    pub request_log: bool,
    /// Idle auto-consolidator. Feature-off never spawns a worker.
    pub auto_consolidation: crate::server::structured_memory_auto::AutoConsolidationControl,
    /// Bounded volatile completion inputs. Only proposals are persisted automatically.
    pub memory_suggestions: crate::server::structured_memory_suggest::Suggestions,
    /// Native Ollama pull/inventory (loopback only; independent of chat generation).
    pub ollama: OllamaControl,
}

/// Single-flight native Ollama pull plus a short-lived tags cache.
pub struct OllamaControl {
    pub pull_gate: GenerationGate,
    pull_abort: Mutex<Option<CancellationToken>>,
    cache: Mutex<Option<(Instant, Value)>>,
}

impl Default for OllamaControl {
    fn default() -> Self {
        Self::new()
    }
}

impl OllamaControl {
    pub fn new() -> Self {
        Self {
            pull_gate: GenerationGate::new(),
            pull_abort: Mutex::new(None),
            cache: Mutex::new(None),
        }
    }

    pub fn register_pull(&self, token: CancellationToken) {
        *self.pull_abort.lock().unwrap_or_else(|p| p.into_inner()) = Some(token);
    }

    pub fn abort_pull(&self) -> bool {
        match self.pull_abort.lock().unwrap_or_else(|p| p.into_inner()).take() {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    pub fn cached_inventory(&self, max_age_sec: u64) -> Option<Value> {
        let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        let (at, value) = cache.as_ref()?;
        if at.elapsed().as_secs() < max_age_sec {
            Some(value.clone())
        } else {
            None
        }
    }

    pub fn store_inventory(&self, value: Value) {
        *self.cache.lock().unwrap_or_else(|p| p.into_inner()) = Some((Instant::now(), value));
    }

    pub fn invalidate_inventory(&self) {
        *self.cache.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
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

    pub fn current_provider(&self) -> String {
        self.cloud_chat
            .provider_name(&self.current_model())
            .unwrap_or(&self.backend.provider)
            .to_string()
    }

    pub fn chat_timeout_sec(&self, model: &str) -> f64 {
        if self.cloud_chat.is_cloud_selection(model) {
            self.cloud_chat.timeout_sec()
        } else {
            self.chat.timeout_sec
        }
    }

    pub fn abort_chat(&self) {
        self.chat.abort_in_flight();
        self.cloud_chat.abort_in_flight();
    }

    pub fn abort_ollama_pull(&self) -> bool {
        self.ollama.abort_pull()
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

    pub fn mcp_tool_allowlist(&self) -> BTreeSet<String> {
        self.mcp_tool_allowlist_override
            .clone()
            .unwrap_or_else(|| self.mcp.broker_allowlist())
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
