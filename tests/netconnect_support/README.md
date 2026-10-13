# tests/netconnect_support

`flag_table.rs`: every combination of the netconnect master switch, one tier
flag and scope, iterated by `netconnect_flags.rs` so tool registration and
refusals are checked per combination. The tier list is `Tier::ALL`; there is no
second flag list.
