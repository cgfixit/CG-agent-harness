# tests

Integration tests, one file per surface, run by CI with `GROK_API_KEY`,
`ANTHROPIC_API_KEY` and `DEEPAGENT_API_KEY` blanked. `invariant_guard.rs` is the
structural gate (I6, shim whitelist, shipped defaults, docs budget); run it
alone after structural or doc edits. `common/` holds the shared in-process
server and fixtures, `fixtures/` the data, `netconnect_support/` the
netconnect flag table. Never run the whole suite locally; CI does.
