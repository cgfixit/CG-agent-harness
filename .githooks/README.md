# .githooks

Agent-neutral git hooks: `pre-commit` and `pre-push` source `_security.sh`
(secret and filename scan, protected control files, core-path reminder,
`cargo fmt`) with repository values from `security.conf`. Install once per
clone with `bash scripts/ensure-githooks.sh`, which points `core.hooksPath`
here. Explainer: [GITHOOKS.md](../docs/GITHOOKS.md).
