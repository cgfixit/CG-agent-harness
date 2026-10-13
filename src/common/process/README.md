# src/common/process

The Unix half of the argv-list subprocess runner in `../process.rs`: `unix.rs`
captures output over nonblocking pipes under one deadline and byte ceiling.
There is no shell-string path; MCP stdio children have their own owner in
`../mcp.rs` ([lifecycle](../../../docs/PROCESS_LIFECYCLE.md)).
