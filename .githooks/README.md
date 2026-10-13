# .githooks

Agent-neutral git hooks. Install once per clone with
`bash scripts/ensure-githooks.sh`, which points `core.hooksPath` here.
Explainer: [docs/GITHOOKS.md](../docs/GITHOOKS.md). A speed bump; CI secret
scanning and the `main` ruleset are the controls.

- [`pre-commit`](pre-commit): every commit: secret and filename scan, protected control files, core-path reminder, `cargo fmt`.
- [`pre-push`](pre-push): every push: refuses pushes to `main` and non-fast-forward pushes; scans every pushed commit.
- [`_security.sh`](_security.sh): the sourced gate both hooks share: scans, protected files, branch and force-push guards.
- [`security.conf`](security.conf): repository values for `_security.sh`: blocked globs, protected files, core paths, allow regex.
