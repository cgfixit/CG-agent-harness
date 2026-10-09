# desktop

The Tauri macOS shell (separate crate, Rust 1.90): `src/` launches and trusts the
bundled sidecar, `ui/` is the setup window, `capabilities/` and `permissions/`
scope what that window may call, `icons/` the app icon. Packaged by
`scripts/package-desktop.sh`, verified by `scripts/verify-desktop-bundle.sh`.
Docs: [DESKTOP.md](../docs/DESKTOP.md), [RELEASING.md](../docs/RELEASING.md).
