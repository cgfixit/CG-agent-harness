#!/usr/bin/env python3
"""Behaviour tests for trim_report.py, run by script-lint.yml.

Each case builds a throwaway git repository with an `origin/main` ref, edits
README.md or docs/*.md on a work branch, runs the helper from inside it and
checks the exit code and verdict. Stdlib only; no network.
"""
import os
import subprocess
import sys
import tempfile
import unittest

HELPER = os.path.join(os.path.dirname(os.path.abspath(__file__)), "trim_report.py")
WORDS = " ".join(f"w{i}" for i in range(100))


def git(repo, *args):
    subprocess.run(["git", "-C", repo, "-c", "user.name=t", "-c", "user.email=t@example.invalid",
                    "-c", "commit.gpgsign=false", *args], check=True, capture_output=True)


class TrimReport(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = self.tmp.name
        os.makedirs(os.path.join(self.repo, "docs"))
        self.write("README.md", WORDS + "\n\nSee [guide](docs/GUIDE.md).\n")
        self.write("docs/GUIDE.md", WORDS + "\n")
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "--no-verify", "-m", "base")
        git(self.repo, "update-ref", "refs/remotes/origin/main", "HEAD")
        git(self.repo, "checkout", "-q", "-b", "work")

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, rel, text):
        with open(os.path.join(self.repo, rel), "w", encoding="utf-8") as handle:
            handle.write(text)

    def commit(self):
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "--no-verify", "-m", "work")

    def run_helper(self, *args):
        done = subprocess.run([sys.executable, "-I", HELPER, *args], cwd=self.repo,
                              capture_output=True, text=True, check=False)
        return done.returncode, done.stdout + done.stderr

    def trimmed(self, keep):
        return " ".join(f"w{i}" for i in range(keep)) + "\n"

    def test_in_band_trim_passes(self):
        self.write("docs/GUIDE.md", self.trimmed(90))
        self.commit()
        code, out = self.run_helper()
        self.assertEqual(code, 0, out)
        self.assertIn("10.0%  ok", out)

    def test_out_of_band_trim_fails(self):
        self.write("docs/GUIDE.md", self.trimmed(50))
        self.commit()
        code, out = self.run_helper()
        self.assertEqual(code, 1, out)
        self.assertIn("OUT OF BAND", out)

    def test_growth_fails_even_with_min_zero(self):
        self.write("docs/GUIDE.md", WORDS + " extra words\n")
        self.commit()
        code, out = self.run_helper("--min", "0")
        self.assertEqual(code, 1, out)

    def test_broken_relative_link_fails_but_fenced_and_external_links_do_not(self):
        self.write("README.md", self.trimmed(90) + "[gone](docs/GONE.md#x) [web](https://example.invalid/a) [here](#top)\n"
                   "```\n[fenced](docs/NOPE.md)\n```\n")
        self.commit()
        code, out = self.run_helper()
        self.assertEqual(code, 1, out)
        self.assertIn("docs/GONE.md#x does not resolve", out)
        self.assertNotIn("NOPE.md", out)
        self.assertNotIn("example.invalid", out)

    def test_new_doc_is_refused(self):
        self.write("docs/NEW.md", "new\n")
        self.commit()
        code, out = self.run_helper("--min", "0")
        self.assertEqual(code, 1, out)
        self.assertIn("new file", out)

    def test_unknown_base_is_an_environment_error(self):
        code, out = self.run_helper("--base", "no-such-ref")
        self.assertEqual(code, 2, out)

    def test_no_changed_docs_passes(self):
        code, out = self.run_helper()
        self.assertEqual(code, 0, out)
        self.assertIn("no README.md or docs/*.md changed", out)


if __name__ == "__main__":
    unittest.main()
