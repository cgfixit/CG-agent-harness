# .cargo

Cargo policy shared by the backend crate and `desktop/`; config discovery walks
up to this directory, so both crates read it.

- [`config.toml`](config.toml): sets `incompatible-rust-versions = "fallback"` so
  dependency resolution prefers versions compatible with each crate's MSRV
  (backend 1.88, desktop 1.90). This is a preference, not a compatibility gate;
  explicit version selection can override it.

Related: [docs/DEPENDENCIES.md](../docs/DEPENDENCIES.md) (pins, watches, audits),
[rust-toolchain.toml](../rust-toolchain.toml), [desktop/rust-toolchain.toml](../desktop/rust-toolchain.toml).
