#!/usr/bin/env python3
"""Measure a cg-doc-sync pass: words per changed doc against a base ref, the
trim band, and relative links that no longer resolve.

Read-only: runs `git show` and reads files; never writes, builds or dials.
Exit 0 when every changed in-scope doc sits in the band and every relative
link resolves, 1 when one does not, 2 when git or the base ref is unusable.

Usage: python3 .claude/skills/cg-doc-sync/trim_report.py [--base origin/main]
       [--min 5] [--max 15] [PATH ...]
"""
import argparse
import os
import re
import subprocess
import sys

# README.md plus the topic guides. docs/learning is operator-only reading
# (AGENTS.md) and screenshots hold no prose.
IN_SCOPE = re.compile(r"^(README\.md|docs/[^/]+\.md)$")
LINK = re.compile(r"\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True, check=False)


def words(text):
    # The DOCS_BUDGET counts whitespace-separated runs, like `wc -w`.
    return len(text.split())


def changed_docs(base):
    out = git("diff", "--name-only", "--diff-filter=AM", f"{base}...HEAD")
    if out.returncode != 0:
        return None
    tracked = out.stdout.split()
    unstaged = git("diff", "--name-only", "--diff-filter=AM").stdout.split()
    return sorted({p for p in tracked + unstaged if IN_SCOPE.match(p)})


def broken_links(path, text):
    bad = []
    in_fence = False
    for number, line in enumerate(text.splitlines(), 1):
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
        if in_fence:
            continue
        for target in LINK.findall(line):
            if re.match(r"^[a-z][a-z0-9+.-]*:", target) or target.startswith("#"):
                continue
            file_part = target.split("#", 1)[0]
            resolved = os.path.normpath(os.path.join(os.path.dirname(path), file_part))
            if not os.path.exists(resolved):
                bad.append((number, target))
    return bad


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base", default="origin/main")
    parser.add_argument("--min", type=float, default=5.0, help="smallest trim percent")
    parser.add_argument("--max", type=float, default=15.0, help="largest trim percent")
    parser.add_argument("paths", nargs="*", help="docs to measure (default: changed in-scope docs)")
    args = parser.parse_args()

    if git("rev-parse", "--verify", "--quiet", args.base).returncode != 0:
        print(f"cg-doc-sync: base ref {args.base!r} not found; fetch it first", file=sys.stderr)
        return 2
    paths = args.paths or changed_docs(args.base)
    if paths is None:
        print("cg-doc-sync: git diff failed", file=sys.stderr)
        return 2
    if not paths:
        print(f"cg-doc-sync: no README.md or docs/*.md changed against {args.base}")
        return 0

    failures = 0
    print(f"{'file':40} {'base':>6} {'now':>6} {'trim%':>7}  verdict")
    for path in paths:
        if not os.path.isfile(path):
            print(f"{path:40} {'-':>6} {'-':>6} {'-':>7}  FAIL missing")
            failures += 1
            continue
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
        now = words(text)
        shown = git("show", f"{args.base}:{path}")
        if shown.returncode != 0:
            print(f"{path:40} {'new':>6} {now:>6} {'-':>7}  FAIL new file (cg-doc-sync edits in place)")
            failures += 1
            continue
        base = words(shown.stdout)
        trim = 100.0 * (base - now) / base if base else 0.0
        verdict = "ok" if args.min <= trim <= args.max else f"OUT OF BAND ({args.min:g}-{args.max:g}%)"
        if verdict != "ok":
            failures += 1
        print(f"{path:40} {base:>6} {now:>6} {trim:>6.1f}%  {verdict}")
        for number, target in broken_links(path, text):
            print(f"  FAIL {path}:{number}: relative link {target} does not resolve")
            failures += 1
    print(f"\n{failures} failure(s) across {len(paths)} file(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
