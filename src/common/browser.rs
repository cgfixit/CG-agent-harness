//! Session authority for general browsing and PR browser verification.
//!
//! The two profiles share the action parser and the origin check. They do
//! not share grants. A page, a model reply, and a candidate tree cannot add
//! an origin. The Playwright process is not started here.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use serde::Deserialize;
use serde_json::Value;
use url::Url;

const MAX_WAIT_MS: u32 = 5_000;
const MAX_TYPE_CHARS: usize = 2_000;
const MAX_GRANT_ORIGINS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Live,
    Takeover,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Handoff { reason: String },
    Refuse { reason: String },
}

impl Decision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Decision::Allow)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyError {
    BadOrigin(String),
    Ambiguous { role: String, name: String },
    Suspended,
    TooManyOrigins,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    None,
    Text,
    Secret,
}

/// Trusted description of a resolved element. The model does not supply this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementFact {
    pub role: String,
    pub name: String,
    pub href: Option<String>,
    pub input: InputKind,
    /// Sidecar-issued handle from the latest aria snapshot; expires with it.
    pub snapshot_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementDraft {
    pub role: String,
    pub name: String,
    pub href: Option<String>,
    pub input: InputKind,
    pub snapshot_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedElement {
    pub ref_id: String,
    pub role: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub generation: u64,
    pub tabs: Vec<String>,
    pub elements: Vec<MintedElement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectationSource {
    Operator { text: String },
    Contract { location: String },
    TrustedTest { id: String },
    Exploratory { text: String },
}

impl ExpectationSource {
    pub fn counts_as_pass(&self) -> bool {
        !matches!(self, ExpectationSource::Exploratory { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssertionStatus {
    Passed,
    Failed,
    Skipped { reason: String },
    Blocked { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionRecord {
    pub name: String,
    pub status: AssertionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub scenario: String,
    pub expectation: ExpectationSource,
    pub revision: String,
    pub assertions: Vec<AssertionRecord>,
    pub containment: &'static str,
}

impl VerifyReport {
    pub fn passed(&self) -> bool {
        self.expectation.counts_as_pass()
            && self
                .assertions
                .iter()
                .all(|row| matches!(row.status, AssertionStatus::Passed))
            && !self.assertions.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OriginKey {
    scheme: String,
    host: String,
    port: u16,
}

impl OriginKey {
    fn display(&self) -> String {
        format!("{}://{}:{}", self.scheme, self.host, self.port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Profile {
    General {
        origins: BTreeSet<OriginKey>,
    },
    Pr {
        owned: OriginKey,
        dependencies: BTreeSet<OriginKey>,
    },
}

#[derive(Debug, Clone)]
struct LiveRef {
    generation: u64,
    fact: ElementFact,
}

#[derive(Debug, Clone)]
pub struct Session {
    profile: Profile,
    phase: Phase,
    generation: u64,
    tabs: Vec<String>,
    refs: BTreeMap<String, LiveRef>,
}

impl Session {
    pub fn general<I, S>(origins: I) -> Result<Self, PolicyError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut set = BTreeSet::new();
        for raw in origins {
            if set.len() >= MAX_GRANT_ORIGINS {
                return Err(PolicyError::TooManyOrigins);
            }
            set.insert(parse_origin(raw.as_ref())?);
        }
        Ok(Self::new(Profile::General { origins: set }))
    }

    pub fn pr_verify(owned: &str, dependencies: &[&str]) -> Result<Self, PolicyError> {
        let owned = parse_origin(owned)?;
        let mut deps = BTreeSet::new();
        for raw in dependencies {
            if deps.len() >= MAX_GRANT_ORIGINS {
                return Err(PolicyError::TooManyOrigins);
            }
            let origin = parse_origin(raw)?;
            if origin != owned {
                deps.insert(origin);
            }
        }
        Ok(Self::new(Profile::Pr {
            owned,
            dependencies: deps,
        }))
    }

    fn new(profile: Profile) -> Self {
        Self {
            profile,
            phase: Phase::Live,
            generation: 0,
            tabs: Vec::new(),
            refs: BTreeMap::new(),
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn results_open(&self) -> bool {
        self.phase != Phase::Revoked
    }

    pub fn begin_takeover(&mut self) {
        if self.phase == Phase::Live {
            self.phase = Phase::Takeover;
        }
    }

    pub fn end_takeover(&mut self) -> Result<(), PolicyError> {
        match self.phase {
            Phase::Takeover => {
                self.phase = Phase::Live;
                Ok(())
            }
            Phase::Revoked => Err(PolicyError::Suspended),
            Phase::Live => Ok(()),
        }
    }

    pub fn revoke(&mut self) {
        self.phase = Phase::Revoked;
        self.refs.clear();
        self.tabs.clear();
    }

    pub fn authorize_url(&self, raw: &str) -> Decision {
        if self.phase == Phase::Revoked {
            return Decision::Refuse {
                reason: "session revoked".into(),
            };
        }
        if self.phase == Phase::Takeover {
            return Decision::Refuse {
                reason: "model suspended during operator takeover".into(),
            };
        }
        match parse_origin(raw) {
            Ok(origin) if self.origin_allowed(&origin) => Decision::Allow,
            Ok(origin) => Decision::Refuse {
                reason: format!("blocked origin {}: not in this session grant", origin.display()),
            },
            Err(PolicyError::BadOrigin(reason)) => Decision::Refuse { reason },
            Err(_) => Decision::Refuse {
                reason: "url rejected".into(),
            },
        }
    }

    pub fn mint(&mut self, tabs: Vec<String>, drafts: Vec<ElementDraft>) -> Result<Observation, PolicyError> {
        if self.phase != Phase::Live {
            return Err(PolicyError::Suspended);
        }
        // A new page invalidates the old refs even when this observation is refused.
        self.generation = self.generation.saturating_add(1);
        self.refs.clear();
        self.tabs.clear();
        let mut seen = BTreeSet::new();
        for draft in &drafts {
            // Same role and name is only ambiguous when no snapshot ref tells them apart.
            let key = (
                draft.role.to_ascii_lowercase(),
                draft.name.clone(),
                draft.snapshot_ref.clone(),
            );
            if !seen.insert(key) {
                return Err(PolicyError::Ambiguous {
                    role: draft.role.clone(),
                    name: draft.name.clone(),
                });
            }
        }
        self.tabs = tabs;
        let mut elements = Vec::with_capacity(drafts.len());
        for (index, draft) in drafts.into_iter().enumerate() {
            let ref_id = format!("g{}-e{}", self.generation, index);
            elements.push(MintedElement {
                ref_id: ref_id.clone(),
                role: draft.role.clone(),
                name: draft.name.clone(),
            });
            self.refs.insert(
                ref_id,
                LiveRef {
                    generation: self.generation,
                    fact: ElementFact {
                        role: draft.role,
                        name: draft.name,
                        href: draft.href,
                        input: draft.input,
                        snapshot_ref: draft.snapshot_ref,
                    },
                },
            );
        }
        Ok(Observation {
            generation: self.generation,
            tabs: self.tabs.clone(),
            elements,
        })
    }

    pub fn decide(&self, wire: &Value) -> Decision {
        if !self.results_open() {
            return Decision::Refuse {
                reason: "session revoked".into(),
            };
        }
        if self.phase == Phase::Takeover {
            return Decision::Refuse {
                reason: "model suspended during operator takeover".into(),
            };
        }
        let action = match parse_wire(wire) {
            Ok(action) => action,
            Err(reason) => return Decision::Refuse { reason },
        };
        match action {
            WireAction::Navigate { url } => self.authorize_url(&url),
            WireAction::Observe => Decision::Allow,
            WireAction::Wait { timeout_ms } => {
                if timeout_ms == 0 || timeout_ms > MAX_WAIT_MS {
                    Decision::Refuse {
                        reason: format!("wait must be 1-{MAX_WAIT_MS} ms"),
                    }
                } else {
                    Decision::Allow
                }
            }
            WireAction::SelectTab { tab } => {
                if self.tabs.iter().any(|name| name == &tab) {
                    Decision::Allow
                } else {
                    Decision::Refuse {
                        reason: format!("unknown tab {tab}"),
                    }
                }
            }
            WireAction::Scroll { ref_id, dy } => {
                if !(-2_000..=2_000).contains(&dy) {
                    return Decision::Refuse {
                        reason: "scroll distance is out of bounds".into(),
                    };
                }
                self.require_live_ref(ref_id.as_deref())
                    .map_or_else(|reason| Decision::Refuse { reason }, |_| Decision::Allow)
            }
            WireAction::FollowLink { ref_id } | WireAction::Click { ref_id } => self.decide_activate(&ref_id),
            WireAction::Type { ref_id, text } => self.decide_type(&ref_id, &text),
            WireAction::PressEnter { ref_id } => self.decide_press_enter(&ref_id),
            WireAction::Select { ref_id, value } => self.decide_select(&ref_id, &value),
        }
    }

    fn decide_activate(&self, ref_id: &str) -> Decision {
        let fact = match self.live_fact(ref_id) {
            Ok(fact) => fact,
            Err(reason) => return Decision::Refuse { reason },
        };
        if fact.input == InputKind::Secret {
            return Decision::Handoff {
                reason: "secret field stays with the operator".into(),
            };
        }
        let role = fact.role.to_ascii_lowercase();
        if role == "tab" {
            return if self.tabs.iter().any(|name| name == &fact.name) {
                Decision::Allow
            } else {
                Decision::Refuse {
                    reason: format!("unknown tab {}", fact.name),
                }
            };
        }
        if role == "link" {
            let href = fact.href.as_deref();
            return match href {
                Some(href) => self.authorize_url(href),
                None => Decision::Handoff {
                    reason: "link has no resolvable href".into(),
                },
            };
        }
        Decision::Handoff {
            reason: format!("effect of {role} is unclassified; operator takeover required"),
        }
    }

    fn decide_type(&self, ref_id: &str, text: &str) -> Decision {
        if text.is_empty() || text.chars().count() > MAX_TYPE_CHARS {
            return Decision::Refuse {
                reason: "text length out of bounds".into(),
            };
        }
        let fact = match self.live_fact(ref_id) {
            Ok(fact) => fact,
            Err(reason) => return Decision::Refuse { reason },
        };
        match fact.input {
            InputKind::Secret => Decision::Handoff {
                reason: "secret field stays with the operator".into(),
            },
            InputKind::Text if accepts_line(&fact.role) => Decision::Allow,
            InputKind::Text => Decision::Handoff {
                reason: "text entry on this role is unclassified".into(),
            },
            InputKind::None => Decision::Refuse {
                reason: "element does not accept text".into(),
            },
        }
    }

    fn decide_press_enter(&self, ref_id: &str) -> Decision {
        let fact = match self.live_fact(ref_id) {
            Ok(fact) => fact,
            Err(reason) => return Decision::Refuse { reason },
        };
        if fact.input == InputKind::Secret {
            return Decision::Handoff {
                reason: "secret field stays with the operator".into(),
            };
        }
        if accepts_line(&fact.role) {
            Decision::Allow
        } else {
            Decision::Refuse {
                reason: "enter is only for a text box".into(),
            }
        }
    }

    fn decide_select(&self, ref_id: &str, value: &str) -> Decision {
        if value.is_empty() || value.chars().count() > MAX_TYPE_CHARS {
            return Decision::Refuse {
                reason: "selection value out of bounds".into(),
            };
        }
        let fact = match self.live_fact(ref_id) {
            Ok(fact) => fact,
            Err(reason) => return Decision::Refuse { reason },
        };
        if fact.input == InputKind::Secret {
            return Decision::Handoff {
                reason: "secret field stays with the operator".into(),
            };
        }
        if fact.role.eq_ignore_ascii_case("combobox") || fact.role.eq_ignore_ascii_case("listbox") {
            Decision::Allow
        } else {
            Decision::Handoff {
                reason: "selection on this role is unclassified".into(),
            }
        }
    }

    fn require_live_ref(&self, ref_id: Option<&str>) -> Result<(), String> {
        match ref_id {
            None => Ok(()),
            Some(id) => self.live_fact(id).map(|_| ()),
        }
    }

    pub fn grant_strings(&self) -> Vec<String> {
        let keys: Vec<&OriginKey> = match &self.profile {
            Profile::General { origins } => origins.iter().collect(),
            Profile::Pr { owned, dependencies } => std::iter::once(owned).chain(dependencies.iter()).collect(),
        };
        keys.iter().map(|key| key.display()).collect()
    }

    pub fn fact(&self, ref_id: &str) -> Option<&ElementFact> {
        self.live_fact(ref_id).ok()
    }

    fn live_fact(&self, ref_id: &str) -> Result<&ElementFact, String> {
        match self.refs.get(ref_id) {
            Some(live) if live.generation == self.generation => Ok(&live.fact),
            Some(_) | None => Err(format!("stale or unknown ref {ref_id}")),
        }
    }

    fn origin_allowed(&self, origin: &OriginKey) -> bool {
        match &self.profile {
            Profile::General { origins } => origins.contains(origin),
            Profile::Pr { owned, dependencies } => origin == owned || dependencies.contains(origin),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum WireAction {
    Navigate { url: String },
    Observe,
    FollowLink { ref_id: String },
    SelectTab { tab: String },
    Scroll { ref_id: Option<String>, dy: i32 },
    Select { ref_id: String, value: String },
    Type { ref_id: String, text: String },
    PressEnter { ref_id: String },
    Wait { timeout_ms: u32 },
    Click { ref_id: String },
}

fn accepts_line(role: &str) -> bool {
    matches!(role.to_ascii_lowercase().as_str(), "textbox" | "searchbox" | "combobox")
}

fn parse_wire(value: &Value) -> Result<WireAction, String> {
    serde_json::from_value(value.clone()).map_err(|_| "malformed browser action".to_string())
}

fn parse_origin(raw: &str) -> Result<OriginKey, PolicyError> {
    let url = Url::parse(raw).map_err(|_| PolicyError::BadOrigin(format!("rejected url {raw}")))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(PolicyError::BadOrigin("url credentials are not a grant".into()));
    }
    if url.scheme() != "https" && url.scheme() != "http" {
        return Err(PolicyError::BadOrigin(format!(
            "scheme {} is not browseable",
            url.scheme()
        )));
    }
    let host = url
        .host_str()
        .ok_or_else(|| PolicyError::BadOrigin("url has no host".into()))?;
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return Err(PolicyError::BadOrigin("url has no host".into()));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip.is_unspecified() || ip.is_multicast() {
            return Err(PolicyError::BadOrigin("unspecified or multicast host".into()));
        }
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| PolicyError::BadOrigin("missing port".into()))?;
    Ok(OriginKey {
        scheme: url.scheme().to_string(),
        host,
        port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page(session: &mut Session) -> Observation {
        session
            .mint(
                vec!["All".into(), "Open".into()],
                vec![
                    ElementDraft {
                        role: "tab".into(),
                        name: "Open".into(),
                        href: None,
                        input: InputKind::None,
                        snapshot_ref: None,
                    },
                    ElementDraft {
                        role: "link".into(),
                        name: "Example docs".into(),
                        href: Some("https://example.com/docs".into()),
                        input: InputKind::None,
                        snapshot_ref: None,
                    },
                    ElementDraft {
                        role: "textbox".into(),
                        name: "Filter".into(),
                        href: None,
                        input: InputKind::Text,
                        snapshot_ref: None,
                    },
                    ElementDraft {
                        role: "textbox".into(),
                        name: "Password".into(),
                        href: None,
                        input: InputKind::Secret,
                        snapshot_ref: None,
                    },
                ],
            )
            .unwrap()
    }

    #[test]
    fn general_grant_does_not_cover_a_second_origin_or_a_pr_port() {
        let mut session = Session::general(["https://example.com/start"]).unwrap();
        let obs = page(&mut session);
        assert!(session.authorize_url("https://example.com/other").is_allow());
        let blocked = session.authorize_url("https://cdn.example.com/app.js");
        assert!(matches!(blocked, Decision::Refuse { .. }));
        assert!(!session.authorize_url("http://127.0.0.1:8792/").is_allow());
        let link = obs.elements.iter().find(|el| el.name == "Example docs").unwrap();
        assert!(session
            .decide(&json!({"action":"follow_link","ref_id": link.ref_id}))
            .is_allow());
    }

    #[test]
    fn pr_scope_rejects_a_public_origin_and_keeps_the_owned_endpoint() {
        let session = Session::pr_verify("http://127.0.0.1:49152/", &["https://example.test/"]).unwrap();
        assert!(session.authorize_url("http://127.0.0.1:49152/health").is_allow());
        assert!(session.authorize_url("https://example.test/fixture").is_allow());
        assert!(!session.authorize_url("https://example.com/").is_allow());
        assert!(!session.authorize_url("http://127.0.0.1:8790/").is_allow());
    }

    #[test]
    fn takeover_and_revoke_stop_the_model() {
        let mut session = Session::general(["https://example.com"]).unwrap();
        let obs = page(&mut session);
        let filter = obs.elements.iter().find(|el| el.name == "Filter").unwrap();
        session.begin_takeover();
        let held = session.decide(&json!({"action":"type","ref_id": filter.ref_id, "text":"open"}));
        assert!(matches!(held, Decision::Refuse { .. }));
        session.end_takeover().unwrap();
        assert!(session
            .decide(&json!({"action":"type","ref_id": filter.ref_id, "text":"open"}))
            .is_allow());
        session.revoke();
        assert!(!session.results_open());
        assert!(matches!(
            session.decide(&json!({"action":"observe"})),
            Decision::Refuse { .. }
        ));
    }

    #[test]
    fn secret_fields_hand_off_and_hostile_json_cannot_grant() {
        let mut session = Session::general(["https://example.com"]).unwrap();
        let obs = page(&mut session);
        let password = obs.elements.iter().find(|el| el.name == "Password").unwrap();
        let decision = session.decide(&json!({"action":"type","ref_id": password.ref_id, "text":"hunter2"}));
        assert!(matches!(decision, Decision::Handoff { .. }));
        let forged = session.decide(&json!({"action":"grant","origin":"https://evil.test","effect":"read"}));
        assert!(matches!(forged, Decision::Refuse { .. }));
        assert!(!session.authorize_url("https://evil.test/").is_allow());
        let filter = obs.elements.iter().find(|el| el.name == "Filter").unwrap();
        assert!(session
            .decide(&json!({"action":"press_enter","ref_id": filter.ref_id}))
            .is_allow());
        let keyed = session.decide(&json!({"action":"press_enter","ref_id": filter.ref_id, "key":"a"}));
        assert!(matches!(keyed, Decision::Refuse { .. }));
    }

    #[test]
    fn stale_and_ambiguous_refs_do_not_act() {
        let mut session = Session::general(["https://example.com"]).unwrap();
        let first = page(&mut session);
        let link = first.elements[1].ref_id.clone();
        let second = page(&mut session);
        let stale = session.decide(&json!({"action":"click","ref_id": link}));
        assert!(matches!(stale, Decision::Refuse { .. }));
        let ambiguous = session.mint(
            vec!["All".into()],
            vec![
                ElementDraft {
                    role: "link".into(),
                    name: "Docs".into(),
                    href: Some("https://example.com/a".into()),
                    input: InputKind::None,
                    snapshot_ref: None,
                },
                ElementDraft {
                    role: "link".into(),
                    name: "Docs".into(),
                    href: Some("https://example.com/b".into()),
                    input: InputKind::None,
                    snapshot_ref: None,
                },
            ],
        );
        assert!(matches!(ambiguous, Err(PolicyError::Ambiguous { .. })));
        // A refused observation still retires the previous page's refs.
        let prior = second.elements[1].ref_id.clone();
        assert!(matches!(
            session.decide(&json!({"action":"follow_link","ref_id": prior})),
            Decision::Refuse { .. }
        ));
        // Repeated "Read more" links are distinct once the snapshot tells them apart.
        let read_more = |href: &str, snap: &str| ElementDraft {
            role: "link".into(),
            name: "Read more".into(),
            href: Some(href.into()),
            input: InputKind::None,
            snapshot_ref: Some(snap.into()),
        };
        let obs = session
            .mint(
                vec![],
                vec![
                    read_more("https://example.com/a", "e3"),
                    read_more("https://evil.test/b", "e4"),
                ],
            )
            .unwrap();
        let (a, b) = (&obs.elements[0].ref_id, &obs.elements[1].ref_id);
        assert!(session.decide(&json!({"action":"follow_link","ref_id": a})).is_allow());
        assert!(!session.decide(&json!({"action":"follow_link","ref_id": b})).is_allow());
    }

    #[test]
    fn exploratory_expectation_is_not_a_pass() {
        let report = VerifyReport {
            scenario: "message survives reload".into(),
            expectation: ExpectationSource::Exploratory {
                text: "model wrote the expected text".into(),
            },
            revision: "261efd9".into(),
            assertions: vec![AssertionRecord {
                name: "reload keeps the message".into(),
                status: AssertionStatus::Passed,
            }],
            containment: "unproven",
        };
        assert!(!report.passed());
        let trusted = VerifyReport {
            expectation: ExpectationSource::TrustedTest {
                id: "fixture-reload".into(),
            },
            ..report
        };
        assert!(trusted.passed());
    }
}
