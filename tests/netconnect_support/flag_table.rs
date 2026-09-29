//! Boolean combinations of the netconnect master switch, one tier flag, and scope.
//!
//! The tier list is [`Tier::ALL`]. There is no second flag list in this file.
//! PR B (tool registration / wired-count per combination) should include this
//! module and iterate [`flag_cases`]:
//!
//! ```ignore
//! #[path = "netconnect_support/flag_table.rs"]
//! mod flag_table;
//! ```

use cgagentharness::netconnect::Tier;

/// One master × one tier flag × empty-or-not scope.
///
/// Other tier flags are left unset in [`FlagCase::yaml`], so they stay off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlagCase {
    pub tier: Tier,
    pub master: bool,
    pub tier_on: bool,
    pub scope_nonempty: bool,
}

impl FlagCase {
    /// The predicate PR B can call. Tests recompute it from the three booleans
    /// and do not treat this method as the source of truth.
    pub fn may_run(self) -> bool {
        self.master && self.tier_on && self.scope_nonempty
    }

    /// YAML that arms only `tier`. `allowed_cidrs` is `[]` or one private /24.
    pub fn yaml(self) -> String {
        let master = if self.master { "true" } else { "false" };
        let flag = if self.tier_on { "true" } else { "false" };
        let cidrs = if self.scope_nonempty {
            "['192.168.1.0/24']"
        } else {
            "[]"
        };
        format!(
            "netconnect:\n  enabled: {master}\n  {}: {flag}\n  allowed_cidrs: {cidrs}\n",
            self.tier.key()
        )
    }
}

/// Every combination of master, that tier's flag, and empty vs non-empty scope.
///
/// `Tier::ALL` is the only tier list. Adding a tier grows this table.
pub fn flag_cases() -> Vec<FlagCase> {
    let mut cases = Vec::with_capacity(Tier::ALL.len() * 8);
    for tier in Tier::ALL {
        for master in [false, true] {
            for tier_on in [false, true] {
                for scope_nonempty in [false, true] {
                    cases.push(FlagCase {
                        tier,
                        master,
                        tier_on,
                        scope_nonempty,
                    });
                }
            }
        }
    }
    cases
}

/// Quoted `"true"` is not a distinct arming state. One row per tier, generated
/// from [`Tier::ALL`], for the master and for that tier's flag.
pub fn quoted_true_yaml(tier: Tier, master_quoted: bool, flag_quoted: bool) -> String {
    let master = if master_quoted { "\"true\"" } else { "true" };
    let flag = if flag_quoted { "\"true\"" } else { "true" };
    format!(
        "netconnect:\n  enabled: {master}\n  {}: {flag}\n  allowed_cidrs: ['192.168.1.0/24']\n",
        tier.key()
    )
}
