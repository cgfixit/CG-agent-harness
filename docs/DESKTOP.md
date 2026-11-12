# CG Agent Harness for macOS

The universal macOS app opens the console in WKWebView and owns its bundled Rust
backend. Ordinary launch needs no Terminal, browser, Rust, or Python; coding
checks need their configured tools. Native acceptance evidence lives in PR bodies.

## Install and launch

1. Unzip the universal archive and move **CG Agent Harness.app** to Applications
   (or a directory you own). Quit an existing copy before replacing it.
2. Open it from Finder or the Dock. The app installs no login item or service.
3. Fresh homes enable HTTPS, accounts, roles and web controls. Enter your setup
   code and a new password of at least 12 characters in **Set your password**.
   **Harness → Setup and recovery** shows the code with a **Copy setup code**
   button until the password is replaced; **Cmd and +** (Cmd, Shift, =) reopens
   it, and it also shows diagnostics. Closing the password form leaves a lock
   card. The webview verifies its owned certificate without system trust
   changes. Existing configuration is preserved.
4. Administrators manage provider credentials in **API Keys**. Web-key (SerpAPI,
   Tavily, Firecrawl) saves apply immediately; other saved values need a
   restart, and inherited environment values win. A harness metadata key never
   grants account access ([roles and TLS recovery](SECURE_RESEARCH.md)).
5. Web starts enabled with one seeded rule, Google search; an administrator grants
   other sources with `/web allow` ([web permissions](SECURE_RESEARCH.md#web-permissions)).
   Launch fetches no content. `/model list` and `/model use <exact-tag>` select
   an installed chat model ([Models](MODELS.md)).

The account bar displays the role. Auditors have read-only status and cannot use
chat, research, users, API Keys, or Sessions data.

This build is **ad-hoc signed, universal (Apple Silicon + Intel), and not notarized**.
Its signature checks integrity, not publisher identity. Gatekeeper may require
per-app approval in Privacy & Security, or reject it under managed policy. Do not
change global Gatekeeper settings or strip quarantine as a blanket workaround.
The bundle targets macOS 12 or newer; older versions are untested.

## Spend and job notifications

The packaged console exposes [spend, estimates and completion
webhooks](SPEND_AND_NOTIFICATIONS.md). Webhook settings and bearer changes need
Cmd-Q and relaunch; the private outbox keeps pending deliveries across quit and
rechecks grants before resuming.

## Home and credentials

Home precedence is `CGAGENTHARNESS_HOME`, then `USERPROFILE` or `HOME` plus
`.CGagentHarness`; overrides must be absolute. Config, sessions, notes, persona,
keys, and run evidence stay outside the bundle, and nothing migrates to
Application Support. Check the home in the footer or Setup, and record it
separately from `Contents/Resources/COMMIT` when comparing app copies.

Desktop and headless `serve` load managed keys from the OS credential store
([key-store contract](../INVARIANTS.md#provider-keys-live-in-the-os-credential-store)).
Keys stay out of user-facing URLs, readiness files and result links, and
config or key parse failures show a setup error without quoting values. No
desktop session elevation exists. Loading a key never grants content, account,
or repository-write access. Linux without a Secret Service session
(`DBUS_SESSION_BUS_ADDRESS` unset) fails closed; headless macOS may refuse
Keychain access for an unsigned binary instead of prompting.

Webview storage is private: sign in after reopening. Sessions, notes, settings,
and retained jobs persist through the backend.

## Ownership, ports and security boundaries

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

Setup invokes `desktop_status`, `retry_backend`, `check_models`, `prepare_cargo`,
and `export_browser_certificate` in `desktop/src/main.rs`. None accepts commands.
Certificate export uses a native save dialog, writes only the owned public leaf,
refuses existing files and never installs trust. Compare its displayed SHA-256
before following [browser trust instructions](SECURE_RESEARCH.md#https-and-native-trust).
Readiness grants no API authority; the backend API and CLI remain independent.

The globe beside **CG Agent / harness** requests native confirmation, then runs
bundled `start-web.sh` in Terminal to open the owned server in the default browser
([browser access](INSTALL.md#7-open-the-console-in-a-browser)). It starts no
second server. The script and paths come from the shell, never page content;
ordinary browser consoles hide this button.

An OS-held home lock prevents concurrent desktop and headless server writers on
Unix and Windows. A private, user/home-scoped Unix socket only focuses an existing
desktop; it carries no secrets, commands, or PID authority. Different homes run
independently. Manually invoked agentic CLI commands are not excluded by the
server home lock; do not run concurrent writers by hand.

Only the owned exact origin loads in the console webview. Navigation to other
origins is denied; validated HTTP(S) links need native confirmation before the
OS opens an external browser. New webviews and downloads are denied; session
export saves Markdown in the harness home, and file pickers remain available.
Model and web content uses the existing text renderer and nonce CSP. The
external console window receives **no native capability**; only bundled Setup
can invoke the five operations above. No generic shell, arbitrary file API,
analytics, updater, or crash upload exists.

I6 is unchanged: the server never imports agentic implementation. All run,
approval, commit, push and publication operations cross the existing shim into
worker mode. Confirmation is not defaulted on; reasons, write gates, revocation,
accepted-tree binding and separate publication controls remain authoritative.
Signing and hardened runtime are not the macOS App Sandbox and do not replace
the Seatbelt execution restrictions. No App Sandbox entitlement is added.

## Finder setup and local inference

Setup discovers Git, gh, Cargo, Rust, Python and Xcode in known directories
(`~/.cargo/bin`, Homebrew, `/usr/local/bin`, system directories), never inherited
PATH or the launch directory. For other locations, a private, user-owned
`desktop-tools.json` in the home may contain:

```json
{"directories":["/absolute/operator-owned/tool-directory"]}
```

At most eight directories are allowed. Each must be absolute, exist, belong to
the user, and reject other-user writes. They apply to backend operations and
preparation; Setup lists default discovery.

**Check installed chat and planner models** queries configured loopback inventories
with a deadline, response bound, no proxy, and no redirect. It reports each exact
tag as present, absent, or failed; it never downloads models, probes cloud
planners or switches a runtime. Send a short chat to test inference
([states and fallback](MODELS.md#exact-model-diagnostics-and-optional-fallback)).
`models.local_llm.inventory` bounds the probe (default 2 s and 262144 bytes;
0.1–30 s and 1024–1048576 bytes); fallback keeps its separate
`probe_timeout_sec` (default 1.5 s, 0.1–30). Malformed values are refused.

**Choose repository and prepare offline** runs the bundled `prepare-cargo.py`
with fixed arguments and a native folder choice. Python 3, Cargo, the selected
Rust toolchain, SDK, Cargo.lock and cached sources are prerequisites. It creates
the read-only preparation inventory without downloading dependencies or models,
discards tool output and shows bounded static results. Missing inputs need
operator preparation; online preparation stays an explicit CLI helper option.

The app adds no automatic toolchain installation, cloud inference, model pull,
external probe or telemetry; a loopback model endpoint does not prove its
service stays offline.

## Closing, quitting and recovery

Close hides a window and keeps the backend alive. Dock reopen and a second
launch focus the existing home instance. Cmd-Q asks **Keep running** or **Cancel
work and quit** when chat, a job, a shim operation or preparation is active.
Shutdown closes the private channel, cancels owned work, and waits before
terminating its direct child; parent death requests the same shutdown through
pipe EOF. Backend failure returns to Setup with a controlled retry. Startup
protocol reads have a 20-second deadline, which OS file-access mediation can
exceed. If startup keeps waiting, check for a macOS file-access prompt, quit,
move the complete app to Applications, and retry. Do not delete the home.

`data/agentic/console-jobs.json` retains at most 32 terminal jobs plus one active
job within 16 MiB. Restart marks a recovered running job interrupted; it never
reattaches or resumes. A persistence failure refuses execution or reports an
unsaved outcome.

`/agent runs` lists bounded recent records and reconciles released
Unix/Windows worker leases to `interrupted`; a live lease remains running. Legacy
running records without ownership evidence stay unknown and cannot be discarded
on a stale-state guess. `/agent status <id>` inspects retained evidence and
diff; `/agent discard <id>` removes interrupted or decided work through the
jailed worker operation. Pending proposals still require a decision. No
approval, commit, push, publication or model request is replayed at startup.
Run evidence is retained separately from the rolling console-job list.

Audit, spend, and optional metrics JSONL sinks retain a current file and one `.1`
file, default 8 MiB each (`logging.max_file_bytes`, 64 KiB–64 MiB); an existing
oversized log becomes the `.1` file until the next rotation. A busy or
unavailable sink, or an oversized event, drops that event with a warning;
logging is best-effort. The desktop discards child stderr and keeps no growing
stdout/stderr log.

Cancellation is **best-effort ancestry cleanup**, not perfect process containment.
Kernel waits can exceed deadlines, backend SIGKILL bypasses destructors, and
children that reparent before observation can survive. Inspect retained runs and
surviving work before further writes ([process lifecycle](PROCESS_LIFECYCLE.md)).
There is no per-run disk, memory or process-count quota.

## Rebuild and verify

Build prerequisites: Apple Silicon macOS, Xcode Command Line Tools, Git, rustup,
backend Rust 1.89 and desktop Rust 1.90. Install both toolchains explicitly;
the packaging script sets `RUSTUP_AUTO_INSTALL=0`.

```sh
rustup toolchain install 1.89 --profile minimal --component rustfmt --component clippy
rustup toolchain install 1.90 --profile minimal --component rustfmt --component clippy
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --locked
cargo deny check
rustup target add --toolchain 1.89 aarch64-apple-darwin x86_64-apple-darwin
rustup target add --toolchain 1.90 aarch64-apple-darwin x86_64-apple-darwin
scripts/package-desktop.sh --universal --dmg
(cd desktop && cargo fmt -- --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked)
cargo deny --manifest-path desktop/Cargo.toml --config desktop/deny.toml check
CGAH_TEST_BINARY='dist/CG Agent Harness.app/Contents/MacOS/cgagentharness' python3 scripts/test-desktop-backend.py
scripts/verify-desktop-bundle.sh 'dist/CG Agent Harness.app' universal
(cd dist && shasum -a 256 -c SHA256SUMS)
```

Use rustup's Cargo so each package's toolchain file applies. The packager
combines both backends with `lipo`, signs that file, then builds both shell
slices against its SHA-256; do not re-sign the sidecar afterward.
`scripts/verify-desktop-bundle.sh` hashes the staged `Contents/MacOS/cgagentharness`
after the final `.app` codesign and requires that SHA-256 to still appear in the
shell binary (`CGAH_BACKEND_SHA256`). Packaging refuses a dirty tree unless
`CGAH_ALLOW_DIRTY=1`, which marks the bundle as development. Outputs land in
`dist/`; `Contents/Resources/COMMIT` identifies the source commit. Omitting
`--universal` keeps an arm64 development build. Cross-building Intel is not
native Intel acceptance, and byte-identical rebuilds are not claimed.

Desktop CI (bundle, release or dispatch) checks both architectures, system
linkage, nested signatures, resources, CLI/worker dispatch, policy tests, the
extracted ZIP, and dependencies; artifacts last 14 days. Pull requests that
change `desktop/`, `src/common/local_tls.rs` (which `desktop/src/backend.rs`
includes through `#[path]`) or `scripts/start-web.sh` run `desktop-check.yml`
instead: fmt, clippy, tests and deny without packaging. A successful build is
not GUI acceptance.

Each crate keeps its own toolchain pin, lockfile and `deny.toml`
([dependency maintenance](DEPENDENCIES.md#retained-constraints)).

## Update, recovery and uninstall

Quit the app before replacing the bundle, verify the archive checksum, then open
the new copy. Keep the home intact. If a same-home server conflict appears, quit
that server; never delete a lock file to defeat live ownership. If durable job
JSON is corrupt or oversized, stop the app and preserve a copy before archiving
that one job-history file; keep run records, workspaces, credentials and
sessions. There is no automatic reset.

To uninstall, quit and move the app to Trash. Removing its home is a separate
destructive action; back up wanted data first. `scripts/reset-macos.sh`
previews selected home and Keychain removals; `--yes` applies them. The per-user
focus directory under `/private/tmp` holds only lock/socket files.

### Reset failure and recovery

Reset checks every selected Keychain service before deletion. A preflight lookup
failure exits 1 without deleting entries or the home. A later lookup or delete
failure also exits 1, reports the successful-delete count and preserves the
home; earlier deletions are not rolled back, so a retained home may lack
provider keys or its saved admin password. Account passwords in `auth.sqlite3`
remain unchanged.

Resolve the Keychain problem, keep the harness stopped, then rerun with the same
`CGAGENTHARNESS_HOME` and `--yes`. Missing entries are skipped; the home is
removed after the remaining credential operations succeed. Before reusing a
retained home, re-enter the keys you need; a home backup does not restore
Keychain entries.

The `security` CLI deletes one item per call, with no transaction or rollback.
Reset covers only the selected home's service names; other homes, credentials
under other service names and shared tools stay outside the purge.
