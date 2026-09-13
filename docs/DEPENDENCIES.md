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

The September 12 sync applied available compatible updates to both locks with
that resolver setting. The backend remains buildable on 1.88, and desktop on
1.90. Exact versions are authoritative in the lockfiles, not this prose. Consult
the [RustSec advisory database](https://rustsec.org/advisories/) and upstream
[Cargo resolver documentation](https://doc.rust-lang.org/cargo/reference/resolver.html#rust-version)
when investigating newly reported drift. A clean dependency-policy result is a
point-in-time advisory/license/source check, not proof of vulnerability absence.

The fresh-install follow-up rechecked both crates with locked metadata/tree
commands and the MSRV-aware update preview on September 12. Both previews
reported zero package changes; the manifests, lockfiles, toolchains and deny
policies were retained. The default/onboarding changes add no dependencies.

## Chat Google-search follow-up (2026-09-12)

The chat web tools and Google result parsers reuse Reqwest, Scraper, Serde and
Tokio already in the lockfiles. No crate, feature, lockfile, toolchain or advisory
exception was added. Both MSRV-aware dry-run updates again found zero compatible
updates; backend and desktop cargo-deny policies passed. Google/SerpAPI are
optional runtime search services, distinct from Cargo dependencies and local
model inference. A public-Google challenge is not a dependency or test pass.

## Optimization sweep (2026-09-13)

Backend (Cargo 1.88.0, lockfile at `093749b`, the `main` tree after
[PR #80](https://github.com/cgfixit/CG-agent-harness/pull/80) merged): `cargo
metadata --locked`, `cargo build --all-targets --locked` and `cargo test --test
invariant_guard --locked` exit 0. `cargo update --dry-run --locked --verbose`
locks 0 packages with the committed `.cargo/config.toml` resolver setting;
`ordered-float` 5.4.0 is retained (5.5.0 requires Rust 1.90) and the remaining
eight "behind latest" entries are semver-incompatible releases. The same checks
were first run on the pre-#80 lockfile at `f47d41d`, where the plain preview
selected `ordered-float` 5.5.0; that is why PR #80 committed the resolver
setting, removed the unused direct `http-body-util` declaration and dropped the
unused `OpenSSL`/`Unicode-DFS-2016` license allowances, and the results above
were re-run on the merged lockfile that this ledger entry ships with. `cargo deny check` (cargo-deny
0.20.2, RustSec advisory database cloned 2026-09-13, run offline because the
sandbox proxy blocks cargo-deny's own fetch): advisories, bans, licenses and
sources ok; duplicate-version warnings unchanged. Backend `Cargo.lock`
carries `windows-sys` 0.52.0/0.59.0/0.60.2/0.61.2, `rand` 0.8.8/0.9.5/0.10.2,
and `thiserror` 2.0.20 only. Desktop (`desktop/Cargo.lock`, Tauri
2.11.5 with the retained `tauri-utils` patch) additionally duplicates
`thiserror` 1.0.69/2.0.20 and `windows-sys` 0.45.0/0.52.0/0.59.0/0.60.2/0.61.2.
`cargo fetch --locked`,
`cargo metadata --locked --offline` and `cargo deny check` with
`desktop/deny.toml` exit 0. No dependency was added or updated; no native
desktop build was run in this sweep.
