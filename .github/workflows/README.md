# .github/workflows

`ci.yml` is the merge gate (fmt, clippy, tests on Linux, macOS and Windows with
planner keys blanked, the required `CGAH_REQUIRE_*` sandbox and lifecycle jobs,
deny, invariant guard); `pr-template-check.yml` enforces the PR body and `base
branch is main`; `review-gate.yml` is advisory and never fails. `desktop.yml`,
`bundle.yml` and `release.yml` package builds. Lint: `zizmor.yml` (zizmor and
actionlint on workflows), `script-lint.yml` (shellcheck, Python compile, `node
--check` on scripts and hooks). Scanners: `codeql.yml`, `devskim.yml`,
`gitleaks.yml`, `advisories.yml`, `rust-clippy.yml` (the ci.yml clippy run
published as SARIF). Path-filtered: `netconnect.yml`, `live-ollama.yml` (real
Ollama fingerprint), `copilot-setup-steps.yml`. Lint workflow edits with
actionlint and zizmor rather than running them.
