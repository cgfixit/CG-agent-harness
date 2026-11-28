---
name: cgagentharness-release
description: Build and verify macOS Harness release artifacts, native acceptance and GitHub workflow provenance.
---

# Release the macOS desktop

Follow the desktop and release guides in `AGENTS.md`. Package committed
source for `Resources/COMMIT` provenance. `CGAH_ALLOW_DIRTY=1` marks development
artifacts. CLI archives use `scripts/package-release.sh`, which checks
extracted payload hashes and runs `scripts/smoke-packaged-cli.py`.

On Apple Silicon with Xcode CLT, use both pinned rustup toolchains and
install both Apple targets for each. Run `scripts/package-desktop.sh --universal`
(optionally `--dmg`). Set `CGAH_RELEASE_VERSION` for a planned version.

Preserve signing order: lipo the backend, sign it, compile the desktop slices
with that whole-file SHA-256, then lipo and sign the shell/bundle. Re-signing
the backend after embedding the digest breaks startup identity.

Verify the ZIP checksum and extract to a fresh path containing spaces.
Run `scripts/verify-desktop-bundle.sh '<extracted app>' universal '<release version>'`
for version, architectures, resources, signatures and linkage. Run
`scripts/test-desktop-backend.py` with `CGAH_TEST_BINARY` set to the packaged
backend. Check desktop policy and the root and desktop cargo-deny settings.

Launch the bundle with a disposable `CGAGENTHARNESS_HOME`. Record PID and
listener ownership. Exercise WKWebView and the browser console with setup,
chat, a guarded action and shutdown. For `/browse`, use a prepared absolute
`browser.sidecar` path. Report Intel cross-build, Rosetta and native Intel
evidence separately. Follow operator authority for Rosetta or TCC changes.

Match workflow, source SHA, artifact COMMIT, version and checksum.
Check the path selector and compile steps in `desktop-check.yml`. Use
authorized `desktop.yml` dispatch for an unmerged artifact. Check
`release.yml` and `scripts/release-plan.py` for main Bundle eligibility,
tag collisions and the post-wait supersession check. Inspect runs, tags
and releases before dispatch. Preserve assets after ambiguous publication.
Publish only with explicit authority.

Report ad-hoc signature and checksum integrity separately from Developer ID,
notarization and independent publisher identity.
