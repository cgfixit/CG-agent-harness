# tests

Integration tests, one file per surface, run by CI with `GROK_API_KEY`,
`ANTHROPIC_API_KEY` and `DEEPAGENT_API_KEY` blanked. `invariant_guard.rs` is the
structural gate (I6, shim whitelist, shipped defaults, docs budget); run it
alone after structural or doc edits. Platform files (`linux_bwrap`,
`mcp_lifecycle`, `mcp_windows`, `windows_private_credentials`) skip a missing
capability unless a `CGAH_REQUIRE_*` variable is set; a dedicated `ci.yml` job
sets each one, so every file runs unskipped somewhere. `macos_cargo` never
skips. Script contracts (`ci_docs_only`, `pr_template_check`,
`start_web_script`, `smoke_wired_count`) lock `scripts/` behavior. `common/`
holds the shared in-process server and fixtures, `fixtures/` the data,
`netconnect_support/` the netconnect flag table. Never run the whole suite
locally; CI does.
