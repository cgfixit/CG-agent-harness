# .github/workflows

Lint workflow edits with actionlint and zizmor. See [CI expectations](../../docs/INSTALL.md#tests-and-cicd).

## Checks and reviews

| File | Purpose and trigger |
|---|---|
| [`ci.yml`](ci.yml) | Every PR gates full Windows and MCP/credential tests through `required gate`. Code PRs also test Linux and macOS. Main pushes and releases tolerate Windows Test failures. |
| [`pr-template-check.yml`](pr-template-check.yml) | PR body and base branch checks; PR open/edit/sync/reopen/ready. |
| [`docs-impact.yml`](docs-impact.yml) | Required `docs impact` checks owning guides against the candidate diff. |
| [`docs-impact-repair.yml`](docs-impact-repair.yml) | Repair missing guide updates on same-repository maintainer PRs (`pull_request_target`, base scripts only). |
| [`review-gate-request.yml`](review-gate-request.yml) | Unprivileged trigger for PRs (including forks and body edits), reviews and review comments. |
| [`review-gate.yml`](review-gate.yml) | Trusted dispatcher; validates request-run identity or handles Codex summary comments, then calls the evaluator. |
| [`review-gate-evaluate.yml`](review-gate-evaluate.yml) | Reusable advisory check: unresolved threads, running reviews and a PR body not edited since an operator's `@codex review` comment are neutral; evaluation errors fail. Re-run the dispatcher after resolving threads. |

The [ruleset](../rulesets/main-protection.json) requires up-to-date squash PRs,
`docs impact`, `required gate` and `check_otel`. Admins can bypass only through PRs. After merge, update `protect-main` and
compare `gh api repos/{owner}/{repo}/rules/branches/main` with the file.
Without bypass, verify failing and outdated PRs block and current passing PRs can merge.

Hosted [docs repair](docs-impact-repair.yml) uses `DOCS_SYNC_OPENAI_API_KEY` and
`DOCS_SYNC_GITHUB_TOKEN` (repository Contents write). `DOCS_SYNC_MODEL` selects
the model. The worker receives a bounded diff and guides and can commit to the exact head. Forks use the [author check](../../scripts/docs-impact.py). Trailers bind guide decisions to the diff.

## Packages and releases

| File | Purpose and trigger |
|---|---|
| [`bundle.yml`](bundle.yml) | CLI archives and universal macOS app artifacts on main pushes; does not publish releases. |
| [`desktop.yml`](desktop.yml) | Reusable universal macOS build, dependency policy and bundle verification; Bundle, release or manual. |
| [`desktop-check.yml`](desktop-check.yml) | Advisory desktop fmt, Clippy, tests and dependency policy for changes to `desktop/`, `local_tls.rs`, `start-web.sh`, `package-desktop.sh`, `prepare-cargo.py` or this workflow; manual dispatch also supported. |
| [`release.yml`](release.yml) | Plan, probe release permission, verify, package and publish; stable tags, each green Bundle on main, or manual preview/publish. Only `publish` uses the `release` environment (wait timer). |

## Lint, security and targeted checks

| File | Purpose and trigger |
|---|---|
| [`workflow-lint.yml`](workflow-lint.yml) | actionlint, zizmor and release/review fixtures; pushes/PRs touching workflow or lint config, or manual. |
| [`diff-lint.yml`](diff-lint.yml) | [`lint-diff.py`](../../scripts/lint-diff.py) is advisory. Checks DevSkim annotations, self-manual drift, docs caps, config prose and assertion patterns; PR or manual. |
| [`telemetry-contract.yml`](telemetry-contract.yml) | [`check_otel.py`](../../.claude/skills/cgagentharness-otel-hardening/check_otel.py) and its mutation self-test: child-environment opt-outs, telemetry-free lock graphs, classified dependencies and spawn files; every PR, merge queue, main push, weekly (dated inventory review) or manual; a required check in the importable ruleset. |
| [`script-lint.yml`](script-lint.yml) | ShellCheck, Python compilation, Node syntax, and disposable-repository Git-hook and `cg-doc-sync` trim-report checks; filtered pushes/PRs or manual. |
| [`clippy-sarif.yml`](clippy-sarif.yml) | Clippy findings in Security; separate from CI's Clippy gate; main push, PR, weekly or manual. |
| [`codeql.yml`](codeql.yml) | Rust and Actions code scanning; main push, PR, weekly or manual. |
| [`devskim.yml`](devskim.yml) | Source security patterns; main push, PR targeting main, weekly or manual. |
| [`gitleaks.yml`](gitleaks.yml) | Full-history secret scan; main push, PR, weekly or manual. |
| [`dependency-advisories.yml`](dependency-advisories.yml) | RustSec (three Cargo lockfiles) and npm (browser sidecar) advisories; daily or manual. |
| [`netconnect.yml`](netconnect.yml) | Scope/flag tests and Linux no-packet proof; filtered pushes/PRs or manual. |
| [`live-ollama.yml`](live-ollama.yml) | Real-model fingerprints on Linux/macOS; filtered PRs, daily or manual; downloads models. |
| [`install-macos.yml`](install-macos.yml) | Installer and launcher acceptance; filtered PRs or manual. |
| [`copilot-setup-steps.yml`](copilot-setup-steps.yml) | Copilot's reserved setup file/job installs tracked hooks; self-edits or manual. |
