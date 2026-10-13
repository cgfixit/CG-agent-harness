# scripts

Operator and CI helpers. Only `verify-local.sh`, `check-pr-template.sh` and
`python3 test-desktop-backend.py` are pre-approved for Claude in
[.claude/settings.json](../.claude/settings.json). The `test-*.mjs` files are
Node `vm` contract tests over slices of `harness.html`, run by the `ci.yml`
chat-browser job and `verify-local.sh`.

## Operator

- [`start-web.sh`](start-web.sh): starts the console on loopback (default 8790) and opens a browser.
- [`web-allowlist-bootstrap.sh`](web-allowlist-bootstrap.sh): grants a technical-docs web allowlist through `cgagentharness web allow` (admin login needed); extra lines from `CGAH_GRANTS_FILE`.
- [`reset-macos.sh`](reset-macos.sh): dry-runs, or with `--yes` deletes, the harness home and Keychain entries.
- [`smoke-ollama.sh`](smoke-ollama.sh): live smoke of the release binary against local Ollama; optional `FINGERPRINT_OUT`.
- [`qwen3.8-27b-mlx-cg.Modelfile`](qwen3.8-27b-mlx-cg.Modelfile): `ollama create` recipe pinning `num_ctx` 32768 ([MODELS.md](../docs/MODELS.md)).
- [`live-ollama/`](live-ollama/): pinned Ollama, two checksummed models, baseline and fingerprint for `live-ollama.yml`.

## Verification and CI

- [`verify-local.sh`](verify-local.sh): full local verification: lint, tests, contract scripts, build, smoke.
- [`check-pr-template.sh`](check-pr-template.sh): local PR-body gate mirroring `pr-template-check.yml`.
- [`ci-docs-only.sh`](ci-docs-only.sh): classifies a diff as docs-only so `ci.yml` and `bundle.yml` skip compile jobs.
- [`browser-acceptance.mjs`](browser-acceptance.mjs): native Chrome acceptance against a disposable fixture; needs a live model, run by `e2e-macos.sh browser`.
- [`e2e-macos.sh`](e2e-macos.sh): staged Apple Silicon e2e (preflight, build, contracts, smoke, browser, desktop) behind `e2e.yml`; also run by hand.
- [`chat-browser-acceptance.mjs`](chat-browser-acceptance.mjs): real-browser slash-command tests against a mock API.
- [`test-analytics.mjs`](test-analytics.mjs): analytics and spend helpers, logout clearing.
- [`test-attachment-preview.mjs`](test-attachment-preview.mjs): preview reuses uploads and never leaks across sessions.
- [`test-auth-deadlines.mjs`](test-auth-deadlines.mjs): console fetch helpers against headers-first replies.
- [`test-delivery-review.mjs`](test-delivery-review.mjs): delivery UI handlers across logout and late replay.
- [`test-header-buttons.mjs`](test-header-buttons.mjs): header automation buttons hide until account state is known.
- [`test-loop-continuation.mjs`](test-loop-continuation.mjs): `/loop` turn prompt, `GOAL_DONE` marker, stall checks.
- [`test-nl-search-routing.mjs`](test-nl-search-routing.mjs): natural-language search predicate against server phrasings.
- [`test-review-reset.mjs`](test-review-reset.mjs): console review handlers without jobs or publishing.
- [`test-schedule-review.mjs`](test-schedule-review.mjs): stale schedule responses cannot activate.
- [`test-session-tokens.mjs`](test-session-tokens.mjs): header token refresh with out-of-order replies.
- [`test-spend-dashboard.mjs`](test-spend-dashboard.mjs): spend summary rows, escaping, unknown days.
- [`test-desktop-backend.py`](test-desktop-backend.py): public sidecar contract tests with disposable homes.
- [`test-grok-acp-probe.py`](test-grok-acp-probe.py), [`test-prepare-cargo.py`](test-prepare-cargo.py), [`test-release-plan.py`](test-release-plan.py): unittest coverage of the matching scripts; the last also gates `release.yml` and `zizmor.yml`.

## Packaging and release

- [`package-desktop.sh`](package-desktop.sh): builds the macOS `.app` with sidecar on Apple Silicon.
- [`package-release.sh`](package-release.sh): stages, optionally codesigns, archives and checksums the CLI binary.
- [`verify-desktop-bundle.sh`](verify-desktop-bundle.sh): verifies `.app` codesign, plist, architectures, system-only linkage.
- [`release-plan.py`](release-plan.py): read-only release planner via `gh api`; fails closed ([RELEASING.md](../docs/RELEASING.md)).
- [`prepare-cargo.py`](prepare-cargo.py): offline-by-default Cargo source and toolchain preparation; bundled into the `.app`.
- [`desktop-icon.swift`](desktop-icon.swift): regenerates the app icon set by hand; no automated caller.

## Hooks, fixtures and probes

- [`ensure-githooks.sh`](ensure-githooks.sh): idempotent `core.hooksPath` setup; run by the SessionStart hook and Copilot ([GITHOOKS.md](../docs/GITHOOKS.md)).
- [`install-githooks.sh`](install-githooks.sh): older once-per-clone installer with a gitleaks hint; nothing automated runs it.
- [`browser-fixture.py`](browser-fixture.py): creates and serves a disposable arithmetic repo fixture; no real GitHub writes.
- [`mcp-fixture.py`](mcp-fixture.py): disposable stdio and SSE MCP server for the MCP and process-lifecycle tests.
- [`mcp-windows-fixture.py`](mcp-windows-fixture.py): Windows Job Object fixture used by `tests/mcp_windows.rs` only.
- [`grok-acp-probe.py`](grok-acp-probe.py): credential-free probe of a fake Grok ACP launch contract; Grok ACP is not shipped.
- [`netconnect-syscall-proof.sh`](netconnect-syscall-proof.sh): strace under `unshare -rn` proving `status` and `devices` send no packets.
