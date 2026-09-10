# CG Agent Harness for macOS

The Apple Silicon desktop app opens the existing harness console in a native
WKWebView and owns its bundled Rust backend. Ordinary launch needs no Terminal,
external browser, frontend server, Rust or Python. Coding checks still need their
configured tools. **Native interaction acceptance is pending**; see
[DESKTOP_ACCEPTANCE.md](DESKTOP_ACCEPTANCE.md) for verified results and gaps.

## Install and launch

1. Unzip the arm64 archive and move **CG Agent Harness.app** to Applications
   (or a directory you own). Quit an existing copy before replacing it.
2. Open it from Finder or the Dock. No login item or service is installed.
3. Start using the harness without entering an API key or logging in. For an
   existing home, set `security.api_key_optional: true` in its `config.yaml` and
   restart once; saved settings are preserved on upgrade. Use **Harness → Setup
   and recovery** (Cmd-,) for diagnostics.
4. Optional key enforcement remains available: set `security.api_key_optional: false`, configure a key in the private home `.env`, restart, then enter the
   matching key in the console. Setup can save a missing key and refuses to
   replace an existing one. Account login is needed only for account management.

This build is **ad-hoc signed, arm64 only, and not notarized**. Its signature
checks integrity; it does not establish a publisher identity or satisfy normal
Developer ID distribution. Gatekeeper may require an explicit per-app approval
through macOS Privacy & Security, or reject it under managed policy. Do not
change global Gatekeeper settings or strip quarantine as a blanket workaround.
The bundle targets macOS 12 or newer; the native test machine runs 26.6.2.
Older supported deployment versions have not been tested.

## Home and credentials

Home precedence stays `CGAGENTHARNESS_HOME`, then `USERPROFILE`/`HOME` plus
`.CGagentHarness`. The override must be absolute. There is no silent migration
to Application Support. Config, sessions, notes, persona, keys and run evidence
stay outside the bundle; updating/uninstalling the app does not remove them.
The default home is `~/.CGagentHarness`.

Desktop and Unix headless `serve` startup load supported managed keys from the home's `.env` **as data**,
without a shell, `eval`, or shell startup files. The file must be a regular file,
owned by the current user, private (0600), at most 64 KiB, and not a symlink.
Explicit inherited environment values retain precedence. Unknown lines are
preserved when initializing a missing key. API keys are absent from argv,
URLs, readiness files and desktop diagnostics. Config/key parse failures show
an actionable setup error without quoting values. Existing per-user auth and
roles remain enabled only when configured; no desktop session elevation exists.

Headless `serve` uses the same private-file validation before starting its async
runtime; a missing file is allowed and an unsafe/unreadable file refuses startup.
Restart after using the key panel. Loading a key neither enables a provider nor
authorizes a connector or a write. This is dotenv support, not macOS Keychain
integration. Windows headless startup still uses explicitly supplied environment
values; native Windows credential loading remains a separate parity action.

Webview storage is private to that window, avoiding cookie collisions between
independent homes on ephemeral ports. If you opt into credential use, re-enter them after reopening.
Sessions, notes, settings and retained jobs persist through the backend, not
browser storage. `/agent jobs` and `/agent runs` rediscover retained work.

## Ownership, ports and security boundaries

The process topology is:

```
CG Agent Harness.app (Tauri/WKWebView)
  └─ bundled cgagentharness desktop (owned 127.0.0.1:ephemeral-port)
       └─ same bundled cgagentharness agentic …
            └─ governed checks in the existing native execution sandbox
```

The shell verifies the sidecar's build-time SHA-256, launches its absolute bundle
path from `/`, and verifies protocol, child PID and a fresh challenge through
inherited stdin/stdout plus authenticated HTTP readiness. The backend binds port
zero before reporting its address; there is no port-probe/rebind race and no
adoption of an existing listener. The retained local HTTP endpoint still exists
and applies optional local API-key access plus origin, CSRF, rate and account-role checks. A readiness
challenge provides no operator API authority. `serve --port …` and worker CLI
behavior remain available independently.

An OS-held home lock prevents concurrent desktop/headless server writers on
Unix. A separate private, user/home-scoped Unix socket only requests focus of an
existing desktop. It carries no secrets, commands or PID authority. Different
homes can run independently. Independent manually invoked agentic CLI commands
are not excluded by the server home lock; do not run concurrent writers manually.

Only the owned exact origin loads in the console webview. Navigation to other
origins is denied; validated HTTP(S) links need native confirmation before the
OS opens an external browser. New webviews and downloads are denied. The
existing console has no download/export command; its plan and PR-body file inputs
remain, with native chooser acceptance pending. Model/web content uses the
existing text renderer and nonce CSP. The external console window receives **no
native capability**. Only bundled Setup can invoke status, retry, local inventory
and an offline preparation action through a native folder picker. There is no
generic shell, arbitrary file API, analytics, updater or crash upload.

I6 is unchanged: the server never imports agentic implementation. All run,
approval, commit, push and publication operations cross the existing shim into
worker mode. Confirmation is not defaulted on; reasons, write gates, revocation,
accepted-tree binding and separate publication controls remain authoritative.
Signing and hardened runtime do not constitute the macOS App Sandbox or replace
the existing Seatbelt execution restrictions. No App Sandbox entitlement is added.

## Finder setup and local inference

Setup reports discovered Git, gh, Cargo, Rust, Python and Xcode tools. Discovery
uses known absolute directories (`~/.cargo/bin`, Homebrew, `/usr/local/bin`, and
system directories), never arbitrary inherited PATH or the launch directory.
Path presence is not authenticated GitHub readiness or a prepared build: the
actual operation still validates its tools and returns their failure.
For explicit operator tool locations, a private, current-user-owned
`desktop-tools.json` in the application home may contain:

```json
{"directories":["/absolute/operator-owned/tool-directory"]}
```

At most eight directories are allowed; each must exist, be absolute, owned by
the user and not writable by others. This is operator configuration, not a
repository-supplied PATH. Setup's path listing shows default discovery; explicit
directories apply to backend operations and preparation.

**Check installed chat and planner models** queries only configured loopback
inventories with a deadline, response bound, no proxy and no redirect. It reports
exact tag presence, absence or service failure separately for each model. It
never downloads models or probes a cloud planner. Use an explicitly submitted
short console chat to test inference. `/model use <exact-tag>` changes chat;
planner settings remain in `agentic.deepagent_github`. Do not infer MLX or any
other backend merely from a tag suffix. Start the already-installed Ollama app
if its endpoint is unavailable; no runtime is automatically switched.

When local fallback is explicitly enabled, its resolver uses the same inventory
contract: HTTP success alone is insufficient; the selected exact model must be
listed. A missing primary tag permits the configured fallback, while two missing
tags leave the primary marked degraded. Disabled fallback makes no inventory
request. No other installed tag is substituted. `models.local_llm.inventory`
sets a default 2-second timeout and 262144-byte response cap; supported bounds
are 0.1–30 seconds and 1024–1048576 bytes. Fallback retains its separate
`probe_timeout_sec` (default 1.5, now validated to 0.1–30 finite seconds).
Malformed values are refused when the inventory operation is used. Existing
homes without the new block retain the old desktop limits without rewriting
their configuration.

**Choose repository and prepare offline** runs the bundled `prepare-cargo.py`
with fixed arguments and an actual native folder choice. Python 3, Cargo, the
selected Rust toolchain, SDK, Cargo.lock and cached sources are prerequisites.
It creates the established read-only preparation inventory; no dependency or
model download occurs. Tool output is discarded, and Setup shows bounded static
results instead of secrets or unbounded output. Missing inputs require explicit
operator preparation; online preparation remains an explicit CLI helper option.
The app has no automatic toolchain installation.

Runtime local-only claims apply to the configured app behavior: no automatic
cloud inference, model pull, external authenticated probe or telemetry is added.
Explicit existing web/GitHub/cloud features retain their configuration and gates.
A localhost model endpoint alone does not prove that its independent service
never accesses the network; monitoring limits are recorded in acceptance.

## Closing, quitting and recovery

Close hides a window and keeps the backend alive. Dock reopen and a second
launch focus the existing home instance. Cmd-Q asks **Keep running** or **Cancel
work and quit** when chat, a job, a shim operation or preparation is active.
Shutdown closes the private channel, cancels owned jobs/chat, and waits before
terminating its direct child if necessary. Parent death causes pipe EOF and
requests the same backend shutdown. Backend failure returns to Setup with a
controlled retry. Startup protocol reads have a 20-second deadline; OS file
access mediation and kernel I/O can delay pre-launch file access beyond it.
If startup keeps waiting, check for a macOS file-access prompt, quit the copy,
move the complete app to Applications, and retry. Do not delete the home.

Console jobs persist to `data/agentic/console-jobs.json` with at most 32 terminal
jobs plus an active job, within 16 MiB. A running job recovered after restart is
interrupted, never reattached or resumed. Persistence failure refuses a new job
before execution; an outcome that cannot be saved is reported as such.

`/agent runs` lists a bounded set of recent run records and reconciles released
Unix worker leases to `interrupted`. A live lease remains running. Legacy running
records without ownership evidence stay unknown and cannot be discarded by a
stale-state guess. `/agent status <id>` inspects retained evidence and available
diff; `/agent discard <id>` removes interrupted or decided work through the
existing jailed worker operation. Pending proposals still require a decision.
No approval, commit, push, publication or model request is replayed at startup.
Run evidence is retained separately from the rolling console-job list.

Audit, spend and optional metrics JSONL sinks retain a current file and one `.1`
generation, default 8 MiB each (`logging.max_file_bytes`, 64 KiB–64 MiB). Existing
oversized logs are retained as the first previous generation until the next
rotation. Busy/unavailable sinks or oversized events produce a warning and drop
that event; logging remains best-effort. The desktop discards arbitrary child
stderr; there is no growing desktop stdout/stderr log. Private frames, model
inventory, job evidence and existing worker capture have explicit size bounds.

Cancellation is **best-effort ancestry cleanup**, not perfect process containment.
Kernel waits can exceed deadlines. Backend SIGKILL bypasses destructors; children
that reparent before observation can survive. Inspect retained runs and surviving
work before further writes. See [PROCESS_LIFECYCLE.md](PROCESS_LIFECYCLE.md).
No per-run disk, memory or process-count quota is introduced.

## Rebuild and verify

Build prerequisites: Apple Silicon macOS, Xcode Command Line Tools, Git, rustup,
backend Rust 1.88 and desktop Rust 1.90. Build-time dependency downloads are
separate from shipped runtime behavior. Install those toolchains explicitly;
the packaging script sets `RUSTUP_AUTO_INSTALL=0`.

```sh
rustup toolchain install 1.88 --profile minimal --component rustfmt --component clippy
rustup toolchain install 1.90 --profile minimal --component rustfmt --component clippy
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --locked
cargo deny check
scripts/package-desktop.sh --dmg
(cd desktop && cargo fmt -- --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked)
cargo deny --manifest-path desktop/Cargo.toml --config desktop/deny.toml check
CGAH_TEST_BINARY='dist/CG Agent Harness.app/Contents/MacOS/cgagentharness' python3 scripts/test-desktop-backend.py
scripts/verify-desktop-bundle.sh 'dist/CG Agent Harness.app'
(cd dist && shasum -a 256 -c SHA256SUMS)
```

Use rustup's Cargo on PATH so each package's toolchain file applies. Build/sign
the sidecar before compiling the shell's embedded hash. Packaging refuses a
dirty tree unless `CGAH_ALLOW_DIRTY=1`, which marks the bundle as development.
It produces `.app`, ZIP, optional DMG and SHA256SUMS in `dist/`; the bundle's
`Contents/Resources/COMMIT` identifies the source commit. Build outputs are not
committed. Dependency lockfiles and packaging steps are repeatable; byte-identical
rebuilds across SDK/signing/compiler environments are not claimed.

Desktop CI checks arm64 architecture, system linkage, nested signatures, resources,
CLI/worker dispatch, policy tests, extracted ZIP and dependencies. Artifacts retain
the exact SHA for 14 days. Backend CI remains separate. A successful build is not
GUI acceptance.

Tauri is pinned to 2.11.5. Its `tauri-utils` uses upstream commit
`dd725f4b13c30a86b398ccc59eb498f151f461c5` to replace the unmaintained rust-unic
chain with ICU through urlpattern 0.6. This requires desktop Rust 1.90; backend
Rust 1.88 and its lockfile are unchanged. Desktop dependency policy uses the same
advisory/license/source rules, scoped to the sole shipped Apple Silicon target,
with only that upstream Git repository permitted and no advisory exceptions.
This unreleased upstream pin needs review when moving to a published replacement.

## Update, recovery and uninstall

Quit the app before replacing the bundle, verify the archive checksum, then open
the new copy. Keep the home intact. If a same-home server conflict appears, quit
that server; never delete a lock file to defeat live ownership. If durable job
JSON is corrupt or oversized, stop the app and preserve a copy for inspection
before an operator explicitly archives that one job-history file. Keep separate
run records, workspaces, credentials and sessions. There is no automatic reset.

To uninstall, quit and move only the app to Trash. Removing the application home
is a separate, optional destructive action after backing up wanted data; it is
not part of uninstall. The per-user focus directory under `/private/tmp` contains
only lock/socket files, no credentials. No persistent service needs removal.

## Primary references

- [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)
- [Sidecars](https://v2.tauri.app/develop/sidecar/), [capabilities](https://v2.tauri.app/security/capabilities/), [CSP](https://v2.tauri.app/security/csp/)
- [Navigation APIs](https://docs.rs/tauri/2.11.5/tauri/webview/struct.WebviewWindowBuilder.html)
- [macOS bundles](https://v2.tauri.app/distribute/macos-application-bundle/), [signing](https://v2.tauri.app/distribute/sign/macos/)
- [macOS WebDriver limitation](https://v2.tauri.app/develop/tests/webdriver/)
- [Pinned upstream dependency change](https://github.com/tauri-apps/tauri/commit/dd725f4b13c30a86b398ccc59eb498f151f461c5)
