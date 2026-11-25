# CG Agent Harness for macOS

## Install and launch

Quit any running copy before installing the complete universal
**CG Agent Harness.app**. It is ad-hoc signed and unnotarized, targeting
macOS 12 or newer. Verify source and checksums before any per-app
Gatekeeper approval. Launch from Finder. The shell opens WKWebView
and runs its bundled backend. Fresh admin setup uses the code in
**Harness → Setup and recovery** (Cmd and +). See [first run](INSTALL.md#6-first-run).

## Home and credentials

`CGAGENTHARNESS_HOME` must be absolute when set. Otherwise the home is
`.CGagentHarness` under the user directory. Confirm it in Setup. Config,
credentials and run state outlive bundle replacement. OS-store key
lookup and restart rules are in [Install](INSTALL.md#supported-managed-keys).
Webview storage is private and a reopened app may need login.

## Ownership, ports and security boundaries

The shell verifies the bundled sidecar's SHA-256, starts its absolute path
from `/` and checks protocol, PID, inherited-pipe challenge and pinned HTTPS
readiness. The sidecar binds loopback port zero, so it cannot adopt another
listener. API guards still apply. Native Setup offers status, retry, model
checks, offline Cargo preparation and public certificate export. Readiness
authenticates the owned service, while the console still needs its own login.
The bundled backend hosts the same guarded API available to the CLI. No setup
operation accepts a command. Export uses a chooser, refuses overwrites and
installs no trust. Compare the shown fingerprint before
[browser trust](SECURE_RESEARCH.md#https-and-native-trust).

The console accepts only the owned origin. Other validated HTTP(S) links
need native confirmation before opening outside the app. New webviews and
downloads are denied. The globe opens the existing backend in a separate
browser with its own sign-in and no native capabilities.
An OS home lock excludes concurrent servers. A private Unix socket can
focus the existing desktop. The coding pipeline still crosses the I6
child boundary. Hardened signing does not provide the check sandbox.

## Finder setup and local inference

Setup discovers tools in known directories, independent of inherited
`PATH`. An owner-private `desktop-tools.json` can supply up to eight
absolute, user-owned directories that reject other-user writes, under
`{"directories":["/absolute/tool-directory"]}`.

**Check installed chat and planner models** probes eligible loopback
inventories. See [states and fallback](MODELS.md#exact-model-diagnostics-and-optional-fallback).
**Choose repository and prepare offline** runs the hash-verified bundled
preparer with fixed arguments. Supply Python, pinned Cargo/Rust, SDK,
`Cargo.lock` and cached dependencies first. Online preparation is CLI-only.

## Closing, quitting and recovery

Window close keeps the backend alive. Dock reopen and second launch focus
the same home. Cmd-Q offers **Keep running** or **Cancel work and quit**
during work. Parent pipe EOF also requests shutdown. Backend failures
return to Setup with retry. Check for a macOS file-access prompt if startup
appears stalled.

Restart marks unfinished retained jobs interrupted and never replays writes.
Use `/agent runs` and `/agent status <id>` before discarding work. A
released worker lease permits reconciliation. Retained console jobs and
longer-lived run evidence have separate storage. Preserve evidence before
archiving damaged job history, with the server stopped. Legacy running records with no
lease evidence stay unknown. Process cleanup has
[platform limits](PROCESS_LIFECYCLE.md).

## Rebuild and verify

Use rustup with pins 1.89 (backend) and 1.90 (desktop), Xcode Command Line
Tools and the requested Apple targets. For universal builds, add
`aarch64-apple-darwin` and `x86_64-apple-darwin` to both rustup toolchains. Build with
`scripts/package-desktop.sh --universal --dmg`. Omitting `--universal`
builds arm64. The packager signs the combined backend before embedding
its hash in the shell. Keep that sidecar signature. Dirty builds require
`CGAH_ALLOW_DIRTY=1` and get a development marker.

Run `scripts/verify-desktop-bundle.sh 'dist/CG Agent Harness.app' universal`,
check `dist/SHA256SUMS` and read `Contents/Resources/COMMIT`.
`scripts/test-desktop-backend.py` accepts `CGAH_TEST_BINARY` for the
bundled sidecar. Use a disposable home for WKWebView acceptance. Check Finder launch,
setup and a guarded action on the actual packaged backend. Release output
includes both Apple CPU slices. Intel cross-build success does not establish
native Intel behavior.
Desktop packaging and native interaction are separate checks.

## Update, recovery and uninstall

Verify the new archive, quit the old owner and replace the bundle.
Preserve the home and any retained run evidence. A live home lock must
be released by its owner. Removing the app leaves the home. Preview any
separate home reset with `scripts/reset-macos.sh`.

### Reset failure and recovery

The reset preflights selected Keychain entries. A later delete or lookup
failure preserves the home and reports the successful-delete count.
Completed Keychain deletions cannot be rolled back. Keep the harness
stopped, fix access and rerun on the same home. Missing items are skipped.
A home backup alone cannot restore Keychain items.

## Browser sessions

Packaged apps do not include the Playwright sidecar. Use an absolute
`browser.sidecar`, prepare its dependencies and restart.
[Browser use](Browser-Use.md) covers Chrome tasks and native computer-use
status.
