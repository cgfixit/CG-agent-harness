# src/common

Primitives shared by every side of the I6 boundary: config and `flag_is_true`
(`config.rs`), the harness home (`home.rs`), audit and redaction (`audit.rs`),
accounts and scrypt (`authn.rs`, `auth_store.rs`), credential store, atomic
writes, rate limits, the tool broker, MCP stdio spawning (`mcp.rs`,
`mcp_worker.rs`) and sandbox wrappers. `process/` is the argv-only subprocess
runner. No module here may import `agentic`.
