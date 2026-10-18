#!/usr/bin/env python3
"""Read-only release planning; fail closed on API errors and version collisions.

release.yml runs this with the GitHub Actions environment. Run by hand outside
Actions, it previews the next release of main exactly like a manual dispatch
with publish=false: `python3 scripts/release-plan.py` (needs `gh auth login`).
Set GITHUB_EVENT_NAME=schedule to see what the next scheduled run would decide.
Nothing here ever publishes.
"""
import json
import os
import re
import subprocess
import sys
from urllib.parse import quote

# Outside Actions only; inside Actions a missing variable is an error.
LOCAL_PREVIEW = {"GITHUB_REPOSITORY": "cgfixit/CG-agent-harness",
                 "GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_REF": "refs/heads/main"}


def api(path):
    return json.loads(subprocess.check_output(["gh", "api", path], text=True))


def bundle_succeeded(runs, sha):
    if not isinstance(runs, dict) or not isinstance(runs.get("workflow_runs"), list):
        raise ValueError("Expected workflow runs list")
    # Bundle runs only on main pushes since #486. Before that, docs-only
    # pull_request runs skipped packaging yet concluded "success", and other PR
    # runs packaged a merge preview, so only push (and pre-#486 dispatch) count.
    return any(
        run.get("head_sha") == sha and run.get("conclusion") == "success"
        and run.get("event") in ("push", "workflow_dispatch")
        for run in runs["workflow_runs"]
    )


def on_main(compare):
    """True when the compared commit is main or an ancestor of it."""
    if not isinstance(compare, dict) or not isinstance(compare.get("status"), str):
        raise ValueError("Expected a commit comparison")
    return compare["status"] in ("identical", "behind")


def plan(event, ref, publish, sha, tree, latest, latest_tree, tags, releases, bundle_ok=True,
         reachable_from_main=False):
    stable = r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Expected an exact source commit")
    match = re.fullmatch(stable, latest["tag_name"])
    current = None if not match or latest["draft"] or latest["prerelease"] else tuple(map(int, match.groups()))
    require_bundle = False
    if event == "push":
        tag = ref.removeprefix("refs/tags/")
        tag_match = re.fullmatch(stable, tag)
        if not ref.startswith("refs/tags/") or not tag_match:
            raise ValueError("Tag releases require vMAJOR.MINOR.PATCH")
        # Whoever can push a tag must not publish --latest from an unmerged branch.
        if reachable_from_main is not True:
            raise ValueError("Tag releases require a commit reachable from main")
        # --latest would make releases/latest serve an older app. An irregular
        # latest release is left for a newer stable tag to replace.
        if current is not None and tuple(map(int, tag_match.groups())) <= current:
            raise ValueError("Tag releases must be newer than the latest release")
        changed = True
        enabled = True
    elif event in ("schedule", "workflow_dispatch"):
        enabled = event == "schedule" or publish == "true"
        if enabled and ref != "refs/heads/main":
            raise ValueError("Automatic/manual publication must run on main")
        if current is None:
            raise ValueError("Latest release must be a regular vMAJOR.MINOR.PATCH release")
        major, minor, patch = current
        tag = f"v{major}.{minor}.{patch + 1}"
        changed = tree != latest_tree
        if changed and tag in tags:
            raise ValueError(f"Candidate tag already exists: {tag}")
        require_bundle = True
    else:
        raise ValueError("Unsupported release event")
    if changed and tag in releases:
        raise ValueError(f"Release already exists (including drafts): {tag}")
    ready = changed and enabled and (bundle_ok if require_bundle else True)
    return {"tag": tag, "source_sha": sha, "changed": str(changed).lower(),
            "bundle_ok": str(bool(bundle_ok)).lower(),
            "should_release": str(ready).lower()}


def setting(name):
    value = os.environ.get(name)
    if value:
        return value
    if os.environ.get("GITHUB_ACTIONS") == "true" or name not in LOCAL_PREVIEW:
        raise SystemExit(f"release-plan: {name} is not set")
    return LOCAL_PREVIEW[name]


def main():
    repo = setting("GITHUB_REPOSITORY")
    prefix = f"repos/{repo}"
    if os.environ.get("GITHUB_SHA") or os.environ.get("GITHUB_ACTIONS") == "true":
        sha = setting("GITHUB_SHA")
    else:
        sha = api(f"{prefix}/commits/{quote(setting('GITHUB_REF').removeprefix('refs/heads/'), safe='')}")["sha"]
        print(f"release-plan: local preview of {repo} at {sha} (nothing is published)", file=sys.stderr)
    latest = api(f"{prefix}/releases/latest")
    latest_tree = api(f"{prefix}/commits/{quote(latest['tag_name'], safe='')}")["commit"]["tree"]["sha"]
    tree = api(f"{prefix}/commits/{sha}")["commit"]["tree"]["sha"]
    if setting("GITHUB_EVENT_NAME") == "push":
        tag_commit = api(f"{prefix}/commits/{quote(setting('GITHUB_REF'), safe='')}")
        if tag_commit["sha"] != sha:
            raise ValueError("Tag moved away from the verified source commit")
        reachable = on_main(api(f"{prefix}/compare/main...{sha}"))
    else:
        reachable = False
    def pages(endpoint):
        values = []
        page = 1
        while True:
            batch = api(f"{prefix}/{endpoint}?per_page=100&page={page}")
            if not isinstance(batch, list):
                raise ValueError("Expected an API list")
            values.extend(batch)
            if len(batch) < 100:
                return values
            page += 1
    event = setting("GITHUB_EVENT_NAME")
    bundle_ok = True
    if event in ("schedule", "workflow_dispatch"):
        runs = api(f"{prefix}/actions/workflows/bundle.yml/runs?head_sha={sha}&status=success&per_page=10")
        bundle_ok = bundle_succeeded(runs, sha)
    result = plan(event, setting("GITHUB_REF"),
                  os.environ.get("PUBLISH", "false"), sha, tree, latest, latest_tree,
                  {t["name"] for t in pages("tags")},
                  {r["tag_name"] for r in pages("releases")},
                  bundle_ok, reachable)
    print(json.dumps(result, indent=2))
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            for key, value in result.items():
                output.write(f"{key}={value}\n")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
            summary.write("Release plan (preview never publishes):\n```json\n" + json.dumps(result, indent=2) + "\n```\n")


if __name__ == "__main__":
    try:
        main()
    except FileNotFoundError:
        raise SystemExit("release-plan: needs the GitHub CLI (gh) on PATH, signed in with `gh auth login`")
    except subprocess.CalledProcessError:
        raise SystemExit("release-plan: a `gh api` call failed (see its message above; check `gh auth status`)")
