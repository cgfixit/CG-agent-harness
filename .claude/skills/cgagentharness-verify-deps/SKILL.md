---
name: cgagentharness-verify-deps
description: |
  Verify Cargo dependencies, security advisories, licenses, and locked versions for CG-Agent-Harness. Run this whenever Cargo.toml changes, after adding/updating dependencies, or before pushing security-sensitive PRs. Checks: Cargo.lock is committed and locked, cargo deny passes (advisories/licenses/bans), no unsafe code regressions, no outdated critical deps, toolchain version matches rust-toolchain.toml, and no feature flags that contradict security posture. Essential for maintaining supply-chain hygiene and reproducible builds.
compatibility: |
  Requires: cargo, cargo-deny (if installed), rustup, Cargo.toml and Cargo.lock in repo
  Context: Single-crate Rust project (cgagentharness crate)
---

# CGagentHarness Dependency Verification

CG-Agent-Harness enforces strict supply-chain controls. This skill verifies that:

1. Dependencies are locked and reproducible
2. No known security advisories exist
3. Licenses are acceptable
4. No blacklisted crates
5. Unsafe code is intentional and scanned
6. Toolchain version is pinned and matches
7. Feature flags don't weaken security posture

## What to check

### 1. Cargo.lock is committed and locked (always)

`Cargo.lock` must exist and be committed to git. Binary crates must have a locked-dependency tree to ensure reproducible builds. This is non-negotiable.

**Verification:**
```bash
# Check Cargo.lock exists and is tracked
ls -la Cargo.lock
git ls-files | grep Cargo.lock

# Check if lock is fresh (no `cargo update` pending)
# Must check exit status separately from output to catch resolution failures
if cargo update --dry-run --locked 2>&1 | grep -q "would"; then
  echo "LOCK NEEDS UPDATE"
elif [ $? -eq 0 ]; then
  echo "Lock is current"
else
  echo "ERROR: Cargo command failed (registry unavailable or resolution failed)"
  return 1
fi

# Verify locked compile succeeds
cargo build --locked 2>&1 | grep -q "error" && echo "BUILD FAILS WITH LOCK" || echo "Locked build OK"
```

**Failure modes:**
- `Cargo.lock` not in git → fix: `git add Cargo.lock && git commit -m "lock dependencies"`
- `cargo update` suggests changes → fix: `cargo update` + commit
- Locked build fails → fix: resolve dep conflict in Cargo.toml

### 2. cargo deny passes (advisories, licenses, bans)

The `deny.toml` file enforces three checks:
- **Advisories:** no known CVEs in any dependency
- **Licenses:** only approved open-source licenses
- **Bans:** blacklisted crates are rejected

**Verification:**
```bash
# Run full deny check
cargo deny check

# If installed. If not, the harness tolerates absence (optional tool):
which cargo-deny > /dev/null || echo "cargo-deny not installed; skipping"

# Check deny.toml for custom rules
cat deny.toml | head -30
```

**Expected output:**
```
✓ Advisories check passed
✓ Licenses check passed
✓ Bans check passed
```

**Failure modes:**
- Advisory found → security issue; fix the dep or bump version
- License mismatch → check `deny.toml` approved list
- Banned crate → remove or whitelist with justification in `deny.toml`

### 3. Unsafe code is scanned and intentional

The codebase uses unsafe code sparingly (primarily in `src/agentic/executor/sandbox.rs` for Seatbelt syscalls, `src/agentic/workspace.rs` for capability-based reads). All unsafe blocks must have a comment explaining WHY.

**Verification:**
```bash
# Find all unsafe blocks and check for preceding SAFETY comments
# The repository places SAFETY comments on the line BEFORE unsafe
grep -rn "unsafe {" src/ --include="*.rs" | while read line; do
  linenum=$(echo "$line" | cut -d: -f2)
  file=$(echo "$line" | cut -d: -f1)
  prevline=$((linenum - 1))
  
  if ! sed -n "${prevline}p" "$file" | grep -q "SAFETY:"; then
    echo "$file:$linenum: unsafe block without SAFETY comment above"
  fi
done

# Count total unsafe blocks
total=$(grep -r "unsafe {" src/ --include="*.rs" | wc -l)
echo "Total unsafe blocks: $total"

# Expected: ~8-12 (sandboxing + clone jail + signal handlers)
# If count jumps, each new unsafe must have a preceding comment
```

**Failure modes:**
- `unsafe {` with no SAFETY comment on the preceding line → add `// SAFETY: <reason>` above it
- New unsafe in unexpected location → code review finding
- unsafe in server side (not sandbox) → likely a security issue

### 4. No outdated critical dependencies

High-severity crates (especially in security-sensitive paths like `cap_std`, `seatbelt`, `sha2`, `ring`, OpenSSL bindings) must be current.

**Verification:**
```bash
# List all dependencies and their versions
cargo tree --depth 1

# Look for outdated dependencies
cargo outdated --root-only 2>/dev/null || echo "cargo-outdated not installed"

# Check security-critical crates specifically
for dep in cap_std seatbelt sha2 ring tokio hyper; do
  echo "=== $dep ==="
  grep "^$dep" Cargo.lock | head -1
done

# Compare against latest on crates.io
# Example: https://crates.io/crates/cap_std
```

**Red flags:**
- Any dependency >6 months behind latest → consider upgrade
- Security deps (crypto, sandbox, network) especially
- Pinned old versions without explanation → needs a comment in Cargo.toml

**Verification:**
```bash
# In Cargo.toml, old deps should have a comment:
# cap_std = "1.0" # pinned for Seatbelt compatibility with MSRV Rust 1.70
```

### 5. Toolchain version matches rust-toolchain.toml (always)

CG-Agent-Harness pins Rust to a specific version for reproducible CI and local builds. Mismatch causes confusing compile-time surprises.

**Verification:**
```bash
# Check pinned version
cat rust-toolchain.toml

# Check running version
rustc --version

# They must match (or running version is newer; pinned is minimum)
# Example: pinned is 1.88.0, running is 1.88.1 ✓
# Example: pinned is 1.88.0, running is 1.87.0 ✗

# Update toolchain if behind
rustup update
rustup override set $(grep channel rust-toolchain.toml | cut -d'"' -f2)
```

**Failure modes:**
- Running version older than pinned → `rustup update`
- Pinned version not installed → `rustup toolchain install 1.88.0` (example)
- Mismatch in CI → CI uses its own rustup; check `.github/workflows/` for override

### 6. No unsafe feature flags

The `deny.toml` also blocks crates that export unsafe feature combinations. For example, a crate might have `feature = "unsafe-crypto"` that should never be enabled. Check:

**Verification:**
```bash
# Check Cargo.toml for feature enables
grep -r "features = \[" Cargo.toml

# Cross-check against deny.toml bans
grep -A 5 "\[deny.bans\]" deny.toml

# Verify no security-weakening features are enabled
# Example: should NOT see features = ["allow-unsafe-crypto"]
```

**Failure modes:**
- Crypto feature that weakens security enabled → remove it
- Sandbox feature that disables isolation enabled → fix
- Known-dangerous feature combination enabled → document or remove

### 7. Dependency tree has no circular imports (dev-only check)

Circular dependencies are allowed only in dev/test (workspace interdependencies for testing). Production must be acyclic.

**Verification:**
```bash
# Show dependency tree
cargo tree

# Check for cycles (cargo will error if they exist in production)
cargo build --all-targets 2>&1 | grep -i "cyclic\|circular"

# If cargo builds, no circular deps in production ✓
```

## Reconciliation workflow

If any check fails:

1. **Advisories:** Fix or upgrade the affected crate. A security advisory is blocking.
2. **Licenses:** Add to `deny.toml` `allow` list with a comment explaining why, or use a different crate.
3. **Locked build fails:** Resolve the conflict in `Cargo.toml` (version range, feature conflict, etc.), then `cargo update` and commit.
4. **Unsafe without comment:** Add `// SAFETY: <reason>` above each unsafe block.
5. **Outdated deps:** Check changelog, run tests, bump version, commit.
6. **Toolchain mismatch:** `rustup override set 1.88.0` (example) and `rustup update`.

## Running the full check

Use this bash function to run all verification at once:

```bash
verify_deps() {
  echo "=== 1. Cargo.lock exists and is in git ==="
  [ -f Cargo.lock ] && git ls-files | grep -q Cargo.lock && echo "✓" || echo "✗ FAIL"

  echo "=== 2. Locked build passes ==="
  cargo build --locked 2>&1 | grep -q "error" && echo "✗ FAIL" || echo "✓"

  echo "=== 3. cargo deny check ==="
  if command -v cargo-deny &> /dev/null; then
    cargo deny check && echo "✓" || echo "✗ FAIL"
  else
    echo "✗ FAIL: cargo-deny not installed (required by quality bar; install with 'cargo install cargo-deny')"
    return 1
  fi

  echo "=== 4. Unsafe code has comments ==="
  unsafe_without_comment=$(grep -r "unsafe" src/ --include="*.rs" | grep -v "// SAFETY:\|// reason:" | wc -l)
  [ "$unsafe_without_comment" -eq 0 ] && echo "✓" || echo "✗ $unsafe_without_comment unsafe blocks lack comments"

  echo "=== 5. Toolchain version ==="
  pinned=$(grep channel rust-toolchain.toml | cut -d'"' -f2)
  running=$(rustc --version | cut -d' ' -f2)
  echo "Pinned: $pinned, Running: $running"

  echo "=== 6. Check for outdated security deps ==="
  for dep in cap-std sha2 ring tokio hyper; do
    echo "=== $dep ==="
    cargo tree --depth 0 --prefix none | grep "^$dep " || echo "  Not found directly"
  done

  echo "=== 7. Dependency tree (summary) ==="
  cargo tree --depth 1 | head -10
}

verify_deps
```

Run this before pushing security-sensitive PRs or when Cargo.toml changes.
