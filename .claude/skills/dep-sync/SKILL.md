---
name: dep-sync
description: |
  Actively fix dependency, build, and deploy drift for CG-Agent-Harness by
  comparing the current branch's Cargo manifests/locks, toolchain pin, deny
  policy, and CI/release/packaging config against the latest origin/main and applying the needed updates. Unlike
  cgagentharness-verify-deps (which only reports), this skill performs the
  sync, then re-verifies. Use when a branch has fallen behind main's dependency/toolchain
  state, after a security advisory lands on main, or whenever asked to "sync
  deps" / "fix dependency drift".
compatibility: |
  Requires: cargo, rustup, cargo-deny, git
  Context: Independent backend (root, Rust 1.88) and desktop (desktop/, Rust
  1.90) Rust crates, each with its own Cargo.lock and deny policy
---

# dep-sync — fix dependency/build/deploy drift against origin

This skill WRITES fixes. If you only want a drift report with no changes, use
`cgagentharness-verify-deps` instead.

## 1. Establish the comparison base

```bash
git fetch origin main
BASE=origin/main
git diff --stat HEAD..."$BASE" -- Cargo.toml Cargo.lock rust-toolchain.toml \
  deny.toml desktop/Cargo.toml desktop/Cargo.lock .github/workflows/ \
  scripts/package-desktop.sh
```

The base is always `origin/main`: a feature branch's own upstream
(`origin/<branch>`) would make the diff falsely empty. `HEAD...$BASE` lists what the base changed since the merge-base, which is what
this branch is missing (`$BASE...HEAD` would show the branch's own edits). If
the diff is empty, the branch is already current on dependency/build/deploy
files: say so and stop; do not manufacture an update. If the branch is behind,
merge the base first (never rebase someone else's branch), then continue.

## 2. Identify what origin/main changed that this branch hasn't picked up

For each file in the diff, extract the semantic change, not just the text:

- `Cargo.toml` / `desktop/Cargo.toml` — added/removed/version-bumped crates,
  changed feature flags
- `Cargo.lock` / `desktop/Cargo.lock` — resolved version changes (these
  usually follow from the manifest changes above; don't hand-edit a lock,
  regenerate it)
- `rust-toolchain.toml` — MSRV/channel bump (backend 1.88, desktop 1.90 are
  independently pinned; a bump to one is not a bump to the other)
- `deny.toml` — advisory ignores, license allowlist, banned-crate changes
- `.github/workflows/*.yml` (`ci.yml`, `rust-clippy.yml`, `release.yml`,
  `bundle.yml`, `desktop.yml`, `codeql.yml`, `devskim.yml`, `gitleaks.yml`,
  `zizmor.yml`) — toolchain versions, cached paths, new required jobs
- `scripts/package-desktop.sh`, `docs/RELEASING.md`, `docs/DESKTOP.md` —
  packaging order or signing steps that reference dependency/toolchain state

## 3. Apply the sync

Backend and desktop are independent — do not let a backend toolchain change
leak into desktop's pin or vice versa; they use separate `Cargo.lock` files
and `desktop/` is not a workspace member.

```bash
# Backend
rustup toolchain install "$(cat rust-toolchain.toml | rg -o '"[0-9.]+"' | tr -d '"')" 2>/dev/null || true
cargo fetch --locked
cargo update --dry-run --locked --verbose --config \
  'resolver.incompatible-rust-versions="fallback"'
# only after reviewing the dry-run output against what origin/main's diff calls for
# (no --locked here: it makes a real update fail with "lock file needs to be updated"):
cargo update --config 'resolver.incompatible-rust-versions="fallback"'
cargo deny check

# Desktop
cd desktop
cargo fetch --locked
cargo update --dry-run --locked --verbose --config \
  'resolver.incompatible-rust-versions="fallback"'
cargo update --config 'resolver.incompatible-rust-versions="fallback"'
cargo deny check
cd ..
```

Respect deliberate pins documented in `docs/DEPENDENCIES.md` (e.g. backend
`ordered-float` held at a specific line, desktop's Tauri patch held
immutable) — a pin that origin/main itself still carries is not drift; only
sync what origin/main actually moved. Never add a `cargo deny` advisory
exception to silence a finding that the sync itself introduced — fix the
version instead.

For CI/build/deploy YAML, port origin/main's exact toolchain version strings
and job structure rather than re-deriving them; copy-diff, don't paraphrase.

## 4. Re-verify

```bash
cargo build --all-targets --locked
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo deny check
# Targeted only; CI runs the suite:
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" cargo test --test <target>
cd desktop && cargo build --all-targets --locked && cd ..
```

Never pipe a pass/fail command into grep or suppress its exit status. A
missing `cargo-deny` binary is a missing required check, not a pass. Do not
change global rustup overrides or global git identity to make verification
succeed.

## 5. Report

List: base ref compared against, exact version/config diffs applied per file,
commands run with exit codes, and any pin from `docs/DEPENDENCIES.md` you
deliberately left untouched and why. If `docs/DEPENDENCIES.md` itself now
states a stale version number as a result of this sync, fix it in the same
pass if it's a one-line number change, or ask the operator to run `/doc-sync`
for a larger rewrite — `doc-sync` is manual-only (`disable-model-invocation: true`),
so Claude cannot self-load it.

Skill selection here does not authorize push/merge — follow this repo's PR
conventions (draft PR, `[infra]` or `[security]` prefix as appropriate,
`scripts/check-pr-template.sh`, quality bar in `AGENTS.md`) for publishing
the result.
