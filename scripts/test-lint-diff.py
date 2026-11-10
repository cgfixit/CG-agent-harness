#!/usr/bin/env python3
"""Fixtures for scripts/lint-diff.py: each rule against the mistake it was
written for, then one run in a disposable repository."""
import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "lint-diff.py")
spec = importlib.util.spec_from_file_location("lint_diff", SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

GUARD = """const FOLDER_README_WORDS: usize = 150;
const DOCS_BUDGET: &[(&str, Kind, usize)] = &[
    ("AGENTS.md", Agent, {agents}),
{extra}];
"""
BUILDER = """import argparse, os
p = argparse.ArgumentParser(); p.add_argument("--root"); p.add_argument("--out")
a = p.parse_args()
with open(os.path.join(a.root, "AGENTS.md")) as f, open(a.out, "w") as out:
    out.write("manual:" + f.read())
"""


class RuleTests(unittest.TestCase):
    def test_insecure_url_follows_the_devskim_rule(self):
        flagged = [
            '"http://example.org/docs/a",',  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
            'let url = format!("http://passages.invalid:{}/manual", address.port());',  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
            'let base = format!("http://{address}/v1"); // a plain comment',  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
            '"http://a.example", // DevSkim: ignore DS162092 because another rule',  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
        ]
        clean = [
            '"http://example.org/docs/a", // DevSkim: ignore DS137138 because the downgrade is refused.',
            '"http://a.example", // DevSkim: ignore DS162092,DS137138 because both apply',
            'let u = "http://localhost:1/x";',
            'let u = "http://127.0.0.1:1/x";',
            'raw.strip_prefix("http://")',
            "// see http://example.com/docs",  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
            'fetch("https://a.example") // formerly http://b.example',  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
        ]
        for line in flagged:
            self.assertTrue(module.insecure_urls(line), line)
        for line in clean:
            self.assertEqual(module.insecure_urls(line), [], line)

    def test_an_annotation_on_the_line_above_is_not_accepted(self):
        above = "// DevSkim: ignore DS137138 because this host resolves to the fixture."
        self.assertEqual(module.insecure_urls(above), [])
        self.assertTrue(module.insecure_urls('let url = format!("http://passages.invalid:{}/", port);'))  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.

    def test_python_lines_use_hash_comments_and_both_quotes(self):
        note = "# DevSkim: " + "ignore DS137138 because fixture."
        url = "http" + "://site.fixture/a"
        python = {"marker": "#", "quotes": "\"'"}
        self.assertTrue(module.insecure_urls(f"URL = '{url}'", **python))
        self.assertTrue(module.insecure_urls(f'URL = "{url}"  # a plain comment', **python))
        self.assertTrue(module.insecure_urls(f"docstring text {url} continues", **python))
        self.assertTrue(module.insecure_urls(f"URL = '# {url}'", **python))
        self.assertEqual(module.insecure_urls(f"x = 1  # see {url}", **python), [])
        self.assertEqual(module.insecure_urls(f"URL = '{url}'  {note}", **python), [])
        self.assertEqual(module.language("scripts/a.py"), ("#", "\"'"))
        self.assertIsNone(module.language("docs/a.md"))

    def test_budget_rows_stop_at_the_table_end(self):
        text = GUARD.format(agents="1_460", extra='    ("docs/A.md", Guide, 90),\n') + '("late.md", Guide, 1),\n'
        self.assertEqual(module.budget_rows(text), {"AGENTS.md": (1460, 2), "docs/A.md": (90, 3)})

    def test_timing_bound_pattern(self):
        hit = module.ELAPSED_BOUND.search("assert!(start.elapsed() < std::time::Duration::from_millis(500));")
        self.assertEqual((hit.group(1), hit.group(2)), ("millis", "500"))
        self.assertIsNone(module.ELAPSED_BOUND.search("let waited = start.elapsed();"))


class RepositoryTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        self.git("init", "-q", "-b", "main")
        self.write("tests/invariant_guard.rs", GUARD.format(agents=5, extra=""))
        self.write("scripts/build-self-manual.py", BUILDER)
        self.write("AGENTS.md", "one two three\n")
        self.write("assets/self/MANUAL.md", "manual:one two three\n")
        self.write("src/lib.rs", "fn main() {}\n")
        self.commit("base")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")

    def tearDown(self):
        self.tmp.cleanup()

    def git(self, *args):
        env = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull)
        subprocess.run(["git", "-C", self.root, *args], check=True, capture_output=True, env=env)

    def write(self, path, text):
        full = os.path.join(self.root, path)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "w", encoding="utf-8") as handle:
            handle.write(text)

    def commit(self, message):
        self.git("add", "-A")
        self.git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false",
                 "-c", "core.hooksPath=" + os.devnull, "commit", "-q", "-m", message)

    def lint(self, *args):
        env = {k: v for k, v in os.environ.items() if k != "GITHUB_ACTIONS"}
        done = subprocess.run([sys.executable, SCRIPT, *args], cwd=self.root, capture_output=True, text=True, env=env, check=False)
        return done.returncode, done.stdout + done.stderr

    def test_clean_branch_passes(self):
        self.write("src/lib.rs", 'fn main() { let _ = "https://example.org"; }\n')
        self.assertEqual(self.lint()[0], 0)

    def test_each_mistake_is_reported_once(self):
        self.write("src/lib.rs", "\n".join([
            'fn main() {',
            '    let _ = "http://site.fixture/a";',  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
            '    let _ = "http://site.fixture/b"; // DevSkim: ignore DS137138 because fixture.',
            '    assert!(start.elapsed() < Duration::from_secs(1));',
            '    while start.elapsed() < Duration::from_secs(1) {}',
            '}', '']))
        self.write("AGENTS.md", "one two three four five six\n")
        self.write("docs/README.md", "word " * 151)
        self.write("scripts/new.py", "URL = 'http" + "://site.fixture/c'\n")
        code, out = self.lint()
        self.assertEqual(code, 1, out)
        self.assertIn("src/lib.rs:2: error CG001", out)
        self.assertIn("scripts/new.py:1: error CG001", out)
        self.assertIn("needs `# DevSkim: ignore", out)
        self.assertEqual(out.count("CG001"), 2, out)
        self.assertIn("assets/self/MANUAL.md: error CG002", out)
        self.assertIn("AGENTS.md: error CG003: 6 words, cap 5", out)
        self.assertIn("docs/README.md: error CG003: 151 words, cap 150", out)
        self.assertIn("src/lib.rs:4: warning CG005", out)
        self.assertEqual(out.count("CG005"), 1, out)
        self.assertIn("5 error(s), 1 warning(s)", out)

    def test_a_raised_cap_needs_a_new_why_comment(self):
        self.write("tests/invariant_guard.rs", GUARD.format(agents=9, extra=""))
        code, out = self.lint()
        self.assertEqual(code, 1, out)
        self.assertIn("error CG004: `AGENTS.md` cap raised from 5 to 9", out)
        self.write("tests/invariant_guard.rs", GUARD.format(agents=9, extra="").replace(
            '    ("AGENTS.md"', '    // why: operator-approved.\n    ("AGENTS.md"'))
        code, out = self.lint()
        self.assertEqual(code, 0, out)
        self.assertIn("warning CG004", out)
        self.write("tests/invariant_guard.rs", GUARD.format(agents=4, extra='    ("docs/NEW.md", Guide, 10),\n'))
        code, out = self.lint()
        self.assertIn("error CG004: `docs/NEW.md` cap new row at 10", out)
        self.assertNotIn("`AGENTS.md` cap", out)

    def test_staged_scope_ignores_unstaged_edits(self):
        self.write("src/lib.rs", 'fn main() { let _ = "http://site.fixture/a"; }\n')  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
        self.assertEqual(self.lint("--staged")[0], 0)
        self.git("add", "-A")
        code, out = self.lint("--staged")
        self.assertEqual(code, 1, out)
        self.assertIn("CG001", out)

    def test_a_missing_base_is_a_usage_error(self):
        self.assertEqual(self.lint("--base", "origin/absent")[0], 2)


if __name__ == "__main__":
    unittest.main()
