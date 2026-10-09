# src/agentic

The coding pipeline, run only as a child process (`cli.rs`, `commands.rs`
dispatch). `writer.rs` and `workspace.rs` are core paths: every write needs
`confirm` and `reason`, stays inside the clone jail, and honours the gates in
`config.rs`. `proposer.rs`/`cloud_proposer.rs` plan, `edits.rs` applies,
`executor/` verifies, `git.rs`/`gh_client.rs` publish, `run_store.rs` records.
Doc: [CODING_PIPELINE.md](../../docs/CODING_PIPELINE.md).
