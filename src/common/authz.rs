//! Role to capability authorization: the one place stored role strings become
//! access decisions (#965). The gate resolves a request to a [`Capability`]
//! through `server::routes::ROUTE_CAPS` and asks [`grants`] whether the
//! caller's [`Role`] holds it. An unlisted route and an unknown stored role
//! both fail closed.

/// Account roles as stored in the account database. Parsed once per request;
/// unknown stored values fail closed instead of inheriting operator access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Admin,
    Operator,
    Auditor,
}

impl Role {
    pub fn parse(stored: &str) -> Option<Self> {
        match stored {
            "admin" => Some(Self::Admin),
            "operator" => Some(Self::Operator),
            super::authn::AUDITOR_ROLE => Some(Self::Auditor),
            _ => None,
        }
    }
}

/// What an authenticated account may do on a route. One variant per access
/// class the current policy distinguishes; `server::routes::ROUTE_CAPS` maps
/// every registered `(method, route template)` to exactly one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// No account required: status, setup-status, login and the static assets.
    Public,
    /// Every authenticated role: whoami, own password change, logout.
    OwnAccount,
    /// Session list, create and single-session read, the surface the
    /// `auditor` chat is allowed to see.
    OwnSessions,
    /// Everything else owner-scoped: session extras, structured memory,
    /// notes corpus, spend, loops, jobs and notifications.
    OwnData,
    /// Local chat turns, cancel, prompt preview and slash parse.
    Chat,
    /// Attachment upload, withheld from `auditor`.
    ChatAttachments,
    /// Web fetch, search, research and inject within the shared URL policy.
    WebUse,
    /// Declared MCP status and tool calls; a call additionally needs the
    /// per-server authority check (`mcp_server_allowed`) once the server name
    /// resolves in the handler.
    UseMcp,
    /// Propose a persona (soul) edit; only admin applies one
    /// (`persona::decide`).
    ProposePersona,
    /// Re-send an owner-scoped notification delivery: an outbound effect, so
    /// admin-only despite the owner scoping (#965 target table).
    NotificationReplay,
    /// Agent runs, decisions, push/publish, jobs, schedules and checks.
    RunCoding,
    /// Global model selection, Ollama inventory, profile and pulls.
    ManageModels,
    /// Shared content: persona document and proposals, memory and soul and
    /// style toggles, pinned-note add and clear, unassigned legacy session
    /// inventory and adoption.
    ManageSharedContent,
    /// Web policy: enable, allow, deny, mode, block and unblock.
    ManageWebPolicy,
    /// Provider keys.
    ManageCredentials,
    /// Account create, disable, password, role and delete.
    ManageAccounts,
    /// Config reload and structured-memory gates.
    ManageConfig,
    /// Browser tasks (`/api/browser/*`); with accounts off the handler still
    /// refuses these (`BROWSER_AUTH_REQUIRED`, #964).
    BrowserUse,
    /// The redacted audit read.
    ReadAudit,
    /// `/diag` status, bounded reads and excerpt staging.
    ReadLogs,
    /// The read-only log-investigation tools in local chat (`chat_diag`), on
    /// top of `ReadLogs` and only when `diagnostics.model_tool` is literal
    /// true. No route maps here; the tool loop checks it per batch (#965).
    InvestigateLogs,
    /// Panel and inventory reads: registry, tools, skills, netconnect.
    ReadPanels,
}

impl Capability {
    /// Every capability in declaration order: the iteration domain for
    /// [`capabilities_for`]. Adding a variant fails compilation in `grants`
    /// and here until both are decided.
    pub const ALL: [Capability; 22] = [
        Capability::Public,
        Capability::OwnAccount,
        Capability::OwnSessions,
        Capability::OwnData,
        Capability::Chat,
        Capability::ChatAttachments,
        Capability::WebUse,
        Capability::UseMcp,
        Capability::ProposePersona,
        Capability::NotificationReplay,
        Capability::RunCoding,
        Capability::ManageModels,
        Capability::ManageSharedContent,
        Capability::ManageWebPolicy,
        Capability::ManageCredentials,
        Capability::ManageAccounts,
        Capability::ManageConfig,
        Capability::BrowserUse,
        Capability::ReadAudit,
        Capability::ReadLogs,
        Capability::InvestigateLogs,
        Capability::ReadPanels,
    ];

    /// The wire name the console keys on, so it can hide controls without
    /// hard-coding role names (#965). Stable: tests pin every name.
    pub fn name(self) -> &'static str {
        match self {
            Capability::Public => "public",
            Capability::OwnAccount => "own_account",
            Capability::OwnSessions => "own_sessions",
            Capability::OwnData => "own_data",
            Capability::Chat => "chat",
            Capability::ChatAttachments => "chat_attachments",
            Capability::WebUse => "web_use",
            Capability::UseMcp => "use_mcp",
            Capability::ProposePersona => "propose_persona",
            Capability::NotificationReplay => "notification_replay",
            Capability::RunCoding => "run_coding",
            Capability::ManageModels => "manage_models",
            Capability::ManageSharedContent => "manage_shared_content",
            Capability::ManageWebPolicy => "manage_web_policy",
            Capability::ManageCredentials => "manage_credentials",
            Capability::ManageAccounts => "manage_accounts",
            Capability::ManageConfig => "manage_config",
            Capability::BrowserUse => "browser_use",
            Capability::ReadAudit => "read_audit",
            Capability::ReadLogs => "read_logs",
            Capability::InvestigateLogs => "investigate_logs",
            Capability::ReadPanels => "read_panels",
        }
    }
}

/// The capability names `role` holds, reported by whoami/login so the console
/// hides controls the caller would only see refused (#965). The server stays
/// authoritative; this is presentation, derived from the same `grants` matrix.
pub fn capabilities_for(role: Role) -> Vec<&'static str> {
    Capability::ALL
        .into_iter()
        .filter(|cap| grants(role, *cap))
        .map(Capability::name)
        .collect()
}

/// [`capabilities_for`] for a stored role string; an unparseable value fails
/// closed to no capabilities, matching the gate's refusal of the same string.
pub fn capabilities_for_stored(stored: &str) -> Vec<&'static str> {
    Role::parse(stored).map(capabilities_for).unwrap_or_default()
}

/// [`grants`] for a stored role string, for the mid-turn re-checks that read
/// a fresh account record (chat_diag): an unparseable value fails closed.
pub fn grants_stored(stored: &str, capability: Capability) -> bool {
    Role::parse(stored).is_some_and(|role| grants(role, capability))
}

/// Whether a stored role string is exactly `role`, for checks that are about
/// role identity rather than a granted capability (the last-admin fold, the
/// schema 3 widening probe): an unparseable value fails closed. Keeping the
/// comparison here leaves `Role::parse` behind the gate (#965).
pub fn is_role(stored: &str, role: Role) -> bool {
    Role::parse(stored) == Some(role)
}

/// Whether the role's surface is method-exact. The restricted role
/// (`auditor`) is refused on any unlisted `(method, route)` pair,
/// while admin and operator keep the router's 405 on a template they fully
/// hold — the legacy gate made the same distinction.
pub fn denies_wrong_method(role: Role) -> bool {
    matches!(role, Role::Auditor)
}

/// The chat context a role's turns may include, decided once per request from
/// the account's role instead of scattered role string comparisons (#965).
/// `chat_inner` and `persona::preview` both build it, so the real turn and
/// its preview withhold identically.
#[derive(Debug, Clone, Copy)]
pub struct ChatPolicy {
    /// Pinned memory, retrieval, selections and episode/suggestion staging.
    pub memory: bool,
    /// The shared persona (soul) is injected.
    pub persona: bool,
    /// Attachment and notes-corpus bytes, including pinned blobs, reach the
    /// prompt.
    pub attachments: bool,
    /// Selected skills are resolved into the prompt.
    pub skills: bool,
    /// Web chat mode, web context and fetched-source carry-forward.
    pub web: bool,
    /// Cloud model selections are accepted.
    pub cloud: bool,
    /// `/loop` turns are accepted.
    pub loops: bool,
    /// Per-request model overrides are accepted.
    pub model_choice: bool,
    /// The live-state block (home path, soul state) is included.
    pub live_state: bool,
}

impl ChatPolicy {
    /// Every context surface: admin, operator and the accounts-off local user.
    pub const FULL: Self = Self {
        memory: true,
        persona: true,
        attachments: true,
        skills: true,
        web: true,
        cloud: true,
        loops: true,
        model_choice: true,
        live_state: true,
    };
    /// Local chat only: the selected local model, no other surface.
    pub const LOCAL_ONLY: Self = Self {
        memory: false,
        persona: false,
        attachments: false,
        skills: false,
        web: false,
        cloud: false,
        loops: false,
        model_choice: false,
        live_state: false,
    };

    pub fn for_role(role: Role) -> Self {
        match role {
            Role::Admin | Role::Operator => Self::FULL,
            Role::Auditor => Self::LOCAL_ONLY,
        }
    }

    /// The policy for the requesting account. With accounts off there is one
    /// local user with the full surface; an unparseable stored role fails
    /// closed to the withheld surface (the gate refuses it first).
    pub fn for_user(user: Option<&super::auth_store::UserSummary>) -> Self {
        match user {
            Some(account) => Role::parse(&account.role)
                .map(Self::for_role)
                .unwrap_or(Self::LOCAL_ONLY),
            None => Self::FULL,
        }
    }
}

/// The whole role × capability matrix, exhaustive on purpose: adding a role
/// or capability fails compilation here until its grant is decided. Admin
/// holds every capability, present and future ("the administrator can do
/// everything the harness allows" — #965); the other roles list every
/// variant explicitly.
pub fn grants(role: Role, capability: Capability) -> bool {
    use Capability::*;
    match role {
        Role::Admin => true,
        Role::Operator => match capability {
            Public | OwnAccount | OwnSessions | OwnData | Chat | ChatAttachments | WebUse | UseMcp | RunCoding
            | ProposePersona | ReadPanels => true,
            ManageModels | ManageSharedContent | ManageWebPolicy | ManageCredentials | ManageAccounts
            | ManageConfig | BrowserUse | ReadAudit | ReadLogs | InvestigateLogs | NotificationReplay => false,
        },
        Role::Auditor => match capability {
            Public | OwnAccount | OwnSessions | Chat | ReadAudit | ReadLogs | InvestigateLogs => true,
            OwnData | ChatAttachments | WebUse | UseMcp | RunCoding | ManageModels | ManageSharedContent
            | ManageWebPolicy | ManageCredentials | ManageAccounts | ManageConfig | BrowserUse | ReadPanels
            | ProposePersona | NotificationReplay => false,
        },
    }
}

/// MCP server authority derived once from the declaration: a stdio server is
/// read-only when it declares no write roots, `network: deny` and
/// non-`job_object` containment; anything else — and every SSE server, which
/// cannot be confined locally — is privileged (#965).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAuthority {
    ReadOnly,
    Privileged,
}

/// Whether `role` may call a server of `authority`. An `operator_allowed:
/// true` declaration widens a privileged server to operators; auditors never
/// call MCP tools and already lack `UseMcp` at the gate.
pub fn mcp_server_allowed(role: Role, authority: McpAuthority, operator_allowed: bool) -> bool {
    match role {
        Role::Admin => true,
        Role::Operator => authority == McpAuthority::ReadOnly || operator_allowed,
        Role::Auditor => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_the_three_stored_roles_and_fails_closed() {
        assert_eq!(Role::parse("admin"), Some(Role::Admin));
        assert_eq!(Role::parse("operator"), Some(Role::Operator));
        assert_eq!(Role::parse("auditor"), Some(Role::Auditor));
        // Stored roles are canonical lowercase; the retired `audit` and
        // `portal_auditor` are migrated on open and must never parse, and
        // anything else is corrupt or forged and must not widen into
        // operator-style access.
        for stored in [
            "",
            "Admin",
            "OPERATOR",
            "root",
            "portal-auditor",
            "audit",
            "portal_auditor",
        ] {
            assert_eq!(Role::parse(stored), None, "{stored}");
        }
    }

    #[test]
    fn is_role_matches_exactly_and_fails_closed() {
        assert!(is_role("admin", Role::Admin));
        assert!(!is_role("admin", Role::Operator));
        assert!(is_role("auditor", Role::Auditor));
        for stored in ["", "Admin", "root", "portal_auditor"] {
            assert!(!is_role(stored, Role::Admin), "{stored}");
            assert!(!is_role(stored, Role::Auditor), "{stored}");
        }
    }

    #[test]
    fn admin_only_capabilities_are_refused_to_every_other_role() {
        for cap in [
            Capability::ManageModels,
            Capability::ManageSharedContent,
            Capability::ManageWebPolicy,
            Capability::ManageCredentials,
            Capability::ManageAccounts,
            Capability::ManageConfig,
            Capability::BrowserUse,
            Capability::NotificationReplay,
        ] {
            assert!(grants(Role::Admin, cap));
            for role in [Role::Operator, Role::Auditor] {
                assert!(!grants(role, cap), "{role:?} must not hold {cap:?}");
            }
        }
    }

    /// The #965 target table: operator keeps day-to-day own-data work and may
    /// propose persona edits, but holds no shared-state, credential, account,
    /// config, browser, model, audit or log authority.
    #[test]
    fn operator_is_narrowed_to_the_target_table() {
        for cap in [
            Capability::OwnAccount,
            Capability::OwnSessions,
            Capability::OwnData,
            Capability::Chat,
            Capability::ChatAttachments,
            Capability::WebUse,
            Capability::UseMcp,
            Capability::RunCoding,
            Capability::ProposePersona,
            Capability::ReadPanels,
        ] {
            assert!(grants(Role::Operator, cap), "operator keeps {cap:?}");
        }
    }

    #[test]
    fn capability_names_are_unique_and_match_the_grants_matrix() {
        let mut names: Vec<&str> = Capability::ALL.iter().map(|c| c.name()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "capability names must be unique");
        for role in [Role::Admin, Role::Operator, Role::Auditor] {
            let reported = capabilities_for(role);
            assert!(!reported.is_empty(), "a known role holds some capabilities");
            for cap in Capability::ALL {
                assert_eq!(reported.contains(&cap.name()), grants(role, cap), "{role:?} {cap:?}");
            }
        }
        assert!(capabilities_for_stored("forged").is_empty());
        assert!(
            capabilities_for_stored("portal_auditor").is_empty(),
            "a retired role fails closed like any unparseable value"
        );
    }

    #[test]
    fn mcp_authority_is_admin_only_unless_read_only_or_widened() {
        use McpAuthority::*;
        assert!(mcp_server_allowed(Role::Admin, ReadOnly, false));
        assert!(mcp_server_allowed(Role::Admin, Privileged, false));
        assert!(mcp_server_allowed(Role::Operator, ReadOnly, false));
        assert!(!mcp_server_allowed(Role::Operator, Privileged, false));
        assert!(mcp_server_allowed(Role::Operator, Privileged, true));
        assert!(!mcp_server_allowed(Role::Auditor, ReadOnly, false));
        assert!(!mcp_server_allowed(Role::Auditor, Privileged, true));
    }
}
