# desktop/src

Rust sources of the desktop shell. The shell holds process ownership, never
API authority. Lifecycle: [docs/PROCESS_LIFECYCLE.md](../../docs/PROCESS_LIFECYCLE.md).

- [`main.rs`](main.rs): Tauri app entry: the setup window, its four commands, status and retry flow.
- [`backend.rs`](backend.rs): spawns the bundled sidecar, pins its home, reads its private status pipe.
- [`instance.rs`](instance.rs): user- and home-scoped single-instance lock over a Unix socket; only requests focus.
- [`trust.rs`](trust.rs): per-webview trust of the exact sidecar certificate; no keychain edits, no bypass.

Contract tests: [scripts/test-desktop-backend.py](../../scripts/test-desktop-backend.py).
