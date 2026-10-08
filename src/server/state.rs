//! Shared application state for the console.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::common::audit::Audit;
use crate::common::auth_store::AuthManager;
use crate::common::config::{AppConfig, SharedConfig};
use crate::common::errors::{HarnessError, Result};
use crate::common::home::{HarnessSettings, Home};
use crate::common::ratelimit::RateLimiter;
use crate::llm::backend::ResolvedLocalBackend;
use crate::llm::cloud_chat::CloudChat;
use crate::llm::openai_chat::ChatClient;

use super::attachments::AttachmentStore;
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

/// `attachments.max_concurrent_uploads`: bound on upload bodies buffered at
/// once (each up to `MAX_REQUEST_BYTES`); the per-home byte quota is checked
/// only after a body is in memory, so this is what caps transient memory.
pub fn upload_concurrency(cfg: &AppConfig) -> Result<usize> {
    match cfg.get("attachments.max_concurrent_uploads") {
        None => Ok(2),
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=8).contains(value))
            .map(|value| value as usize)
            .ok_or_else(|| HarnessError::config("attachments.max_concurrent_uploads must be an integer from 1 to 8")),
    }
}

/// `attachments.body_timeout_sec`: server-side deadline for reading one
/// upload body. It bounds how long a stalled client can hold an upload
/// permit; a body that does not arrive in time is refused and its permit
/// released.
pub fn upload_body_timeout(cfg: &AppConfig) -> Result<std::time::Duration> {
    match cfg.get("attachments.body_timeout_sec") {
        None => Ok(std::time::Duration::from_secs(120)),
        Some(value) => value
            .as_u64()
            .filter(|value| (5..=600).contains(value))
            .map(std::time::Duration::from_secs)
            .ok_or_else(|| HarnessError::config("attachments.body_timeout_sec must be an integer from 5 to 600")),
    }
}

/// Longest a `/api/ps` window read may take (a turn may hold the generation gate).
const WINDOW_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// Error code when the loaded window shrank below what a request was sized for.
pub const WINDOW_CHANGED: &str = "OLLAMA_WINDOW_CHANGED";
/// How old a `/api/ps` read may be and still set a tuned model's caps.
/// [`AppState::verify_window`] takes a fresh one just before each use.
const VERIFIED_WINDOW_MAX_AGE_SEC: u64 = 10;

pub struct AppState {
    pub home: Home,
    pub cfg: SharedConfig,
    pub runtime: std::sync::RwLock<Arc<super::config_reload::RuntimeLimits>>,
    pub settings: Mutex<HarnessSettings>,
    pub store: SessionStore,
    pub chat_owner: super::web_research::ResearchState,
    pub attachments: AttachmentStore,
    pub notes_corpus: crate::server::notes_corpus::NotesCorpus,
    pub notes_ingest_permits: Arc<tokio::sync::Semaphore>,
    pub backend: ResolvedLocalBackend,
    pub chat: ChatClient,
    pub cloud_chat: CloudChat,
    pub audit: Audit,
    pub rate_limiter: RateLimiter,
    pub loop_rate_limiter: RateLimiter,
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
    /// Test hook. Production entrypoints leave this empty and use the OS store.
    pub credential_store: Option<Arc<dyn crate::common::credential_store::CredentialStore>>,
    /// Startup migration warnings. Names and reasons only; never secret values.
    pub credential_warnings: Vec<String>,
    pub auth: Option<Arc<AuthManager>>,
    /// Limits concurrent memory-hard scrypt derivations for this app instance.
    pub auth_operation_permits: Arc<tokio::sync::Semaphore>,
    /// Limits attachment upload bodies held in memory at once.
    pub upload_permits: Arc<tokio::sync::Semaphore>,
    /// Deadline for reading one upload body while holding a permit.
    pub upload_body_timeout: std::time::Duration,
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
    /// Persisted recurring agent jobs (`/api/agent/schedules`).
    pub schedules: crate::server::agent_schedules::ScheduleStore,
    /// Append-only inference ledger (`logging.spend_file`).
    pub spend_file: PathBuf,
    /// `logging.request_log`: one structured tracing line per request.
    pub request_log: bool,
    /// Idle auto-consolidator. Feature-off never spawns a worker.
    pub auto_consolidation: crate::server::structured_memory_auto::AutoConsolidationControl,
    /// Bounded volatile completion inputs. Only proposals are persisted automatically.
    pub memory_suggestions: crate::server::structured_memory_suggest::Suggestions,
    /// Native Ollama pull/inventory (loopback only; independent of chat generation).
    pub ollama: OllamaControl,
    /// `models.local_llm.auto_tune` result for one model; read through the
    /// accessors below, which ignore it for any other model.
    pub tuning: Mutex<Option<Arc<crate::server::model_limits::Tuning>>>,
    /// Passive collectors for `GET /api/netconnect`. Fixed at startup.
    ///
    /// [`crate::server::build_app`] stores [`crate::netconnect::tools::PassiveSources::live`].
    /// Tests pass fixtures only through [`crate::server::build_app_with_sources`].
    /// No route, slash command, reload, or config key replaces this value.
    /// Any runtime path that does needs a policy review.
    pub(crate) netconnect_sources: crate::netconnect::tools::PassiveSources,
}

/// Single-flight native Ollama pull plus a short-lived tags cache.
pub struct OllamaControl {
    pub pull_gate: GenerationGate,
    pull_abort: Mutex<Option<CancellationToken>>,
    cache: Mutex<Option<(Instant, Value)>>,
    /// Last loaded window read from `/api/ps`: when, for which model, and size
    /// (`None` when that read failed or did not report one).
    window: Mutex<Option<(Instant, String, Option<u64>)>>,
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
            window: Mutex::new(None),
        }
    }

    /// `Some` while a read for `model` is fresh, including a failed one.
    pub fn cached_window(&self, model: &str, max_age_sec: u64) -> Option<Option<u64>> {
        let window = self.window.lock().unwrap_or_else(|p| p.into_inner());
        let (at, cached, size) = window.as_ref()?;
        (cached == model && at.elapsed().as_secs() < max_age_sec).then_some(*size)
    }

    pub fn store_window(&self, model: &str, size: Option<u64>) {
        *self.window.lock().unwrap_or_else(|p| p.into_inner()) = Some((Instant::now(), model.to_string(), size));
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
    pub fn runtime_limits(&self) -> Arc<super::config_reload::RuntimeLimits> {
        self.runtime.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// One web operation keeps its limits while sharing cancellation, fetch
    /// permits and cache/policy mutation locks with every other snapshot.
    pub fn web_snapshot(&self) -> WebTool {
        let mut web = self.web.clone();
        web.limits = self.model_web_limits(&self.current_model(), self.runtime_limits().web.clone());
        web
    }

    pub fn tuning_for(&self, model: &str) -> Option<Arc<crate::server::model_limits::Tuning>> {
        self.tuning
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .filter(|tuning| tuning.model == model)
            .cloned()
    }

    /// Install (or clear) the auto-tuned limits and the matching chat deadline.
    pub fn set_tuning(&self, tuning: Option<crate::server::model_limits::Tuning>) {
        let deadline = tuning
            .as_ref()
            .and_then(|t| t.timeout_sec.map(|seconds| (t.model.clone(), seconds as f64)));
        *self.tuning.lock().unwrap_or_else(|p| p.into_inner()) = tuning.map(Arc::new);
        self.chat.set_timeout_override(deadline);
    }

    /// `web` (the live snapshot), lowered to `model`'s tuning, with
    /// `web.total_tokens` held at the cap for its measured window (32000 when
    /// none is measured).
    pub fn model_web_limits(&self, model: &str, mut web: super::web_search::Limits) -> super::web_search::Limits {
        let tuning = self.tuning_for(model);
        if let Some(tuning) = &tuning {
            tuning.apply_web(&mut web);
        }
        web.total_tokens = web
            .total_tokens
            .min(super::compaction::web_total_cap(self.verified_window(model)));
        web
    }

    /// The context window Ollama has `model` loaded with, read from `/api/ps`
    /// and cached for the inventory refresh interval. `None` when unknown (not
    /// loaded, not reported, not a loopback Ollama endpoint); a read that
    /// cannot run never blocks a turn. A failed read is cached too, so an
    /// endpoint without `/api/ps` is not probed (up to the inventory timeout)
    /// on every turn.
    pub async fn live_window(&self, model: &str) -> Option<u64> {
        let refresh = crate::llm::ollama::clamped(&self.cfg, "models.local_llm.inventory.refresh_sec", 30, 1, 3600);
        if let Some(window) = self.ollama.cached_window(model, refresh) {
            return window;
        }
        self.probe_window(model).await
    }

    /// Before a turn, research run, reload or profile sizes its limits: re-read
    /// `/api/ps` now, ignoring the cache, when `model` has a tuning (its caps
    /// follow the verified window). Untuned, the caps are the defaults and
    /// nothing is read. Read-only: a model Ollama does not report loaded gets
    /// the defaults here, and [`Self::ensure_window_allows`] loads it later,
    /// under the caller's generation gate.
    pub async fn verify_window(&self, model: &str) {
        if self.tuning_for(model).is_some() {
            self.probe_window_state(model).await;
        }
    }

    /// Just before a model call on a tuned `model`, whose limits follow the
    /// verified window: re-read `/api/ps` and refuse the call if the window
    /// Ollama reports no longer allows `prompt_limit` and `web_total`, so a
    /// restart with a different window mid-turn cannot truncate the prompt.
    /// Callers hold the generation gate, so a model Ollama reports not loaded
    /// (a restart, a keep_alive expiry) is loaded here with the bounded warmup
    /// request and read again: the check uses the window it will actually serve.
    /// A tuned model's window was measured, so one Ollama still does not report
    /// is refused too, not treated as the 32768 defaults.
    /// An untuned model has the default caps and returns at once without a read.
    pub async fn ensure_window_allows(&self, model: &str, prompt_limit: u64, web_total: u64) -> Result<()> {
        if self.tuning_for(model).is_none() {
            return Ok(());
        }
        if self.probe_window_state(model).await == crate::llm::ollama::LoadedWindow::NotLoaded {
            self.load_for_window(model).await;
        }
        let Some(window) = self.verified_window(model) else {
            return Err(HarnessError::new(
                WINDOW_CHANGED,
                format!(
                    "Ollama did not report the window {model} is loaded with, even after loading it; nothing was sent. \
                     Check that Ollama is running, then send it again."
                ),
            ));
        };
        let window = Some(window);
        if prompt_limit <= super::compaction::prompt_cap(window)
            && web_total <= super::compaction::web_total_cap(window)
        {
            return Ok(());
        }
        Err(HarnessError::new(
            WINDOW_CHANGED,
            format!(
                "Ollama no longer reports {model} loaded with the window this request was sized for; nothing was sent. \
                 Send it again: limits now follow the window Ollama reports."
            ),
        ))
    }

    /// Load `model` with the bounded warmup request, then read its window.
    async fn load_for_window(&self, model: &str) {
        let Some(native) = crate::llm::ollama::native_base_url(&self.chat.base_url) else {
            return;
        };
        let keep_alive = crate::llm::ollama::clamped(&self.cfg, "models.local_llm.warmup.keep_alive_sec", 300, 1, 3600);
        let timeout = Duration::from_secs(crate::llm::ollama::clamped(
            &self.cfg,
            "models.local_llm.warmup.timeout_sec",
            30,
            1,
            120,
        ));
        if crate::llm::ollama::load_model(&native, model, keep_alive, timeout).await {
            self.probe_window_state(model).await;
        }
    }

    /// One bounded `/api/ps` read for `model`, cached (failures too).
    async fn probe_window(&self, model: &str) -> Option<u64> {
        match self.probe_window_state(model).await {
            crate::llm::ollama::LoadedWindow::Loaded(window) => Some(window),
            _ => None,
        }
    }

    async fn probe_window_state(&self, model: &str) -> crate::llm::ollama::LoadedWindow {
        use crate::llm::ollama::LoadedWindow;
        let Some(native) = crate::llm::ollama::native_base_url(&self.chat.base_url) else {
            return LoadedWindow::Unknown;
        };
        let Ok(mut limits) = crate::llm::inventory::InventoryLimits::from_config(&self.cfg) else {
            return LoadedWindow::Unknown;
        };
        // A turn may hold the generation gate here: a loopback /api/ps answers in
        // milliseconds, so a slow one is skipped (and cached) rather than waited on.
        limits.timeout = limits.timeout.min(WINDOW_PROBE_TIMEOUT);
        let state = crate::llm::ollama::loaded_window_state(&native, model, limits).await;
        let window = match state {
            LoadedWindow::Loaded(window) => Some(window),
            _ => None,
        };
        self.ollama.store_window(model, window);
        state
    }

    /// The window a tuned model's caps follow: the smaller of the one `auto_tune`
    /// measured and a `/api/ps` read from the last few seconds
    /// ([`Self::verify_window`]), so a restart with a smaller window shrinks
    /// them. An unloaded model, an unanswered or old read give `None`: the
    /// 32768-window defaults.
    pub fn verified_window(&self, model: &str) -> Option<u64> {
        let tuned = self.tuning_for(model)?.window;
        let live = self
            .ollama
            .cached_window(model, VERIFIED_WINDOW_MAX_AGE_SEC)
            .flatten()?;
        Some(tuned.min(live))
    }

    /// The prompt cap for `model`: 30000 scaled to its verified window.
    pub fn prompt_cap(&self, model: &str) -> u64 {
        super::compaction::prompt_cap(self.verified_window(model))
    }

    /// The most `chat.compact_prompt_tokens` can be for `model`, whatever is
    /// configured: its prompt cap, lowered to any tuned compaction budget.
    pub fn compact_ceiling(&self, model: &str) -> u64 {
        let cap = self.prompt_cap(model);
        self.tuning_for(model)
            .map_or(cap, |tuning| cap.min(tuning.compact_prompt_tokens))
    }

    /// The most `web.total_tokens` can be for `model`, whatever is configured:
    /// its window cap (like [`Self::prompt_cap`]), lowered to any tuned budget.
    pub fn web_total_ceiling(&self, model: &str) -> u64 {
        let cap = super::compaction::web_total_cap(self.verified_window(model));
        self.tuning_for(model)
            .and_then(|tuning| tuning.web.as_ref().map(|web| web.total_tokens))
            .map_or(cap, |budget| cap.min(budget))
    }

    /// `models.local_llm.max_tokens`, lowered to `model`'s tuning.
    pub fn chat_max_tokens(&self, model: &str) -> u64 {
        let configured = self
            .cfg
            .u64_or("models.local_llm.max_tokens", super::compaction::DEFAULT_REPLY_TOKENS);
        self.tuning_for(model)
            .map_or(configured, |tuning| configured.min(tuning.max_tokens))
    }

    /// `chat.compact_prompt_tokens`, lowered to `model`'s tuning.
    pub fn compact_prompt_tokens(&self, model: &str) -> u64 {
        let configured = self
            .cfg
            .u64_or("chat.compact_prompt_tokens", super::compaction::DEFAULT_PROMPT_TOKENS);
        self.tuning_for(model)
            .map_or(configured, |tuning| configured.min(tuning.compact_prompt_tokens))
    }

    /// True when tuning found `model` cannot take web tools (none declared, or
    /// no web-chat prompt fits its window): chat then sends it no tools.
    pub fn chat_tools_unsupported(&self, model: &str) -> bool {
        self.tuning_for(model).is_some_and(|tuning| tuning.tools_off)
    }

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
        self.provider_for(&self.current_model())
    }

    pub fn provider_for(&self, model: &str) -> String {
        self.cloud_chat
            .provider_name(model)
            .unwrap_or(&self.backend.provider)
            .to_string()
    }

    pub fn chat_timeout_sec(&self, model: &str) -> f64 {
        if self.cloud_chat.is_cloud_selection(model) {
            self.cloud_chat.timeout_sec()
        } else {
            self.chat.timeout_for(model)
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

#[cfg(test)]
mod upload_concurrency_tests {
    use super::{upload_body_timeout, upload_concurrency};
    use crate::common::config::AppConfig;

    #[test]
    fn upload_body_timeout_is_configured_and_bounded() {
        for (raw, expected) in [("{}", Some(120)), ("attachments: {body_timeout_sec: 30}", Some(30))] {
            let cfg = AppConfig::from_str(raw, std::path::Path::new("fixture.yaml")).unwrap();
            assert_eq!(upload_body_timeout(&cfg).ok().map(|d| d.as_secs()), expected);
        }
        for raw in ["4", "601", "'60'"] {
            let cfg = AppConfig::from_str(
                &format!("attachments: {{body_timeout_sec: {raw}}}"),
                std::path::Path::new("fixture.yaml"),
            )
            .unwrap();
            assert!(upload_body_timeout(&cfg).is_err(), "raw={raw}");
        }
    }

    #[test]
    fn upload_cap_is_configured_and_bounded() {
        for (raw, expected) in [("{}", Some(2)), ("attachments: {max_concurrent_uploads: 4}", Some(4))] {
            let cfg = AppConfig::from_str(raw, std::path::Path::new("fixture.yaml")).unwrap();
            assert_eq!(upload_concurrency(&cfg).ok(), expected);
        }
        for raw in ["0", "9", "'2'"] {
            let cfg = AppConfig::from_str(
                &format!("attachments: {{max_concurrent_uploads: {raw}}}"),
                std::path::Path::new("fixture.yaml"),
            )
            .unwrap();
            assert!(upload_concurrency(&cfg).is_err(), "raw={raw}");
        }
    }
}
