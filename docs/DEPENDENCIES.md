# Dependency maintenance

The backend and desktop are separate Cargo crates, with independently committed
manifests, lockfiles, toolchains and `deny.toml` policies. A third lockfile,
the Seatbelt test fixture `tests/fixtures/cargo-sandbox`, gets the root policy's
advisories, bans and sources checks in CI. The backend pins Rust
1.88; `desktop/` pins 1.90. Use rustup's Cargo so the local toolchain files take
effect. CI/release builds use locked resolution. Downloads during dependency
preparation are separate from the application's offline Cargo verification.

## Runtime additions and boundaries

| Crate | Purpose / selected features |
|---|---|
| `rusqlite` 0.40 | Transactional account store; bundled SQLite avoids a machine-specific SQLite dependency |
| `axum-server` 0.8, `rustls` 0.23 | Shared standalone/sidecar HTTPS; explicit ring provider, ordinary certificate verification |
| `rcgen` 0.14, `x509-parser` 0.18 | Local P-256 certificate generation and validity/key/name checks; no OpenSSL subprocess |
| `scraper` 0.27, `html5ever` 0.39, `robotstxt` 0.3 | HTML extraction and permitted robots handling; no browser execution |
| `tantivy` 0.27 | Embedded BM25 passage retrieval; bounded derived index, no daemon or embeddings |
| `rmcp = "=3.7.0"` | Inbound MCP memory server (streamable HTTP); exact pin, bumped deliberately (from 3.5.1 on 2026-10-31) |
| `zip` 9.1.3, `quick-xml` 0.42 | DOCX attachment reader: in-memory ZIP, deflate only; namespace-aware XML. No PDF reader or parser subprocess; limits in [CONSOLE.md](CONSOLE.md#docx-attachments) |
| `objc2` / Foundation / WebKit / Security bindings | Native authentication-challenge handling for the exact owned loopback certificate |
| Keyring ecosystem 1.x stores | OS store for managed provider keys: macOS Keychain, Windows Credential Manager and the maintained D-Bus Linux Secret Service backend |

Reqwest 0.13 serves both crates. Public fetching disables proxies,
redirects, compression, retries and connection reuse and pins checked DNS
addresses. Native/CLI certificate pinning applies only to the owned loopback
service; it never replaces public-web trust. Feature review is manual: cargo-deny
does not certify safe runtime use of a crate or inspect arbitrary feature names.

Reqwest's `rustls` feature enables AWS-LC alongside the explicit ring provider.
Locked `aws-lc-sys` 0.47.0 normally builds with a C compiler; its Rust `cmake`
build dependency does not itself require a CMake executable. See
[source-build prerequisites](INSTALL.md#51-standalone-backend) for alternate builds.

## Retained constraints

- Backend `ordered-float` remains locked at 5.4: 5.5 requires Rust 1.90.
  A semver-compatible release can still exceed the backend MSRV, so
  `.cargo/config.toml` sets `resolver.incompatible-rust-versions = "fallback"`
  for both crates: plain `cargo update` prefers releases within each crate's
  `rust-version`, and `--ignore-rust-version` or `--precise` remain explicit
  overrides. `uuid` 1.26 stays for the same reason (1.28 needs Rust 1.89).
- Incompatible major lines are separate API migrations, not drift: backend
  `base64` 0.22, `rand` 0.8, `scrypt` 0.11, `html5ever` 0.39, `windows-sys`
  0.59 and the `rmcp` pin; desktop `toml` 0.8.2 and `brotli-decompressor` 6.2,
  held by upstream constraints with `generic-array` and `matchit`. Newer
  available versions alone do not make a valid lockfile stale or justify
  expanding a security refresh into a framework/toolchain migration.
- The backend license allowlist names only identifiers the locked graph uses.
  Add an identifier deliberately, with the crate that needs it, rather than
  keeping unused allowances that would admit a future dependency silently.
- Desktop Tauri is pinned to `=2.12.4`. Its published `tauri-utils` 2.10.2
  routes `urlpattern` 0.6 through ICU, so no `unic-*` crate remains in the
  desktop lock. The former `[patch.crates-io]` override of `tauri-utils` at
  upstream `dd725f4b13c30a86b398ccc59eb498f151f461c5` carried that correction
  until Tauri 2.12 shipped it; Cargo then reported the patch unused, and it was
  removed on 2026-10-13 without changing the resolved graph. No Git source
  remains in either lock; `cargo tree` must show none.
- Tantivy 0.27 resolves `lru` 0.18.2 or newer, so the former
  `RUSTSEC-2026-0253` exception is no longer needed. Desktop has no advisory
  exception.
- Backend `RUSTSEC-2024-0449` waives unmaintained `unsafe-libyaml` via
  `serde_yaml_ng`. Owner: `cgfixit`; review maintained, MSRV-compatible YAML
  replacements weekly and on Cargo Dependabot PRs. Remove the waiver when
  `unsafe-libyaml` leaves the backend lockfile, after both dependency policies
  and configuration parsing checks pass. Unused-ignore warnings do not detect
  available replacements.
- Duplicate versions are warnings under the existing policy. Advisories,
  unapproved licenses, wildcard requirements and unknown sources remain blocking.
- Both `deny.toml` files ban push-telemetry and analytics SDK crates
  (OpenTelemetry, Sentry, PostHog, Segment, Mixpanel, Amplitude, Datadog,
  Honeycomb, Bugsnag, RudderStack, Statsig, LaunchDarkly, minitrace) by exact
  name, the CI half of the otel-hardening skill's T5 pattern sweep.
- Optional runtime services (Google, SerpAPI, Ollama, cloud providers) are not
  Cargo dependencies; a live-service check is not a dependency or test pass.

## Check and synchronize

Run in the repository root, then repeat inside `desktop/`; check the fixture with
`cargo deny --manifest-path tests/fixtures/cargo-sandbox/Cargo.toml --config deny.toml check advisories bans sources`:

```bash
rustup show active-toolchain
cargo metadata --locked --format-version 1 > /private/tmp/cgah-dependencies.json
cargo tree --locked --depth 1
cargo update --dry-run --locked --verbose
cargo build --locked
cargo deny check
```

Use another private report path on non-macOS systems. Use cargo-deny 0.20 or
newer (CI's action bundles 0.20.2); 0.18 rejects `deny.toml`'s `unsound = "all"`.
Check Cargo's exit status;
do not infer success from a grep match or suppress errors. The update command is
a preview and leaves the lockfile unchanged; `--locked` alone does not mean all
available versions are installed. Review compatibility and upstream changes.
For an authorized update, omit `--dry-run --locked` (the MSRV-aware resolver
setting is committed in `.cargo/config.toml`), inspect the manifest/lock diff,
and rerun both crates' formatting,
Clippy, tests, dependency policy, release builds and applicable native/package
acceptance. Do not change global rustup overrides to satisfy a dependency.
The `dep-sync` skill scripts this pass against `origin/main`.

Exact versions are authoritative in the lockfiles. Consult
the [RustSec advisory database](https://rustsec.org/advisories/) and upstream
[Cargo resolver documentation](https://doc.rust-lang.org/cargo/reference/resolver.html#rust-version)
when investigating newly reported drift. A clean dependency-policy result is a
point-in-time advisory/license/source check, not proof of vulnerability absence.
Workflow action pins live in `.github/workflows/` and move with those files,
not with the lockfiles.

## Last synchronization (2026-10-30)

Both locks took every compatible MSRV-aware release: shared `aws-lc-sys` 0.47.0
with `aws-lc-rs` 1.19.0, `rustix` 1.1.6, `log` 0.4.35, the `wasm-bindgen` 0.2.130
family, `utf8_iter` 1.1 (which adds `utf_types`), `erased-serde` 0.4.11 and
`openssl-probe` 0.2.2; backend `zip` 9.1.3 (with `typed-path` 0.13) and
`sketches-ddsketch` 0.4.2; desktop `brotli` 9.1.1 with `brotli-decompressor`
6.2.0, `rfd` 0.17.2 and `tauri-plugin-dialog` 2.8.4, which also drops the desktop
lock's duplicate `windows-sys` 0.60 and `windows-targets` 0.53 line. `tauri`
stays `=2.12.4`; no manifest, feature, toolchain or advisory exception changed.
On 2026-10-31 the backend `rmcp` pin moved deliberately from 3.5.1 to 3.7.0
(its graph gains `libc`, already locked); nothing else moved.
Dated results describe that pass only; rerun the checks above before trusting them.
