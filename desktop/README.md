# desktop

The Tauri macOS 12+ shell, a separate crate on Rust 1.90. It launches and trusts
the bundled `cgagentharness` sidecar; it holds process ownership, never API
authority. Packaged by [scripts/package-desktop.sh](../scripts/package-desktop.sh),
verified by [scripts/verify-desktop-bundle.sh](../scripts/verify-desktop-bundle.sh).
Docs: [DESKTOP.md](../docs/DESKTOP.md), [RELEASING.md](../docs/RELEASING.md),
[PROCESS_LIFECYCLE.md](../docs/PROCESS_LIFECYCLE.md).

- [`Cargo.toml`](Cargo.toml) and [`Cargo.lock`](Cargo.lock): the `cg-agent-harness-desktop` manifest (Tauri 2) and its lockfile.
- [`rust-toolchain.toml`](rust-toolchain.toml): pins Rust 1.90 with clippy and rustfmt.
- [`deny.toml`](deny.toml): Apple-target cargo-deny policy; telemetry bans match the backend.
- [`build.rs`](build.rs): embeds the SHA-256 of the prebuilt backend as `CGAH_BACKEND_SHA256`, then `tauri_build`.
- [`tauri.conf.json`](tauri.conf.json): strict CSP, the `setup` capability, `ui/` as `frontendDist`, ad-hoc signing.
- [`Info.plist`](Info.plist): bundle metadata: identifier, icon, local networking only.
- [`src/`](src/README.md): Rust sources: Tauri app, sidecar spawn, single instance, certificate trust.
- [`ui/`](ui/README.md): the static setup window.
- [`capabilities/`](capabilities/README.md) and [`permissions/`](permissions/README.md): what the setup window may call.
- [`icons/`](icons/README.md): app icon sources.
