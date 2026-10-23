"""Exercise the tracked hooks with synthetic indexes and commit histories."""
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BASH = os.environ.get("BASH", "/bin/bash")
GIT = shutil.which("git") or "git"
ZERO = "0" * 40


class GitHooksTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="githooks-test-")
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name) / "repo"
        self.repo.mkdir()
        home = Path(self.temp.name) / "home"
        home.mkdir()
        self.env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("GIT_")
            and key not in ("HOOK_OPERATOR_ACK", "ALLOW_MAIN_PUSH", "ALLOW_FORCE_WITH_LEASE")
        }
        self.env.update(
            HOME=str(home),
            XDG_CONFIG_HOME=str(home / "config"),
            GIT_CONFIG_NOSYSTEM="1",
            GIT_CONFIG_GLOBAL=os.devnull,
            GIT_AUTHOR_NAME="Hook Fixture",
            GIT_COMMITTER_NAME="Hook Fixture",
            GIT_AUTHOR_EMAIL="hook@example.invalid",
            GIT_COMMITTER_EMAIL="hook@example.invalid",
            # Exercise the always-on fallback independently of optional scanners.
            PATH="/usr/bin:/bin:/usr/sbin:/sbin",
        )
        self.git("init", "-q", "-b", "feature")
        (self.repo / ".githooks").mkdir()
        for name in ("_security.sh", "security.conf", "pre-commit", "pre-push"):
            shutil.copy2(ROOT / ".githooks" / name, self.repo / ".githooks" / name)
        self.base = self.commit({})
        self.git("update-ref", "HEAD", self.base)

    def run_command(self, *args, input=None, check=True):
        return subprocess.run(
            args, cwd=self.repo, env=self.env, input=input, text=True,
            capture_output=True, check=check,
        )

    def git(self, *args, input=None):
        return self.run_command(GIT, *args, input=input).stdout.strip()

    def stage(self, files):
        for name, content in files.items():
            path = self.repo / name
            if content is None:
                path.unlink()
            else:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content, encoding="utf-8")
            self.git("add", "-f", "-A", "--", name)

    def commit(self, files, parent=None):
        # commit-tree builds deliberately unverified history for pre-push checks.
        self.git("read-tree", parent if parent else "--empty")
        self.stage(files)
        tree = self.git("write-tree")
        parents = ["-p", parent] if parent else []
        return self.git("commit-tree", tree, *parents, input="Synthetic fixture\n")

    def hook(self, name, input=None):
        return self.run_command(
            BASH, str(self.repo / ".githooks" / name), "origin",
            input=input, check=False,
        )

    def push(self, head, remote=None, ref="refs/heads/feature"):
        return self.hook("pre-push", f"refs/heads/feature {head} {ref} {remote or ZERO}\n")

    def assert_refused(self, result, reason):
        self.assertNotEqual(result.returncode, 0, result.stderr)
        self.assertIn(reason, result.stderr)

    def test_plain_blocked_filename(self):
        self.stage({"plain.pem": "synthetic nonsecret\n"})
        self.assert_refused(self.hook("pre-commit"), "credential or runtime-state file")

    def test_quoted_and_colon_filenames_at_commit(self):
        for name in ['double"quote.pem', "slash\\name.pem", "tab\tname.pem",
                     "line\nname.pem", "colon:name.txt"]:
            with self.subTest(name=name):
                self.git("read-tree", self.base)
                self.stage({name: "synthetic nonsecret\n"})
                self.assert_refused(self.hook("pre-commit"), "filename")

    def test_quoted_and_colon_filenames_at_push(self):
        for name in ['double"quote.pem', "slash\\name.pem", "tab\tname.pem",
                     "line\nname.pem", "colon:name.txt"]:
            with self.subTest(name=name):
                head = self.commit({name: "synthetic nonsecret\n"}, self.base)
                self.assert_refused(self.push(head, self.base), "filename")

    def test_inline_allow_marker_in_filename_cannot_hide_secret(self):
        for name in ["gitleaks:allow.txt", "gitleaks:allow/note.txt"]:
            with self.subTest(name=name):
                head = self.commit({name: "hf_" + "A" * 34 + "\n"}, self.base)
                self.assert_refused(self.hook("pre-commit"), "filename")
                self.assert_refused(self.push(head, self.base), "filename")

    def test_quoted_protected_deletion(self):
        name = '.githooks/policy"fixture.sh'
        head = self.commit({name: "# synthetic policy\n"}, self.base)
        self.git("update-ref", "HEAD", head)
        self.stage({name: None})
        self.assert_refused(self.hook("pre-commit"), "filename")

    def test_quoted_protected_rename(self):
        name = '.githooks/policy"fixture.sh'
        head = self.commit({name: "# synthetic policy\n"}, self.base)
        self.git("update-ref", "HEAD", head)
        self.stage({name: None, "renamed.txt": "# synthetic policy\n"})
        self.assert_refused(self.hook("pre-commit"), "filename")

    def test_spaces_unicode_and_inline_fixture_marker_still_pass(self):
        files = {
            "ordinary space.txt": "ordinary text\n",
            "café.txt": "ordinary text\n",
            "fixture.txt": "hf_" + "A" * 34 + " # gitleaks:allow\n",
        }
        head = self.commit(files, self.base)
        self.assertEqual(self.hook("pre-commit").returncode, 0)
        self.assertEqual(self.push(head, self.base).returncode, 0)

    def test_secret_removed_at_tip_is_still_refused(self):
        first = self.commit({"secret.txt": "hf_" + "A" * 34 + "\n"}, self.base)
        tip = self.commit({"secret.txt": None}, first)
        self.assert_refused(self.push(tip, self.base), "credential-shaped string")

    def test_protected_branch_push_and_delete(self):
        head = self.commit({"ordinary.txt": "ordinary text\n"}, self.base)
        self.assert_refused(self.push(head, self.base, "refs/heads/main"), "protected branch")
        self.assert_refused(self.push(ZERO, head, "refs/heads/main"), "protected branch")
        self.assertEqual(self.push(ZERO, head).returncode, 0)

    def test_fast_forward_passes_and_rewrite_is_refused(self):
        left = self.commit({"left.txt": "left\n"}, self.base)
        right = self.commit({"right.txt": "right\n"}, self.base)
        self.assertEqual(self.push(left, self.base).returncode, 0)
        self.assert_refused(self.push(right, left), "ref rewrite")

    def test_existing_tag_cannot_move_to_descendant(self):
        head = self.commit({"ordinary.txt": "ordinary text\n"}, self.base)
        self.git("tag", "-a", "old", self.base, "-m", "Synthetic old tag")
        self.git("tag", "-a", "new", head, "-m", "Synthetic new tag")
        self.assert_refused(
            self.push(self.git("rev-parse", "new"), self.git("rev-parse", "old"),
                      "refs/tags/release"), "ref rewrite",
        )


if __name__ == "__main__":
    unittest.main()
