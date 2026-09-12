---
name: cgagentharness-runtime-invariant-check
description: |
  Verify that core security invariants still hold after code changes. Run this before pushing core-path PRs (touching src/shim, guards.rs, headers.rs, writer.rs, sandbox.rs, workspace.rs, or config.default.yaml). Checks: I6 process isolation (server never imports agentic), guard chain order, CSRF contracts, write gates closed by default, RUN_ID_PATTERN consistency across boundary, clone jail contracts, and config fail-closed defaults. Complements tests/invariant_guard.rs by providing human-readable guidance on what to check and how to fix violations.
compatibility: |
  Requires: ripgrep, cargo test infrastructure
  Context: Rust codebase with INVARIANTS.md, tests/invariant_guard.rs, and security-critical modules
---

# CGagentHarness Runtime Invariant Check

This skill verifies that changes to core security code preserve the invariants that make CG-Agent-Harness safe. It complements the automated test `tests/invariant_guard.rs` by explaining what each invariant means, why it matters, and how to fix violations.

## The five core invariants

### I6: Process isolation (server ≠ agentic)

**The rule:** The HTTP console (`src/server`, `src/shim`, `src/llm`) never imports or calls code from the agentic pipeline (`src/agentic`). The only edge is `src/shim/mod.rs`, which spawns the agentic binary as a **child process** with a hard timeout.

**Why:** A SIGKILL'd child can leak a clone but cannot corrupt the server's memory, bypass its guard chain, or escalate privilege. Process boundaries are harder to violate than module imports.

**How to check:**

```bash
# Check all four console-side modules (server, shim, llm, common) for any agentic reference
for module in server shim llm common; do
  echo "=== Checking src/$module ==="
  if grep -r "crate::agentic\|use.*agentic" src/$module --include="*.rs" 2>/dev/null; then
    echo "✗ FAIL: Found agentic reference in $module"
  else
    echo "✓ OK"
  fi
done

# Verify shim spawns child (not in-process call)
grep -A 10 "fn dispatch" src/shim/mod.rs | grep -E "spawn|child|Command"

# Verify agentic never imports server
grep -r "use crate::server" src/agentic --include="*.rs"
# Should return NOTHING
```

**Fix:** If you find a server→agentic import:
1. Extract the shared logic into `src/common`
2. Import from `src/common` on both sides
3. Verify server does NOT import from agentic
4. Add a test to `tests/invariant_guard.rs` if it's new

**Test:** Run `cargo test --test invariant_guard` — it will fail if this invariant breaks.

---

### I2: Guard chain order is inviolable

**The rule:** Every operator route that mutates state passes through guards in this exact order:
```
rate_limit → same_origin → api_key → csrf
```

This applies to operator/API routes (e.g., `/api/agent/run`, `/api/keys`). Account/session routes use a separate auth partition (e.g., bootstrap/login use `auth_open`, account mutations use `auth_sess` middleware).

Order is load-bearing. A wrong key against a spent budget returns 429 (rate limit), not 401 (auth). A missing CSRF against a wrong key returns 401 first.

**Why:** The early guards (rate limit, origin) protect the auth layer itself. Checking them first prevents exhausting auth checks with garbage requests.

**How to check:**

```bash
# Find the guard chain in guards.rs
grep -n "rate_limit\|same_origin\|api_key\|csrf" src/server/guards.rs | head -20

# Verify the order in a specific route (example: /api/agent/run)
grep -B 5 "async fn.*agent.*run" src/server/routes/*.rs | grep -E "Layer|guard"

# Cross-check against INVARIANTS.md
grep -A 2 "rate limit ->" INVARIANTS.md
```

**Expected pattern in code:**
```rust
router
  .layer(tower_http::trace::TraceLayer::new_for_http())
  .layer(RateLimitLayer::new(...))      // 1st
  .layer(SameOriginLayer::new())         // 2nd
  .layer(ApiKeyLayer::new(...))          // 3rd
  .layer(CsrfLayer::new(...))            // 4th
  .route("/api/agent/run", post(...))
```

**Fix:** If order is wrong:
1. Physically rearrange the layers in the code
2. Verify each route is wrapped in the same order
3. Update the guard test if it changed
4. Test manually: wrong-auth-key against rate-limited IP should 429, not 401

**Test:** `cargo test --test auth_guards && cargo test --test security_headers` should both pass.

---

### I3: CSRF placeholders are contractual

**The rule:** Two placeholder names in the HTML asset and matching header name in code must NEVER change:
- `__CYCLAW_CSRF_TOKEN__` (in HTML, replaced with token value)
- `__CYCLAW_CSP_NONCE__` (in HTML, replaced with CSP nonce)
- `X-CyClaw-CSRF` (header name in both HTML and code)

**Why:** These names are inherited from CyClaw (the precursor harness). Changing them breaks the browser-console protocol.

**How to check:**

```bash
# Check HTML has placeholders
grep "__CYCLAW_CSRF_TOKEN__" assets/static/harness.html && echo "✓ HTML token placeholder found"
grep "__CYCLAW_CSP_NONCE__" assets/static/harness.html && echo "✓ HTML nonce placeholder found"

# Check header name in HTML
grep -i "x-cyclaw-csrf" assets/static/harness.html && echo "✓ HTML header name found"

# Check header name in code (guard chain constant is CSRF_HEADER in guards.rs; HTTP headers are case-insensitive)
grep -i "csrf_header\|x-cyclaw-csrf" src/server/guards.rs && echo "✓ Code CSRF constant found"

# Verify token placeholders in HTML
grep -E "__CYCLAW_CSRF_TOKEN__|__CYCLAW_CSP_NONCE__" assets/static/harness.html && echo "✓ Token/nonce placeholders found"
```

**Fix:** Do NOT rename these placeholders. If you change them:
1. Revert the change
2. Update the corresponding name in the other location (HTML ↔ code)
3. Run `cargo test --test security_headers` to catch mismatches

**Test:** `cargo test --test security_headers` checks exact placeholder names.

---

### I4: Write gates ship closed (fail-safe by default)

**The rule:** These write gates in `assets/config.default.yaml` must default to a closed (`false`) state —
this is exactly what `tests/invariant_guard.rs::shipped_config_keeps_every_gate_closed` asserts:
- `agentic.enabled: false`
- `deepagent_github.enabled: false` (if present)
- `deepagent_github.allow_git_write_tools: false`

Two related settings are NOT part of this closed-by-default set and intentionally ship open — do not "fix"
them to false, and do not add them to the list above:
- `agentic.writes_enabled: true` — safe because the master `agentic.enabled` gate and `mode` still block execution
- `security.api_key_optional: true` (unquoted boolean) — intentionally permits direct loopback access

**Why:** An operator who hasn't explicitly enabled write access should not be able to mutate repositories. Fail-safe defaults prevent accidental damage.

**How to check:**

```bash
# Extract all boolean gates for a quick look
grep -E "enabled|writes_enabled|allow_.*:" assets/config.default.yaml | grep -v "^#"

# Verify only the three gates that must be false actually are
for gate in enabled allow_git_write_tools; do
  echo "=== $gate ==="
  grep -n "^[[:space:]]*$gate:" assets/config.default.yaml
done
```

`agentic.writes_enabled: true` and unquoted `security.api_key_optional: true` will show up in a broad
`true`/`enabled` grep — that's expected, not a finding.

**Shipped fail-closed gates (from tests/invariant_guard.rs::shipped_config_keeps_every_gate_closed):**
- `agentic.enabled` must be `false`
- `deepagent_github.enabled` must be `false` (if present)
- `deepagent_github.allow_git_write_tools` must be `false`

Note: `agentic.writes_enabled` intentionally ships `true` (master layer + mode gate keep writes closed); `security.api_key_optional` intentionally ships unquoted boolean `true` (direct loopback access).

**Verify in code:** The `flag_is_true` helper checks quoted `"true"` as false:

```bash
# Find flag_is_true usage
grep -rn "flag_is_true" src/common --include="*.rs" -A 3
# Should confirm: quoted "true" is treated as false
```

**Fix:** If `agentic.enabled`, `deepagent_github.enabled`, or `deepagent_github.allow_git_write_tools`
defaults to `true` when it should be `false` (do NOT apply this to `writes_enabled` or `api_key_optional` —
those are supposed to be `true`):
1. Change it to `false` immediately
2. Add a comment explaining why it's safe
3. File a security issue if this was merged
4. Run `cargo test shipped_config_keeps_every_gate_closed`

**Test:** `cargo test invariant_guard shipped_config_keeps_every_gate_closed` enforces this.

---

### I5: Constants are duplicated and tested for sync

**The rule:** Three constants are intentionally duplicated across the I6 process boundary and kept in sync by tests:
- `RUN_ID_PATTERN` (server `agent_policy` vs agentic `run_store`)
- Planner/check timeout constants (shim vs agentic)
- Check-profile lookup table (shim vs agentic)

**Why:** I6 forbids importing across the boundary, so we can't `use` a constant from server in agentic. We duplicate and test that they stay in sync.

**How to check:**

```bash
# Find RUN_ID_PATTERN in both locations
echo "=== Server side ==="
grep -n "RUN_ID_PATTERN" src/server/agent_policy.rs

echo "=== Agentic side ==="
grep -n "RUN_ID_PATTERN" src/agentic/run_store.rs

# They should match exactly (same regex)

# Find planner timeout on both sides (note: these have different names)
echo "=== Shim side: REAL_REPO_RUN_FALLBACK_PLANNER_SEC ==="
grep -n "REAL_REPO_RUN_FALLBACK_PLANNER_SEC" src/shim/mod.rs

echo "=== Agentic side: DEFAULT_PLANNER_TIMEOUT_SEC ==="
grep -n "DEFAULT_PLANNER_TIMEOUT_SEC" src/agentic/config.rs

# Or run the actual test that syncs them (recommended for accuracy — the
# real name is duplicated_constants_still_agree, not "constant_sync"; a
# nonexistent-name filter would silently match zero tests and look like a pass)
echo "=== Invariant test for constant sync ==="
cargo test --test invariant_guard duplicated_constants_still_agree 2>&1 | grep -E "test result|FAILED|passed"
```

**Fix:** If constants drift:
1. Update both copies to the same value
2. Verify the test catches the drift (or the test is broken)
3. DO NOT create a shared constant in `src/common` — duplication is intentional
4. Add a comment above each duplicate explaining why

**Test:** `cargo test invariant_guard constant_sync` (if it exists) or manual grep comparison.

---

## Additional checks (per-module)

### If touching `src/agentic/real_repo_loop.rs` or `src/agentic/workspace.rs`

Write preflight checks happen in two stages:
1. **real_repo_loop** — initial scanning and protected-path/budget decisions
2. **workspace::apply_proposal** — rechecks protected destinations and aggregate size before staging

Protected paths are destinations to REFUSE, not a scope that paths must be within.

```bash
# Verify initial checks in real_repo_loop
grep -n "protected_write_paths\|budget" src/agentic/real_repo_loop.rs | head -10

# Verify workspace applies rechecks
grep -n "protected_write_paths\|apply_proposal" src/agentic/workspace.rs | head -10
```

### If touching `src/agentic/executor/sandbox.rs`

The sandbox varies by platform with platform-specific guarantees:
- **macOS:** Seatbelt denies network, filesystem reads outside allowed roots, data exfiltration
- **Linux:** `unshare --net` blocks network only; no filesystem confinement
- **Windows:** Job Object provides process-tree control and kill-on-close; no network/filesystem confinement

All platforms: process cannot outlive sandbox (kill_on_drop).

See `INVARIANTS.md` → "Native macOS Cargo boundary" and `docs/OFFLINE_CARGO.md` for detailed platform capabilities.

```bash
# Verify platform detection and capabilities
grep -n "target_os\|cfg.*unix\|cfg.*windows" src/agentic/executor/sandbox.rs | head -10

# Verify network isolation (macOS Seatbelt, Linux unshare)
grep -n "unshare\|Seatbelt" src/agentic/executor/sandbox.rs

# Verify process lifetime control
grep -n "kill_on_drop\|Drop\|impl Drop" src/agentic/executor/sandbox.rs
```

### If touching `src/agentic/workspace.rs` (clone jail)

Three guarantees:
1. Name-equivalence refusal (dots, spaces, case folding)
2. Capability-based reads (openat + O_NOFOLLOW)
3. Exact path tracking (symlinks land to their true target)

```bash
# Verify name-equivalence
grep -n "name.*equivalent\|\.git\|dangling" src/agentic/workspace.rs | head -10

# Verify capability-based reads
grep -n "openat\|O_NOFOLLOW\|cap_std" src/agentic/workspace.rs | head -10

# Verify path resolution
grep -n "resolve\|landed.*path\|symlink" src/agentic/workspace.rs | head -10
```

---

## Running the full check

Use this bash function to verify all invariants:

```bash
verify_invariants() {
  echo "=== I6: Process isolation ==="
  # A hand-rolled grep here is a trap: it must cover all four console-side
  # modules (server, shim, llm, common — not just server+llm), strip comment
  # lines (a doc comment is allowed to NAME the far side of the boundary),
  # and match qualified references too, not just `use` statements. The real
  # test (tests/invariant_guard.rs::server_side_never_references_agentic)
  # already does exactly this — call it instead of reimplementing it:
  if cargo test --test invariant_guard server_side_never_references_agentic 2>&1 | grep -q "test result: ok"; then
    echo "✓ PASS"
  else
    echo "✗ FAIL: server-side code references agentic — see test output"
    return 1
  fi

  echo "=== I2: Guard chain order ==="
  # Extract guard function NAMES from the fully-guarded chain (the block that
  # calls enforce_csrf — a partial chain omits it), not line numbers: sorting
  # line numbers from an already-line-ordered `grep -n` output is a no-op that
  # can never fail regardless of the code's actual order.
  chain=$(grep -A6 "pub async fn guarded" src/server/guards.rs \
    | grep -oE "enforce_(rate_limit|same_origin|api_key_or_optional|csrf)")
  expected=$'enforce_rate_limit\nenforce_same_origin\nenforce_api_key_or_optional\nenforce_csrf'
  if [ "$chain" = "$expected" ]; then
    echo "✓ PASS"
  else
    echo "✗ FAIL: guard order is wrong (found: $chain)"
    return 1
  fi

  echo "=== I3: CSRF placeholders ==="
  csrf_html=$(grep -ic "x-cyclaw-csrf\|__CYCLAW_CSRF_TOKEN__" assets/static/harness.html)
  csrf_code=$(grep -ic "x-cyclaw-csrf" src/server/guards.rs)
  [ "$csrf_html" -ge 1 ] && [ "$csrf_code" -ge 1 ] && echo "✓ PASS" || echo "✗ FAIL"

  echo "=== I4: Write gates closed ==="
  # Only these three are asserted closed; writes_enabled and api_key_optional
  # intentionally ship true and must not be flagged.
  closed=1
  if grep -A1 "^\s*agentic:" assets/config.default.yaml | grep -q "enabled: true"; then closed=0; fi
  if grep -A1 "^\s*deepagent_github:" assets/config.default.yaml | grep -q "enabled: true"; then closed=0; fi
  if grep "allow_git_write_tools" assets/config.default.yaml | grep -q ": true"; then closed=0; fi
  [ "$closed" -eq 1 ] && echo "✓ PASS" || { echo "✗ FAIL: a fail-closed gate defaults to true"; return 1; }

  echo "=== I5: Constants duplicated ==="
  server_pattern=$(grep "RUN_ID_PATTERN.*=" src/server/agent_policy.rs | grep -oE '"[^"]*"')
  agentic_pattern=$(grep "RUN_ID_PATTERN.*=" src/agentic/run_store.rs | grep -oE '"[^"]*"')
  [ "$server_pattern" = "$agentic_pattern" ] && echo "✓ PASS" || echo "⚠ Check manually"

  echo ""
  echo "=== Running automated tests ==="
  set -o pipefail
  cargo test --test invariant_guard 2>&1 | grep -E "test result|FAILED|passed"
  local cargo_status=$?
  set +o pipefail
  
  [ $cargo_status -eq 0 ] && echo "✓ PASS" || echo "✗ FAIL: invariant_guard did not pass"
  return $cargo_status
}

verify_invariants
```

## What to do if an invariant breaks

1. **Automated test fails** (cargo test invariant_guard): Stop. The invariant guard is right; fix the code.
2. **Manual check finds a violation**: Same — fix it before pushing.
3. **Violation is intentional**: Rare. Document it in the PR body with explicit reasoning, and file a follow-up to restore the invariant once you understand the tradeoff.

Never weaken an invariant to make CI green. Fix the code instead.
