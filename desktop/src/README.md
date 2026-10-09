# desktop/src

Rust sources of the desktop shell: `main.rs` (Tauri app), `backend.rs` (spawns
the bundled `cgagentharness` sidecar and pins its home), `instance.rs` (single
instance), `trust.rs` (sidecar digest and local TLS trust). The shell holds
process ownership, never API authority. Lifecycle:
[PROCESS_LIFECYCLE.md](../../docs/PROCESS_LIFECYCLE.md).
