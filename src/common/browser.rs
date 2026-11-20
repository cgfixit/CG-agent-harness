//! Session authority for general browsing and PR browser verification.
//!
//! The two profiles share the action parser and the origin check. They do
//! not share grants. A page, a model reply, and a candidate tree cannot add
//! an origin. The Playwright process is not started here.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value;
use url::Url;

const MAX_WAIT_MS: u32 = 5_000;
const MAX_TYPE_CHARS: usize = 2_000;
const MAX_GRANT_ORIGINS: usize = 32;
/// Actions an operator may approve for one native session.
const MAX_DESKTOP_ACTIONS: usize = 32;
/// Why a native action the operator did not list is not run.
pub const DESKTOP_HANDOFF: &str = "native action stays with the operator: it is not on this session's approved list";
/// Why nothing is typed or chosen in a native session.
pub const DESKTOP_INPUT_OFF: &str = "typing, Enter and selection are off in a native session";
/// An observation is one page of 40 elements and 4,000 characters.
const MAX_OBSERVE_PAGE: u32 = 50;
const MAX_FIND_CHARS: usize = 80;
/// URLs a model task may navigate to: the grants' roots, the task's own URLs,
/// page URLs and observed link targets (#901). Past this, new ones are not added.
const MAX_SEEN_URLS: usize = 4_096;
/// Page text kept for typed-text provenance; observations of one page add up to it.
const MAX_PAGE_TEXT_CHARS: usize = 200_000;
/// The refusal a composed navigation gets. It never names the URL, so the
/// path or query it carried stays out of the transcript and the report.
pub const COMPOSED_URL_REFUSAL: &str = "provenance: navigate only to a URL the task names, a granted site's root, or a URL a page showed; follow a link by its ref instead of composing one";
/// The hold typed text gets when the task and the current page do not contain it.
pub const TYPED_TEXT_HOLD: &str =
    "provenance: typed text must appear in the task or on this page; held for the operator";

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
    Ambiguous {
        role: String,
        name: String,
    },
    Suspended,
    TooManyOrigins,
    /// A native session's approved list is over its limit or has a blank entry.
    BadDesktopActions,
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
    /// A native app window (#996). There is no network layer under a native
    /// control to hold what it sends, so nothing is activated unless the
    /// operator listed its role and exact name; nothing is typed or chosen.
    Desktop {
        approved: BTreeSet<(String, String)>,
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
    /// Set for a model-driven task: where its URLs and typed text may come from.
    provenance: Option<Provenance>,
}

/// What a model-driven task was given or shown (#901 steps 2 and 3). A
/// scripted fixture has none: its steps are the operator's own.
#[derive(Debug, Clone, Default)]
struct Provenance {
    task: String,
    urls: BTreeSet<String>,
    page_url: Option<String>,
    page_text: String,
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

    /// A native session. `approved` lists the only elements that may be
    /// activated, each as a role and the element's exact accessible name; the
    /// operator supplies it, never the model. An empty list is a read-only
    /// session. It has no origins: every URL is refused.
    pub fn desktop<I, R, N>(approved: I) -> Result<Self, PolicyError>
    where
        I: IntoIterator<Item = (R, N)>,
        R: AsRef<str>,
        N: AsRef<str>,
    {
        let mut set = BTreeSet::new();
        for (role, name) in approved {
            let (role, name) = (role.as_ref().trim().to_ascii_lowercase(), name.as_ref().to_string());
            if role.is_empty() || name.trim().is_empty() || set.len() >= MAX_DESKTOP_ACTIONS {
                return Err(PolicyError::BadDesktopActions);
            }
            set.insert((role, name));
        }
        Ok(Self::new(Profile::Desktop { approved: set }))
    }

    fn is_desktop(&self) -> bool {
        matches!(self.profile, Profile::Desktop { .. })
    }

    fn new(profile: Profile) -> Self {
        Self {
            profile,
            phase: Phase::Live,
            generation: 0,
            tabs: Vec::new(),
            refs: BTreeMap::new(),
            provenance: None,
        }
    }

    /// Bind this session to a model-driven task. From now on `navigate` takes
    /// only a URL the task names, a granted origin's root, a page URL or an
    /// observed link target, and `type` text must appear in the task or on the
    /// current page, else it is held for the operator.
    pub fn bind_task(&mut self, task: &str) {
        let mut provenance = Provenance {
            task: collapse_space(task),
            ..Provenance::default()
        };
        for origin in self.grant_strings() {
            remember_url(&mut provenance.urls, &format!("{origin}/"));
        }
        for url in task_urls(task) {
            remember_url(&mut provenance.urls, url);
        }
        self.provenance = Some(provenance);
    }

    /// Record what a step showed: the page URL and its visible text, element
    /// names and option labels. Text from one page adds up; a new page replaces it.
    pub fn saw_page(&mut self, url: Option<&str>, text: &[&str]) {
        let Some(provenance) = self.provenance.as_mut() else {
            return;
        };
        let key = url.and_then(url_key);
        if let Some(key) = &key {
            if provenance.urls.len() < MAX_SEEN_URLS {
                provenance.urls.insert(key.clone());
            }
        }
        if key.is_some() && key != provenance.page_url {
            provenance.page_url = key;
            provenance.page_text.clear();
        }
        for piece in text {
            let piece = collapse_space(piece);
            if piece.is_empty() || provenance.page_text.chars().count() + piece.chars().count() > MAX_PAGE_TEXT_CHARS {
                continue;
            }
            provenance.page_text.push('\n');
            provenance.page_text.push_str(&piece);
        }
    }

    /// Whether `raw` is a URL this task was given or shown. Unbound sessions
    /// (scripted fixtures) answer true.
    fn given_url(&self, raw: &str) -> bool {
        let Some(provenance) = &self.provenance else {
            return true;
        };
        url_key(raw).is_some_and(|key| provenance.urls.contains(&key))
    }

    /// Whether typed `text` appears in the task or on the current page.
    fn given_text(&self, text: &str) -> bool {
        let Some(provenance) = &self.provenance else {
            return true;
        };
        let text = collapse_space(text);
        !text.is_empty() && (provenance.task.contains(&text) || provenance.page_text.contains(&text))
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
        if let Some(provenance) = self.provenance.as_mut() {
            for href in drafts.iter().filter_map(|draft| draft.href.as_deref()) {
                if provenance.urls.len() < MAX_SEEN_URLS {
                    remember_url(&mut provenance.urls, href);
                }
            }
        }
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
        if self.is_desktop() {
            // A native window has no pages or tabs to go to, and input is off.
            // A ref is still checked first, so a stale one reads as stale.
            match &action {
                WireAction::Navigate { .. } => {
                    return Decision::Refuse {
                        reason: "a native session has no page to navigate".into(),
                    }
                }
                WireAction::SelectTab { .. } => {
                    return Decision::Handoff {
                        reason: DESKTOP_HANDOFF.into(),
                    }
                }
                WireAction::Type { ref_id, .. }
                | WireAction::PressEnter { ref_id }
                | WireAction::Select { ref_id, .. } => {
                    return match self.live_fact(ref_id) {
                        Err(reason) => Decision::Refuse { reason },
                        Ok(fact) if fact.input == InputKind::Secret => Decision::Handoff {
                            reason: "secret field stays with the operator".into(),
                        },
                        Ok(_) => Decision::Handoff {
                            reason: DESKTOP_INPUT_OFF.into(),
                        },
                    };
                }
                _ => {}
            }
        }
        match action {
            WireAction::Navigate { url } => match self.authorize_url(&url) {
                Decision::Allow if !self.given_url(&url) => Decision::Refuse {
                    reason: COMPOSED_URL_REFUSAL.into(),
                },
                other => other,
            },
            WireAction::Observe { page, find } => {
                let find_ok = find.is_none_or(|text| (1..=MAX_FIND_CHARS).contains(&text.trim().chars().count()));
                if page.is_none_or(|page| (1..=MAX_OBSERVE_PAGE).contains(&page)) && find_ok {
                    Decision::Allow
                } else {
                    Decision::Refuse {
                        reason: format!(
                            "observe takes page 1-{MAX_OBSERVE_PAGE} and find of 1-{MAX_FIND_CHARS} characters"
                        ),
                    }
                }
            }
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
        if let Profile::Desktop { approved } = &self.profile {
            // No role is safe by default here: a native button can send, pay or delete.
            return if approved.contains(&(role, fact.name.clone())) {
                Decision::Allow
            } else {
                Decision::Handoff {
                    reason: DESKTOP_HANDOFF.into(),
                }
            };
        }
        if role == "tab" {
            return if self.tabs.iter().any(|name| name == &fact.name) {
                Decision::Allow
            } else {
                Decision::Refuse {
                    reason: format!("unknown tab {}", fact.name),
                }
            };
        }
        // A link's target must be granted. A link with no target (a scripted
        // `role="link"`) is a button: whatever its script loads still meets the
        // page route, the network gate and the proxy, which hold the grants.
        if role == "link" {
            if let Some(href) = fact.href.as_deref() {
                return self.authorize_url(href);
            }
        }
        // Buttons, menus and the like may run: the sidecar holds every non-GET
        // request, so a click can change the page but not write to a server.
        Decision::Allow
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
            InputKind::Text if accepts_line(&fact.role) && self.given_text(text) => Decision::Allow,
            // A GET form sends what is typed in its URL: only text the task or
            // the page supplied may go there without the operator (#901).
            InputKind::Text if accepts_line(&fact.role) => Decision::Handoff {
                reason: TYPED_TEXT_HOLD.into(),
            },
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
            Profile::Desktop { .. } => Vec::new(),
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
            Profile::Desktop { .. } => false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum WireAction {
    Navigate { url: String },
    Observe { page: Option<u32>, find: Option<String> },
    FollowLink { ref_id: String },
    SelectTab { tab: String },
    Scroll { ref_id: Option<String>, dy: i32 },
    Select { ref_id: String, value: String },
    Type { ref_id: String, text: String },
    PressEnter { ref_id: String },
    Wait { timeout_ms: u32 },
    Click { ref_id: String },
}

/// The form two URLs compare in: parsed, fragment kept. A fragment never
/// reaches the server, but the page's scripts read it, so a composed one can
/// carry data out as surely as a query (Codex review on #982).
fn url_key(raw: &str) -> Option<String> {
    let url = Url::parse(raw.trim()).ok()?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return None;
    }
    Some(url.to_string())
}

fn remember_url(urls: &mut BTreeSet<String>, raw: &str) {
    if let Some(key) = url_key(raw) {
        urls.insert(key);
    }
}

/// The http(s) URLs written in an operator's task: each as typed and, when
/// it ends in punctuation, also without it. Both are the operator's own
/// text, so `Function_(mathematics)` and `?q=hello!` keep their last
/// character while `(https://a/b).` still yields `https://a/b`.
fn task_urls(task: &str) -> impl Iterator<Item = &str> {
    task.split_whitespace().flat_map(|word| {
        let start = word.find("https://").or_else(|| word.find("http://"));
        let typed = start.map(|start| &word[start..]);
        let trimmed =
            typed.map(|url| url.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '"', '\'']));
        let trimmed = trimmed.filter(|url| Some(*url) != typed);
        typed.into_iter().chain(trimmed).filter(|url| !url.is_empty())
    })
}

fn collapse_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
    // The parsed host, not its text: an IPv6 `host_str` keeps its brackets and
    // would never parse as an address.
    let unroutable = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_unspecified() || ip.is_multicast(),
        Some(url::Host::Ipv6(ip)) => ip.is_unspecified() || ip.is_multicast(),
        _ => false,
    };
    if unroutable {
        return Err(PolicyError::BadOrigin("unspecified or multicast host".into()));
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
    fn a_link_without_a_target_is_clicked_like_a_button_and_a_target_must_be_granted() {
        let mut session = Session::general(["https://example.com/"]).unwrap();
        let link = |name: &str, href: Option<&str>| ElementDraft {
            role: "link".into(),
            name: name.into(),
            href: href.map(str::to_string),
            input: InputKind::None,
            snapshot_ref: None,
        };
        let obs = session
            .mint(
                vec![],
                vec![
                    link("Scripted", None),
                    link("Away", Some("https://other.example/")),
                    link("Home", Some("https://example.com./start")),
                ],
            )
            .unwrap();
        let decide =
            |action: &str, i: usize| session.decide(&json!({"action": action,"ref_id": obs.elements[i].ref_id}));
        // Was a handoff, which ended the whole task.
        assert!(decide("click", 0).is_allow());
        assert!(decide("follow_link", 0).is_allow());
        assert!(matches!(decide("follow_link", 1), Decision::Refuse { .. }));
        // A trailing dot names the same grant here and in the sidecar's originOf.
        assert!(decide("follow_link", 2).is_allow());
    }

    #[test]
    fn unspecified_and_multicast_literals_are_not_grantable_in_either_family() {
        for raw in [
            "https://0.0.0.0/",
            "https://224.0.0.1/",
            "https://[::]/",
            "https://[::]:8080/",
            "https://[ff02::1]/",
        ] {
            assert!(matches!(parse_origin(raw), Err(PolicyError::BadOrigin(_))), "{raw}");
        }
        // A routable literal still parses; the grant keeps the brackets Chrome sends.
        assert_eq!(
            parse_origin("https://[2606:4700::1111]/").unwrap().host,
            "[2606:4700::1111]"
        );
        assert_eq!(parse_origin("https://8.8.8.8/").unwrap().host, "8.8.8.8");
    }

    #[test]
    fn pr_scope_rejects_a_public_origin_and_keeps_the_owned_endpoint() {
        let session = Session::pr_verify("http://127.0.0.1:49152/", &["https://example.test/"]).unwrap();
        assert!(session.authorize_url("http://127.0.0.1:49152/health").is_allow());
        assert!(session.authorize_url("https://example.test/fixture").is_allow());
        assert!(!session.authorize_url("https://example.com/").is_allow());
        assert!(!session.authorize_url("http://127.0.0.1:8790/").is_allow());
    }

    fn native_window(session: &mut Session) -> Observation {
        let draft = |role: &str, name: &str, input| ElementDraft {
            role: role.into(),
            name: name.into(),
            href: None,
            input,
            snapshot_ref: Some("snap-1:0".into()),
        };
        session
            .mint(
                vec!["General".into()],
                vec![
                    draft("AXButton", "Refresh", InputKind::None),
                    draft("AXButton", "Delete all", InputKind::None),
                    draft("AXTextField", "Search", InputKind::Text),
                    draft("AXSecureTextField", "Password", InputKind::Secret),
                    draft("AXLink", "Help", InputKind::None),
                ],
            )
            .unwrap()
    }

    /// #996: a native control has no network layer under it, so only what the
    /// operator listed runs; everything else is handed off, and nothing is typed.
    #[test]
    fn a_native_session_activates_only_listed_actions_and_types_nothing() {
        let mut session = Session::desktop([("AXButton", "Refresh")]).unwrap();
        let seen = native_window(&mut session);
        let id = |name: &str| seen.elements.iter().find(|el| el.name == name).unwrap().ref_id.clone();
        let click = |session: &Session, name: &str| session.decide(&json!({"action":"click","ref_id": id(name)}));
        assert_eq!(click(&session, "Refresh"), Decision::Allow);
        for name in ["Delete all", "Search", "Help"] {
            assert_eq!(
                click(&session, name),
                Decision::Handoff {
                    reason: DESKTOP_HANDOFF.into()
                },
                "{name}"
            );
        }
        assert!(matches!(click(&session, "Password"), Decision::Handoff { reason } if reason.contains("secret")));
        // The same name on another role is not the listed action.
        assert_ne!(
            Session::desktop([("AXLink", "Refresh")]).map(|mut other| {
                let seen = native_window(&mut other);
                let id = seen
                    .elements
                    .iter()
                    .find(|el| el.name == "Refresh")
                    .unwrap()
                    .ref_id
                    .clone();
                other.decide(&json!({"action":"click","ref_id": id}))
            }),
            Ok(Decision::Allow)
        );
        let off = Decision::Handoff {
            reason: DESKTOP_INPUT_OFF.into(),
        };
        for wire in [
            json!({"action":"type","ref_id": id("Search"),"text":"x"}),
            json!({"action":"press_enter","ref_id": id("Search")}),
            json!({"action":"select","ref_id": id("Search"),"value":"x"}),
        ] {
            assert_eq!(session.decide(&wire), off, "{wire}");
        }
        assert!(matches!(
            session.decide(&json!({"action":"type","ref_id": id("Password"),"text":"x"})),
            Decision::Handoff { reason } if reason.contains("secret")
        ));
        assert!(matches!(
            session.decide(&json!({"action":"select_tab","tab":"General"})),
            Decision::Handoff { .. }
        ));
        for url in ["https://example.com/", "file:///etc/passwd"] {
            assert!(matches!(
                session.decide(&json!({"action":"navigate","url": url})),
                Decision::Refuse { .. }
            ));
            assert!(matches!(session.authorize_url(url), Decision::Refuse { .. }));
        }
        assert!(session.grant_strings().is_empty());
        // Reading stays open; a stale ref is still stale.
        assert_eq!(session.decide(&json!({"action":"observe"})), Decision::Allow);
        assert_eq!(session.decide(&json!({"action":"scroll","dy": 400})), Decision::Allow);
        let old = id("Refresh");
        native_window(&mut session);
        assert!(matches!(
            session.decide(&json!({"action":"click","ref_id": old})),
            Decision::Refuse { reason } if reason.contains("stale")
        ));
    }

    #[test]
    fn a_native_approved_list_is_bounded_and_an_empty_one_is_read_only() {
        let mut read_only = Session::desktop(Vec::<(&str, &str)>::new()).unwrap();
        let seen = native_window(&mut read_only);
        for el in &seen.elements {
            assert!(
                !read_only
                    .decide(&json!({"action":"click","ref_id": el.ref_id}))
                    .is_allow(),
                "{}",
                el.name
            );
        }
        for bad in [vec![("", "Refresh")], vec![("AXButton", "  ")]] {
            assert_eq!(Session::desktop(bad).err(), Some(PolicyError::BadDesktopActions));
        }
        let many: Vec<(String, String)> = (0..=MAX_DESKTOP_ACTIONS)
            .map(|n| ("AXButton".to_string(), format!("Button {n}")))
            .collect();
        assert_eq!(Session::desktop(many).err(), Some(PolicyError::BadDesktopActions));
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
    fn buttons_may_be_clicked_but_secret_fields_never() {
        let mut session = Session::general(["https://example.com"]).unwrap();
        let draft = |role: &str, name: &str, input| ElementDraft {
            role: role.into(),
            name: name.into(),
            href: None,
            input,
            snapshot_ref: None,
        };
        let obs = session
            .mint(
                vec![],
                vec![
                    draft("button", "Start", InputKind::None),
                    draft("menuitem", "Games", InputKind::None),
                    draft("textbox", "Password", InputKind::Secret),
                ],
            )
            .unwrap();
        let click = |i: usize| session.decide(&json!({"action":"click","ref_id": obs.elements[i].ref_id}));
        assert!(click(0).is_allow());
        assert!(click(1).is_allow());
        assert!(matches!(click(2), Decision::Handoff { .. }));
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
    fn observe_takes_only_a_bounded_page_and_find() {
        let session = Session::general(["https://example.com"]).unwrap();
        for ok in [
            json!({"action":"observe"}),
            json!({"action":"observe","page":1}),
            json!({"action":"observe","page":50,"find":"Download"}),
        ] {
            assert!(session.decide(&ok).is_allow(), "{ok}");
        }
        for bad in [
            json!({"action":"observe","page":0}),
            json!({"action":"observe","page":51}),
            json!({"action":"observe","page":-1}),
            json!({"action":"observe","find":"  "}),
            json!({"action":"observe","find":"x".repeat(81)}),
            json!({"action":"observe","script":"alert(1)"}),
        ] {
            assert!(matches!(session.decide(&bad), Decision::Refuse { .. }), "{bad}");
        }
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

    #[test]
    fn a_bound_task_navigates_only_to_given_urls_and_types_only_given_text() {
        let mut session = Session::general(["https://example.com"]).unwrap();
        let navigate = |session: &Session, url: &str| session.decide(&json!({"action":"navigate","url":url}));
        // Unbound (a scripted fixture): any URL on a granted origin.
        assert!(navigate(&session, "https://example.com/any?q=1").is_allow());
        session.bind_task("Open https://example.com/start?x=1. then compare with (https://example.com/b).");
        assert!(navigate(&session, "https://example.com").is_allow(), "the granted root");
        assert!(
            navigate(&session, "https://example.com/start?x=1").is_allow(),
            "named in the task"
        );
        assert!(
            navigate(&session, "https://example.com/b").is_allow(),
            "punctuation around the URL trimmed"
        );
        // A fragment is page-readable data: a composed one is refused (Codex review on #982).
        for composed in ["https://example.com/b#frag", "https://example.com/#leak=secret123"] {
            assert!(!navigate(&session, composed).is_allow(), "{composed}");
        }
        // A URL that ends in valid URL punctuation keeps it as typed.
        session.bind_task(
            "Read https://example.com/wiki/Function_(mathematics) and https://example.com/s?q=hello! then https://example.com/f#part",
        );
        for typed in [
            "https://example.com/wiki/Function_(mathematics)",
            "https://example.com/s?q=hello!",
            "https://example.com/f#part",
        ] {
            assert!(navigate(&session, typed).is_allow(), "{typed}");
        }
        assert!(!navigate(&session, "https://example.com/f#other").is_allow());
        session.bind_task("Open https://example.com/start?x=1. then compare with (https://example.com/b).");
        for composed in [
            "https://example.com/start?x=2",
            "https://example.com/?d=secret",
            "https://example.com/docs",
        ] {
            match navigate(&session, composed) {
                Decision::Refuse { reason } => {
                    assert_eq!(reason, COMPOSED_URL_REFUSAL);
                    assert!(!reason.contains("secret"));
                }
                other => panic!("{composed}: {other:?}"),
            }
        }
        // The origin check still comes first.
        assert!(
            matches!(navigate(&session, "https://collect.example/"), Decision::Refuse { reason } if reason.contains("blocked origin"))
        );
        // An observed link target and a page URL become given.
        page(&mut session);
        assert!(navigate(&session, "https://example.com/docs").is_allow());
        session.saw_page(
            Some("https://example.com/landing?ok=1"),
            &["Search  the   archive", "Results"],
        );
        assert!(navigate(&session, "https://example.com/landing?ok=1").is_allow());

        let obs = page(&mut session);
        let filter = obs
            .elements
            .iter()
            .find(|el| el.name == "Filter")
            .unwrap()
            .ref_id
            .clone();
        let typed =
            |session: &Session, text: &str| session.decide(&json!({"action":"type","ref_id":filter,"text":text}));
        assert!(typed(&session, "compare with").is_allow(), "from the task");
        assert!(
            typed(&session, "the archive").is_allow(),
            "from the page, spacing collapsed"
        );
        assert!(
            matches!(typed(&session, "my api key is 123"), Decision::Handoff { reason } if reason == TYPED_TEXT_HOLD)
        );
        assert!(
            matches!(typed(&session, "The Archive"), Decision::Handoff { .. }),
            "case is kept"
        );
        // A new page replaces the text the old one showed.
        session.saw_page(Some("https://example.com/other"), &["Other page"]);
        assert!(matches!(typed(&session, "the archive"), Decision::Handoff { .. }));
    }
}
