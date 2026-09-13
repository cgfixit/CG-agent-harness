---
name: cgagentharness-doc-sync
description: |
  Verify that documentation (AGENTS.md, INVARIANTS.md, README.md, config.default.yaml, and inline code comments) stays in sync with actual code changes. Run this whenever making code changes that could affect documented behavior, configuration contracts, shim actions, API routes, or architectural invariants. Detects hardcoded values that should live in config, missing route documentation, undocumented gate names, stale invariant claims, and config drift. Essential for keeping truth sources aligned — code > config > INVARIANTS.md > AGENTS.md > README.md.
compatibility: |
  Requires: ripgrep, git (for diff context)
  Context: Cargo project with INVARIANTS.md, AGENTS.md, README.md, assets/config.default.yaml
---

# Documentation drift check

Read code, `assets/config.default.yaml`, `INVARIANTS.md`, `AGENTS.md`, README and
setup guide in that order. Check all relevant callers and current configuration;
old acceptance records are historical evidence, not current contracts.

## Current contracts

- The combined repository write policy ships closed: master, deepagent and
  clone-write flags are false. `writes_enabled: true` alone cannot arm writes.
  Reason and explicit per-call confirmation remain required.
- Fresh `auth.enabled` and `tls.enabled` are true. Missing legacy fields stay off;
  invalid switch types refuse configuration. Fresh web settings start enabled
  with an empty URL allowlist; reads need a current grant. Existing web choices
  are retained, while missing/invalid legacy web values remain off. The generic `flag_is_true` quoted-string behavior
  does not silently disable malformed auth/TLS switches.
- Guard order: rate limit, same origin, direct loopback/no forwarding headers,
  account/RBAC, then mutation CSRF. Minimal status/login/setup still receive
  early guards. The optional harness key is metadata, never account authority.
- SQLite is authoritative after transactional legacy JSON migration. Research
  and web selections are account scoped; sessions/jobs/persona/notes are shared.
- I6 remains server/common/LLM/shim to child only through the shim whitelist.
  Do not deduplicate deliberately separated constants across that boundary.
- Preserve `__CYCLAW_CSRF_TOKEN__`, `__CYCLAW_CSP_NONCE__` and `X-CyClaw-CSRF`.
- Public CLI families are `serve`, `account`, `web`, `tls`; hidden `agentic`
  and `desktop` preserve private boundaries. New routes must appear in the
  registered inventory; count-based grep is insufficient for multiline routes.

## Verify and reconcile

```bash
cargo test --locked --test invariant_guard
cargo test --locked registered_paths_are_unique_and_cover_every_router_route
cargo test --locked --test auth_guards --test secure_portal --test security_headers
```

Use isolated homes and blank provider keys as described in `AGENTS.md`. Inspect
actual exit status, failures and skips. Do not pipe pass/fail commands to grep.
Review new constants against validated config, CLI examples against `--help`,
UI instructions against current controls and links against existing paths.
Search mirrored `.claude`/`.codex` guidance for old auth/key/TLS/web assumptions.
Update primary docs and mirrors in the same change; explicitly label historical
records rather than rewriting their past results. Dependency drift is covered
by `cgagentharness-verify-deps` and `docs/DEPENDENCIES.md`.

For core paths, include before/after invariants and test evidence in the actual
PR template. Do not weaken tests or configuration to match obsolete prose.
