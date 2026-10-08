# .github/workflows

`ci.yml` is the merge gate (fmt, clippy, tests with planner keys blanked, deny,
invariant guard); `pr-template-check.yml` enforces the PR body and `base branch
is main`; `review-gate.yml` is advisory and never fails. `desktop.yml`,
`bundle.yml` and `release.yml` package macOS builds. The rest are scanners:
`codeql.yml`, `devskim.yml`, `gitleaks.yml`, `advisories.yml`, `rust-clippy.yml`,
`zizmor.yml` (workflow lint), plus `netconnect.yml` and `copilot-setup-steps.yml`.
Lint workflow edits with actionlint and zizmor rather than running them.
