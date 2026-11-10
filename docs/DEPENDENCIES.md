# Dependency maintenance

The backend and desktop are separate Cargo crates, with independently committed
manifests, lockfiles, toolchains and `deny.toml` policies. A third lockfile,
the Seatbelt test fixture `tests/fixtures/cargo-sandbox`, gets the root policy's
advisories, bans and sources checks in CI. The backend pins Rust
1.89; `desktop/` pins 1.90. Use rustup's Cargo so the local toolchain files take
effect. CI/release builds use locked resolution. Downloads during dependency
preparation are separate from the application's offline Cargo verification.

## Runtime additions and boundaries

| Crate | Purpose / selected features |
|---|---|
| `rusqlite` 0.41 | Transactional account store; bundled SQLite avoids a machine-specific SQLite dependency |
| `axum-server` 0.8, `rustls` 0.23 | Shared standalone/sidecar HTTPS; explicit ring provider, ordinary certificate verification |
| `rcgen` 0.14, `x509-parser` 0.18 | Local P-256 certificate generation and validity/key/name checks; no OpenSSL subprocess |
| `scraper` 0.27, `html5ever` 0.39, `robotstxt` 0.3 | HTML extraction and permitted robots handling; no browser execution |
| `tantivy` 0.27 | Embedded BM25 passage retrieval; bounded derived index, no daemon or embeddings |
| `rmcp = "=3.9.0"` | Inbound MCP memory server (streamable HTTP); exact pin, bumped deliberately (RUSTSEC-2026-0462 fix, 2026-11-09) |
| `zip` 9.2.2, `quick-xml` 0.42 | DOCX attachment reader: in-memory ZIP, deflate only; namespace-aware XML. No PDF reader or parser subprocess; limits in [CONSOLE.md](CONSOLE.md#docx-attachments) |
| `objc2` / Foundation / WebKit / Security bindings | Native authentication-challenge handling for the exact owned loopback certificate |
| Keyring ecosystem 1.x stores | OS store for managed provider keys: macOS Keychain, Windows Credential Manager and the maintained D-Bus Linux Secret Service backend |

Reqwest 0.13 serves both crates: public fetching disables proxies, redirects,
compression, retries and reuse and pins checked DNS addresses, while loopback
certificate pinning never replaces public-web trust. cargo-deny does not certify
safe runtime use of a crate.

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
  overrides. Rust 1.89 admits `rusqlite` 0.41 (RUSTSEC-2026-0459); `uuid` 1.27
  and later are no longer MSRV-held.
- Incompatible major lines are separate API migrations, not drift: backend
  `axum-server` 0.8, `base64` 0.22, `rand` 0.8, `scrypt` 0.11, `scraper` 0.27 with `html5ever`
  0.39, `windows-sys` 0.59 and the `rmcp` pin; desktop `toml` 0.8.2 and `brotli-decompressor` 6.2,
  held by upstream constraints with `generic-array` and `matchit`. Newer
  available versions alone do not make a valid lockfile stale or justify
  expanding a security refresh into a framework/toolchain migration.
- Manual refreshes follow Dependabot's 7-day cooldown (`.github/dependabot.yml`):
  a compatible release younger than a week is not drift yet. A release that
  fixes an advisory against a locked crate skips the wait, as Dependabot
  security updates do.
- The backend license allowlist names only identifiers the locked graph uses.
  Add an identifier deliberately, with the crate that needs it, rather than
  keeping unused allowances that would admit a future dependency silently.
- Desktop Tauri is pinned to `=2.12.4`; its `tauri-utils` 2.10.2 routes
  `urlpattern` through ICU, so no `unic-*` crate and no Git source remains in
  either lock (`cargo tree` must show none).
- Tantivy 0.27 resolves `lru` 0.18.2 or newer; desktop has no advisory exception.
- Backend `RUSTSEC-2024-0449` waives unmaintained `unsafe-libyaml` via
  `serde_yaml_ng`. Owner: `cgfixit`; review maintained, MSRV-compatible YAML
  replacements weekly and on Cargo Dependabot PRs. Remove the waiver when
  `unsafe-libyaml` leaves the backend lockfile, after both dependency policies
  and configuration parsing checks pass. Unused-ignore warnings do not detect
  available replacements.
- GHSA-qwgh-2vcv-g2f7 (`block-buffer`) affects only `>=0.11.0, <0.12.1`;
  the range was corrected upstream on 2026-10-30. Both locks carry 0.10.4 and
  0.12.1, outside it, so no residual or `deny.toml` ignore is needed (#713,
  #757). A `scrypt` 0.12 move stays an optional API migration, not a fix.
- Duplicate versions are warnings under the existing policy. Advisories,
  unapproved licenses, wildcard requirements and unknown sources remain blocking.
- Both `deny.toml` files ban push-telemetry and analytics SDK crates
  (OpenTelemetry, Sentry, PostHog, Segment, Mixpanel, Amplitude, Datadog,
  Honeycomb, Bugsnag, RudderStack, Statsig, LaunchDarkly, minitrace) by exact
  name, the CI half of the otel-hardening skill's T5 pattern sweep.
- Optional runtime services (Google, SerpAPI, Tavily, Firecrawl, Ollama, cloud providers) are not
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

Use cargo-deny 0.20 or
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

## Last synchronization (2026-11-09)

Advisory refresh only, so the cooldown did not apply. RUSTSEC-2026-0459 moved
the backend to Rust 1.89, `rusqlite` 0.41.1 and `libsqlite3-sys` 0.39.1
(bundled SQLite 3.54.1); RUSTSEC-2026-0462 moved the `rmcp` pin to 3.9.0.
Dependabot had already moved `rmcp` to 3.8.0 (#798) and `zip` to 9.2.2 (#800).
The desktop and fixture locks carry neither crate and did not change. The
2026-11-07 pass found every other compatible candidate younger than the
cooldown; `tauri` 2.12.5 waits on its pin and `axum-server` 0.9.0 is a new
major line. Dated results describe that pass only; rerun the checks above
before trusting them.
