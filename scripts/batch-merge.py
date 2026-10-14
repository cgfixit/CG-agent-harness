#!/usr/bin/env python3
"""Explicit, ordered squash queue. Preview by default; never checks out PR code."""

import fnmatch
import json
import os
import re
import subprocess
import sys
import time


class Stop(RuntimeError):
    """Leave the remaining list untouched and report a concrete blocker."""


def numbers(value):
    if not re.fullmatch(r"\s*#?[1-9][0-9]*(?:[\s,]+#?[1-9][0-9]*)*\s*", value):
        raise Stop("Use comma/space-separated positive PR numbers in dependency order.")
    result = [int(n) for n in re.findall(r"[0-9]+", value)]
    if len(result) > 20 or len(set(result)) != len(result):
        raise Stop("Supply 1–20 distinct PR numbers.")
    return result


def api(path, method="GET", body=None, paginate=False):
    args = ["gh", "api", "--method", method, path]
    if paginate:
        args += ["--paginate", "--slurp"]
    if body is not None:
        args += ["--input", "-"]
    result = subprocess.run(
        args,
        input=json.dumps(body) if body is not None else None,
        text=True,
        capture_output=True,
        timeout=60,
        check=False,
    )
    if result.returncode:
        raise Stop(f"GitHub {method} {path} failed: {result.stderr.strip()}")
    value = json.loads(result.stdout)
    if isinstance(value, dict) and value.get("errors"):
        raise Stop("GitHub returned GraphQL errors; no merge attempted.")
    return value


# Unconditional PR/push workflows form an absence-detection floor. Conditional
# runs and all external checks are also checked, not just branch requirements.
CORE = {"ci.yml", "codeql.yml", "devskim.yml", "gitleaks.yml", "rust-clippy.yml"}
PATHS = {
    "zizmor.yml": [
        ".github/workflows/**",
        "scripts/*release-plan.py",
        "scripts/*batch-merge.py",
    ],
    "script-lint.yml": [
        "scripts/**",
        ".githooks/**",
        ".claude/hooks/**",
        ".claude/skills/**",
        ".github/workflows/script-lint.yml",
    ],
    "netconnect.yml": [
        "src/netconnect/**",
        "tests/netconnect*",
        "tests/netconnect_support/**",
        ".github/workflows/netconnect.yml",
        "scripts/netconnect-syscall-proof.sh",
        "assets/config.default.yaml",
        "src/main.rs",
        "src/lib.rs",
        "src/common/config.rs",
        "src/common/errors.rs",
        "src/common/home.rs",
        "src/common/process.rs",
        "src/common/process/**",
        "Cargo.toml",
        "Cargo.lock",
    ],
    "live-ollama.yml": [
        ".github/workflows/live-ollama.yml",
        "scripts/live-ollama/**",
        "scripts/smoke-ollama.sh",
        "src/server/model_limits.rs",
        "src/server/routes/ollama.rs",
        "src/server/slash.rs",
        "src/llm/ollama.rs",
        "src/llm/inventory.rs",
        "src/llm/profile.rs",
    ],
    "copilot-setup-steps.yml": [".github/workflows/copilot-setup-steps.yml"],
}


def expected_workflows(files, event):
    expected = set(CORE)
    for workflow, patterns in PATHS.items():
        if event == "push" and workflow == "live-ollama.yml":
            continue
        if any(
            fnmatch.fnmatchcase(path, pattern) for path in files for pattern in patterns
        ):
            expected.add(workflow)
    if event == "push":
        expected.add("bundle.yml")
    return expected


def workflows_ready(runs, expected):
    latest = {}
    for run in runs:
        path = run["path"].split("@", 1)[0].rsplit("/", 1)[-1]
        if path not in latest or run["id"] > latest[path]["id"]:
            latest[path] = run
    for run in latest.values():
        if run["status"] == "completed" and run["conclusion"] != "success":
            raise Stop(
                f"Workflow did not pass: {run['html_url']} ({run['conclusion']})"
            )
    return expected <= latest.keys() and all(
        r["status"] == "completed" for r in latest.values()
    )


def review_clear(pr):
    if pr["reviewDecision"] == "CHANGES_REQUESTED":
        raise Stop("Changes requested; resolve the review before resuming.")
    for key in ("reviewThreads", "comments", "reviews"):
        if pr[key]["pageInfo"]["hasNextPage"]:
            raise Stop(
                "Review exceeds the 100-item inspection bound; inspect manually."
            )
    latest = {}
    for review in pr["reviews"]["nodes"]:
        if review.get("author") and review["state"] in (
            "APPROVED",
            "CHANGES_REQUESTED",
            "DISMISSED",
        ):
            latest[review["author"]["login"]] = review["state"]
    if "CHANGES_REQUESTED" in latest.values():
        raise Stop(
            "A reviewer still requests changes; resolve the review before resuming."
        )
    if any(not t["isResolved"] for t in pr["reviewThreads"]["nodes"]):
        raise Stop("Unresolved review threads; resolve them before resuming.")
    if any(
        c.get("author")
        and c["author"]["login"]
        in ("chatgpt-codex-connector", "chatgpt-codex-connector[bot]")
        and "codex-pull-request-review-summary" in c["body"]
        and "**Running**" in c["body"]
        for c in pr["comments"]["nodes"]
    ):
        raise Stop("Codex review is still running.")
    return pr["reviewDecision"] != "REVIEW_REQUIRED"


def checks_ready(rollup):
    if not rollup:
        return False
    contexts = rollup["contexts"]
    if contexts["pageInfo"]["hasNextPage"]:
        raise Stop("More than 100 check contexts; inspect manually.")
    ready = bool(contexts["nodes"])
    for check in contexts["nodes"]:
        if check["__typename"] == "CheckRun":
            if check["status"] != "COMPLETED":
                ready = False
            elif check["conclusion"] not in ("SUCCESS", "SKIPPED", "NEUTRAL"):
                raise Stop(
                    f"Check did not pass: {check['name']} ({check['conclusion']})"
                )
        elif check["state"] == "PENDING":
            ready = False
        elif check["state"] != "SUCCESS":
            raise Stop(f"Commit status did not pass: {check['context']}")
    return ready


class Queue:
    def __init__(self, repo, execute=False, minutes=300):
        if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
            raise Stop("GITHUB_REPOSITORY must be owner/repository.")
        self.repo, self.execute = repo, execute
        self.prefix = f"repos/{repo}"
        self.deadline = time.monotonic() + minutes * 60

    def get(self, suffix, **kwargs):
        return api(f"{self.prefix}/{suffix}", **kwargs)

    def pause(self):
        if time.monotonic() + 30 >= self.deadline:
            raise Stop("Wait budget exhausted; rerun the same list to resume.")
        time.sleep(30)

    def main(self):
        return self.get("git/ref/heads/main")["object"]["sha"]

    def pr(self, n, preview=False):
        pr = self.get(f"pulls/{n}")
        if pr["base"]["ref"] != "main" or pr["base"]["repo"]["full_name"] != self.repo:
            raise Stop(f"#{n} must target this repository's main.")
        if not pr["head"]["repo"] or pr["head"]["repo"]["full_name"] != self.repo:
            raise Stop(f"#{n} is a fork or its branch repository is unavailable.")
        if not preview and not pr["merged"] and (pr["state"] != "open" or pr["draft"]):
            raise Stop(f"#{n} is closed or draft; batch merge never changes readiness.")
        if not preview and pr.get("auto_merge"):
            raise Stop(
                f"#{n} already has independent auto-merge enabled; disable it first."
            )
        return pr

    def protection(self):
        # REST merge binds only the PR head, not the base. Strict protection
        # closes the final-read/merge race if another actor advances main.
        policy = self.get("branches/main/protection")
        required = policy.get("required_status_checks") or {}
        if (
            not required.get("strict")
            or not (required.get("contexts") or required.get("checks"))
            or not policy.get("enforce_admins", {}).get("enabled")
        ):
            raise Stop(
                "Execution requires main branch protection: required checks, strict up-to-date checks, "
                "and enforcement for administrators. No repository settings were changed."
            )

    def details(self, n):
        owner, name = self.repo.split("/")
        query = """query($owner:String!,$name:String!,$n:Int!){repository(owner:$owner,name:$name){
          pullRequest(number:$n){headRefOid mergeStateStatus reviewDecision
            reviewThreads(first:100){nodes{isResolved}pageInfo{hasNextPage}}
            reviews(first:100){nodes{author{login}state}pageInfo{hasNextPage}}
            comments(first:100){nodes{author{login}body}pageInfo{hasNextPage}}
            commits(last:1){nodes{commit{statusCheckRollup{contexts(first:100){pageInfo{hasNextPage}
              nodes{__typename ... on CheckRun{name status conclusion}
                ... on StatusContext{context state}}}}}}}
          }}}"""
        return api(
            "graphql",
            "POST",
            {"query": query, "variables": {"owner": owner, "name": name, "n": n}},
        )["data"]["repository"]["pullRequest"]

    def files(self, n):
        return [
            f["filename"]
            for page in self.get(f"pulls/{n}/files?per_page=100", paginate=True)
            for f in page
        ]

    def runs_ready(self, sha, event, expected):
        runs = [
            r
            for page in self.get(
                f"actions/runs?head_sha={sha}&event={event}&per_page=100", paginate=True
            )
            for r in page["workflow_runs"]
        ]
        return workflows_ready(runs, expected)

    def wait_main(self, sha, files):
        # Two green observations cover late-registering downstream analysis jobs.
        green = 0
        while green < 2:
            if self.main() != sha:
                raise Stop(
                    "main moved outside this batch; preview and restart against the new base."
                )
            green = (
                green + 1
                if self.runs_ready(sha, "push", expected_workflows(files, "push"))
                else 0
            )
            if green < 2:
                self.pause()
        self.checked_base = sha

    def process(self, n, expected_head):
        green = 0
        while True:
            pr = self.pr(n)
            if pr["merged"] or pr["head"]["sha"] != expected_head:
                raise Stop(f"#{n} changed outside this batch; preview and restart.")
            base = self.main()
            if base != self.checked_base:
                raise Stop(
                    "main changed after its CI was checked; preview and restart."
                )
            comparison = self.get(f"compare/{base}...{expected_head}")
            if comparison["status"] not in ("ahead", "identical"):
                if pr["mergeable"] is False:
                    raise Stop(
                        f"#{n} conflicts with main; resolve it manually, then restart."
                    )
                if pr["mergeable"] is None:
                    self.pause()
                    continue
                self.get(
                    f"pulls/{n}/update-branch",
                    method="PUT",
                    body={"expected_head_sha": expected_head},
                )
                old = expected_head
                while expected_head == old:
                    self.pause()
                    expected_head = self.pr(n)["head"]["sha"]
                # An unrelated contributor push is not an authorized branch update.
                commit = self.get(f"git/commits/{expected_head}")
                if {p["sha"] for p in commit["parents"]} != {old, base}:
                    raise Stop(
                        f"#{n} update included unexpected commits; inspect before resuming."
                    )
                green = 0
                continue
            details = self.details(n)
            if details["headRefOid"] != expected_head:
                raise Stop(f"#{n} head changed during inspection.")
            files = self.files(n)
            rollup = details["commits"]["nodes"][0]["commit"]["statusCheckRollup"]
            ready = review_clear(details) and checks_ready(rollup)
            ready = (
                self.runs_ready(
                    expected_head,
                    "pull_request",
                    expected_workflows(files, "pull_request"),
                )
                and ready
            )
            ready = ready and details["mergeStateStatus"] == "CLEAN"
            green = green + 1 if ready else 0
            if green < 2:
                print(f"#{n}: waiting for current-head CI/reviews", flush=True)
                self.pause()
                continue
            # GitHub enforces branch rules and the expected head on the write too.
            if self.main() != base or self.pr(n)["head"]["sha"] != expected_head:
                raise Stop(f"#{n}: base/head changed before merge; restart.")
            self.protection()
            merged = self.get(
                f"pulls/{n}/merge",
                method="PUT",
                body={"sha": expected_head, "merge_method": "squash"},
            )
            if not merged.get("merged") or not self.pr(n)["merged"]:
                raise Stop(
                    f"#{n}: merge was not confirmed; inspect GitHub before resuming."
                )
            print(
                f"#{n}: merged {merged['sha']}; waiting for main CI and Bundle",
                flush=True,
            )
            self.wait_main(merged["sha"], files)
            return

    def run(self, prs):
        # Preflight the complete list before the first mutation. Capture every
        # head so edits to later PRs cannot silently extend this authorization.
        planned = [(n, self.pr(n, preview=not self.execute)) for n in prs]
        for n, pr in planned:
            label = (
                "already merged"
                if pr["merged"]
                else "blocked: draft"
                if pr["draft"]
                else pr["state"]
            )
            print(f"#{n}: {pr['head']['sha']} {label}", flush=True)
        if not self.execute:
            for n, pr in planned:
                if pr["merged"]:
                    continue
                try:
                    detail = self.details(n)
                    if detail["headRefOid"] != pr["head"]["sha"]:
                        raise Stop("head changed during preview")
                    reviewed = review_clear(detail)
                    rollup = detail["commits"]["nodes"][0]["commit"][
                        "statusCheckRollup"
                    ]
                    ready = checks_ready(rollup)
                    ready = (
                        self.runs_ready(
                            pr["head"]["sha"],
                            "pull_request",
                            expected_workflows(self.files(n), "pull_request"),
                        )
                        and ready
                    )
                    print(
                        f"#{n}: CI {'passed' if ready else 'pending/missing'}; "
                        f"reviews {'clear' if reviewed else 'approval required'}; {detail['mergeStateStatus']}"
                    )
                except Stop as error:
                    print(f"#{n}: blocked: {error}")
            print(
                "Execution also requires BATCH_MERGE_TOKEN and strict main protection including administrators."
            )
            print(
                "Preview only: listed order retained; no branches updated or PRs merged."
            )
            return
        self.protection()
        self.wait_main(self.main(), [])
        for n, pr in planned:
            if not pr["merged"]:
                self.process(n, pr["head"]["sha"])


def main():
    prs = numbers(os.environ.get("PR_LIST", ""))
    execute = os.environ.get("EXECUTE", "false")
    if execute not in ("true", "false"):
        raise Stop("EXECUTE must be true or false.")
    minutes = os.environ.get("TIMEOUT_MINUTES", "300")
    if not minutes.isdecimal() or not 1 <= int(minutes) <= 330:
        raise Stop("TIMEOUT_MINUTES must be 1–330.")
    if execute == "true" and (
        os.environ.get("GITHUB_REF") != "refs/heads/main"
        or os.environ.get("BATCH_TOKEN_PRESENT") != "true"
    ):
        raise Stop(
            "Execution requires a main dispatch and the BATCH_MERGE_TOKEN repository secret."
        )
    Queue(os.environ.get("GITHUB_REPOSITORY", ""), execute == "true", int(minutes)).run(
        prs
    )


if __name__ == "__main__":
    try:
        main()
    except (Stop, subprocess.TimeoutExpired) as error:
        print(f"Stopped: {error}", file=sys.stderr)
        sys.exit(1)
