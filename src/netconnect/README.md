# src/netconnect

Fail-closed LAN observation. Every gate ships `false`; tools register only when
armed; only [`connect.rs`](connect.rs) opens a socket, and the guard scans every
other file for socket types. Docs: [docs/netconnect.md](../../docs/netconnect.md);
contract: [INVARIANTS.md](../../INVARIANTS.md).

- [`cli.rs`](cli.rs): `netconnect status|devices|diag`; exit codes 0/2/3/4; only `diag` dials.
- [`collect.rs`](collect.rs): scope filter over neighbor, route and interface sources; drops out-of-scope rows.
- [`config.rs`](config.rs): typed `netconnect` section: `Tier`, `tier_may_run`; gates read through `flag_is_true`.
- [`connect.rs`](connect.rs): the budgeted TCP `Dialer`; the only file that opens a socket.
- [`mod.rs`](mod.rs): module list and re-exports; documents the fail-closed contract.
- [`parse.rs`](parse.rs): parsers for ARP, route and interface tables; drops IPv6, skips malformed rows.
- [`sanitize.rs`](sanitize.rs): wraps device-supplied text as display-only `UntrustedString`.
- [`scope.rs`](scope.rs): operator IPv4 CIDRs inside RFC1918 or 127/8, prefix /16 to /32; `check_target`.
- [`slash.rs`](slash.rs): `/net` and exact aliases; read-only subcommands, "did you mean", never connects.
- [`sources.rs`](sources.rs): passive readers: `/proc/net`, `getifaddrs`, fixed `arp`/`netstat` argv on macOS.
- [`tools.rs`](tools.rs): model-facing `netconnect_*` tools and the console panel; register only when armed.

Tests: `tests/netconnect_*.rs` with [tests/netconnect_support/](../../tests/netconnect_support/README.md);
workflow [.github/workflows/netconnect.yml](../../.github/workflows/netconnect.yml).
