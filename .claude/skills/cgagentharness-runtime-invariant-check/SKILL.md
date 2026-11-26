---
name: cgagentharness-runtime-invariant-check
description: |
  Report-only walkthrough of the runtime invariants before pushing a core-path PR (src/shim, guards.rs, headers.rs, routes/mod.rs, authz.rs, writer.rs, sandbox.rs, workspace.rs, config.default.yaml): I6 isolation, request guards and roles, browser argv, shipped gates, write policy and kill switch, web evidence, /browse takeover binding. Maps each boundary to the tests that lock it.
compatibility: |
  Requires: ripgrep, cargo test infrastructure
  Context: Rust codebase with INVARIANTS.md, tests/invariant_guard.rs, and security-critical modules
---

# Runtime invariant verification

Read current `INVARIANTS.md` and source first, not old guard diagrams. This
checks the supplied implementation; it authorizes no policy change.

| Boundary | Current contract | Runnable evidence |
|---|---|---|
| I6 | Server/common/LLM/shim never call pipeline code; only shim spawns the whitelisted child | `cargo test --locked --test invariant_guard` |
| Requests | Exact origin, direct loopback/no proxy, rate, mutation CSRF; account roles resolve through `routes::ROUTE_CAPS` and `common::authz`, an unlisted route or method is denied, and a handler role check calls `authz` or is pinned | `cargo test --locked --test auth_guards --test secure_portal --test security_headers`; `cargo test --locked --lib -- route_cap authz`; `cargo test --locked --test invariant_guard account_role_comparisons_stay_pinned_to_authz` |
| Browser arguments | Fixed check-profile names, bounded arguments, no arbitrary command; reason/confirm explicit | `cargo test --locked --test shim_and_agent_routes` |
| Default configuration | Master/deepagent/clone-write gates false; fresh auth/TLS and web settings true, URL allowlist seeded with only `https://www.google.com/*`; existing choices retained; quoted security switches invalid | `cargo test --locked --test invariant_guard --test common_layer` |
| Mutation policy | Reloaded write gates, clone jail, exact edits and reviewed-tree approval before commit/push/publication; the env kill switch only disables | `cargo test --locked --test write_policy --test real_repo_loop --test write_kill_switch` |
| `/browse` takeover | Administrator's own job; one held request sent only with its id, hash, `confirm` and `reason`, after both gates and the block list are rechecked | `cargo test --locked --test secure_portal --test auth_guards approving_a_held`; `cargo test --locked --lib browser_jobs`; the `browser takeover` fixture (CI, non-root Chrome) |
| Web evidence | Exact/wildcard permission, checked DNS pinning, bounded retrieval and current-policy revocation | `cargo test --locked --test panels --test web_research` |
| Native TLS | Owned handshake, exact certificate/origin and platform trust validation | Desktop tests plus actual native acceptance recorded in the PR body |

The optional harness API key grants no authority and cannot bypass login.
`writes_enabled: true` and mode `write` do not arm the combined coding policy.
Public login/minimal status still get early guards. Web selection, chat sessions,
detached jobs and schedules are private to the account (unassigned legacy records
stay quarantined until explicit adoption); persona, pinned notes, model selection,
spend and agentic run records remain shared portal resources.

Run with isolated homes and provider credentials excluded. Check Cargo's own
exit status and every failure/skip. Required native sandbox tests may need the
operator's normal terminal rather than an outer restricted sandbox; never weaken
the application profile. Backend/fixture tests do not prove native GUI or live
model behavior. Record tested source, commands, outcomes and unresolved limits.
