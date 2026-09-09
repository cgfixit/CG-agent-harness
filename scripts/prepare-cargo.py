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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repository", type=Path)
    parser.add_argument("harness_home", type=Path)
    parser.add_argument("--online", action="store_true")
    args = parser.parse_args()
    repo = args.repository.resolve(strict=True)
    lock = (repo / "Cargo.lock").read_bytes()
    digest = hashlib.sha256(lock).hexdigest()
    root = args.harness_home.resolve() / "data/agentic/cargo-prepared"
    root.mkdir(parents=True, exist_ok=True)
    dest = root / digest
    if dest.exists():
        raise SystemExit("snapshot already exists; use it or choose a separate application home")
    env = dict(os.environ, RUSTUP_AUTO_INSTALL="0")
    executable = lambda name: name + (".exe" if os.name == "nt" else "")
    # Toolchain preparation is explicit: this probe cannot auto-install rustup tools.
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], cwd=repo, env=env, text=True).strip()).resolve(strict=True)
    for tool in ("cargo", "rustc", "rustdoc"):
        if not (sysroot / "bin" / executable(tool)).is_file():
            raise SystemExit(f"missing {tool} in selected toolchain; provision it before preparation")
    sdk_root = linker = None
    if shutil.which("xcrun"):
        sdk_root = str(Path(subprocess.check_output(["xcrun", "--show-sdk-path"], text=True).strip()).resolve(strict=True))
        linker = str(Path(subprocess.check_output(["xcrun", "--find", "clang"], text=True).strip()).resolve(strict=True))
    runtime_roots = set()
    if shutil.which("otool"):
        pending = [sysroot / "bin" / "cargo", sysroot / "bin" / "rustc", sysroot / "bin" / "rustdoc"]
        seen = set()
        while pending:
            binary = pending.pop().resolve()
            if binary in seen:
                continue
            seen.add(binary)
            output = subprocess.check_output(["otool", "-L", str(binary)], text=True)
            for line in output.splitlines()[1:]:
                name = line.strip().split(" (", 1)[0]
                if name.startswith(("/opt/homebrew/", "/usr/local/")):
                    library = Path(name).resolve(strict=True)
                    runtime_roots.add(str(library.parent))
                    pending.append(library)
    with tempfile.TemporaryDirectory(prefix=".prepare-", dir=root) as temporary:
        stage = Path(temporary)
        cmd = [str(sysroot / "bin" / executable("cargo")), "vendor", "--locked", "--versioned-dirs"]
        if not args.online:
            cmd.append("--offline")
        cmd.append(str(stage / "vendor"))
        config = subprocess.check_output(cmd, cwd=repo, env=env, text=True)
        if (repo / "Cargo.lock").read_bytes() != lock:
            raise SystemExit("lockfile changed during preparation; refusing snapshot")
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
            "rustc_version": subprocess.check_output([str(sysroot / "bin" / executable("rustc")), "--version"], text=True).strip(),
        }, indent=2) + "\n")
        # Destination is new and remains outside the candidate clone.
        shutil.copytree(stage, dest)
    print(f"Prepared {digest}; configure the harness with this application home.")
    print("Verification requires this unchanged lockfile and installed toolchain; it never fetches dependencies.")


if __name__ == "__main__":
    main()
