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

# Check if lock is fresh (no `cargo update` pending).
# Capture cargo's own exit status BEFORE inspecting output: `$?` right after
# an `if cmd | grep ...` reflects grep's status, not cargo's, so a run that
# genuinely failed (registry unavailable, resolution error) would otherwise
# look identical to "nothing to update" and get reported as fine.
update_output=$(cargo update --dry-run --locked 2>&1)
update_status=$?
if [ "$update_status" -ne 0 ]; then
  echo "ERROR: cargo update --dry-run --locked failed (exit $update_status): registry unavailable or resolution failed"
elif echo "$update_output" | grep -q "would"; then
  echo "LOCK NEEDS UPDATE"
else
  echo "Lock is current"
fi

# Verify locked compile succeeds — check Cargo's own exit status, not a text
# match on "error": several real dependency names contain that substring
# (e.g. "Compiling thiserror" on a cold build), which would misreport success
# as a failure, and a failure whose diagnostic doesn't say "error" would slip
# through the other way.
if cargo build --locked > /dev/null 2>&1; then
  echo "Locked build OK"
else
  echo "BUILD FAILS WITH LOCK"
fi
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

The repo does not put a `SAFETY:` comment on the line directly above every single `unsafe { ... }` — one
comment often covers a whole cluster of related calls in the same function (e.g. the Windows Job Object
sequence in `sandbox.rs` has one `// SAFETY:` near the top of the block covering several subsequent
`unsafe { CloseHandle(...) }` / `unsafe { TerminateJobObject(...) }` calls). A pure "is the immediately
preceding line a SAFETY comment" grep will flag most of that cluster as false positives.

```bash
# List every `unsafe {` occurrence and whether a SAFETY comment appears anywhere
# in the ~20 lines above it. Treat matches as candidates to eyeball, not an
# automatic pass/fail — confirm by reading the surrounding function whether one
# comment is meant to cover the whole cluster.
grep -rn "unsafe {" src/ --include="*.rs" | cut -d: -f1,2 | while IFS=: read -r file linenum; do
  start=$(( linenum > 20 ? linenum - 20 : 1 ))
  if ! sed -n "${start},${linenum}p" "$file" | grep -q "SAFETY:"; then
    echo "$file:$linenum: no SAFETY comment within 20 lines above — review this one"
  fi
done

# Count total unsafe blocks (not string/prose occurrences of the word "unsafe")
total=$(grep -r "unsafe {" src/ --include="*.rs" | wc -l)
echo "Total unsafe blocks: $total"
```

**Failure modes:**
- `unsafe {` with no `SAFETY:` comment anywhere in the enclosing function/cluster → add `// SAFETY: <reason>`
- New unsafe in unexpected location → code review finding
- unsafe in server side (not sandbox) → likely a security issue

### 4. No outdated critical dependencies

High-severity crates (especially in security-sensitive paths like `cap-std`, `sha2`, `ring`, and the HTTP
stack) must be current. Note the crate name is `cap-std` (hyphen) — `cap_std` is only the Rust module path
after import; Seatbelt itself is this repo's own macOS sandbox code in `src/agentic/executor/sandbox.rs`,
not an external crate, so it won't show up in a dependency tree at all.

**Verification:**
```bash
# List all dependencies and their versions
cargo tree --depth 1

# Look for outdated dependencies
cargo outdated --root-only 2>/dev/null || echo "cargo-outdated not installed"

# Check security-critical crates specifically. Cargo.lock stores entries as
# `name = "pkg"` (not `^pkg `), so grepping the lockfile directly is fragile —
# resolve each package through cargo instead, which also catches a rename or
# a version bump against MSRV. `head -1` always exits 0, so piping straight to
# it swallows a failed/ambiguous `cargo tree -p` lookup — capture cargo's own
# status first:
for dep in cap-std sha2 ring tokio hyper; do
  echo "=== $dep ==="
  tree_output=$(cargo tree -p "$dep" 2>/dev/null)
  if [ $? -eq 0 ]; then
    echo "$tree_output" | head -1
  else
    echo "  Not a dependency (or ambiguous/renamed — check manually)"
  fi
done

# Compare against latest on crates.io
# Example: https://crates.io/crates/cap-std
```

**Red flags:**
- Any dependency >6 months behind latest → consider upgrade
- Security deps (crypto, sandbox, network) especially
- Pinned old versions without explanation → needs a comment in Cargo.toml

In Cargo.toml, an intentionally old/pinned dep should carry a comment explaining why, e.g.
`cap-std = "..." # pinned for <reason>`.

### 5. Toolchain version matches rust-toolchain.toml (always)

CG-Agent-Harness pins Rust to a specific version for reproducible CI and local builds. Mismatch causes confusing compile-time surprises.

**Verification:**
```bash
# Check pinned version
cat rust-toolchain.toml

# Check running version
rustc --version

# Exact match against rust-toolchain.toml channel (not "newer is fine").
# rustup channel "1.88" selects toolchain 1.88-<target>; rustc reports 1.88.0.
# Example: pinned is 1.88, active is 1.88-x86_64-unknown-linux-gnu ✓
# Example: pinned is 1.88, active is 1.89-x86_64-unknown-linux-gnu ✗

# Update toolchain if behind
rustup update
rustup override set $(grep channel rust-toolchain.toml | cut -d'"' -f2)
```

**Failure modes:**
- Running version older than pinned → `rustup update`
- Pinned version not installed → `rustup toolchain install 1.88.0` (example)
- Mismatch in CI → CI uses its own rustup; check `.github/workflows/` for override

### 6. No unsafe feature flags

`deny.toml`'s `[bans]` section (not `[deny.bans]` — there's no `deny.` prefix on the section names) only
polices `multiple-versions` and `wildcards`; it has no rule that inspects which Cargo features a dependency
enables. Checking for a security-weakening feature (e.g. a crate offering `features = ["insecure-tls"]`) is
a manual read of `Cargo.toml`, not something this repo's tooling enforces automatically today — don't present
the grep below as enforcement, treat it as a prompt for a human read:

**Verification:**
```bash
# Eyeball what features are enabled per dependency
grep -r "features = \[" Cargo.toml

# The actual deny.toml policy (multiple-versions, wildcards, licenses, advisories, sources) — this section
# does NOT cover feature flags
sed -n '/^\[bans\]/,/^\[/p' deny.toml
```

**Failure modes:**
- Crypto feature that weakens security enabled → remove it
- Sandbox feature that disables isolation enabled → fix
- Known-dangerous feature combination enabled → document or remove
- If this needs to be enforced automatically rather than caught by eye, that's a `deny.toml` gap to fix, not
  a check this skill can currently claim to perform

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
  if [ -f Cargo.lock ] && git ls-files | grep -q Cargo.lock; then
    echo "✓"
  else
    echo "✗ FAIL: Cargo.lock missing or untracked"
    return 1
  fi

  echo "=== 2. Locked build passes ==="
  # Check cargo's own exit status, not a text match on "error" — dependency
  # names like "thiserror" contain that substring on a normal, successful
  # "Compiling thiserror" line.
  if cargo build --locked > /dev/null 2>&1; then
    echo "✓"
  else
    echo "✗ FAIL: locked build does not compile"
    return 1
  fi

  echo "=== 3. cargo deny check ==="
  if command -v cargo-deny &> /dev/null; then
    if cargo deny check; then
      echo "✓"
    else
      echo "✗ FAIL: cargo deny check reported an advisory/license/ban violation"
      return 1
    fi
  else
    echo "✗ FAIL: cargo-deny not installed (required by quality bar; install with 'cargo install cargo-deny')"
    return 1
  fi

  echo "=== 4. Unsafe code has comments (candidates — eyeball clustered comments) ==="
  grep -rn "unsafe {" src/ --include="*.rs" | cut -d: -f1,2 | while IFS=: read -r file linenum; do
    start=$(( linenum > 20 ? linenum - 20 : 1 ))
    if ! sed -n "${start},${linenum}p" "$file" | grep -q "SAFETY:"; then
      echo "  $file:$linenum: no SAFETY comment within 20 lines above"
    fi
  done

  echo "=== 5. Toolchain version ==="
  pinned=$(grep channel rust-toolchain.toml | cut -d'"' -f2)
  running=$(rustc --version | cut -d' ' -f2)
  echo "Pinned: $pinned, Running: $running"
  # rust-toolchain.toml names a rustup channel. Exact match: active
  # toolchain is that channel, optionally with rustup's -<target> suffix.
  # "1.88" matches 1.88-x86_64-unknown-linux-gnu, not 1.89-* / 1.88.1-*.
  active=$(rustup show active-toolchain 2>/dev/null | awk '{print $1}')
  case "$active" in
    "$pinned"|"$pinned"-*)
      echo "✓ PASS"
      ;;
    *)
      echo "✗ FAIL: toolchain mismatch (pinned=$pinned active=${active:-none})"
      return 1
      ;;
  esac

  echo "=== 6. Check for outdated security deps ==="
  # cargo tree's --depth is a MAXIMUM display depth, not a target depth — depth 0
  # only prints the root package, so every one of these deps would report "not
  # found" regardless of whether it's actually a dependency. Resolve each
  # package directly instead:
  for dep in cap-std sha2 ring tokio hyper; do
    echo "=== $dep ==="
    tree_output=$(cargo tree -p "$dep" 2>/dev/null)
    if [ $? -eq 0 ]; then
      echo "$tree_output" | head -1
    else
      echo "  Not a dependency (or ambiguous/renamed — check manually)"
    fi
  done

  echo "=== 7. Dependency tree (summary) ==="
  cargo tree --depth 1 | head -10
}

verify_deps
```

Run this before pushing security-sensitive PRs or when Cargo.toml changes.
