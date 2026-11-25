# .github/workflows

Lint workflow edits with actionlint and zizmor. See [CI expectations](../../docs/INSTALL.md#tests-and-cicd).

## Checks and reviews

| File | Purpose and trigger |
|---|---|
| [`ci.yml`](ci.yml) | Rust formatting, Clippy, dependency policy, browser, sandbox and lifecycle tests; PR, main push, merge queue and release calls. |
| [`pr-template-check.yml`](pr-template-check.yml) | PR body and base branch checks; PR open/edit/sync/reopen/ready. |
| [`docs-impact.yml`](docs-impact.yml) | Check owning guides on PRs. Repair missing reviews on same-repository maintainer PRs. |
| [`review-gate-request.yml`](review-gate-request.yml) | Unprivileged wake-up signal from PR (body edits included), review and review-comment events, including forks. |
| [`review-gate.yml`](review-gate.yml) | Trusted dispatcher; validates request-run identity or handles Codex summary comments, then calls the evaluator. |
| [`review-gate-evaluate.yml`](review-gate-evaluate.yml) | Reusable advisory check: unresolved threads, running reviews and a PR body not edited since an operator's `@codex review` comment are neutral; evaluation errors fail. Re-run the dispatcher after resolving threads. |

The [docs-only classifier](../../scripts/ci-docs-only.sh) keeps browser,
formatting and invariant checks. Verify live settings against the
[importable ruleset](../rulesets/main-protection.json).

Enable hosted docs repair once with Actions secrets
`DOCS_SYNC_OPENAI_API_KEY` and `DOCS_SYNC_GITHUB_TOKEN` (fine-grained token
for this repository, Contents write). The worker sends a bounded diff
and owning guides to OpenAI and can append a docs commit at the exact
head. `DOCS_SYNC_MODEL` can select another structured-output model.
Forks use the [author check](../../scripts/docs-impact.py). Docs-impact
trailers bind unchanged-guide decisions to the production diff.

## Packages and releases

| File | Purpose and trigger |
|---|---|
| [`bundle.yml`](bundle.yml) | CLI archives and universal macOS app artifacts on main pushes; does not publish releases. |
| [`desktop.yml`](desktop.yml) | Reusable universal macOS build, dependency policy and bundle verification; Bundle, release or manual. |
| [`desktop-check.yml`](desktop-check.yml) | Desktop crate fmt, Clippy, tests and dependency policy when `desktop/`, `local_tls.rs` or `scripts/start-web.sh` change; PR, merge queue and manual dispatch, skipped otherwise; advisory until required. |
| [`release.yml`](release.yml) | Plan, probe release permission, verify, package and publish; stable tags, each green Bundle on main, or manual preview/publish. Only `publish` uses the `release` environment (wait timer). |

## Lint, security and targeted checks

| File | Purpose and trigger |
|---|---|
| [`workflow-lint.yml`](workflow-lint.yml) | actionlint, zizmor and release/review fixtures; pushes/PRs touching workflow or lint config, or manual. |
| [`diff-lint.yml`](diff-lint.yml) | [`lint-diff.py`](../../scripts/lint-diff.py) checks DevSkim annotations, self-manual drift, docs caps, config prose and assertion patterns; PR or manual. |
| [`telemetry-contract.yml`](telemetry-contract.yml) | [`check_otel.py`](../../.claude/skills/cgagentharness-otel-hardening/check_otel.py) and its mutation self-test: child-environment opt-outs, telemetry-free lock graphs, classified dependencies and spawn files; every PR, merge queue, main push, weekly (dated inventory review) or manual; a required check in the importable ruleset. |
| [`script-lint.yml`](script-lint.yml) | ShellCheck, Python compilation, Node syntax and disposable Git-hook behavior checks; filtered pushes/PRs or manual. |
| [`clippy-sarif.yml`](clippy-sarif.yml) | Clippy findings in Security; separate from CI's Clippy gate; main push, PR, weekly or manual. |
| [`codeql.yml`](codeql.yml) | Rust and Actions code scanning; main push, PR, weekly or manual. |
| [`devskim.yml`](devskim.yml) | Source security patterns; main push, PR targeting main, weekly or manual. |
| [`gitleaks.yml`](gitleaks.yml) | Full-history secret scan; main push, PR, weekly or manual. |
| [`dependency-advisories.yml`](dependency-advisories.yml) | RustSec (three Cargo lockfiles) and npm (browser sidecar) advisories; daily or manual. |
| [`netconnect.yml`](netconnect.yml) | Scope/flag tests and Linux no-packet proof; filtered pushes/PRs or manual. |
| [`live-ollama.yml`](live-ollama.yml) | Real-model fingerprints on Linux/macOS; filtered PRs, daily or manual; downloads models. |
| [`install-macos.yml`](install-macos.yml) | Installer and launcher acceptance; filtered PRs or manual. |
| [`copilot-setup-steps.yml`](copilot-setup-steps.yml) | Copilot's reserved setup file/job installs tracked hooks; self-edits or manual. |
