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

Reqwest 0.12 already serves both crates. Public fetching disables proxies,
redirects, compression, retries and connection reuse and pins checked DNS
addresses. Native/CLI certificate pinning applies only to the owned loopback
service; it never replaces public-web trust. Feature review is manual: cargo-deny
does not certify safe runtime use of a crate or inspect arbitrary feature names.

## Retained constraints

- Backend `ordered-float` remains locked at 5.4: 5.5 requires Rust 1.90.
  A semver-compatible release can still exceed the backend MSRV.
- Desktop Tauri is pinned to 2.11.5. The immutable upstream `tauri-utils`
  patch at `dd725f4b13c30a86b398ccc59eb498f151f461c5` replaces its unmaintained
  rust-unic dependency path. The desktop policy allows only that upstream Git
  source; no advisory ignore was added. Remove the patch only after a published
  version provides the same dependency correction and passes native packaging.
- Major-version updates to existing libraries are separate API migrations.
  Newer available versions alone do not make a valid lockfile stale or justify
  expanding this security feature into a framework/toolchain migration.
- Duplicate versions are warnings under the existing policy. Advisories,
  unapproved licenses, wildcard requirements and unknown sources remain blocking.

## Check and synchronize

Run in the repository root, then repeat inside `desktop/`:

```bash
rustup show active-toolchain
cargo metadata --locked --format-version 1 > /private/tmp/cgah-dependencies.json
cargo tree --locked --depth 1
cargo update --dry-run --locked --verbose --config 'resolver.incompatible-rust-versions="fallback"'
cargo build --locked
cargo deny check
```

Use another private report path on non-macOS systems. Check Cargo's exit status;
do not infer success from a grep match or suppress errors. The update command is
a preview and leaves the lockfile unchanged; `--locked` alone does not mean all
available versions are installed. Review compatibility and upstream changes.
For an authorized update, omit `--dry-run --locked`, keep the MSRV-aware resolver
setting, inspect the manifest/lock diff, and rerun both crates' formatting,
Clippy, tests, dependency policy, release builds and applicable native/package
acceptance. Do not change global rustup overrides to satisfy a dependency.

The September 12 sync applied available compatible updates to both locks with
that resolver setting. The backend remains buildable on 1.88, and desktop on
1.90. Exact versions are authoritative in the lockfiles, not this prose. Consult
the [RustSec advisory database](https://rustsec.org/advisories/) and upstream
[Cargo resolver documentation](https://doc.rust-lang.org/cargo/reference/resolver.html#rust-version)
when investigating newly reported drift. A clean dependency-policy result is a
point-in-time advisory/license/source check, not proof of vulnerability absence.
