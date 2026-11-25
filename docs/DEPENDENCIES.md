# Dependency maintenance

The [backend](../Cargo.toml) and [desktop](../desktop/Cargo.toml) have separate
locks, deny policies and Rust pins (1.89 and 1.90). CI and packaging use
`--locked`. The Seatbelt fixture lock is checked with the root deny policy.
`browser/package-lock.json` pins Playwright separately. The browser uses
installed Google Chrome. The daily advisory workflow audits the npm lock.

## Retained constraints

- Cargo's MSRV fallback prefers compatible releases. Backend
  `ordered-float` 5.4 is held by Rust 1.89. Check the target lock before
  treating newer major versions as drift. Keep provider/runtime upgrades
  separate from version refreshes.
- `rmcp` is exactly pinned at 3.9.0. Desktop Tauri is pinned at 2.12.4.
  Desktop `tauri-utils` 2.10.2 uses ICU for `urlpattern`.
  Keep `cargo tree` free of Git sources and retired `unic-*` crates. Linux-only
  GTK3 dependencies `glib` and `proc-macro-error` are absent from both
  shipped Apple target graphs. Keep desktop deny ignores empty.
  Owner `cgfixit` revisits this if adding Linux desktop, on a Tauri GTK
  migration or by 2027-02-15. See [#782](https://github.com/cgfixit/CG-agent-harness/issues/782).
- Root [deny policy](../deny.toml) records the unmaintained
  `unsafe-libyaml` (RUSTSEC-2024-0449), `chrono` (RUSTSEC-2026-0543)
  and `chrono-tz` (RUSTSEC-2026-0555) waivers. Owner `cgfixit` reviews YAML
  replacements weekly and on Cargo Dependabot PRs, next by 2027-02-15,
  and the schedule dependency chain by 2027-02-17. Remove each waiver
  when its crate leaves the lock.
- Bundled SQLite, bounded Tantivy BM25 and the child PDF parser are
  intentional. Reqwest uses pinned DNS for public web egress and OS
  public trust. The locked AWS-LC build needs a C compiler.
  Review feature changes with their call sites. The DOCX reader uses
  ZIP deflate and namespace-aware XML. The PDF crate runs out of process
  with its default features disabled.
- Both policies ban telemetry SDKs. Duplicate versions warn.
  Advisories, licenses, bans and sources follow the checked-in policies.

## Check and synchronize

Use rustup's pinned Cargo. In root and `desktop/`, inspect
`cargo metadata --locked --format-version 1` and `cargo tree --locked`.
Preview with `cargo update --dry-run --locked --verbose`. Respect
Dependabot's seven-day cooldown unless fixing a locked advisory. Inspect
manifest, lock and transitive edge changes before verifying a refresh.
Use `cargo deny check` with cargo-deny 0.20 or newer. CI also checks root
all-features and the fixture with:

```bash
cargo deny --manifest-path tests/fixtures/cargo-sandbox/Cargo.toml --config deny.toml check advisories bans sources
```

Exact versions live in the locks. Action pins live in workflows. Match
CI's [workflow commands](../.github/workflows/README.md) and verify the
affected application surface. Policy checks do not establish runtime
behavior.
