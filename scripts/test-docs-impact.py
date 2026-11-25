#!/usr/bin/env python3
"""Exercise the docs check and trusted repair with disposable Git history."""
import base64
import json
import os
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

SCRIPTS = Path(__file__).resolve().parent
IMPACT = runpy.run_path(str(SCRIPTS / "docs-impact.py"))
REPAIR = runpy.run_path(str(SCRIPTS / "repair-docs-impact.py"))
COMPUTER = "src/common/computer.rs"
WEB = "src/server/web_provider.rs"
BROWSER_GUIDE = "docs/Browser-Use.md"
WEB_GUIDE = "docs/WEB_DECISION_ROUTER.md"
REPO = "fixture/harness"


class DocsImpact(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        old_cwd = Path.cwd()
        os.chdir(temp.name)
        self.addCleanup(os.chdir, old_cwd)
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Docs Fixture")
        self.git("config", "user.email", "fixture@example.com")
        for file in (COMPUTER, WEB):
            self.write(file, "const ENABLED: bool = false;\n")
        for file in (BROWSER_GUIDE, WEB_GUIDE, "docs/MEMORY_GUIDE.md", "docs/STRUCTURED_MEMORY.md"):
            self.write(file, "# Fixture\n\nThe feature is disabled.\n")
        self.write("tests/invariant_guard.rs", """const DOCS_BUDGET: &[(&str, Kind, usize)] = &[
    ("docs/Browser-Use.md", Guide, 100),
    ("docs/WEB_DECISION_ROUTER.md", Guide, 100),
];
""")
        self.write(IMPACT["CONFIG"], "web:\n  enabled: false\nstructured_memory:\n  enabled: false\n")
        self.base = self.commit("fixture baseline")

    def git(self, *args):
        return subprocess.check_output(["git", *args], text=True).strip()

    def write(self, file, text):
        path = Path(file)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def commit(self, message):
        self.git("add", "--all")
        self.git("commit", "--allow-empty", "-qm", message)
        return self.git("rev-parse", "HEAD")

    def plan(self, head="HEAD"):
        return IMPACT["review_plan"](self.base, head)

    def test_author_update_and_diff_bound_explanation(self):
        self.write(COMPUTER, "const ENABLED: bool = true;\n")
        self.write(WEB, "const ENABLED: bool = true;\n")
        self.commit("change browser and web")
        pending, fingerprints = self.plan()
        self.assertEqual(pending, {BROWSER_GUIDE: [COMPUTER], WEB_GUIDE: [WEB]})
        self.write(BROWSER_GUIDE, "# Fixture\n\nThe  feature is disabled.\n")
        self.commit("whitespace alone")
        self.assertIn(BROWSER_GUIDE, self.plan()[0])
        self.write(BROWSER_GUIDE, "# Fixture\n\nThe feature is enabled.\n")
        self.commit(f"docs review\n\nDocs-Impact: {WEB_GUIDE} {fingerprints[WEB_GUIDE]} The public behavior is unchanged.")
        self.commit("another metadata-only commit")
        self.assertFalse(self.plan()[0])
        self.write(WEB, "const ENABLED: bool = false;\nconst FALLBACK: bool = true;\n")
        self.commit("change a previously reviewed feature")
        self.assertEqual(self.plan()[0], {WEB_GUIDE: [WEB]})

    def test_a_whitespace_only_guide_edit_is_unchanged_on_every_git(self):
        # git 2.43 prints no numstat record for a whitespace-only edit under -w;
        # a git that prints one with zero counts must read the same way.
        changed = IMPACT["content_changed"]
        self.assertEqual(changed(""), set())
        self.assertEqual(changed("0\t0\tdocs/a.md\0"), set())
        records = ("0\t0\tdocs/a.md", "2\t1\tdocs/b c.md", "-\t-\tdocs/d.md", "")
        self.assertEqual(changed("\0".join(records)), {"docs/b c.md", "docs/d.md"})

    def test_config_only_and_no_production_diff(self):
        self.assertFalse(self.plan()[0])
        self.write("tests/local.rs", "// only a fixture\n")
        self.commit("test-only")
        self.assertFalse(self.plan()[0])
        self.write(IMPACT["CONFIG"], "web:\n  enabled: false\nstructured_memory:\n  enabled: true\n")
        self.commit("change structured memory default")
        self.assertEqual(set(self.plan()[0]), {"docs/MEMORY_GUIDE.md", "docs/STRUCTURED_MEMORY.md"})
        self.assertEqual(IMPACT["guides_for"]("src/common/tool_broker.rs"), {"CONSOLE", "MCP"})

    def test_hosted_repair_updates_only_docs_at_the_expected_head(self):
        self.write(COMPUTER, "const ENABLED: bool = true;\n")
        self.write(WEB, "const ENABLED: bool = true;\n")
        head = self.commit("candidate")
        self.git("update-ref", "refs/pull/7/head", head)
        self.git("remote", "add", "origin", ".")
        self.git("checkout", "-q", "--detach", self.base)
        repo = {"full_name": REPO}
        pr = {"number": 7, "state": "open", "author_association": "OWNER",
              "base": {"ref": "main", "repo": repo}, "head": {"ref": "codex/fixture", "sha": head, "repo": repo}}
        event = {"pull_request": pr, "repository": {"default_branch": "main"}}
        receipts = [
            {"guide": BROWSER_GUIDE, "decision": "edit", "reason": "The feature is now enabled.",
             "edits": [{"old": "The feature is disabled.", "new": "The feature is enabled."}]},
            {"guide": WEB_GUIDE, "decision": "unchanged", "reason": "The public behavior stayed the same.", "edits": []},
        ]
        written = {"data": {"createCommitOnBranch": {"commit": {"oid": "0" * 40}}}}

        with (patch.dict(os.environ, {"DOCS_SYNC_OPENAI_API_KEY": "synthetic-model-key",
                                     "DOCS_SYNC_GITHUB_TOKEN": "synthetic-writer"}),
              patch.dict(REPAIR["repair"].__globals__) as scope):
            scope["proposal"] = Mock(return_value=receipts)
            scope["gh_api"] = Mock(side_effect=[pr, pr, written])
            REPAIR["repair"](event, REPO)
            model_args = scope["proposal"].call_args.args
            self.assertEqual(set(model_args[0]), {BROWSER_GUIDE, WEB_GUIDE})
            self.assertIn("+const ENABLED: bool = true;", model_args[1])
            self.assertEqual(scope["gh_api"].call_count, 3)
            mutation = scope["gh_api"].call_args.kwargs
            payload = json.loads(mutation["data"])["variables"]["input"]
            self.assertEqual(payload["expectedHeadOid"], head)
            self.assertEqual(payload["branch"], {"branchName": "codex/fixture", "repositoryNameWithOwner": REPO})
            self.assertIn(f"Docs-Impact: {WEB_GUIDE} ", payload["message"]["body"])
            additions = payload["fileChanges"]["additions"]
            self.assertEqual([file["path"] for file in additions], [BROWSER_GUIDE])
            self.assertIn(b"The feature is enabled.", base64.b64decode(additions[0]["contents"]))
            self.assertEqual(self.git("rev-parse", "HEAD"), self.base)
            self.assertFalse(self.git("status", "--porcelain"))

            self.git("checkout", "-q", "--detach", head)
            for file in additions:
                self.write(file["path"], base64.b64decode(file["contents"]).decode())
            repaired = self.commit(payload["message"]["headline"] + "\n\n" + payload["message"]["body"])
            self.assertFalse(self.plan(repaired)[0])
            self.git("checkout", "-q", "--detach", self.base)

            os.environ.update(DOCS_SYNC_OPENAI_API_KEY="synthetic-model-key", DOCS_SYNC_GITHUB_TOKEN="synthetic-writer")
            moved = {**pr, "head": {**pr["head"], "sha": "1" * 40}}
            scope["gh_api"] = Mock(side_effect=[pr, moved])
            REPAIR["repair"](event, REPO)
            self.assertEqual(scope["gh_api"].call_count, 2)

            fork = {**pr, "head": {**pr["head"], "repo": {"full_name": "someone/else"}}}
            scope["gh_api"].reset_mock()
            REPAIR["repair"]({**event, "pull_request": fork}, REPO)
            scope["gh_api"].assert_not_called()

    def test_regeneration_reads_the_candidate_inputs_without_running_candidate_code(self):
        repository = SCRIPTS.parent
        inputs = runpy.run_path(str(SCRIPTS / "build-self-manual.py"))["INPUTS"]
        for name in [*inputs, REPAIR["MANUAL"]]:
            self.write(name, (repository / name).read_text())
        candidate = self.commit("manual input snapshot")
        name = "docs/CONSOLE.md"
        before = Path(name).read_text()
        anchor = "| Command family | Supported use |"
        self.assertIn(anchor, before)
        after = before.replace(anchor + "\n|---|---|", anchor + "\n|---|---|\n| Example slot | Manual fixture |")
        self.assertNotEqual(before, after)
        updates = {name: after}
        REPAIR["regenerate_manual"](candidate, updates)
        self.assertIn("Example slot: Manual fixture", updates[REPAIR["MANUAL"]])
        self.assertEqual(Path(name).read_text(), before)

    def test_repair_rejects_an_unowned_path_and_heading_changes(self):
        apply = REPAIR["apply_reviews"]
        guides = {BROWSER_GUIDE: "# Stable\n\nOld text.\n"}
        review = {"guide": "AGENTS.md", "decision": "edit", "reason": "This would be an unrelated edit.",
                  "edits": [{"old": "# Stable", "new": "# Drift"}]}
        with self.assertRaisesRegex(ValueError, "ineligible guide"):
            apply(guides, {}, [review], {BROWSER_GUIDE: 100})
        with self.assertRaisesRegex(ValueError, "keep headings"):
            apply(guides, {}, [{**review, "guide": BROWSER_GUIDE}], {BROWSER_GUIDE: 100})
        with self.assertRaisesRegex(ValueError, "did not cover"):
            apply(guides, {}, [], {BROWSER_GUIDE: 100})


if __name__ == "__main__":
    unittest.main()
