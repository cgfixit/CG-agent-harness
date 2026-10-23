# .github/workflows

Lint workflow edits with actionlint and zizmor rather than running them. The
docs-only classifier is [scripts/ci-docs-only.sh](../../scripts/ci-docs-only.sh).
CI expectations: [docs/INSTALL.md](../../docs/INSTALL.md#tests-and-cicd);
release flow: [docs/RELEASING.md](../../docs/RELEASING.md).

## Checks and review

- [`ci.yml`](ci.yml): main push, PR, merge queue and `workflow_call`: backend fmt, Clippy, cross-platform tests, `CGAH_REQUIRE_*` sandbox/lifecycle jobs, fake-model bakeoff, browser contracts, deny and invariant guard. Qualifying docs-only PRs skip selected compile/test jobs; fmt, browser, deny and invariant checks still run.
- [`pr-template-check.yml`](pr-template-check.yml): PR open/edit/sync/reopen/ready events: PR-body template and base-is-main checks.
- [`review-gate-request.yml`](review-gate-request.yml): PR, review and review-comment events, fork and Dependabot PRs included, with no permissions or secrets: a wake-up signal that cannot write a verdict.
- [`review-gate.yml`](review-gate.yml): `workflow_run` of that request, plus Codex summary comments: the base-branch dispatcher. It checks the source run's identity through the API, never downloads its artifacts or runs PR code, then calls the evaluator.
- [`review-gate-evaluate.yml`](review-gate-evaluate.yml): `workflow_call` only: marks the `review gate` check in progress, polls open threads and running Codex reviews with a read-only token, then publishes the check. Open threads are neutral; only an evaluation error closes the check as failed. Resolving a thread alone fires no event: re-run the dispatcher after the poll window. Advisory, never a merge blocker.
- [`../rulesets/main-protection.json`](../rulesets/main-protection.json): importable template for required CI checks, queued squash merges, deletion/rewrite protection and PR-only admin bypass. Its presence does not prove active enforcement. Required checks, workflow enablement, merge queue and auto-merge are separate repository settings.

## Packaging

- [`bundle.yml`](bundle.yml): `main` pushes only: packages CLI tarballs and calls `desktop.yml`; PR validation stays separate.
- [`desktop.yml`](desktop.yml): bundle/release call or manual: universal macOS packaging, desktop fmt/Clippy/tests/deny, packaged backend contracts and extracted-bundle verification.
- [`release.yml`](release.yml): `v*` tag, twice-daily cron or manual: plan, verify via `ci.yml`, package and publish through the release environment; manual defaults to preview.

## Lint and scanners

- [`zizmor.yml`](zizmor.yml): path-filtered: zizmor and pinned actionlint on workflows, plus `test-release-plan.py` and `test-review-gate.mjs`.
- [`script-lint.yml`](script-lint.yml): path-filtered: shellcheck, Python compile, `node --check` on scripts and hooks.
- [`rust-clippy.yml`](rust-clippy.yml): push, PR, weekly: the `ci.yml` clippy invocation published as SARIF; not the gate.
- [`codeql.yml`](codeql.yml): push, PR, weekly: CodeQL for Rust and Actions.
- [`devskim.yml`](devskim.yml): push and PR to `main`, weekly: DevSkim SARIF scan.
- [`gitleaks.yml`](gitleaks.yml): push, PR, weekly: pinned gitleaks over the full reachable history.
- [`advisories.yml`](advisories.yml): daily or manual: cargo-deny RustSec advisories on backend and desktop manifests.

## Path-filtered and live

- [`netconnect.yml`](netconnect.yml): netconnect paths: flag, scope and strace no-packet proof.
- [`live-ollama.yml`](live-ollama.yml): daily, manual and path-filtered PRs: real Ollama fingerprint on Linux and macOS against `scripts/live-ollama/baseline.json`; not required.
- [`install-macos.yml`](install-macos.yml): installer/launcher PRs and manual: runs `install-macos.sh` and `start-macos.sh` end to end on `macos-latest`; not required.
- [`copilot-setup-steps.yml`](copilot-setup-steps.yml): Copilot agent pre-step wiring `.githooks`; runs on edits to itself or manually.
