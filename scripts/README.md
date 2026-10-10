# scripts

Operator and CI helpers. Verification: `verify-local.sh`, `check-pr-template.sh`,
`ci-docs-only.sh`, `test-desktop-backend.py`, the `test-*.mjs` browser checks,
`smoke-ollama.sh` (with `FINGERPRINT_OUT`, plus `live-ollama/` for a pinned
Ollama and models, the live-model fingerprint CI compares).
`qwen3.8-27b-mlx-cg.Modelfile` is the local `ollama create` recipe that pins
`num_ctx` 32768; it is not a fingerprint model. Packaging: `package-desktop.sh`, `package-release.sh`,
`verify-desktop-bundle.sh`, `release-plan.py`, `prepare-cargo.py`,
`desktop-icon.swift`. Hooks: `ensure-githooks.sh`, `install-githooks.sh`.
Fixtures and probes: `browser-fixture.py`, `mcp-fixture.py`,
`mcp-windows-fixture.py`, `grok-acp-probe.py`, `netconnect-syscall-proof.sh`.
Only the few named in `.claude/settings.json` are pre-approved for Claude.
