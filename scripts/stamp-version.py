#!/usr/bin/env python3
"""Stamp a release tag's version into the build workspace; fail closed.

Release-only and never committed: rewrites the root package version in
Cargo.toml, the exact `=X.Y.Z` pin on its self dev-dependency (cargo-deny
bans wildcard path deps, and an unstamped pin fails resolution), that
package's own Cargo.lock entry, and the two bundle version keys in
desktop/Info.plist. Every edit must hit exactly one target, and
nothing else in Cargo.lock may change, so `cargo build --locked` still holds.

Usage: stamp-version.py --tag vMAJOR.MINOR.PATCH [--root DIR]
       stamp-version.py --tag vX.Y.Z --check-version "<cgagentharness --version output>"
       stamp-version.py --tag vX.Y.Z --check-app-zip CG-Agent-Harness-macos-universal.zip
"""
import argparse
import plistlib
import re
import sys
import zipfile
from pathlib import Path

PACKAGE = "cgagentharness"
# Same stable-tag grammar as scripts/release-plan.py: no leading zeros,
# no pre-release or build metadata.
TAG = re.compile(r"v((?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*))")
SEMVER = r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
OLD = rf'"{SEMVER}"'


def version_of(tag):
    match = TAG.fullmatch(tag)
    if not match:
        raise ValueError(f"Expected a stable vMAJOR.MINOR.PATCH tag, got {tag!r}")
    return match.group(1)


def replace_once(text, pattern, repl, what):
    new, count = re.subn(pattern, repl, text, flags=re.MULTILINE)
    if count != 1:
        raise ValueError(f"Expected exactly one {what}, found {count}")
    return new


def stamp_manifest(text, version):
    # The first table must be [package]; only its version line is rewritten.
    head, sep, rest = text.partition("\n[")
    if not head.startswith("[package]\n") or f'\nname = "{PACKAGE}"\n' not in head + "\n":
        raise ValueError(f"Cargo.toml must start with the [package] table for {PACKAGE}")
    head = replace_once(head, rf"^version = {OLD}$", f'version = "{version}"', "[package] version")
    rest = replace_once(
        rest,
        rf'^({PACKAGE} = \{{ path = "\.", version = "=){SEMVER}(")',
        rf"\g<1>{version}\g<2>",
        f"{PACKAGE} self dev-dependency pin",
    )
    return head + sep + rest


def stamp_lock(text, version):
    # The root package is the only lock entry with this name and no source.
    pattern = rf'(^\[\[package\]\]\nname = "{PACKAGE}"\nversion = ){OLD}(\n(?!source = ))'
    new = replace_once(text, pattern, rf'\g<1>"{version}"\g<2>', f"{PACKAGE} Cargo.lock entry")
    before, after = text.splitlines(), new.splitlines()
    changed = [i for i, (a, b) in enumerate(zip(before, after)) if a != b]
    if len(before) != len(after) or len(changed) > 1:
        raise ValueError("Cargo.lock stamp touched more than the root package version")
    return new


def stamp_plist(text, version):
    for key in ("CFBundleShortVersionString", "CFBundleVersion"):
        text = replace_once(
            text,
            rf"(<key>{key}</key>\s*<string>)[^<]*(</string>)",
            rf"\g<1>{version}\g<2>",
            f"{key} entry",
        )
    return text


def stamp(root, tag):
    version = version_of(tag)
    root = Path(root)
    targets = [
        (root / "Cargo.toml", stamp_manifest),
        (root / "Cargo.lock", stamp_lock),
        (root / "desktop" / "Info.plist", stamp_plist),
    ]
    # Compute every edit before writing any, so a failure leaves no partial stamp.
    edits = [(path, fn(path.read_text(encoding="utf-8"), version)) for path, fn in targets]
    for path, text in edits:
        path.write_text(text, encoding="utf-8")
    return version


def check_version(output, tag):
    expected = f"{PACKAGE} {version_of(tag)}"
    if output.strip() != expected:
        raise ValueError(f"Built binary reports {output.strip()!r}, expected {expected!r}")


APP_PLIST = "CG Agent Harness.app/Contents/Info.plist"


def check_app_zip(path, tag):
    version = version_of(tag)
    with zipfile.ZipFile(path) as archive:
        names = [n for n in archive.namelist() if n == APP_PLIST]
        if len(names) != 1:
            raise ValueError(f"Expected exactly one {APP_PLIST} in {path}")
        info = plistlib.loads(archive.read(APP_PLIST))
    for key in ("CFBundleShortVersionString", "CFBundleVersion"):
        if info.get(key) != version:
            raise ValueError(f"Bundled {key} is {info.get(key)!r}, expected {version!r}")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--tag", required=True)
    parser.add_argument("--root", default=".")
    checks = parser.add_mutually_exclusive_group()
    checks.add_argument("--check-version")
    checks.add_argument("--check-app-zip")
    args = parser.parse_args(argv)
    try:
        if args.check_app_zip is not None:
            check_app_zip(args.check_app_zip, args.tag)
            print(f"bundle Info.plist matches {args.tag}")
        elif args.check_version is not None:
            check_version(args.check_version, args.tag)
            print(f"version matches {args.tag}")
        else:
            print(f"stamped {stamp(args.root, args.tag)}")
    except (ValueError, OSError, zipfile.BadZipFile, plistlib.InvalidFileException) as err:
        print(f"stamp-version: {err}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
