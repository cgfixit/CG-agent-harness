---
name: cgagentharness-config-guard
description: Check shipped Harness configuration defaults and fail-closed gate semantics before config changes or a requested defaults audit.
---

# Config guard

Assess `assets/config.default.yaml` against current code and `INVARIANTS.md`.
This is a read-oriented assessment; corrections need an authorized change set.
Use `cgagentharness-invariant-guard` for implementation and topology review.

## Contracts

Preserve these check IDs when reporting. A broken contract is FAIL; an
unexecuted check is NOT RUN, not a merge recommendation.

| ID | Check and authoritative evidence |
|---|---|
| H1 | `flag_is_true` accepts only literal YAML `true`; missing keys and strings stay off. `tests/common_layer.rs::quoted_true_is_off`. |
| H2 | Read the complete closed-gate list in `tests/invariant_guard.rs::shipped_config_enforces_accounts_tls_and_keeps_execution_gates_closed`: execution, unslop, diagnostics, outbound/inbound MCP, netconnect, notifications and repository retrieval stay off; inbound tools and netconnect scope stay empty. Auth/TLS ship on. Fresh web settings seed on with no content grants; existing/legacy choices follow `home_layout_seeds_config_registry_and_skills_once` and `existing_web_settings_preserve_choices_and_keep_legacy_values_off` in `tests/common_layer.rs`. |
| H3 | The shipped-config test also locks literal YAML needles, including `allow_plaintext_key_file: false`, and keeps that opt-in restart-only. `security.api_key_optional` is deprecated metadata, not account authority. |
| H4 | Writes require human reason and per-call confirm; inspect writer, `tests/real_repo_loop.rs` and shim route assertions. |
| H5 | `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` only disables; `EXECUTION_ENABLED` cannot be flipped to fake a passing refusal. `tests/write_kill_switch.rs`. |
| H6 | Serve remains loopback-only; distinguish its bind policy in `src/server/mod.rs` from model endpoint validation in `src/llm`. |
| H7 | Mode/write-enabled alone grants no mutation: fresh disk policy, master/deepagent/clone-write gates and separate intents remain required. `tests/write_policy.rs`. |
| H8 | The shipped-config test locks the prompt-filter pattern count; inspect its current assertion rather than copying the count here. |
| H9 | `shipped_defaults_protect_agent_instruction_files` and `shipped_defaults_deny_sensitive_read_basenames` retain protected instructions and sensitive-read exclusions. |
| H10 | Auth/TLS reject invalid switch types instead of treating them as generic off gates. `tests/common_layer.rs::security_switches_require_boolean_values`. |
| H11 | The shipped-config test locks pinned/structured-memory defaults. Saved explicit choices win; proposals remain pending until reviewed. Use `tests/common_layer.rs` and `tests/structured_memory_suggestions.rs` for persistence and proposal behavior. |

## Execute and interpret

With the root rustup toolchain, run the affected filters; these examples select
shipped defaults and the two distinct boolean contracts:

```sh
GROK_API_KEY='' ANTHROPIC_API_KEY='' DEEPAGENT_API_KEY='' \
  cargo test --locked --test invariant_guard shipped_ -- --nocapture
GROK_API_KEY='' ANTHROPIC_API_KEY='' DEEPAGENT_API_KEY='' \
  cargo test --locked --test common_layer -- quoted_true_is_off security_switches_require_boolean_values --nocapture
```

Confirm the output names the intended tests. For changed gates, inspect their
behavioral assertions and exercise the relevant refusal path; a static default
check does not prove runtime enforcement. Follow `AGENTS.md` for scoped checks.

Report H1–H11 with commands, results, source SHA, deliberate default changes and
remaining gaps. Re-tuning requires matching code, comments and contract updates
with an explicit PR invariant statement. Keep valid assertions and security
boundaries; isolate environment failures before attributing a product defect.
