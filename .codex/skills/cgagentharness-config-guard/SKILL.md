---
name: cgagentharness-config-guard
description: Check shipped Harness defaults, missing-key behavior and fail-closed gates before config changes or a defaults audit.
---

# Config guard

Compare `assets/config.default.yaml`, the consuming accessor and the startup
or reload path. Test a fresh home, a missing key and an explicit value.
Seeding leaves existing config files intact.

| Contract | Evidence |
|---|---|
| Literal switches | `flag_is_true` accepts only YAML boolean true. `flag_missing_on` treats absence as on and refuses malformed values for `MISSING_MEANS_ON` keys. `tests/common_layer.rs::quoted_true_is_off` and `src/common/config.rs` unit tests. |
| Strict load | `AppConfig::load`, `FLOAT_KEYS`, `numeric_config_reads_are_type_checked_at_load`, `security_switches_require_boolean_values`. Verify missing numbers retain defaults and malformed values fail. |
| Shipped switches | `tests/invariant_guard.rs::shipped_config_enforces_accounts_tls_and_keeps_execution_gates_closed` asserts literal booleans. Check first-run and existing-home seeding in `tests/common_layer.rs`. |
| Writes and input protection | Use `cgagentharness-write-policy-redteam` for reason, confirm, revocation and the disable-only kill switch. Find the `shipped_defaults_protect_` and `shipped_defaults_deny_` filters in `tests/invariant_guard.rs`. |
| Network and credentials | Follow the bind in `src/server/mod.rs`, API-key storage in `INVARIANTS.md`, and model URL and locality checks in `src/llm`. |
| Restart-only gates | Check `src/server/config_reload.rs::RELOADABLE` and `src/server/routes/browser.rs` for browser and takeover gates. `web_units::Ceilings::load` requires an enabled Firecrawl daily ceiling to cover one read's PDF reservation. |
| Listing reads | `src/server/web_provider.rs::ResultReads` uses `flag_missing_on` for the parent, literal true for `arxiv_pdf`. Exercise their missing-key cases independently. |
| Memory persistence | Check saved choices and pending proposals in `tests/common_layer.rs` and `tests/structured_memory_suggestions.rs`. |

Use the pinned rustup toolchain and select affected filters, for example:

```sh
GROK_API_KEY='' ANTHROPIC_API_KEY='' DEEPAGENT_API_KEY='' \
  cargo test --locked --test common_layer quoted_true_is_off -- --exact
```

Exercise the affected refusals. Run `python3 scripts/lint-diff.py` (CG009)
for shipped-value claims. Follow `AGENTS.md` for invariant statements.
