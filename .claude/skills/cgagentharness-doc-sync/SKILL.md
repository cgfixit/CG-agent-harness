---
name: cgagentharness-doc-sync
description: |
  Verify that documentation (AGENTS.md, INVARIANTS.md, README.md, config.default.yaml, and inline code comments) stays in sync with actual code changes. Run this whenever making code changes that could affect documented behavior, configuration contracts, shim actions, API routes, or architectural invariants. Detects hardcoded values that should live in config, missing route documentation, undocumented gate names, stale invariant claims, and config drift. Essential for keeping truth sources aligned — code > config > INVARIANTS.md > AGENTS.md > README.md.
compatibility: |
  Requires: ripgrep, git (for diff context)
  Context: Cargo project with INVARIANTS.md, AGENTS.md, README.md, assets/config.default.yaml
---

# CGagentHarness Doc-Sync Verification

This skill verifies that your code changes stay synchronized with the five authoritative documentation sources for CG-Agent-Harness, in order of precedence:

1. **Code** — the executable source of truth
2. **assets/config.default.yaml** — every runtime tunable
3. **INVARIANTS.md** — architectural and security invariants
4. **AGENTS.md** — operating manual and expectations
5. **README.md** — high-level orientation

When code and docs disagree, code wins. This skill catches the most common sources of drift.

## What to check

### 1. Shim Actions (if touching src/shim/mod.rs or src/agentic/commands.rs)

The shim ACTIONS whitelist in `src/shim/mod.rs` must match:
- The CLI dispatch in `src/agentic/commands.rs::dispatch`
- The invariant guard whitelist in `tests/invariant_guard.rs`
- References in `AGENTS.md` under "Project Codex skills" (if new actions are documented)

**Verification:**
```bash
# ACTIONS is declared as `pub const ACTIONS: [&str; N] = [` with entries on
# following lines, so the match must read past the declaration line (-A) into
# the array body, stopping at the closing bracket.
grep -A 30 "pub const ACTIONS:.*\[&str;" src/shim/mod.rs | sed '/^\];/q' | grep -oP '"[a-z_-]+"' | sort

# Check dispatch covers all
grep "^[[:space:]]*\"" src/agentic/commands.rs | grep "=>" | grep -oP '"[a-z_-]+"' | sort

# Cross-check invariant guard's whitelist assertion the same way
grep -A 30 "ACTIONS.*\[&str;" tests/invariant_guard.rs | sed '/^\];/q' | grep -oP '"[a-z_-]+"' | sort
```

If counts don't match, a new action is missing from one location. The hyphen pattern accounts for action names like `real-repo-run`. Prefer `cargo test --test invariant_guard` over this grep when you just need a pass/fail signal — it already knows the real whitelist logic.

### 2. Config Gates (if touching assets/config.default.yaml)

Not every gate ships `false` — derive the closed set from every gate the test actually asserts, not from
memory. `tests/invariant_guard.rs::shipped_config_keeps_every_gate_closed` asserts exactly five:
- `agentic.enabled` must be `false`
- `agentic.deepagent_github.enabled` must be `false`
- `agentic.deepagent_github.allow_git_write_tools` must be `false`
- `auth.enabled` must be `false`
- `unslop.enabled` must be `false`

Two gates intentionally ship open and are NOT contract violations:
- `agentic.writes_enabled: true` — safe because the master `agentic.enabled` gate and `mode` still block execution
- `security.api_key_optional: true` (unquoted boolean) — intentionally permits direct loopback access

Every gate mentioned in code must still exist in config with a descriptive comment, and its literal value must
match what `flag_is_true` treats as on/off (quoted `"true"` is OFF).

**Verification:**
```bash
# Find all gate checks in code
grep -r "flag_is_true\|get_bool\|config\..*enabled" src/ | grep -oE '\w+\.\w+' | sort -u

# A grep can't safely disambiguate five different `enabled:` keys living
# under different YAML parents (agentic:, agentic.deepagent_github:, auth:,
# unslop:) without a real parser, and matching the wrong one is worse than
# not checking. Run the test — it's the source of truth for this list:
cargo test --test invariant_guard shipped_config_keeps_every_gate_closed

# Confirm quoted "true" is OFF (flag_is_true) — do not treat this as a violation to "fix"
grep '"true"' assets/config.default.yaml
```

If one of the five fail-closed gates above is missing or defaults to `true`, that's a contract violation.
`writes_enabled: true` or unquoted `api_key_optional: true` are NOT violations — never "fix" them to false.

### 3. API Routes (if touching src/server/routes/mod.rs)

Every new route must be added to:
- `routes/mod.rs::REGISTERED_PATHS` (so `/api/tools` reports it as wired)
- `views.rs` (if the console lists it)
- `AGENTS.md` under the route description (if it's user-facing)

A raw grep count comparison here will not match even on an unchanged tree: `.route(...)` calls that wrap onto
multiple lines aren't captured by a single-line grep, and `registered_paths()` deliberately adds five
`/api/auth/*` routes outside the `REGISTERED_PATHS` const array (see `routes/mod.rs::registered_paths`). Don't
treat a raw count mismatch as drift.

**Verification:**
```bash
# Check REGISTERED_PATHS (match array type annotation, extract all entries)
grep -A 200 "REGISTERED_PATHS.*\[&str;" src/server/routes/mod.rs | sed '/^\]/q' | grep -oE '"/api/[^"]*"' | sort -u

# The reliable check is the existing unit test in src/server/routes/mod.rs —
# it already accounts for the extra auth routes and multiline .route() calls.
# It's a lib test (in a #[cfg(test)] mod), not a tests/ integration target,
# so run it by name, not with --test:
cargo test registered_paths_are_unique_and_cover_every_router_route
```

Use the grep above only to eyeball whether a specific new route was added to the array; use the test to decide
pass/fail.

### 4. Guard Chain Order (if touching src/server/guards.rs or src/server/headers.rs)

The guard chain order in `guards.rs` must match `INVARIANTS.md` exactly:
`rate limit -> same-origin -> API key -> CSRF`

**Verification:**
```bash
# Extract guard order from code
grep -E "rate_limit|same_origin|api_key|csrf" src/server/guards.rs | head -10

# Verify against INVARIANTS.md
grep "rate limit ->" INVARIANTS.md
```

If the order differs, invariant I2 is violated.

### 5. CSRF Placeholders (if touching assets/static/harness.html or src/server/guards.rs)

The CSRF token placeholder names are contractual and must NOT change:
- `__CYCLAW_CSRF_TOKEN__` (HTML placeholder)
- `__CYCLAW_CSP_NONCE__` (CSP nonce placeholder)
- `X-CyClaw-CSRF` (header name in HTML; the Rust constant is `CSRF_HEADER` in `src/server/guards.rs`, stored
  lowercase as `"x-cyclaw-csrf"` — HTTP header names are case-insensitive, so this is not a mismatch)

**Verification:**
```bash
# Check placeholders exist in HTML
grep -c "__CYCLAW_CSRF_TOKEN__\|__CYCLAW_CSP_NONCE__" assets/static/harness.html

# Check the header name in code (guards.rs, not headers.rs) case-insensitively
grep -i "csrf_header\|x-cyclaw-csrf" src/server/guards.rs
grep -i "x-cyclaw-csrf" assets/static/harness.html
```

If the Rust constant is renamed or its value changes, `cargo test --test security_headers` catches the
browser/server mismatch — treat that test as authoritative over a manual grep count.

### 6. Hardcoded Values (if adding new constants to code)

Never hardcode values that should live in `config.default.yaml`. Check for:
- Timeouts (planner/check should be in config AND duplicated across I6 boundary with sync test)
- Port numbers (should be config, default to 8790 or specified env var)
- Path defaults (always use `CGAGENTHARNESS_HOME` or `~/.CGagentHarness`)
- Feature gate defaults (should be in config, not `const`)

**Verification:**
```bash
# Look for suspect patterns in code
grep -rn "const.*TIMEOUT\|const.*PORT\|const.*PATH" src/agentic src/server | grep -v "^[^:]*:.*//" | head -20

# Cross-check against config
grep -E "timeout|port|path" assets/config.default.yaml | head -20
```

### 7. Invariant Claims (if making architectural changes)

If your PR touches these core files, your PR body must explicitly state which invariants you preserve:
- `src/shim/mod.rs` (I6 boundary)
- `src/server/guards.rs` (guard chain order)
- `src/server/headers.rs` (CSRF contracts)
- `src/agentic/writer.rs` (preflight injection/scope/budget checks)
- `src/agentic/executor/sandbox.rs` (Seatbelt/unshare/Job Object)
- `src/agentic/workspace.rs` (clone jail)
- `assets/config.default.yaml` (fail-closed defaults)

**Verification:**
```bash
# Check PR body for invariant statement
# This is manual: does the PR description name the invariants?
# Should say something like:
# "This change preserves I6 (process isolation) by..."
# "This change preserves the guard chain order by..."
```

## Reconciliation workflow

If docs are out of sync:

1. **Code is always right.** If code and docs differ, update docs.
2. **Only the five asserted gates ship closed.** If `agentic.enabled`, `agentic.deepagent_github.enabled`,
   `agentic.deepagent_github.allow_git_write_tools`, `auth.enabled`, or `unslop.enabled` defaults to `true`,
   that's a violation — fix it. Do not change `agentic.writes_enabled` or `security.api_key_optional`; they
   intentionally ship open.
3. **Truth order precedence.** Use the ranked sources above to decide what's authoritative.
4. **Tests catch drift.** If `tests/invariant_guard.rs` fails, the code is broken, not the docs.

## Running the full check

Use this bash function to run all checks at once:

```bash
doc_sync_check() {
  echo "=== Shim Actions ==="
  grep -A 30 "pub const ACTIONS:.*\[&str;" src/shim/mod.rs | sed '/^\];/q' | grep -oP '"[a-z_-]+"' | wc -l
  grep "^[[:space:]]*\"" src/agentic/commands.rs | grep "=>" | wc -l
  # If these two counts differ, a new action is missing from one location.

  echo "=== Config Gates ==="
  grep -r "flag_is_true\|get_bool" src/ | wc -l
  grep '^\s*\w\+:' assets/config.default.yaml | grep -v '^#' | wc -l

  echo "=== API Routes (informational only — see note below) ==="
  # A raw count here will not match: registered_paths() adds 5 auth routes
  # outside REGISTERED_PATHS, and multiline .route() calls aren't grep-single-line-friendly.
  # Use the unit test below to actually verify coverage. Capture cargo's own
  # exit status separately — piping straight to grep would let a printed
  # "FAILED" line succeed (grep found its match) and mask the real failure.
  route_test_output=$(cargo test registered_paths_are_unique_and_cover_every_router_route 2>&1)
  route_test_status=$?
  echo "$route_test_output" | grep -E "test result|FAILED"
  [ "$route_test_status" -eq 0 ] && echo "✓ PASS" || echo "✗ FAIL: route coverage regressed"

  echo "=== CSRF Placeholders ==="
  grep -l "__CYCLAW_CSRF_TOKEN__" assets/static/harness.html
  grep -il "x-cyclaw-csrf" src/server/guards.rs assets/static/harness.html

  echo "=== Guard Chain ==="
  grep -E "rate_limit|same_origin|api_key|csrf" src/server/guards.rs | head -5

  echo "=== No hardcoded values ==="
  grep -rn "const.*\(TIMEOUT\|PORT\|PATH\)" src/agentic src/server | grep -v "//" | wc -l
}

doc_sync_check
```

If counts diverge or grep finds issues, file a finding. Otherwise, you're good.
