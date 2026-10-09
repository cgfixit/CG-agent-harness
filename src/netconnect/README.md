# src/netconnect

Passive, fail-closed LAN observation: `cli.rs` (`status`, `devices`; exit codes
0/2/3/4), `scope.rs` (operator CIDRs inside RFC1918 or 127/8, prefix /16 or
longer), `config.rs` (`tier_may_run`), `tools.rs` (tier tools register only when
armed), `slash.rs` (`/net` and aliases), `collect.rs`/`sources.rs` (readers),
`sanitize.rs`/`parse.rs` (untrusted strings). Every gate ships false. Docs:
[netconnect.md](../../docs/netconnect.md).
