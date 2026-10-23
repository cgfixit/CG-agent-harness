# desktop/src

Rust sources of the desktop shell. The shell holds process ownership, never
API authority. Lifecycle: [docs/PROCESS_LIFECYCLE.md](../../docs/PROCESS_LIFECYCLE.md).

- [`main.rs`](main.rs): Tauri app entry: the setup window, its four commands, status and retry flow.
- [`backend.rs`](backend.rs): spawns the bundled sidecar, pins its home, reads its private status pipe.
- [`instance.rs`](instance.rs): user- and home-scoped file lock; a Unix socket only requests focus.
- [`trust.rs`](trust.rs): per-webview trust of the exact sidecar certificate; no keychain edits, no bypass.
- [`web_launcher.rs`](web_launcher.rs): recognizes the owned console's exact
  open-web signal and launches the integrity-checked bundled script in Terminal
  after native confirmation.

Contract tests: [scripts/test-desktop-backend.py](../../scripts/test-desktop-backend.py).
