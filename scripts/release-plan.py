#!/usr/bin/env python3
"""Read-only release planning; fail closed on API errors and version collisions."""
import json
import os
import re
import subprocess
from urllib.parse import quote


def api(path):
    return json.loads(subprocess.check_output(["gh", "api", path], text=True))


def bundle_succeeded(runs, sha):
    if not isinstance(runs, dict) or not isinstance(runs.get("workflow_runs"), list):
        raise ValueError("Expected workflow runs list")
    return any(
        run.get("head_sha") == sha and run.get("conclusion") == "success"
        for run in runs["workflow_runs"]
    )


def plan(event, ref, publish, sha, tree, latest, latest_tree, tags, releases, bundle_ok=True):
    stable = r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Expected an exact source commit")
    require_bundle = False
    if event == "push":
        tag = ref.removeprefix("refs/tags/")
        if not ref.startswith("refs/tags/") or not re.fullmatch(stable, tag):
            raise ValueError("Tag releases require vMAJOR.MINOR.PATCH")
        changed = True
        enabled = True
    elif event in ("schedule", "workflow_dispatch"):
        enabled = event == "schedule" or publish == "true"
        if enabled and ref != "refs/heads/main":
            raise ValueError("Automatic/manual publication must run on main")
        match = re.fullmatch(stable, latest["tag_name"])
        if not match or latest["draft"] or latest["prerelease"]:
            raise ValueError("Latest release must be a regular vMAJOR.MINOR.PATCH release")
        major, minor, patch = map(int, match.groups())
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


def main():
    repo = os.environ["GITHUB_REPOSITORY"]
    sha = os.environ["GITHUB_SHA"]
    prefix = f"repos/{repo}"
    latest = api(f"{prefix}/releases/latest")
    latest_tree = api(f"{prefix}/commits/{quote(latest['tag_name'], safe='')}")["commit"]["tree"]["sha"]
    tree = api(f"{prefix}/commits/{sha}")["commit"]["tree"]["sha"]
    if os.environ["GITHUB_EVENT_NAME"] == "push":
        tag_commit = api(f"{prefix}/commits/{quote(os.environ['GITHUB_REF'], safe='')}")
        if tag_commit["sha"] != sha:
            raise ValueError("Tag moved away from the verified source commit")
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
    event = os.environ["GITHUB_EVENT_NAME"]
    bundle_ok = True
    if event in ("schedule", "workflow_dispatch"):
        runs = api(f"{prefix}/actions/workflows/bundle.yml/runs?head_sha={sha}&status=success&per_page=10")
        bundle_ok = bundle_succeeded(runs, sha)
    result = plan(event, os.environ["GITHUB_REF"],
                  os.environ.get("PUBLISH", "false"), sha, tree, latest, latest_tree,
                  {t["name"] for t in pages("tags")},
                  {r["tag_name"] for r in pages("releases")},
                  bundle_ok)
    print(json.dumps(result, indent=2))
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            for key, value in result.items():
                output.write(f"{key}={value}\n")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
            summary.write("Release plan (preview never publishes):\n```json\n" + json.dumps(result, indent=2) + "\n```\n")


if __name__ == "__main__":
    main()
