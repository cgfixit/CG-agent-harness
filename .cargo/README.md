# .cargo

Cargo policy shared by the backend crate and `desktop/` (config discovery walks
up to this directory). `config.toml` sets `incompatible-rust-versions = "fallback"`
so `cargo update` respects each crate's MSRV (backend 1.88, desktop 1.90).
See [DEPENDENCIES.md](../docs/DEPENDENCIES.md).
