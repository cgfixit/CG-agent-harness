//! Fail-closed turn, tool-call, and connector admission.
//!
//! Provenance: source-assisted reading of the public Apache-2.0 tree
//! `openai/codex` at `8c076942cb70fba8e5c4732c23f1faabc4aed484` (2026-10-18),
//! plus the published harness descriptions at
//! <https://developers.openai.com/blog/codex-as-a-platform> and
//! <https://openai.com/index/unlocking-the-codex-harness/>.
//! REA was not run: that skill skips ordinary analysis of a complete source
//! repository. This file is original. It is not Codex, not a vendored copy,
//! and not a verified equivalent.
//!
//! Observed Codex behavior that this module deliberately does not adopt:
//! catalog tools default to enabled with automatic approval; missing
//! destructive and open-world hints are treated as true and still run when
//! those classes are allowed; connector discovery can consult a remote
//! directory. Here, an undeclared tool never runs, `confirm` is never
//! defaulted, a blank `reason` never admits a call, `/loop` never receives a
//! connector tool, and nothing in this module spawns a process or dials out.

use std::collections::{BTreeMap, BTreeSet};

use super::is_visibly_blank;

pub const TARGET_REVISION: &str = "8c076942cb70fba8e5c4732c23f1faabc4aed484";
pub const MAX_TOOL_RESULT_BYTES: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    Auto,
    Prompt,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogToolInput<'a> {
    pub connector_id: Option<&'a str>,
    pub link_id: Option<&'a str>,
    pub tool_name: &'a str,
    pub tool_title: Option<&'a str>,
    pub destructive_hint: Option<bool>,
    pub open_world_hint: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogToolConfig {
    pub enabled: Option<bool>,
    pub approval: Option<ApprovalMode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CatalogLinkConfig {
    pub default_tools_approval: Option<ApprovalMode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CatalogAppConfig {
    pub enabled: bool,
    pub default_tools_enabled: Option<bool>,
    pub default_tools_approval: Option<ApprovalMode>,
    pub destructive_enabled: Option<bool>,
    pub open_world_enabled: Option<bool>,
    pub tools: BTreeMap<String, CatalogToolConfig>,
    pub links: BTreeMap<String, CatalogLinkConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CatalogSnapshot {
    pub has_apps_config: bool,
    pub default_enabled: Option<bool>,
    pub default_tools_approval: Option<ApprovalMode>,
    pub default_destructive_enabled: Option<bool>,
    pub default_open_world_enabled: Option<bool>,
    pub apps: BTreeMap<String, CatalogAppConfig>,
    pub managed_disabled: BTreeSet<String>,
    pub managed_tool_approval: BTreeMap<(String, String), ApprovalMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogPolicy {
    pub enabled: bool,
    pub approval: ApprovalMode,
}

/// Decision table observed in Codex `app_tool_policy`, reimplemented here for
/// comparison. Callers must not use a `true` result as permission to execute.
pub fn catalog_tool_policy(snapshot: &CatalogSnapshot, input: CatalogToolInput<'_>) -> CatalogPolicy {
    let managed = input.connector_id.and_then(|id| {
        snapshot
            .managed_tool_approval
            .get(&(id.to_string(), input.tool_name.to_string()))
            .copied()
    });
    if !snapshot.has_apps_config {
        return CatalogPolicy {
            enabled: true,
            approval: managed.unwrap_or(ApprovalMode::Auto),
        };
    }

    let app = input.connector_id.and_then(|id| snapshot.apps.get(id));
    let tool_config = app.and_then(|app| {
        app.tools
            .get(input.tool_name)
            .or_else(|| input.tool_title.and_then(|title| app.tools.get(title)))
    });
    let approval = managed
        .or_else(|| tool_config.and_then(|tool| tool.approval))
        .or_else(|| {
            input
                .link_id
                .and_then(|link_id| app?.links.get(link_id).and_then(|link| link.default_tools_approval))
        })
        .or_else(|| app.and_then(|app| app.default_tools_approval))
        .or_else(|| input.connector_id.and(snapshot.default_tools_approval))
        .unwrap_or(ApprovalMode::Auto);

    if !catalog_app_enabled(snapshot, input.connector_id) {
        return CatalogPolicy {
            enabled: false,
            approval,
        };
    }
    if let Some(enabled) = tool_config.and_then(|tool| tool.enabled) {
        return CatalogPolicy { enabled, approval };
    }
    if let Some(enabled) = app.and_then(|app| app.default_tools_enabled) {
        return CatalogPolicy { enabled, approval };
    }

    let destructive_enabled = app
        .and_then(|app| app.destructive_enabled)
        .unwrap_or_else(|| snapshot.default_destructive_enabled.unwrap_or(true));
    let open_world_enabled = app
        .and_then(|app| app.open_world_enabled)
        .unwrap_or_else(|| snapshot.default_open_world_enabled.unwrap_or(true));
    let destructive_hint = input.destructive_hint.unwrap_or(true);
    let open_world_hint = input.open_world_hint.unwrap_or(true);
    CatalogPolicy {
        enabled: (destructive_enabled || !destructive_hint) && (open_world_enabled || !open_world_hint),
        approval,
    }
}

pub fn catalog_app_enabled(snapshot: &CatalogSnapshot, connector_id: Option<&str>) -> bool {
    if let Some(id) = connector_id {
        if snapshot.managed_disabled.contains(id) {
            return false;
        }
    }
    let default_enabled = snapshot.default_enabled.unwrap_or(true);
    connector_id
        .and_then(|id| snapshot.apps.get(id))
        .map(|app| app.enabled)
        .unwrap_or(default_enabled)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorRecord {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub is_accessible: bool,
    pub plugin_display_names: Vec<String>,
}

/// Directory entries start inaccessible. An accessible record may replace a
/// placeholder name (name equal to id) and fill a missing description.
/// Plugin display names are unioned, sorted, and de-duplicated.
pub fn merge_connector_records(
    directory: Vec<ConnectorRecord>,
    accessible: Vec<ConnectorRecord>,
) -> Vec<ConnectorRecord> {
    let mut merged: BTreeMap<String, ConnectorRecord> = BTreeMap::new();
    for mut record in directory {
        record.is_accessible = false;
        merged.insert(record.id.clone(), record);
    }
    for mut record in accessible {
        record.is_accessible = true;
        let id = record.id.clone();
        if let Some(existing) = merged.get_mut(&id) {
            existing.is_accessible = true;
            if existing.name == existing.id && record.name != record.id {
                existing.name = record.name;
            }
            if existing.description.is_none() {
                existing.description = record.description;
            }
            existing.plugin_display_names.extend(record.plugin_display_names);
        } else {
            merged.insert(id, record);
        }
    }
    let mut out: Vec<_> = merged.into_values().collect();
    for record in &mut out {
        record.plugin_display_names.sort_unstable();
        record.plugin_display_names.dedup();
    }
    out.sort_by(|left, right| {
        right
            .is_accessible
            .cmp(&left.is_accessible)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.id.cmp(&right.id))
    });
    out
}

/// Suggestion list: discoverable ids that are not already accessible.
/// Accessible records are excluded even when some other flag would disable them.
pub fn filter_discoverable(
    directory: Vec<ConnectorRecord>,
    accessible: &[ConnectorRecord],
    discoverable_ids: &BTreeSet<String>,
) -> Vec<ConnectorRecord> {
    let accessible_ids: BTreeSet<&str> = accessible
        .iter()
        .filter(|record| record.is_accessible)
        .map(|record| record.id.as_str())
        .collect();
    let mut out: Vec<_> = directory
        .into_iter()
        .filter(|record| !accessible_ids.contains(record.id.as_str()))
        .filter(|record| discoverable_ids.contains(&record.id))
        .collect();
    out.sort_by(|left, right| left.name.cmp(&right.name).then_with(|| left.id.cmp(&right.id)));
    out
}

pub fn connector_name_slug(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            normalized.push(character.to_ascii_lowercase());
        } else {
            normalized.push('-');
        }
    }
    let trimmed = normalized.trim_matches('-');
    if trimmed.is_empty() {
        "app".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Pure reconstruction of the observed install-url shape. The origin is an
/// argument. This module does not read `CODEX_APP_SERVER_CHATGPT_BASE_URL`
/// and the live engine never emits the URL.
pub fn observed_install_url(origin: &str, name: &str, connector_id: &str) -> String {
    let origin = origin.trim_end_matches('/').trim_end_matches("/backend-api");
    format!("{origin}/apps/{}/{connector_id}", connector_name_slug(name))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: String,
    pub parallel: bool,
    pub destructive: bool,
    pub open_world: bool,
    pub connector_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorGrant {
    pub id: String,
    pub enabled: bool,
    pub tools: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedCall {
    pub call_id: String,
    pub tool_name: String,
    pub connector_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalReply {
    pub allow: bool,
    pub confirm: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    UserMessage,
    AgentMessage,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopEvent {
    ThreadStarted { thread_id: String },
    TurnStarted { turn_id: u64 },
    ItemStarted { kind: ItemKind, id: String },
    ItemCompleted { kind: ItemKind, id: String },
    ApprovalRequested { call_id: String },
    ToolRefused { call_id: String, code: &'static str },
    TurnCompleted { turn_id: u64 },
    TurnInterrupted { turn_id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopError {
    EmptyToolName,
    DuplicateCallId,
    UnknownTool,
    ConnectorDisabled,
    ToolNotGranted,
    LoopForbidsConnector,
    ConfirmRequired,
    Denied,
    Overlap,
    NotPending,
    Interrupted,
    TurnBusy,
    NoOpenTurn,
    ResultTooLarge,
    ApprovalStillOpen,
    InFlight,
}

struct PendingCall {
    spec_parallel: bool,
    approved: bool,
    in_flight: bool,
    finished: bool,
}

struct OpenTurn {
    id: u64,
    interrupted: bool,
    calls: BTreeMap<String, PendingCall>,
    seen_ids: BTreeSet<String>,
}

pub struct TurnEngine {
    thread_id: String,
    next_turn: u64,
    turn: Option<OpenTurn>,
    tools: BTreeMap<String, ToolSpec>,
    grants: BTreeMap<String, ConnectorGrant>,
    loop_session: bool,
    parallel_in_flight: usize,
    exclusive_in_flight: bool,
}

impl TurnEngine {
    pub fn new(thread_id: impl Into<String>, loop_session: bool) -> (Self, Vec<LoopEvent>) {
        let thread_id = thread_id.into();
        let events = vec![LoopEvent::ThreadStarted {
            thread_id: thread_id.clone(),
        }];
        (
            Self {
                thread_id,
                next_turn: 1,
                turn: None,
                tools: BTreeMap::new(),
                grants: BTreeMap::new(),
                loop_session,
                parallel_in_flight: 0,
                exclusive_in_flight: false,
            },
            events,
        )
    }

    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }

    pub fn register_tool(&mut self, spec: ToolSpec) {
        self.tools.insert(spec.name.clone(), spec);
    }

    pub fn register_grant(&mut self, grant: ConnectorGrant) {
        self.grants.insert(grant.id.clone(), grant);
    }

    pub fn begin_turn(&mut self, user_item_id: &str) -> Result<Vec<LoopEvent>, LoopError> {
        if self.turn.as_ref().is_some_and(|turn| !turn_is_idle(turn)) {
            return Err(LoopError::TurnBusy);
        }
        let id = self.next_turn;
        self.next_turn += 1;
        self.turn = Some(OpenTurn {
            id,
            interrupted: false,
            calls: BTreeMap::new(),
            seen_ids: BTreeSet::new(),
        });
        self.parallel_in_flight = 0;
        self.exclusive_in_flight = false;
        Ok(vec![
            LoopEvent::TurnStarted { turn_id: id },
            LoopEvent::ItemStarted {
                kind: ItemKind::UserMessage,
                id: user_item_id.to_string(),
            },
            LoopEvent::ItemCompleted {
                kind: ItemKind::UserMessage,
                id: user_item_id.to_string(),
            },
        ])
    }

    pub fn propose(&mut self, call: ProposedCall) -> Result<Vec<LoopEvent>, LoopError> {
        let turn = self.turn.as_mut().ok_or(LoopError::NoOpenTurn)?;
        if turn.interrupted {
            return Err(LoopError::Interrupted);
        }
        if call.tool_name.trim().is_empty() {
            return Err(LoopError::EmptyToolName);
        }
        if !turn.seen_ids.insert(call.call_id.clone()) {
            return Err(LoopError::DuplicateCallId);
        }
        let spec = self.tools.get(&call.tool_name).ok_or(LoopError::UnknownTool)?.clone();
        if spec.connector_id != call.connector_id {
            return Err(LoopError::ToolNotGranted);
        }
        if let Some(connector_id) = spec.connector_id.as_deref() {
            if self.loop_session {
                turn.calls.insert(call.call_id.clone(), refused_pending(spec.parallel));
                return Ok(vec![LoopEvent::ToolRefused {
                    call_id: call.call_id,
                    code: "LOOP_HAS_NO_CONNECTORS",
                }]);
            }
            let grant = self.grants.get(connector_id);
            let allowed = grant.is_some_and(|grant| grant.enabled && grant.tools.contains(&spec.name));
            if grant.is_none() || grant.is_some_and(|grant| !grant.enabled) {
                turn.calls.insert(call.call_id.clone(), refused_pending(spec.parallel));
                return Ok(vec![LoopEvent::ToolRefused {
                    call_id: call.call_id,
                    code: "CONNECTOR_DISABLED",
                }]);
            }
            if !allowed {
                turn.calls.insert(call.call_id.clone(), refused_pending(spec.parallel));
                return Ok(vec![LoopEvent::ToolRefused {
                    call_id: call.call_id,
                    code: "TOOL_NOT_GRANTED",
                }]);
            }
        }
        turn.calls.insert(
            call.call_id.clone(),
            PendingCall {
                spec_parallel: spec.parallel,
                approved: false,
                in_flight: false,
                finished: false,
            },
        );
        Ok(vec![LoopEvent::ApprovalRequested { call_id: call.call_id }])
    }

    pub fn resolve_approval(&mut self, call_id: &str, reply: ApprovalReply) -> Result<Vec<LoopEvent>, LoopError> {
        let turn = self.turn.as_mut().ok_or(LoopError::NoOpenTurn)?;
        if turn.interrupted {
            return Err(LoopError::Interrupted);
        }
        let pending = turn.calls.get_mut(call_id).ok_or(LoopError::NotPending)?;
        if pending.finished || pending.approved || pending.in_flight {
            return Err(LoopError::NotPending);
        }
        if !reply.allow {
            pending.finished = true;
            return Ok(vec![LoopEvent::ToolRefused {
                call_id: call_id.to_string(),
                code: "DENIED",
            }]);
        }
        if !reply.confirm || is_visibly_blank(&reply.reason) {
            return Err(LoopError::ConfirmRequired);
        }
        pending.approved = true;
        Ok(vec![])
    }

    pub fn begin_execution(&mut self, call_id: &str) -> Result<(), LoopError> {
        let turn = self.turn.as_mut().ok_or(LoopError::NoOpenTurn)?;
        if turn.interrupted {
            return Err(LoopError::Interrupted);
        }
        let pending = turn.calls.get_mut(call_id).ok_or(LoopError::NotPending)?;
        if !pending.approved || pending.finished || pending.in_flight {
            return Err(LoopError::NotPending);
        }
        if pending.spec_parallel {
            if self.exclusive_in_flight {
                return Err(LoopError::Overlap);
            }
            self.parallel_in_flight += 1;
        } else if self.exclusive_in_flight || self.parallel_in_flight > 0 {
            return Err(LoopError::Overlap);
        } else {
            self.exclusive_in_flight = true;
        }
        pending.in_flight = true;
        Ok(())
    }

    pub fn complete_tool(&mut self, call_id: &str, result_bytes: usize) -> Result<Vec<LoopEvent>, LoopError> {
        if result_bytes > MAX_TOOL_RESULT_BYTES {
            return Err(LoopError::ResultTooLarge);
        }
        let turn = self.turn.as_mut().ok_or(LoopError::NoOpenTurn)?;
        if turn.interrupted {
            return Err(LoopError::Interrupted);
        }
        let pending = turn.calls.get_mut(call_id).ok_or(LoopError::NotPending)?;
        if !pending.in_flight || pending.finished {
            return Err(LoopError::NotPending);
        }
        let parallel = pending.spec_parallel;
        pending.in_flight = false;
        pending.finished = true;
        if parallel {
            self.parallel_in_flight = self.parallel_in_flight.saturating_sub(1);
        } else {
            self.exclusive_in_flight = false;
        }
        Ok(vec![
            LoopEvent::ItemStarted {
                kind: ItemKind::Tool,
                id: call_id.to_string(),
            },
            LoopEvent::ItemCompleted {
                kind: ItemKind::Tool,
                id: call_id.to_string(),
            },
        ])
    }

    pub fn interrupt(&mut self) -> Result<Vec<LoopEvent>, LoopError> {
        let turn = self.turn.as_mut().ok_or(LoopError::NoOpenTurn)?;
        turn.interrupted = true;
        for pending in turn.calls.values_mut() {
            pending.in_flight = false;
            pending.finished = true;
        }
        self.parallel_in_flight = 0;
        self.exclusive_in_flight = false;
        Ok(vec![LoopEvent::TurnInterrupted { turn_id: turn.id }])
    }

    pub fn finish_turn(&mut self, assistant_item_id: &str) -> Result<Vec<LoopEvent>, LoopError> {
        let turn = self.turn.as_mut().ok_or(LoopError::NoOpenTurn)?;
        if turn.interrupted {
            return Err(LoopError::Interrupted);
        }
        if self.parallel_in_flight > 0 || self.exclusive_in_flight {
            return Err(LoopError::InFlight);
        }
        if turn.calls.values().any(|pending| !pending.finished) {
            return Err(LoopError::ApprovalStillOpen);
        }
        let turn_id = turn.id;
        self.turn = None;
        Ok(vec![
            LoopEvent::ItemStarted {
                kind: ItemKind::AgentMessage,
                id: assistant_item_id.to_string(),
            },
            LoopEvent::ItemCompleted {
                kind: ItemKind::AgentMessage,
                id: assistant_item_id.to_string(),
            },
            LoopEvent::TurnCompleted { turn_id },
        ])
    }
}

fn refused_pending(parallel: bool) -> PendingCall {
    PendingCall {
        spec_parallel: parallel,
        approved: false,
        in_flight: false,
        finished: true,
    }
}

fn turn_is_idle(turn: &OpenTurn) -> bool {
    turn.interrupted
        || (turn.calls.values().all(|pending| pending.finished)
            && !turn.calls.values().any(|pending| pending.in_flight))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> CatalogSnapshot {
        CatalogSnapshot {
            has_apps_config: true,
            ..CatalogSnapshot::default()
        }
    }

    #[test]
    fn missing_catalog_enables_with_automatic_approval() {
        let policy = catalog_tool_policy(
            &CatalogSnapshot::default(),
            CatalogToolInput {
                connector_id: None,
                link_id: None,
                tool_name: "shell",
                tool_title: None,
                destructive_hint: None,
                open_world_hint: None,
            },
        );
        assert_eq!(
            policy,
            CatalogPolicy {
                enabled: true,
                approval: ApprovalMode::Auto
            }
        );
    }

    #[test]
    fn managed_disable_beats_an_enabled_app_and_keeps_approval() {
        let mut snapshot = snapshot();
        snapshot.managed_disabled.insert("docs".into());
        snapshot
            .managed_tool_approval
            .insert(("docs".into(), "lookup".into()), ApprovalMode::Deny);
        snapshot.apps.insert(
            "docs".into(),
            CatalogAppConfig {
                enabled: true,
                ..CatalogAppConfig::default()
            },
        );
        let policy = catalog_tool_policy(
            &snapshot,
            CatalogToolInput {
                connector_id: Some("docs"),
                link_id: None,
                tool_name: "lookup",
                tool_title: None,
                destructive_hint: Some(false),
                open_world_hint: Some(false),
            },
        );
        assert_eq!(
            policy,
            CatalogPolicy {
                enabled: false,
                approval: ApprovalMode::Deny
            }
        );
    }

    #[test]
    fn absent_hints_are_destructive_and_open_world() {
        let mut snapshot = snapshot();
        snapshot.default_destructive_enabled = Some(false);
        snapshot.default_open_world_enabled = Some(true);
        snapshot.apps.insert(
            "docs".into(),
            CatalogAppConfig {
                enabled: true,
                ..CatalogAppConfig::default()
            },
        );
        let blocked = catalog_tool_policy(
            &snapshot,
            CatalogToolInput {
                connector_id: Some("docs"),
                link_id: None,
                tool_name: "lookup",
                tool_title: None,
                destructive_hint: None,
                open_world_hint: Some(false),
            },
        );
        assert!(!blocked.enabled);
        let allowed = catalog_tool_policy(
            &snapshot,
            CatalogToolInput {
                connector_id: Some("docs"),
                link_id: None,
                tool_name: "lookup",
                tool_title: None,
                destructive_hint: Some(false),
                open_world_hint: Some(false),
            },
        );
        assert!(allowed.enabled);
    }

    #[test]
    fn explicit_tool_enabled_overrides_class_flags() {
        let mut snapshot = snapshot();
        snapshot.default_destructive_enabled = Some(false);
        let mut app = CatalogAppConfig {
            enabled: true,
            destructive_enabled: Some(false),
            ..CatalogAppConfig::default()
        };
        app.tools.insert(
            "lookup".into(),
            CatalogToolConfig {
                enabled: Some(true),
                approval: Some(ApprovalMode::Prompt),
            },
        );
        snapshot.apps.insert("docs".into(), app);
        let policy = catalog_tool_policy(
            &snapshot,
            CatalogToolInput {
                connector_id: Some("docs"),
                link_id: None,
                tool_name: "missing",
                tool_title: Some("lookup"),
                destructive_hint: Some(true),
                open_world_hint: Some(true),
            },
        );
        assert_eq!(
            policy,
            CatalogPolicy {
                enabled: true,
                approval: ApprovalMode::Prompt
            }
        );
    }

    #[test]
    fn approval_precedence_is_managed_then_tool_then_link_then_app_then_default() {
        let mut snapshot = snapshot();
        snapshot.default_tools_approval = Some(ApprovalMode::Auto);
        let mut app = CatalogAppConfig {
            enabled: true,
            default_tools_approval: Some(ApprovalMode::Prompt),
            ..CatalogAppConfig::default()
        };
        app.links.insert(
            "acct".into(),
            CatalogLinkConfig {
                default_tools_approval: Some(ApprovalMode::Deny),
            },
        );
        app.tools.insert(
            "lookup".into(),
            CatalogToolConfig {
                enabled: None,
                approval: Some(ApprovalMode::Prompt),
            },
        );
        snapshot.apps.insert("docs".into(), app);
        snapshot
            .managed_tool_approval
            .insert(("docs".into(), "lookup".into()), ApprovalMode::Deny);
        let policy = catalog_tool_policy(
            &snapshot,
            CatalogToolInput {
                connector_id: Some("docs"),
                link_id: Some("acct"),
                tool_name: "lookup",
                tool_title: None,
                destructive_hint: Some(false),
                open_world_hint: Some(false),
            },
        );
        assert_eq!(policy.approval, ApprovalMode::Deny);
    }

    #[test]
    fn merge_replaces_only_a_placeholder_name_and_sorts_accessible_first() {
        let merged = merge_connector_records(
            vec![ConnectorRecord {
                id: "cal".into(),
                name: "cal".into(),
                description: None,
                is_accessible: true,
                plugin_display_names: vec!["Zed".into()],
            }],
            vec![ConnectorRecord {
                id: "cal".into(),
                name: "Calendar".into(),
                description: Some("events".into()),
                is_accessible: false,
                plugin_display_names: vec!["Alpha".into(), "Alpha".into()],
            }],
        );
        assert_eq!(merged.len(), 1);
        assert!(merged[0].is_accessible);
        assert_eq!(merged[0].name, "Calendar");
        assert_eq!(merged[0].description.as_deref(), Some("events"));
        assert_eq!(
            merged[0].plugin_display_names,
            vec!["Alpha".to_string(), "Zed".to_string()]
        );
    }

    #[test]
    fn filter_drops_accessible_ids_even_when_discoverable() {
        let directory = vec![
            ConnectorRecord {
                id: "b".into(),
                name: "Beta".into(),
                description: None,
                is_accessible: false,
                plugin_display_names: Vec::new(),
            },
            ConnectorRecord {
                id: "a".into(),
                name: "Alpha".into(),
                description: None,
                is_accessible: false,
                plugin_display_names: Vec::new(),
            },
        ];
        let accessible = [ConnectorRecord {
            id: "a".into(),
            name: "Alpha".into(),
            description: None,
            is_accessible: true,
            plugin_display_names: Vec::new(),
        }];
        let mut discoverable = BTreeSet::new();
        discoverable.insert("a".into());
        discoverable.insert("b".into());
        let filtered = filter_discoverable(directory, &accessible, &discoverable);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "b");
    }

    #[test]
    fn slug_and_install_url_change_with_the_supplied_origin() {
        assert_eq!(connector_name_slug("Google Calendar"), "google-calendar");
        assert_eq!(connector_name_slug("---"), "app");
        assert_eq!(connector_name_slug("A--B"), "a--b");
        assert_eq!(
            observed_install_url("https://chatgpt.com/backend-api/", "Google Calendar", "cal"),
            "https://chatgpt.com/apps/google-calendar/cal"
        );
        assert_eq!(
            observed_install_url("https://example.invalid", "Google Calendar", "cal"),
            "https://example.invalid/apps/google-calendar/cal"
        );
    }

    fn engine_with_tools(loop_session: bool) -> TurnEngine {
        let (mut engine, _) = TurnEngine::new("thread-1", loop_session);
        engine.register_tool(ToolSpec {
            name: "read".into(),
            parallel: true,
            destructive: false,
            open_world: false,
            connector_id: None,
        });
        engine.register_tool(ToolSpec {
            name: "write".into(),
            parallel: false,
            destructive: true,
            open_world: false,
            connector_id: None,
        });
        engine.register_tool(ToolSpec {
            name: "lookup".into(),
            parallel: true,
            destructive: false,
            open_world: true,
            connector_id: Some("docs".into()),
        });
        engine.register_grant(ConnectorGrant {
            id: "docs".into(),
            enabled: true,
            tools: BTreeSet::from(["lookup".into()]),
        });
        engine
    }

    fn allow(reason: &str) -> ApprovalReply {
        ApprovalReply {
            allow: true,
            confirm: true,
            reason: reason.into(),
        }
    }

    #[test]
    fn confirm_is_required_and_deny_does_not_execute() {
        let mut engine = engine_with_tools(false);
        engine.begin_turn("u1").unwrap();
        let events = engine
            .propose(ProposedCall {
                call_id: "c1".into(),
                tool_name: "write".into(),
                connector_id: None,
            })
            .unwrap();
        assert_eq!(events, vec![LoopEvent::ApprovalRequested { call_id: "c1".into() }]);
        assert_eq!(
            engine.resolve_approval(
                "c1",
                ApprovalReply {
                    allow: true,
                    confirm: false,
                    reason: "because".into(),
                }
            ),
            Err(LoopError::ConfirmRequired)
        );
        assert_eq!(
            engine.resolve_approval(
                "c1",
                ApprovalReply {
                    allow: true,
                    confirm: true,
                    reason: "   ".into(),
                }
            ),
            Err(LoopError::ConfirmRequired)
        );
        engine
            .resolve_approval(
                "c1",
                ApprovalReply {
                    allow: false,
                    confirm: false,
                    reason: String::new(),
                },
            )
            .unwrap();
        assert_eq!(engine.begin_execution("c1"), Err(LoopError::NotPending));
    }

    #[test]
    fn parallel_calls_overlap_and_an_exclusive_call_does_not() {
        let mut engine = engine_with_tools(false);
        engine.begin_turn("u1").unwrap();
        for id in ["a", "b"] {
            engine
                .propose(ProposedCall {
                    call_id: id.into(),
                    tool_name: "read".into(),
                    connector_id: None,
                })
                .unwrap();
            engine.resolve_approval(id, allow("read the file")).unwrap();
            engine.begin_execution(id).unwrap();
        }
        engine
            .propose(ProposedCall {
                call_id: "w".into(),
                tool_name: "write".into(),
                connector_id: None,
            })
            .unwrap();
        engine.resolve_approval("w", allow("patch the file")).unwrap();
        assert_eq!(engine.begin_execution("w"), Err(LoopError::Overlap));
        engine.complete_tool("a", 4).unwrap();
        engine.complete_tool("b", 4).unwrap();
        engine.begin_execution("w").unwrap();
        assert_eq!(
            engine.complete_tool("w", MAX_TOOL_RESULT_BYTES + 1),
            Err(LoopError::ResultTooLarge)
        );
        engine.complete_tool("w", 8).unwrap();
        let finished = engine.finish_turn("assistant").unwrap();
        assert!(finished.contains(&LoopEvent::TurnCompleted { turn_id: 1 }));
    }

    #[test]
    fn loop_session_refuses_a_granted_connector_tool() {
        let mut engine = engine_with_tools(true);
        engine.begin_turn("u1").unwrap();
        let events = engine
            .propose(ProposedCall {
                call_id: "c".into(),
                tool_name: "lookup".into(),
                connector_id: Some("docs".into()),
            })
            .unwrap();
        assert_eq!(
            events,
            vec![LoopEvent::ToolRefused {
                call_id: "c".into(),
                code: "LOOP_HAS_NO_CONNECTORS"
            }]
        );
        assert_eq!(engine.begin_execution("c"), Err(LoopError::NotPending));
    }

    #[test]
    fn disabled_grant_and_changed_connector_id_do_not_run() {
        let mut engine = engine_with_tools(false);
        engine.register_grant(ConnectorGrant {
            id: "docs".into(),
            enabled: false,
            tools: BTreeSet::from(["lookup".into()]),
        });
        engine.begin_turn("u1").unwrap();
        let refused = engine
            .propose(ProposedCall {
                call_id: "c".into(),
                tool_name: "lookup".into(),
                connector_id: Some("docs".into()),
            })
            .unwrap();
        assert_eq!(
            refused[0],
            LoopEvent::ToolRefused {
                call_id: "c".into(),
                code: "CONNECTOR_DISABLED"
            }
        );
        assert_eq!(
            engine.propose(ProposedCall {
                call_id: "d".into(),
                tool_name: "lookup".into(),
                connector_id: Some("other".into()),
            }),
            Err(LoopError::ToolNotGranted)
        );
        assert_eq!(
            engine.propose(ProposedCall {
                call_id: "c".into(),
                tool_name: "read".into(),
                connector_id: None,
            }),
            Err(LoopError::DuplicateCallId)
        );
        assert_eq!(
            engine.propose(ProposedCall {
                call_id: "e".into(),
                tool_name: " ".into(),
                connector_id: None,
            }),
            Err(LoopError::EmptyToolName)
        );
    }

    #[test]
    fn interrupt_blocks_completion_and_the_next_turn_can_start() {
        let mut engine = engine_with_tools(false);
        engine.begin_turn("u1").unwrap();
        engine
            .propose(ProposedCall {
                call_id: "a".into(),
                tool_name: "read".into(),
                connector_id: None,
            })
            .unwrap();
        engine.resolve_approval("a", allow("read")).unwrap();
        engine.begin_execution("a").unwrap();
        assert_eq!(engine.finish_turn("assistant"), Err(LoopError::InFlight));
        engine.interrupt().unwrap();
        assert_eq!(engine.complete_tool("a", 1), Err(LoopError::Interrupted));
        assert_eq!(engine.finish_turn("assistant"), Err(LoopError::Interrupted));
        let next = engine.begin_turn("u2").unwrap();
        assert!(next.contains(&LoopEvent::TurnStarted { turn_id: 2 }));
    }

    #[test]
    fn target_revision_is_recorded() {
        let (engine, events) = TurnEngine::new("thread-1", false);
        assert_eq!(TARGET_REVISION.len(), 40);
        assert_eq!(engine.thread_id(), "thread-1");
        assert_eq!(
            events,
            vec![LoopEvent::ThreadStarted {
                thread_id: "thread-1".into()
            }]
        );
    }
}
