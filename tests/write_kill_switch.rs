//! Dedicated process: env mutations here must not race other cargo-test binaries.
//!
//! Proves the operator emergency write kill-switch still disables execution, and
//! that clearing it restores the shipped `EXECUTION_ENABLED=true` path used by
//! write-gate tests. Does not change production defaults.

use cgagentharness::agentic::writer::{disabled_by_env, env_value_disables, execution_enabled, WRITE_DISABLE_ENV};

#[test]
fn operator_kill_switch_still_disables_and_isolation_restores_shipped_execution() {
    std::env::set_var(WRITE_DISABLE_ENV, "1");
    assert!(disabled_by_env(), "exporting {WRITE_DISABLE_ENV}=1 must disable writes");
    assert!(
        !execution_enabled(),
        "kill switch is AND-ed with EXECUTION_ENABLED; it cannot be bypassed"
    );

    std::env::remove_var(WRITE_DISABLE_ENV);
    assert!(
        !disabled_by_env(),
        "cargo test must drop the operator kill switch so later gates are observable"
    );
    assert!(
        execution_enabled(),
        "shipped EXECUTION_ENABLED is true; isolation must not flip the const"
    );
}

#[test]
fn kill_switch_parse_table_is_disable_only() {
    for (raw, disables) in [
        ("1", true),
        ("true", true),
        ("YES", true),
        ("on", true),
        ("0", false),
        ("", false),
        ("false", false),
        ("maybe", false),
    ] {
        assert_eq!(env_value_disables(raw), disables, "{raw}");
    }
}
