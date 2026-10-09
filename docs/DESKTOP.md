# CG Agent Harness for macOS

The universal macOS app (Apple Silicon and Intel) opens the console in a native
WKWebView and owns its bundled Rust backend. Ordinary launch needs no Terminal,
external browser, frontend server, Rust, or Python. Coding checks still need their
configured tools. Native acceptance evidence lives in PR bodies, not in a
standing checklist.

## Install and launch

1. Unzip the universal archive and move **CG Agent Harness.app** to Applications
   (or a directory you own). Quit an existing copy before replacing it.
2. Open it from Finder or the Dock. The app installs no login item or service.
3. Fresh homes enable HTTPS, accounts, roles and web controls. The sign-in
   screen shows `admin` / `admin`.
   Signing in opens password replacement; closing it does not unlock the portal.
   Replace the password with at least 12 characters.
   The native webview verifies its owned certificate without system trust changes.
   Existing home configuration is preserved. Use **Harness → Setup and recovery**
   (Cmd-,) for diagnostics.
4. Administrators manage provider credentials in **API Keys**. SerpAPI saves
   apply immediately; other saved values need a restart. Inherited environment values win. A
   harness metadata key never grants account access. See [migration, roles and
   TLS recovery](SECURE_RESEARCH.md). After restart, `/model use grok` or
   `/model use claude` selects that cloud provider. Local prompt context is
   excluded; the coding planner remains separate.
5. Web starts enabled with an empty URL allowlist. An administrator grants sources
   with `/web allow`; operators can then ask chat to fetch permitted URLs or search
   Google. `/web pages` and `/web research` retain permitted-page retrieval.
   Launch fetches no content. With the shipped local model tag installed, chat
   needs no config edit. `/model list` and
   `/model use <exact-tag>` select another installed chat model.

The account bar displays the role. Auditors have read-only status and cannot use
chat, research, users, API Keys, or Sessions data.

This build is **ad-hoc signed, universal (Apple Silicon + Intel), and not notarized**.
Its signature checks integrity, not publisher identity. Gatekeeper may require per-app approval
through macOS Privacy & Security, or reject it under managed policy. Do not
change global Gatekeeper settings or strip quarantine as a blanket workaround.
The bundle targets macOS 12 or newer; the 2026-09-21 development acceptance ran
on macOS 27.0 (26A428).
Older supported versions remain untested.

## Spend and job notifications

The packaged console exposes [spend and optional completion webhooks](SPEND_AND_NOTIFICATIONS.md).
**Tokens and cost** reads the ledger;
**Estimate draft** counts the selected cloud draft without generation. Claude
counting sends that draft to the provider. Webhook settings and saved bearer
changes require Cmd-Q and relaunch. The private outbox persists pending
deliveries across quit and rechecks grants before resuming.

## Home and credentials

Home precedence is `CGAGENTHARNESS_HOME`, then `USERPROFILE` or `HOME` plus
`.CGagentHarness`; overrides must be absolute. The app does not migrate data to
Application Support. Config, sessions, notes, persona, keys, and run evidence
stay outside the bundle. Check the home in the footer or Setup, and record it
separately from `Contents/Resources/COMMIT` when comparing app copies.

Desktop and headless `serve` load managed keys from the OS credential
store (macOS Keychain, Linux Secret Service, Windows Credential Manager). The
service name includes the canonical home, so two homes do not share entries.
The keyring target uses the platform default: a macOS keychain domain or the sole
Windows credential name. An inherited environment value wins, including empty.
A one-time migration reads legacy home `.env`: each managed line is written,
read back, and removed only after the values match. Unknown lines stay. A failed
store write or verify leaves that line, shows a setup warning without the value,
and does not load it. API keys stay out of argv, user-facing URLs, readiness
files, diagnostics, and errors. SerpAPI sends its key only in that
fixed provider's HTTPS request query, never to result URLs. Config/key parse
failures show a setup error without quoting values. Existing configuration is
preserved; fresh homes enable accounts and roles. No desktop session elevation exists.

Settings save and clear go to the OS store. If it is unavailable, save is refused
and `.env` is unchanged. Literal `security.allow_plaintext_key_file: true`
(default false, restart required) enables the legacy private `.env`; quoted
`"true"` stays off. On Unix, reads require a regular, singly linked file owned by
the current uid with mode 0600. On Windows, reads require a regular non-reparse
file owned by the effective user with a verifiable DACL that allows only that
user. The Windows writer creates and verifies an empty stage with an explicit owner
and protected DACL, then writes, syncs and replaces through that handle.
The reader verifies owner and DACL but does not require the DACL's
protected flag. Unsafe or unreadable files refuse startup. SerpAPI key changes
apply immediately; other keys need a restart.
A SerpAPI key selects API-backed Google results without a Google page grant;
linked pages still require URL permission. Without a key, public Google may hit
JavaScript or CAPTCHA blocks. Other provider gates
remain independent. Loading a key never grants content, account, or repository-write
access. Linux without a Secret Service session (`DBUS_SESSION_BUS_ADDRESS` unset)
fails closed. Headless macOS may refuse Keychain access for an unsigned binary
instead of prompting.

Webview storage is private. Sign in after reopening; provider keys remain
server-side. Sessions, notes, settings, and retained jobs persist through
the backend. `/agent jobs` and `/agent runs` rediscover retained work.

## Ownership, ports and security boundaries

The process topology is:

```
CG Agent Harness.app (Tauri/WKWebView)
  └─ bundled cgagentharness desktop (owned 127.0.0.1:ephemeral-port)
       └─ same bundled cgagentharness agentic …
            └─ governed checks in the existing native execution sandbox
```

The shell verifies the sidecar's build-time SHA-256, launches its absolute bundle
path from `/`, then verifies the protocol, child PID, and a fresh challenge over
inherited stdin/stdout and certificate-pinned HTTPS readiness. The backend binds
port zero before reporting its address, so it cannot adopt an existing listener.
The endpoint applies account, origin, CSRF, and rate checks.

The WebView reaches the shell through four Tauri commands in `desktop/src/main.rs`:
`desktop_status`, `retry_backend`, `check_models`, and `prepare_cargo`. They accept
no command line; the backend HTTP API remains the operational surface.
Explicit legacy HTTP configuration is preserved. A readiness
challenge provides no operator API authority. `serve --port …` and worker CLI
behavior remain available independently.

An OS-held home lock prevents concurrent desktop and headless server writers on
Unix. A private, user/home-scoped Unix socket only focuses an existing desktop;
it carries no secrets, commands, or PID authority. Different
homes can run independently. Independent manually invoked agentic CLI commands
are not excluded by the server home lock; do not run concurrent writers manually.

Only the owned exact origin loads in the console webview. Navigation to other
origins is denied; validated HTTP(S) links need native confirmation before the
OS opens an external browser. New webviews and downloads are denied. The
console has no download/export command; plan and PR-body file inputs remain,
with native chooser acceptance pending. Model/web content uses the
existing text renderer and nonce CSP. The external console window receives **no
native capability**. Only bundled Setup can invoke status, retry, local inventory,
and offline preparation through a native folder picker. There is no generic shell,
arbitrary file API, analytics, updater, or crash upload.

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
Path presence does not prove GitHub readiness or a prepared build; each operation
validates its tools.
For explicit operator tool locations, a private, current-user-owned
`desktop-tools.json` in the application home may contain:

```json
{"directories":["/absolute/operator-owned/tool-directory"]}
```

At most eight directories are allowed. Each must be absolute, exist, belong to
the user, and reject other-user writes. Setup lists default discovery; explicit
directories apply to backend operations and preparation.

**Check installed chat and planner models** queries configured loopback inventories
with a deadline, response bound, no proxy, and no redirect. It reports each exact
tag as present, absent, or failed. It never downloads models or probes cloud planners.
Submit a short console chat to test inference. `/model use <exact-tag>` changes chat;
planner settings remain in `agentic.deepagent_github`. Do not infer MLX or any
other backend merely from a tag suffix. Start the already-installed Ollama app
if its endpoint is unavailable; no runtime is automatically switched.

When local fallback is enabled, HTTP success is insufficient; inventory must list
the exact model. A missing primary permits fallback; two missing tags leave the
primary degraded. Disabled fallback makes no inventory request, and no other tag
is substituted. `models.local_llm.inventory`
sets a default 2-second timeout and 262144-byte response cap; supported bounds
are 0.1–30 seconds and 1024–1048576 bytes. Fallback retains its separate
`probe_timeout_sec` (default 1.5, now validated to 0.1–30 finite seconds).
Inventory refuses malformed values. Existing homes without this block retain
their old limits without a config rewrite.

**Choose repository and prepare offline** runs the bundled `prepare-cargo.py`
with fixed arguments and an actual native folder choice. Python 3, Cargo, the
selected Rust toolchain, SDK, Cargo.lock and cached sources are prerequisites.
It creates the read-only preparation inventory without downloading dependencies
or models. Setup discards tool output and shows bounded static results. Missing inputs require explicit
operator preparation; online preparation remains an explicit CLI helper option.
The app has no automatic toolchain installation.

The app adds no automatic cloud inference, model pull, authenticated external
probe, or telemetry. Existing web, GitHub, and cloud features retain their gates.
A loopback model endpoint does not prove its separate service stays offline.

## Closing, quitting and recovery

Close hides a window and keeps the backend alive. Dock reopen and a second
launch focus the existing home instance. Cmd-Q asks **Keep running** or **Cancel
work and quit** when chat, a job, a shim operation or preparation is active.
Shutdown closes the private channel, cancels owned work, and waits before
terminating its direct child. Parent death requests the same shutdown through
pipe EOF. Backend failure returns to Setup with a
controlled retry. Startup protocol reads have a 20-second deadline; OS file
access mediation and kernel I/O can delay pre-launch file access beyond it.
If startup keeps waiting, check for a macOS file-access prompt, quit the copy,
move the complete app to Applications, and retry. Do not delete the home.

`data/agentic/console-jobs.json` retains at most 32 terminal jobs plus one active
job within 16 MiB. Restart marks a recovered running job interrupted; it never
reattaches or resumes. A persistence failure refuses execution or reports an
unsaved outcome.

`/agent runs` lists bounded recent records and reconciles released
Unix worker leases to `interrupted`. A live lease remains running. Legacy running
records without ownership evidence stay unknown and cannot be discarded by a
stale-state guess. `/agent status <id>` inspects retained evidence and available
diff; `/agent discard <id>` removes interrupted or decided work through the
existing jailed worker operation. Pending proposals still require a decision.
No approval, commit, push, publication or model request is replayed at startup.
Run evidence is retained separately from the rolling console-job list.

Audit, spend, and optional metrics JSONL sinks retain a current file and one `.1`
file, default 8 MiB each (`logging.max_file_bytes`, 64 KiB–64 MiB). Existing
oversized logs are retained as the first previous generation until the next
rotation. Busy/unavailable sinks or oversized events produce a warning and drop
that event; logging remains best-effort. The desktop discards arbitrary child
stderr and keeps no growing stdout/stderr log. Private frames, model
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
rustup target add --toolchain 1.88 aarch64-apple-darwin x86_64-apple-darwin
rustup target add --toolchain 1.90 aarch64-apple-darwin x86_64-apple-darwin
scripts/package-desktop.sh --universal --dmg
(cd desktop && cargo fmt -- --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked)
cargo deny --manifest-path desktop/Cargo.toml --config desktop/deny.toml check
CGAH_TEST_BINARY='dist/CG Agent Harness.app/Contents/MacOS/cgagentharness' python3 scripts/test-desktop-backend.py
scripts/verify-desktop-bundle.sh 'dist/CG Agent Harness.app' universal
(cd dist && shasum -a 256 -c SHA256SUMS)
```

Use rustup's Cargo so each package's toolchain file applies. Build and sign the
universal sidecar before compiling either shell slice's embedded hash. The
packager combines both backends with `lipo`, signs that file, then builds both
shells against its SHA-256. Do not re-sign the sidecar afterward.
`scripts/verify-desktop-bundle.sh` hashes the staged `Contents/MacOS/cgagentharness`
after the final `.app` codesign and requires that SHA-256 to still appear in the
shell binary (`CGAH_BACKEND_SHA256`). Packaging refuses a
dirty tree unless `CGAH_ALLOW_DIRTY=1`, which marks the bundle as development.
It produces `.app`, ZIP, optional DMG, and SHA256SUMS in `dist/`; the bundle's
`Contents/Resources/COMMIT` identifies the source commit. Build outputs are not
committed. Universal archives are named `CG-Agent-Harness-macos-universal.zip`;
omitting `--universal` retains an arm64 development build. Tagged releases wait for
both backend CI and this desktop job before attaching CLI and app packages.
Cross-building Intel is not native Intel acceptance. Packaging is repeatable;
byte-identical rebuilds across environments are not claimed.

Reusable Desktop CI checks both architectures, system linkage, nested signatures,
resources, CLI/worker dispatch, policy tests, extracted ZIP, and dependencies. Artifacts retain
the exact SHA for 14 days. Backend CI remains separate. A successful build is not
GUI acceptance.

Tauri is pinned to 2.11.5. Its `tauri-utils` uses upstream commit
`dd725f4b13c30a86b398ccc59eb498f151f461c5` to replace the unmaintained rust-unic
chain with ICU through urlpattern 0.6. This requires desktop Rust 1.90; backend
Rust 1.88 remains independently pinned. Each crate owns its lockfile; see
[dependency maintenance](DEPENDENCIES.md). Desktop dependency policy uses the same
advisory/license/source rules, scoped to both shipped macOS targets,
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
