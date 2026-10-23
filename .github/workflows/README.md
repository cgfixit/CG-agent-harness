# .github/workflows

Find a workflow by purpose below; its `name` appears in the Actions sidebar.
Lint edits with actionlint and zizmor. See [CI expectations](../../docs/INSTALL.md#tests-and-cicd)
and [release controls](../../docs/RELEASING.md).

## Checks and reviews

| File | Purpose and trigger |
|---|---|
| [`ci.yml`](ci.yml) | Rust formatting, Clippy, dependency policy, browser, sandbox and lifecycle tests; PR, main push, merge queue and release calls. |
| [`pr-template-check.yml`](pr-template-check.yml) | PR-body requirements and base-is-main checks; PR open/edit/sync/reopen/ready events. |
| [`review-gate-request.yml`](review-gate-request.yml) | Unprivileged wake-up signal from PR, review and review-comment events, including forks. |
| [`review-gate.yml`](review-gate.yml) | Trusted dispatcher; validates request-run identity or handles Codex summary comments, then calls the evaluator. |
| [`review-gate-evaluate.yml`](review-gate-evaluate.yml) | Reusable advisory check: unresolved threads/running reviews are neutral; evaluation errors fail. Re-run the dispatcher after resolving threads. |

The [docs-only classifier](../../scripts/ci-docs-only.sh) skips compile jobs only
on qualifying PRs; browser, formatting and invariant checks still run.
[main-protection.json](../rulesets/main-protection.json) is an importable ruleset,
not evidence of active protection. Required checks, merge queue, auto-merge and
workflow enablement are separate GitHub settings. Inspect them before relying
on enforcement; the review gate remains advisory.

## Packages and releases

| File | Purpose and trigger |
|---|---|
| [`bundle.yml`](bundle.yml) | CLI archives and universal macOS app artifacts on main pushes; does not publish releases. |
| [`desktop.yml`](desktop.yml) | Reusable universal macOS build, dependency policy and bundle verification; Bundle, release or manual. |
| [`desktop-check.yml`](desktop-check.yml) | Desktop crate fmt, Clippy, tests and dependency policy when `desktop/` or `local_tls.rs` change; PR and merge queue, skipped otherwise; advisory until required. |
| [`release.yml`](release.yml) | Plan, verify, package and publish; stable tags, twice-daily schedule or manual preview/publish. Uses the release environment. |

## Lint, security and targeted checks

| File | Purpose and trigger |
|---|---|
| [`workflow-lint.yml`](workflow-lint.yml) | actionlint, zizmor and release/review fixtures; filtered pushes/PRs or manual. |
| [`script-lint.yml`](script-lint.yml) | ShellCheck, Python compilation, Node syntax and disposable Git-hook behavior checks; filtered pushes/PRs or manual. |
| [`clippy-sarif.yml`](clippy-sarif.yml) | Clippy findings in Security; separate from CI's Clippy gate; main push, PR, weekly or manual. |
| [`codeql.yml`](codeql.yml) | Rust and Actions code scanning; main push, PR, weekly or manual. |
| [`devskim.yml`](devskim.yml) | Source security patterns; main push, PR targeting main or weekly. |
| [`gitleaks.yml`](gitleaks.yml) | Full-history secret scan; main push, PR, weekly or manual. |
| [`dependency-advisories.yml`](dependency-advisories.yml) | RustSec advisories for backend, desktop and the sandbox fixture; daily or manual. |
| [`netconnect.yml`](netconnect.yml) | Scope/flag tests and Linux no-packet proof; filtered pushes/PRs or manual. |
| [`live-ollama.yml`](live-ollama.yml) | Real-model fingerprints on Linux/macOS; filtered PRs, daily or manual; downloads models. |
| [`install-macos.yml`](install-macos.yml) | Installer and launcher acceptance; filtered PRs or manual. |
| [`copilot-setup-steps.yml`](copilot-setup-steps.yml) | Copilot's reserved setup file/job installs tracked hooks; self-edits or manual. |
