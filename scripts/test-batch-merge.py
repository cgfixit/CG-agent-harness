#!/usr/bin/env python3
"""Exercise merge authority and sequencing without performing GitHub writes."""

import ast
import copy
import importlib.util
import pathlib
import re
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "batch", pathlib.Path(__file__).with_name("batch-merge.py")
)
batch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(batch)


def pr(n):
    return {
        "head": {"sha": f"head{n}", "repo": {"full_name": "owner/repo"}},
        "base": {"ref": "main", "repo": {"full_name": "owner/repo"}},
        "merged": False,
        "state": "open",
        "draft": False,
        "mergeable": True,
    }


def rollup(conclusion="SUCCESS"):
    return {
        "contexts": {
            "pageInfo": {"hasNextPage": False},
            "nodes": [
                {
                    "__typename": "CheckRun",
                    "name": "test",
                    "status": "COMPLETED",
                    "conclusion": conclusion,
                }
            ],
        }
    }


def details(head):
    return {
        "headRefOid": head,
        "mergeStateStatus": "CLEAN",
        "reviewDecision": "",
        "reviewThreads": {"pageInfo": {"hasNextPage": False}, "nodes": []},
        "reviews": {"pageInfo": {"hasNextPage": False}, "nodes": []},
        "comments": {"pageInfo": {"hasNextPage": False}, "nodes": []},
        "commits": {"nodes": [{"commit": {"statusCheckRollup": rollup()}}]},
    }


class GitHub:
    """Stateful API fixture: updating and merging change the next reads."""

    def __init__(self):
        self.prs = {n: pr(n) for n in (1, 2)}
        self.base = "base0"
        self.parents = {}
        self.writes = []
        self.waits = []
        self.fail_head = None
        self.main_pending = False
        self.foreign_update = False
        self.review = None

    def __call__(self, path, method="GET", body=None, paginate=False):
        if path == "graphql":
            p = self.prs[body["variables"]["n"]]
            d = details(p["head"]["sha"])
            if self.review:
                d.update(self.review)
            return {"data": {"repository": {"pullRequest": d}}}
        suffix = path.removeprefix("repos/owner/repo/")
        if suffix == "git/ref/heads/main":
            return {"object": {"sha": self.base}}
        if suffix == "branches/main/protection":
            return {
                "required_status_checks": {"strict": True, "contexts": ["test"]},
                "enforce_admins": {"enabled": True},
            }
        if suffix.startswith("compare/"):
            head = suffix.split("...")[1]
            return {
                "status": "ahead"
                if self.base in self.parents.get(head, [])
                else "diverged"
            }
        if suffix.startswith("git/commits/"):
            return {
                "parents": [{"sha": s} for s in self.parents[suffix.split("/")[-1]]]
            }
        if suffix.startswith("actions/runs?"):
            from urllib.parse import parse_qs

            query = parse_qs(suffix.split("?", 1)[1])
            sha, event = query["head_sha"][0], query["event"][0]
            self.waits.append((sha, event, len(self.writes)))
            names = batch.expected_workflows(["src/server/example.rs"], event)
            return [
                {
                    "workflow_runs": [
                        {
                            "id": i,
                            "path": f".github/workflows/{name}",
                            "html_url": f"fixture/run/{i}",
                            "status": "in_progress"
                            if self.main_pending and sha == "merged1"
                            else "completed",
                            "conclusion": "failure"
                            if sha == self.fail_head
                            else "success",
                        }
                        for i, name in enumerate(sorted(names))
                    ]
                }
            ]
        n = int(suffix.split("/")[1])
        p = self.prs[n]
        if suffix.endswith("/files?per_page=100"):
            return [[{"filename": "src/server/example.rs"}]]
        if method == "GET":
            return copy.deepcopy(p)
        self.writes.append((n, suffix.split("/")[-1], copy.deepcopy(body)))
        if suffix.endswith("update-branch"):
            old = p["head"]["sha"]
            if body["expected_head_sha"] != old:
                raise AssertionError("unbound update")
            p["head"]["sha"] = f"updated{n}"
            self.parents[f"updated{n}"] = [
                "foreign" if self.foreign_update else old,
                self.base,
            ]
            return {"message": "updating"}
        if suffix.endswith("/merge"):
            if body != {"sha": p["head"]["sha"], "merge_method": "squash"}:
                raise AssertionError("unbound merge")
            p["merged"] = True
            self.base = f"merged{n}"
            return {"merged": True, "sha": self.base}
        raise AssertionError(suffix)


class BatchTests(unittest.TestCase):
    def setUp(self):
        self.github = GitHub()
        self.api = patch.object(batch, "api", self.github)
        self.api.start()
        self.addCleanup(self.api.stop)
        self.queue = batch.Queue("owner/repo", execute=True)
        self.queue.pause = lambda: None

    def test_order_update_new_ci_merge_and_main_before_next(self):
        self.queue.run([2, 1])
        self.assertEqual(
            [(n, op) for n, op, _ in self.github.writes],
            [(2, "update-branch"), (2, "merge"), (1, "update-branch"), (1, "merge")],
        )
        self.assertIn(("updated2", "pull_request", 1), self.github.waits)
        self.assertIn(("merged2", "push", 2), self.github.waits)

    def test_failure_does_not_touch_later_pr(self):
        self.github.fail_head = "updated1"
        with self.assertRaisesRegex(batch.Stop, "did not pass"):
            self.queue.run([1, 2])
        self.assertEqual(
            [(n, op) for n, op, _ in self.github.writes], [(1, "update-branch")]
        )

    def test_main_bundle_wait_stops_before_next_update(self):
        self.github.main_pending = True

        def pause():
            if self.github.base == "merged1":
                raise batch.Stop("budget exhausted")

        self.queue.pause = pause
        with self.assertRaisesRegex(batch.Stop, "budget"):
            self.queue.run([1, 2])
        self.assertEqual([n for n, _, _ in self.github.writes], [1, 1])

    def test_unexpected_update_is_not_authorized(self):
        self.github.foreign_update = True
        with self.assertRaisesRegex(batch.Stop, "unexpected commits"):
            self.queue.run([1, 2])
        self.assertEqual(len(self.github.writes), 1)

    def test_preflight_all_drafts_before_first_write(self):
        self.github.prs[2]["draft"] = True
        with self.assertRaisesRegex(batch.Stop, "draft"):
            self.queue.run([1, 2])
        self.assertEqual(self.github.writes, [])

    def test_preview_accepts_drafts_without_mutation(self):
        self.github.prs[2]["draft"] = True
        self.queue.execute = False
        self.queue.run([1, 2])
        self.assertEqual(self.github.writes, [])

    def test_resume_skips_merged_entries(self):
        self.github.prs[1]["merged"] = True
        self.queue.run([1, 2])
        self.assertEqual([n for n, _, _ in self.github.writes], [2, 2])

    def test_review_thread_stops_merge(self):
        self.github.review = {
            "reviewThreads": {
                "pageInfo": {"hasNextPage": False},
                "nodes": [{"isResolved": False}],
            }
        }
        with self.assertRaisesRegex(batch.Stop, "Unresolved"):
            self.queue.run([1])
        self.assertEqual(len(self.github.writes), 1)

    def test_missing_workflow_is_not_green(self):
        self.assertFalse(batch.workflows_ready([], batch.CORE))

    def test_checks_failed_pending_and_skipped(self):
        with self.assertRaises(batch.Stop):
            batch.checks_ready(rollup("FAILURE"))
        value = rollup()
        value["contexts"]["nodes"][0]["status"] = "QUEUED"
        self.assertFalse(batch.checks_ready(value))
        self.assertTrue(batch.checks_ready(rollup("SKIPPED")))

    def test_inputs_reject_duplicates_shell_text_and_empty(self):
        self.assertEqual(batch.numbers("#2, 1"), [2, 1])
        for value in ("", "1,1", "0", "1;echo hello", "$(date)", "-1", "1,"):
            with self.subTest(value=value), self.assertRaises(batch.Stop):
                batch.numbers(value)

    def test_strict_protection_required(self):
        with (
            patch.object(self.queue, "get", return_value={}),
            self.assertRaisesRegex(batch.Stop, "protection"),
        ):
            self.queue.protection()

    def test_external_head_change_stops_before_next_pr(self):
        old_wait = self.queue.wait_main

        def wait(sha, files):
            old_wait(sha, files)
            if sha == "merged1":
                self.github.prs[2]["head"]["sha"] = "unreviewed"

        self.queue.wait_main = wait
        with self.assertRaisesRegex(batch.Stop, "outside this batch"):
            self.queue.run([1, 2])
        self.assertEqual([n for n, _, _ in self.github.writes], [1, 1])

    def test_running_bot_review_is_a_blocker(self):
        value = details("head1")
        value["comments"]["nodes"] = [
            {
                "author": {"login": "chatgpt-codex-connector[bot]"},
                "body": "<!-- codex-pull-request-review-summary --> **Running**",
            }
        ]
        with self.assertRaisesRegex(batch.Stop, "still running"):
            batch.review_clear(value)

    def test_changes_requested_without_required_review_rule(self):
        value = details("head1")
        value["reviews"]["nodes"] = [
            {"author": {"login": "reviewer"}, "state": "CHANGES_REQUESTED"},
            {"author": {"login": "reviewer"}, "state": "COMMENTED"},
        ]
        with self.assertRaisesRegex(batch.Stop, "requests changes"):
            batch.review_clear(value)
        value["reviews"]["nodes"].append(
            {"author": {"login": "reviewer"}, "state": "APPROVED"}
        )
        self.assertTrue(batch.review_clear(value))

    def test_expected_path_filters_match_workflow_sources(self):
        # This deliberately accepts only the repository's current simple YAML
        # shapes. A changed trigger must update the queue's absence policy too.
        root = pathlib.Path(__file__).resolve().parents[1]
        for name, expected in batch.PATHS.items():
            text = (root / ".github/workflows" / name).read_text()
            section = re.search(
                r"^  pull_request:\n(.*?)(?=^  \w|^\S)", text, re.MULTILINE | re.DOTALL
            ).group(1)
            inline = re.search(r"paths: (\[.*\])", section)
            actual = (
                ast.literal_eval(inline.group(1))
                if inline
                else re.findall(
                    r'^      - ["\']?([^"\'\n]+)["\']?$', section, re.MULTILINE
                )
            )
            self.assertEqual(set(actual), set(expected), name)


if __name__ == "__main__":
    unittest.main()
