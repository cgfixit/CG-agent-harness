# Dependency maintenance

The backend and desktop are separate Cargo crates, with independently committed
manifests, lockfiles, toolchains and `deny.toml` policies. The backend pins Rust
1.88; `desktop/` pins 1.90. Use rustup's Cargo so the local toolchain files take
effect. CI/release builds use locked resolution. Downloads during dependency
preparation are separate from the application's offline Cargo verification.

## Runtime additions and boundaries

| Crate | Purpose / selected features |
|---|---|
| `rusqlite` 0.40 | Transactional account store; bundled SQLite avoids a machine-specific SQLite dependency |
| `axum-server` 0.8, `rustls` 0.23 | Shared standalone/sidecar HTTPS; explicit ring provider, ordinary certificate verification |
| `rcgen` 0.14, `x509-parser` 0.18 | Local P-256 certificate generation and validity/key/name checks; no OpenSSL subprocess |
| `scraper` 0.27, `robotstxt` 0.3 | HTML extraction and permitted robots handling; no browser execution |
| `tantivy` 0.26 | Embedded BM25 passage retrieval; bounded derived index, no daemon or embeddings |
| `objc2` / Foundation / WebKit / Security bindings | Native authentication-challenge handling for the exact owned loopback certificate |
| `keyring` 3.6.3 | OS store for managed provider keys. Features: `apple-native`, `windows-native`, `sync-secret-service`, `crypto-rust`. Linux uses Secret Service, not keyutils. A Linux build needs `pkg-config` and `libdbus-1-dev` |

Reqwest 0.12 already serves both crates. Public fetching disables proxies,
redirects, compression, retries and connection reuse and pins checked DNS
addresses. Native/CLI certificate pinning applies only to the owned loopback
service; it never replaces public-web trust. Feature review is manual: cargo-deny
does not certify safe runtime use of a crate or inspect arbitrary feature names.

## Retained constraints

- Backend `ordered-float` remains locked at 5.4: 5.5 requires Rust 1.90.
  A semver-compatible release can still exceed the backend MSRV, so
  `.cargo/config.toml` sets `resolver.incompatible-rust-versions = "fallback"`
  for both crates: plain `cargo update` prefers releases within each crate's
  `rust-version`, and `--ignore-rust-version` or `--precise` remain explicit
  overrides.
- The backend license allowlist names only identifiers the locked graph uses.
  Add an identifier deliberately, with the crate that needs it, rather than
  keeping unused allowances that would admit a future dependency silently.
- Desktop Tauri is pinned to 2.11.5. The immutable upstream `tauri-utils`
  patch at `dd725f4b13c30a86b398ccc59eb498f151f461c5` replaces its unmaintained
  rust-unic dependency path. The desktop policy allows only that upstream Git
  source. Remove the patch only after a published
  version provides the same dependency correction and passes native packaging.
- Backend `deny.toml` temporarily ignores `RUSTSEC-2026-0253` for Tantivy 0.26.2's
  `lru` 0.16 dependency (locked at 0.16.4). The policy records its
  `StoreReader` call-pattern justification and removal condition, Tantivy 0.27
  or a fixed dependency path. Owner: `cgfixit`, reviewed during the weekly
  maintenance pass and Cargo Dependabot PR review. Daily advisories check the
  locked graph; they do not detect an available replacement for an ignored
  advisory. When compatible Tantivy uses fixed `lru` (at least 0.18.2), update
  the backend lockfile, remove this exception, and run both dependency policies
  plus affected retrieval checks. Desktop currently has no advisory exception.
- Major-version updates to existing libraries are separate API migrations.
  Newer available versions alone do not make a valid lockfile stale or justify
  expanding this security feature into a framework/toolchain migration.
- Duplicate versions are warnings under the existing policy. Advisories,
  unapproved licenses, wildcard requirements and unknown sources remain blocking.
- Both `deny.toml` files ban push-telemetry and analytics SDK crates
  (OpenTelemetry, Sentry, PostHog, Segment, Mixpanel, Amplitude, Datadog,
  Honeycomb, Bugsnag, RudderStack, Statsig, LaunchDarkly, minitrace) by exact
  name, the CI half of the otel-hardening skill's T5 pattern sweep.

## Check and synchronize

Run in the repository root, then repeat inside `desktop/`:

```bash
rustup show active-toolchain
cargo metadata --locked --format-version 1 > /private/tmp/cgah-dependencies.json
cargo tree --locked --depth 1
cargo update --dry-run --locked --verbose
cargo build --locked
cargo deny check
```

Use another private report path on non-macOS systems. Check Cargo's exit status;
do not infer success from a grep match or suppress errors. The update command is
a preview and leaves the lockfile unchanged; `--locked` alone does not mean all
available versions are installed. Review compatibility and upstream changes.
For an authorized update, omit `--dry-run --locked` (the MSRV-aware resolver
setting is committed in `.cargo/config.toml`), inspect the manifest/lock diff,
and rerun both crates' formatting,
Clippy, tests, dependency policy, release builds and applicable native/package
acceptance. Do not change global rustup overrides to satisfy a dependency.

Exact versions are authoritative in the lockfiles. Consult
the [RustSec advisory database](https://rustsec.org/advisories/) and upstream
[Cargo resolver documentation](https://doc.rust-lang.org/cargo/reference/resolver.html#rust-version)
when investigating newly reported drift. A clean dependency-policy result is a
point-in-time advisory/license/source check, not proof of vulnerability absence.

## Chat Google-search follow-up (2026-09-12)

The chat web tools and Google result parsers reuse Reqwest, Scraper, Serde and
Tokio already in the lockfiles. No crate, feature, lockfile, toolchain or advisory
exception was added. Google/SerpAPI are
optional runtime search services, distinct from Cargo dependencies and local
model inference. A public-Google challenge is not a dependency or test pass.

## Compatibility audit (2026-09-16)

Both crates resolve independently under their pinned Rust versions and do not
exchange Rust types across an ABI. Duplicate transitive versions in `cargo tree
-d` are warnings, not resolution failures. `ordered-float` 5.5 still exceeds the
backend MSRV; incompatible major lines require separate API migrations. The
Tauri patch remains required. No dependency update is part of this documentation
refresh.

The September 16 audit applied all compatible updates within the manifest and
Rust-version constraints: 13 backend and 12 desktop lock entries changed. Shared
updates included `cc` 1.4.6, `cfg-if` 1.0.5, `lru-slab` 0.1.3, `quinn` 0.11.12,
`quinn-proto` 0.11.18, `tinyvec` 1.13.3, `yoke-derive` 0.8.3, and
`zerofrom-derive` 0.1.8. The backend moved to Clap 4.6.7. Desktop moved to
`camino` 1.2.6, `redox_users` 0.5.3, and `zlib-rs` 0.6.8. Cargo added
`synstructure` 0.14 and removed `tinyvec_macros` through those upstream changes.

The audit retained incompatible lines of base64, rand, reqwest, scrypt, sha2,
and windows-sys for separate migrations. Upstream constraints retain
generic-array, matchit, and desktop TOML compatibility lines. The Serde sandbox
fixture lock remains reproducibility data, not part of the product dependency
graph.

At that audit, CodeQL moved to v4.38.0, zizmor-action to v0.6.4 with zizmor
1.30.1, and actionlint to v1.7.12. Verification covered both crates' formatting,
all-target/all-feature Clippy, all-target tests, cargo-deny policies, compatible
update previews, release packaging, and isolated native/backend acceptance.
Those dated results describe that audit; they do not replace current checks.

## DOCX attachment reader (issue #148)

| Dependency | License | Purpose |
|---|---|---|
| `zip` 8.6.0 | MIT | In-memory ZIP reader; default features off, deflate only |
| `quick-xml` 0.42.0 | MIT | Incremental, namespace-aware main-document XML reading |

No PDF reader or parser subprocess is included. The application lockfile
SHA-256 `530a6ac90fc335589ac494f7a60936b882b71760c185d119a98c60e900ba6148`
was audited with `cargo deny check` and prepared with `scripts/prepare-cargo.py`
for offline verification. The DOCX limits and deliberate format exclusions are
in [CONSOLE.md](CONSOLE.md#docx-attachments).
