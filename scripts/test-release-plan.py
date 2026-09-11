#!/usr/bin/env python3
import importlib.util
import os
import subprocess
import tempfile
import sys
from pathlib import Path
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("release_plan", os.path.join(os.path.dirname(__file__), "release-plan.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ReleasePlanTests(unittest.TestCase):
    def plan(self, **overrides):
        args = dict(event="schedule", ref="refs/heads/main", publish="false", sha="a" * 40,
                    tree="new", latest={"tag_name": "v0.1.0", "draft": False, "prerelease": False},
                    latest_tree="old", tags={"v0.1.0"}, releases={"v0.1.0"})
        args.update(overrides)
        return module.plan(**args)

    def test_changed_main_publishes_next_patch(self):
        result = self.plan()
        self.assertEqual(result["tag"], "v0.1.1")
        self.assertEqual(result["should_release"], "true")
        self.assertEqual(result["source_sha"], "a" * 40)

    def test_identical_tree_skips_even_with_different_commit(self):
        self.assertEqual(self.plan(tree="old")["should_release"], "false")

    def test_preview_on_feature_branch_never_publishes(self):
        self.assertEqual(self.plan(event="workflow_dispatch", ref="refs/heads/codex/test")["should_release"], "false")

    def test_manual_main_publish(self):
        self.assertEqual(self.plan(event="workflow_dispatch", publish="true")["should_release"], "true")

    def test_off_main_publication_refused(self):
        for event in ("schedule", "workflow_dispatch"):
            with self.subTest(event=event), self.assertRaises(ValueError):
                self.plan(event=event, publish="true", ref="refs/heads/codex/test")

    def test_existing_tag_and_draft_collisions_refused(self):
        for args in ({"tags": {"v0.1.1"}}, {"releases": {"v0.1.1"}}):
            with self.subTest(args=args), self.assertRaises(ValueError):
                self.plan(**args)

    def test_explicit_new_stable_tag(self):
        self.assertEqual(self.plan(event="push", ref="refs/tags/v1.0.0")["tag"], "v1.0.0")

    def test_published_tag_cannot_be_reuploaded(self):
        with self.assertRaises(ValueError):
            self.plan(event="push", ref="refs/tags/v0.1.0")

    def test_invalid_inputs_refused(self):
        for args in ({"sha": "main"}, {"event": "pull_request"},
                     {"event": "push", "ref": "refs/heads/main"},
                     {"event": "push", "ref": "refs/tags/v1.0.0-rc.1"},
                     {"latest": {"tag_name": "v0.1.0\ninjected=true", "draft": False, "prerelease": False}}):
            with self.subTest(args=args), self.assertRaises(ValueError):
                self.plan(**args)

    def test_main_writes_preview_outputs_from_api(self):
        replies = [
            {"tag_name": "v0.1.0", "draft": False, "prerelease": False},
            {"commit": {"tree": {"sha": "old"}}},
            {"commit": {"tree": {"sha": "new"}}},
            [{"name": "v0.1.0"}], [{"tag_name": "v0.1.0"}],
        ]
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "output"
            env = {"GITHUB_REPOSITORY": "example/repo", "GITHUB_SHA": "a" * 40,
                   "GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_REF": "refs/heads/codex/test",
                   "PUBLISH": "false", "GITHUB_OUTPUT": str(output)}
            with patch.dict(os.environ, env, clear=True), patch.object(module, "api", side_effect=replies):
                module.main()
            self.assertIn("changed=true", output.read_text())
            self.assertIn("should_release=false", output.read_text())

    def test_tag_move_refused_before_outputs(self):
        replies = [
            {"tag_name": "v0.1.0"},
            {"commit": {"tree": {"sha": "old"}}},
            {"commit": {"tree": {"sha": "new"}}},
            {"sha": "b" * 40},
        ]
        env = {"GITHUB_REPOSITORY": "example/repo", "GITHUB_SHA": "a" * 40,
               "GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/tags/v0.2.0"}
        with patch.dict(os.environ, env, clear=True), patch.object(module, "api", side_effect=replies):
            with self.assertRaisesRegex(ValueError, "Tag moved"):
                module.main()

    def test_api_errors_are_not_unchanged_success(self):
        with patch.object(module.subprocess, "check_output", side_effect=subprocess.CalledProcessError(1, "gh")):
            with self.assertRaises(subprocess.CalledProcessError):
                module.api("repos/example/repo/releases/latest")
        with patch.object(module.subprocess, "check_output", return_value="not JSON"):
            with self.assertRaises(ValueError):
                module.api("repos/example/repo/releases/latest")


if __name__ == "__main__":
    unittest.main()
