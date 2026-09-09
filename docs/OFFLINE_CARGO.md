# Offline Cargo verification on macOS

The operator prepares dependencies; untrusted checks never fetch them. This flow
was exercised on macOS 26.6.2, arm64 Apple M5 Pro, Homebrew Rust/Cargo 1.98.0.
Rustup toolchains use their resolved sysroot; the preparation command runs from
the selected repository, so its toolchain selection applies. Homebrew does not
automatically honor rust-toolchain.toml. Provision required tools/components
explicitly before preparing. No toolchain or model is installed by the harness.

## Prepare once per lockfile and application home

From this harness source checkout, with Python 3 available:

```sh
python3 scripts/prepare-cargo.py /path/to/selected/repository /path/to/isolated/harness-home
# Only if dependency retrieval is authorized and the local source cache is incomplete:
python3 scripts/prepare-cargo.py /path/to/selected/repository /path/to/isolated/harness-home --online
```

Both commands require an existing Cargo.lock and installed cargo/rustc/rustdoc.
The first is offline; the second permits Cargo dependency retrieval. Neither runs
build scripts or tests. Cargo vendor uses locked resolution and produces a
separate source snapshot under `data/agentic/cargo-prepared/<lock-sha256>`.
Existing snapshots are refused, never overwritten; use a separate disposable
home to reprepare. Do not place the home or snapshot inside the candidate.
The snapshot records the concrete toolchain and macOS SDK/linker/runtime paths.
Moving/removing those installations requires preparation again.

Run the harness with `CGAGENTHARNESS_HOME` set to that application home. Keep
Cargo.lock unchanged in proposals; new dependencies require separately reviewed
preparation. No model retry can make uncached dependencies available offline.
Missing components, sources, permission failures and Cargo timeouts return setup
errors; compile/test failures remain feedback for a correction.

## Execution boundary

Every verification creates and removes owned scratch containing HOME, CARGO_HOME,
build outputs and temporary files. The real user home and credentials are not
passed through. Cargo receives the prepared source configuration, explicit
compiler/rustdoc, `--frozen`, and `CARGO_NET_OFFLINE=true`. SDKROOT and the direct
Clang linker avoid xcrun attempting to write into the operator's cache.

On macOS, Seatbelt denies network operations including loopback. Candidate files,
`.git`, vendor sources, toolchain and runtime inputs are read-only. File data reads
are restricted to those inputs, scratch, named OS/SDK roots, the root directory
itself and the system OpenSSL configuration file needed by Homebrew Cargo. The
whole home, `/usr/local`, `/opt/homebrew`, `/private` and `/Library` are not granted.
Only scratch is writable. Tests that generate source-tree artifacts must instead
use the supplied temporary directory; granting candidate writes would also expose
Git metadata and hooks to build scripts.

## Reproduce the required native gate

```sh
# Authorized preparation, outside verification:
cargo fetch --locked
cargo fetch --locked --manifest-path tests/fixtures/cargo-sandbox/Cargo.toml
# Actual native offline verification, with cloud test credentials empty:
GROK_API_KEY= ANTHROPIC_API_KEY= DEEPAGENT_API_KEY= CARGO_NET_OFFLINE=true \
  cargo test --test macos_cargo --locked --offline -- --nocapture
```

Run on macOS outside an outer sandbox that prohibits nested Seatbelt. Missing
sandbox capability fails this gate. The tests compile minimal and serde-bearing
fixtures, execute build scripts/unit tests/doctests, deny synthetic outside and
symlink reads, deny candidate/Git/cache/outside writes, deny access to an active
loopback listener, repeat execution, check scratch cleanup and fail on stale or
missing prepared inputs. Synthetic markers contain no real secrets.

## Limits

File metadata discovery remains allowed. OS libraries and SDKs are readable.
Seatbelt is not a memory/disk quota; pipe capture and escaped child process groups
remain separate work. A stopped request may leave surviving work. Linux network
namespaces and Windows Job Objects do not provide this macOS filesystem policy.
Windows preparation uses platform executable suffixes/path separators but native
Windows acceptance is unverified. This gate proves Cargo execution and tested
boundaries, not complete harness or real-model acceptance.

Preparation adds no Rust dependencies. Python 3 standard-library availability is
an explicit setup requirement; Cargo remains responsible for lock/checksum
validation. See upstream [Cargo vendor](https://doc.rust-lang.org/cargo/commands/cargo-vendor.html)
and [Cargo configuration](https://doc.rust-lang.org/cargo/reference/config.html).
