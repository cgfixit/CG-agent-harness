# src/server

The loopback HTTP/HTTPS console. `routes/` registers every path; `guards.rs` and
`headers.rs` are core paths (auth, CSRF, security headers). Chat, sessions,
structured memory, web research, MCP client and inbound memory server, Ollama
management, spend, notifications, schedules and config reload each have a module.
Agentic work goes out through `crate::shim`, never in-process. Routes:
[API_ROUTES.md](../../docs/API_ROUTES.md).
