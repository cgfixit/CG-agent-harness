---
name: dep-sync
description: |
  Actively fix dependency, build, and deploy drift for CG-Agent-Harness by
  comparing the current branch's Cargo manifests/locks, toolchain pin, deny
  policy, and CI/release/packaging config against the latest origin/main and applying the needed updates, then
  re-verifying. Use when a branch has fallen behind main's dependency/toolchain
  state, after a security advisory lands on main, or whenever asked to "sync
  deps" / "fix dependency drift".
compatibility: |
  Requires: cargo, rustup, cargo-deny, git; npm only for the browser lock
  Context: Independent backend (root, Rust 1.89) and desktop (desktop/, Rust
  1.90, macOS-only) Rust crates, each with its own Cargo.lock and deny policy,
  plus the npm lock of the /browse Playwright sidecar (browser/)
---

# dep-sync — fix dependency/build/deploy drift against origin

This skill WRITES fixes. For a report only, run steps 1, 2 and 4 with no edits
and record commands, exit codes and advisory freshness.

## 1. Establish the comparison base

```bash
git fetch origin main
BASE=origin/main
git diff --stat HEAD..."$BASE" -- Cargo.toml Cargo.lock rust-toolchain.toml \
  deny.toml .cargo/config.toml desktop/Cargo.toml desktop/Cargo.lock \
  desktop/rust-toolchain.toml desktop/deny.toml browser/package.json \
  browser/package-lock.json tests/fixtures/cargo-sandbox \
  .github/workflows/ .github/dependabot.yml scripts/package-desktop.sh
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
- `rust-toolchain.toml` / `desktop/rust-toolchain.toml` — MSRV/channel bump
  (backend 1.89, desktop 1.90 are independently pinned; a bump to one is not a
  bump to the other)
- `deny.toml` / `desktop/deny.toml` — advisory ignores, license allowlist,
  banned crates (both carry the same telemetry-SDK bans; the desktop file
  evaluates Apple targets only and keeps its ignores empty)
- `browser/package.json` + `package-lock.json` — the sidecar's Playwright pin;
  `docs/Browser-Use.md` names the version, so move both together
- `tests/fixtures/cargo-sandbox` — a third lock CI checks with the root policy
- `.github/workflows/*.yml` (list the directory; the dependency-bearing ones
  are `ci.yml`, `clippy-sarif.yml`, `dependency-advisories.yml`, `desktop.yml`,
  `desktop-check.yml`, `bundle.yml`, `release.yml`, `install-macos.yml`) —
  toolchain versions, cached paths, new required jobs
- `.github/dependabot.yml` — ecosystems (Actions, cargo for both crates, npm
  for `browser/`) and the 7-day cooldown
- `scripts/package-desktop.sh`, `docs/RELEASING.md`, `docs/DESKTOP.md` —
  packaging order or signing steps that reference dependency/toolchain state

## 3. Apply the sync

Backend and desktop are independent — do not let a backend toolchain change
leak into desktop's pin or vice versa; they use separate `Cargo.lock` files
and `desktop/` is not a workspace member.

`.cargo/config.toml` already sets `resolver.incompatible-rust-versions =
"fallback"` for both crates, so a plain `cargo update` stays inside each MSRV.

```bash
# Backend (rustup's cargo reads rust-toolchain.toml and installs 1.89)
cargo fetch --locked
cargo update --dry-run --locked --verbose
# after reviewing the dry run against origin/main's diff, one package at a
# time (no --locked: a real update would fail on it):
cargo update -p <crate>
cargo deny check

# Desktop (resolves anywhere; compiles only on macOS)
cd desktop
cargo fetch --locked
cargo update --dry-run --locked --verbose
cargo update -p <crate>
cargo deny --all-features check
cd ..

# Browser sidecar lock (installs nothing)
(cd browser && npm audit --package-lock-only)
```

Respect the pins and waivers in `docs/DEPENDENCIES.md#retained-constraints`:
exact pins (`rmcp`, `keyring-core`, desktop `tauri`), MSRV holds
(`ordered-float`), the incompatible major lines listed there, and dated
advisory waivers with an owner. A pin origin/main still carries is not drift;
only sync what origin/main actually moved. A compatible release younger than
Dependabot's 7-day cooldown is not drift either, unless it fixes an advisory
against a locked crate. Move one package per `cargo update -p` and check that
no unrelated edge re-resolved. Never add a `cargo deny` advisory
exception to silence a finding that the sync itself introduced — fix the
version instead.

For CI/build/deploy YAML, port origin/main's exact toolchain version strings
and job structure rather than re-deriving them; copy-diff, don't paraphrase.

## 4. Re-verify

Inspect new Cargo features and each new `unsafe` block's safety argument.

```bash
cargo build --all-targets --locked
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo deny check
# Targeted only; CI runs the suite:
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" cargo test --test <target>
# macOS only; on Linux the desktop compile is CI's `desktop compile (macos-latest)`:
cd desktop && cargo build --all-targets --locked && cd ..
```

A network crate that moved (reqwest, rmcp, rustls, hyper, tauri, a new direct
dependency) also drifts the telemetry oracle (`telemetry-contract.yml`): ask
the operator to run `/cgagentharness-otel-hardening` (operator-only).

Never pipe a pass/fail command into grep or suppress its exit status. A
missing `cargo-deny` binary is a missing required check, not a pass. Do not
change global rustup overrides or global git identity to make verification
succeed.

## 5. Report

List: base ref compared against, exact version/config diffs applied per file,
commands run with exit codes, and any pin from `docs/DEPENDENCIES.md` you
deliberately left untouched and why. Update its dated "Last synchronization"
section. If the file now states a stale version number, fix it in the same
pass if it's a one-line number change, or ask the operator to run `/doc-sync`
for a larger rewrite — `doc-sync` is manual-only (`disable-model-invocation: true`),
so Claude cannot self-load it.

Skill selection here does not authorize push/merge — follow this repo's PR
conventions (draft PR, `[infra]` or `[security]` prefix as appropriate,
`scripts/check-pr-template.sh`, quality bar in `AGENTS.md`) for publishing
the result.
