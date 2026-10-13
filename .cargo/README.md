# .cargo

Cargo policy shared by the backend crate and `desktop/`; config discovery walks
up to this directory, so both crates read it.

- [`config.toml`](config.toml): sets `incompatible-rust-versions = "fallback"` so
  `cargo update` respects each crate's MSRV (backend 1.88, desktop 1.90).

Related: [docs/DEPENDENCIES.md](../docs/DEPENDENCIES.md) (pins, watches, audits),
[rust-toolchain.toml](../rust-toolchain.toml), [desktop/rust-toolchain.toml](../desktop/rust-toolchain.toml).
