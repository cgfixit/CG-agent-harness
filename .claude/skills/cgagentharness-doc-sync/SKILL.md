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
# Extract ACTIONS from shim (match type annotation)
grep "ACTIONS.*\[&str;" src/shim/mod.rs | grep -oP '"[a-z_-]+"' | sort

# Check dispatch covers all
grep "^[[:space:]]*\"" src/agentic/commands.rs | grep "=>" | grep -oP '"[a-z_-]+"' | sort

# Cross-check invariant guard
grep "ACTIONS.*\[&str;" tests/invariant_guard.rs -A 30 | grep -oP '"[a-z_-]+"' | sort
```

If counts don't match, a new action is missing from one location. The hyphen pattern accounts for action names like `real-repo-run`.

### 2. Config Gates (if touching assets/config.default.yaml)

Every gate mentioned in code must:
- Exist in `assets/config.default.yaml`
- Have a descriptive inline comment
- Ship with `false` or `"false"` (fail-closed)
- Match its `flag_is_true` check in code

**Verification:**
```bash
# Find all gate checks in code
grep -r "flag_is_true\|get_bool\|config\..*enabled" src/ | grep -oE '\w+\.\w+' | sort -u

# Verify each exists in config with "false" default
for gate in agentic.enabled mode writes_enabled; do
  echo "Checking $gate:"
  grep -A 2 "^\s*$gate:" assets/config.default.yaml
done

# Verify quoted "true" is OFF
grep '"true"' assets/config.default.yaml
```

If any gate is missing or defaults to `true`, that's a contract violation.

### 3. API Routes (if touching src/server/routes/mod.rs)

Every new route must be added to:
- `routes/mod.rs::REGISTERED_PATHS` (so `/api/tools` reports it as wired)
- `views.rs` (if the console lists it)
- `AGENTS.md` under the route description (if it's user-facing)

**Verification:**
```bash
# Extract routes from handlers
grep -r "Router::new\|\.route\|\.post\|\.get" src/server/routes/ | grep -oE '"(/api/[^"]+)"' | sort -u

# Check REGISTERED_PATHS (match array type annotation, extract all entries)
grep -A 200 "REGISTERED_PATHS.*\[&str;" src/server/routes/mod.rs | sed '/^\]/q' | grep -oE '"/api/[^"]*"' | sort -u

# Count should match
```

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

### 5. CSRF Placeholders (if touching assets/static/harness.html or src/server/headers.rs)

The CSRF token placeholder names are contractual and must NOT change:
- `__CYCLAW_CSRF_TOKEN__` (HTML placeholder)
- `__CYCLAW_CSP_NONCE__` (CSP nonce placeholder)
- `X-CyClaw-CSRF` (header name in code and HTML)

**Verification:**
```bash
# Check placeholders exist in HTML
grep -c "__CYCLAW_CSRF_TOKEN__\|__CYCLAW_CSP_NONCE__" assets/static/harness.html

# Check header name matches
grep "X-CyClaw-CSRF" src/server/headers.rs assets/static/harness.html | wc -l
# Should be consistent (2+ matches)
```

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
2. **Config defaults ship closed.** If a gate has `true` as default, change it to `false`.
3. **Truth order precedence.** Use the ranked sources above to decide what's authoritative.
4. **Tests catch drift.** If `tests/invariant_guard.rs` fails, the code is broken, not the docs.

## Running the full check

Use this bash function to run all checks at once:

```bash
doc_sync_check() {
  echo "=== Shim Actions ==="
  grep -A 30 "ACTIONS = \[" src/shim/mod.rs | grep -oP '"[a-z_]+"' | wc -l
  grep "^[[:space:]]*\"" src/agentic/commands.rs | grep "=>" | wc -l

  echo "=== Config Gates ==="
  grep -r "flag_is_true\|get_bool" src/ | wc -l
  grep '^\s*\w\+:' assets/config.default.yaml | grep -v '^#' | wc -l

  echo "=== API Routes ==="
  grep -r "Router::new\|\.route\|\.post\|\.get" src/server/routes/ | grep -oE '"(/api/[^"]+)"' | sort -u | wc -l
  grep -A 100 "REGISTERED_PATHS = \[" src/server/routes/mod.rs | grep -oE '"/api/[^"]*"' | sort -u | wc -l

  echo "=== CSRF Placeholders ==="
  grep -l "__CYCLAW_CSRF_TOKEN__" assets/static/harness.html src/server/headers.rs

  echo "=== Guard Chain ==="
  grep -E "rate_limit|same_origin|api_key|csrf" src/server/guards.rs | head -5

  echo "=== No hardcoded values ==="
  grep -rn "const.*\(TIMEOUT\|PORT\|PATH\)" src/agentic src/server | grep -v "//" | wc -l
}

doc_sync_check
```

If counts diverge or grep finds issues, file a finding. Otherwise, you're good.
