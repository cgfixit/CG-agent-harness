#!/usr/bin/env python3
"""Prepare locked Cargo sources/toolchain metadata; never run builds or tests.

Usage: python3 scripts/prepare-cargo.py REPOSITORY HARNESS_HOME [--online]
Default preparation is offline. --online explicitly permits dependency retrieval.
Verification itself always remains offline. Existing snapshots are never replaced.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import signal
import sys
import threading


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repository", type=Path)
    parser.add_argument("harness_home", type=Path)
    parser.add_argument("--online", action="store_true")
    parser.add_argument("--desktop-parent", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.desktop_parent:
        # The native owner creates this process group and holds the only stdin
        # writer. EOF closes the helper's owned group after parent death.
        if os.name != "posix" or os.getpgrp() != os.getpid():
            raise SystemExit("desktop preparation requires its own process group")
        def parent_watch():
            sys.stdin.buffer.read(1)
            os.killpg(os.getpgrp(), signal.SIGTERM)
        threading.Thread(target=parent_watch, daemon=True).start()
    repo = args.repository.resolve(strict=True)
    lock = (repo / "Cargo.lock").read_bytes()
    digest = hashlib.sha256(lock).hexdigest()
    root = args.harness_home.resolve() / "data/agentic/cargo-prepared"
    root.mkdir(parents=True, exist_ok=True)
    dest = root / digest
    if dest.exists():
        raise SystemExit("snapshot already exists; use it or choose a separate application home")
    # term.quiet (ambient or candidate config) would suppress the source TOML.
    env = dict(os.environ, RUSTUP_AUTO_INSTALL="0", CARGO_TERM_QUIET="false")
    # Resolve tools outside the candidate (and the caller's cwd). A repository
    # rust-toolchain path must never choose code to run before confinement.
    # PATH and RUSTUP_TOOLCHAIN remain explicit operator-controlled selections.
    tool_cwd = os.path.abspath(os.sep)
    executable = lambda name: name + (".exe" if os.name == "nt" else "")
    # Toolchain preparation is explicit: this probe cannot auto-install rustup tools.
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], cwd=tool_cwd, env=env, text=True).strip()).resolve(strict=True)
    for tool in ("cargo", "rustc", "rustdoc"):
        if not (sysroot / "bin" / executable(tool)).is_file():
            raise SystemExit(f"missing {tool} in selected toolchain; provision it before preparation")
    sdk_root = linker = None
    if shutil.which("xcrun"):
        sdk_root = str(Path(subprocess.check_output(["xcrun", "--show-sdk-path"], cwd=tool_cwd, env=env, text=True).strip()).resolve(strict=True))
        linker = str(Path(subprocess.check_output(["xcrun", "--find", "clang"], cwd=tool_cwd, env=env, text=True).strip()).resolve(strict=True))
    runtime_roots = set()
    if shutil.which("otool"):
        pending = [sysroot / "bin" / "cargo", sysroot / "bin" / "rustc", sysroot / "bin" / "rustdoc"]
        seen = set()
        while pending:
            binary = pending.pop().resolve()
            if binary in seen:
                continue
            seen.add(binary)
            output = subprocess.check_output(["otool", "-L", str(binary)], cwd=tool_cwd, env=env, text=True)
            for line in output.splitlines()[1:]:
                name = line.strip().split(" (", 1)[0]
                if name.startswith(("/opt/homebrew/", "/usr/local/")):
                    library = Path(name).resolve(strict=True)
                    runtime_roots.add(str(library.parent))
                    pending.append(library)
    with tempfile.TemporaryDirectory(prefix=".prepare-", dir=root) as temporary:
        # Renamed into place below; the temporary directory itself stays for
        # TemporaryDirectory to remove on every Python 3 version.
        stage = Path(temporary) / "snapshot"
        stage.mkdir(mode=0o700)
        cmd = [str(sysroot / "bin" / executable("cargo")), "vendor", "--locked", "--versioned-dirs"]
        if not args.online:
            cmd.append("--offline")
        cmd.append(str(stage / "vendor"))
        config = subprocess.check_output(cmd, cwd=repo, env=env, text=True)
        if (repo / "Cargo.lock").read_bytes() != lock:
            raise SystemExit("lockfile changed during preparation; refusing snapshot")
        vendored = stage / "vendor"
        if not config.strip() and vendored.is_dir() and any(vendored.iterdir()):
            raise SystemExit("cargo vendor printed no source configuration; refusing an unusable snapshot")
        # Cargo emits TOML quoted paths; rewrite only the generated vendor location.
        old = json.dumps(str(stage / "vendor"), ensure_ascii=False)
        new = json.dumps(str(dest / "vendor"), ensure_ascii=False)
        if config.strip() and old not in config:
            raise SystemExit("unrecognized Cargo vendor configuration; refusing a stale source path")
        config = config.replace(old, new)
        (stage / "config.toml").write_text(config)
        (stage / "Cargo.lock").write_bytes(lock)
        (stage / "prepared.json").write_text(json.dumps({
            "lock_sha256": digest, "toolchain_root": str(sysroot),
            "runtime_roots": sorted(runtime_roots), "sdk_root": sdk_root, "linker": linker,
            "rustc_version": subprocess.check_output([str(sysroot / "bin" / executable("rustc")), "--version"], cwd=tool_cwd, env=env, text=True).strip(),
        }, indent=2) + "\n")
        # Destination is new and remains outside the candidate clone. stage and
        # dest share root, so publishing is one atomic rename: a killed run
        # leaves only a .prepare-* directory, never a partial snapshot.
        os.rename(stage, dest)
    print(f"Prepared {digest}; configure the harness with this application home.")
    print("Verification requires this unchanged lockfile and installed toolchain; it never fetches dependencies.")


if __name__ == "__main__":
    main()
