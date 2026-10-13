# src

The `cgagentharness` crate. Isolation (I6): `server/`, `shim/`, `llm/`,
`common/` and `netconnect/` never reference `agentic/`, and `agentic/` never
references the server or shim. The only crossing is `shim/`, which spawns
`cgagentharness agentic <action>` as a child process.
[tests/invariant_guard.rs](../tests/invariant_guard.rs) fails the build if that
changes. Contracts: [INVARIANTS.md](../INVARIANTS.md); map: [AGENTS.md](../AGENTS.md#the-map).

- [`main.rs`](main.rs): binary entry; clap CLI routing the public subcommands plus hidden `agentic`, `desktop`, `mcp-stdio-worker`.
- [`lib.rs`](lib.rs): library root; declares the six modules, documents the I6 layout, exports `VERSION`.
- [`server/`](server/README.md): the loopback HTTP/HTTPS console behind `serve`; `build_app` is the test boundary.
- [`shim/`](shim/README.md): **core path.** The only server-to-agentic edge; spawns `agentic` children.
- [`agentic/`](agentic/README.md): the coding pipeline, child process only.
- [`llm/`](llm/README.md): local OpenAI-compatible and Ollama backends, the explicit cloud client, spend ledger.
- [`common/`](common/README.md): primitives shared by both sides (config, home, audit, auth, process runner).
- [`netconnect/`](netconnect/README.md): fail-closed LAN observation; gates ship closed, only `diag` dials.
