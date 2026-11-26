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
WRAPPED = """    (
        "docs/long/SKILL.md",
        Agent,
        {cap},
    ),
"""
BUILDER = """import argparse, os
INPUTS = [
    "AGENTS.md",
]
p = argparse.ArgumentParser(); p.add_argument("--root"); p.add_argument("--out")
a = p.parse_args()
with open(os.path.join(a.root, "AGENTS.md")) as f, open(a.out, "w") as out:
    out.write("manual:" + f.read())
"""


class RuleTests(unittest.TestCase):
    def test_insecure_url_follows_the_devskim_rule(self):
        # Split so this file's own lines carry one suppression each: DevSkim reads only the first.
        other_rule = "// DevSkim: " + "ignore DS162092 because another rule"
        flagged = [
            '"http://example.org/docs/a",',  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
            'let url = format!("http://passages.invalid:{}/manual", address.port());',  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
            'let base = format!("http://{address}/v1"); // a plain comment',  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
            '"http://a.example", ' + other_rule,  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
            '"http://a.example", ' + other_rule + " // DevSkim: " + "ignore DS137138 too late",  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
            '"http://a.example", // DevSkim: ' + "ignore DS162092, DS137138 because the space ends the list",  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
        ]
        clean = [
            '"http://example.org/docs/a", // DevSkim: ignore DS137138 because the downgrade is refused.',
            '"http://a.example", // DevSkim: ignore DS162092,DS137138 because both apply',
            'let u = "http://localhost:1/x";',
            'let u = "http://127.0.0.1:1/x";',
            'raw.strip_prefix("http://")',
            '"http://a.example", // devskim: ' + "ignore all because case and `all` both count",  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
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

    def test_an_added_line_that_looks_like_a_file_header_is_content(self):
        diff = "\n".join([
            "diff --git a/docs/a.md b/docs/a.md",
            "--- a/docs/a.md",
            "+++ b/docs/a.md",
            "@@ -2,0 +3,4 @@",
            "+plain",
            "+++ looks like a header",
            "+++x looks like one too",
            "+after",
            "@@ -9 +11 @@",
            "-old",
            "+one line, no count",
            "\\ No newline at end of file",
            "diff --git a/src/b.rs b/src/b.rs",
            "--- a/src/b.rs",
            "+++ b/src/b.rs",
            "@@ -0,0 +1,2 @@",
            "+fn b() {}",
            "+",
        ])
        self.assertEqual(module.parse_added(diff), {
            "docs/a.md": {3: "plain", 4: "++ looks like a header", 5: "++x looks like one too", 6: "after",
                          11: "one line, no count"},
            "src/b.rs": {1: "fn b() {}", 2: ""},
        })

    def test_budget_rows_stop_at_the_table_end(self):
        text = GUARD.format(agents="1_460", extra='    ("docs/A.md", Guide, 90),\n') + '("late.md", Guide, 1),\n'
        self.assertEqual(module.budget_rows(text), {"AGENTS.md": (1460, 2), "docs/A.md": (90, 3)})

    def test_budget_rows_read_the_multiline_form_rustfmt_writes(self):
        extra = '    // ("docs/commented.md", Guide, 1),\n' + WRAPPED.format(cap=500) + '    ("docs/A.md", Guide, 90),\n'
        rows, unparsed = module.budget_table(GUARD.format(agents=5, extra=extra))
        self.assertEqual(rows, {"AGENTS.md": (5, 2), "docs/long/SKILL.md": (500, 4), "docs/A.md": (90, 9)})
        self.assertEqual(unparsed, 0)

    def test_an_unreadable_row_is_counted_not_skipped(self):
        rows, unparsed = module.budget_table(GUARD.format(agents=5, extra='    ("docs/A.md", Guide, 90 + 1),\n'))
        self.assertEqual((sorted(rows), unparsed), (["AGENTS.md"], 1))

    def test_every_row_of_the_real_table_is_read(self):
        guard = os.path.join(os.path.dirname(SCRIPT), os.pardir, "tests", "invariant_guard.rs")
        with open(guard, encoding="utf-8") as handle:
            rows, unparsed = module.budget_table(handle.read())
        self.assertEqual(unparsed, 0)
        self.assertGreater(len(rows), 50)
        self.assertIn(".claude/skills/cgagentharness-write-policy-redteam/SKILL.md", rows)

    def test_weak_hash_names_follow_the_alerts(self):
        # Built at run time so this file's own lines carry no hash name.
        weak, md = "SHA" + "-1", "md" + "5"
        hashed = "# DevSkim: ignore DS" + "126858 because label"
        shell = {"marker": "#", "quotes": "\"'"}
        self.assertEqual(module.weak_hashes(f"awk '/^{weak} hash:/ {{print $3}}'", **shell), [weak])
        self.assertEqual(module.weak_hashes(f'h = hashlib.{md}(b"x")', **shell), [md])
        self.assertEqual(module.weak_hashes(f"let h = {weak.replace('-', '')}::new();"), [weak.replace("-", "")])
        for line, syntax in [
            (f"awk '/^{weak} hash:/' {hashed}", shell),
            ("use sha2::Sha256;", {}),
            (f"let x = 1; // {weak} only in a comment", {}),
            (f'h = "{md}sum-free text"', shell),
        ]:
            self.assertEqual(module.weak_hashes(line, **syntax), [], line)

    def test_password_literals_follow_the_codeql_alert(self):
        # The #1093 shape: a literal bound to a password name, then hashed.
        for line in [
            '        let password = "migrated-reader-password";',
            '    let mut old_password: &str = "x";',
            '    let hash = authn::hash_password("preserved-admin-password").unwrap();',
            '    authn::verify_password("literal", &record);',
        ]:
            self.assertTrue(module.password_literals(line), line)
        for line in [
            "        let password = crate::common::random_urlsafe(24);",
            '    let hash = authn::hash_password(&password).unwrap();',
            '    // let password = "only in a comment";',
            '    let password_hash: String = row.get(1)?;',
            '    assert!(body.contains("password"));',
        ]:
            self.assertEqual(module.password_literals(line), [], line)

    def test_assert_messages_follow_the_cleartext_logging_alerts(self):
        # bee4525: as_account sends the session cookie, and every body it returned was printed.
        source = """async fn as_account(s: &S, who: &Who, m: Method) -> (u16, Value) {
    let req = s.req(m).header("cookie", who.cookie());
}
async fn plain(s: &S) -> (u16, Value) { todo!() }
let (status, first) = as_account(&s, &admin, Method::GET).await;
assert_eq!(status, 200, "{first}");
assert_eq!(
    status, 403,
    "operator {method} {path}: {first}"
);
assert_eq!(status, 200, "saw {}", first);
let (status, other) = plain(&s).await;
assert_eq!(status, 200, "{other}");
assert_eq!(status, 200, "the first admin save");
let line = format!("{first}");
assert!(token.len() > 8, "{token}");
assert_eq!(status, 200, "{first}"); // lint-diff: allow CG011 until 2099-01-01 approved by the operator: fixture
assert_eq!(status, 200, "{first}"); // lint-diff: allow CG011 until 2001-01-01 approved by the operator: expired
"""
        lines = source.splitlines()
        bound = module.credentialed_bodies(source)
        self.assertEqual(bound, {"first"})
        today = module.datetime.date(2026, 11, 22)

        def hits(text):
            return module.message_leaks(lines, lines.index(text), bound, today)

        self.assertEqual(hits('assert_eq!(status, 200, "{first}");'), ["first"])
        self.assertEqual(hits('    "operator {method} {path}: {first}"'), ["first"])
        self.assertEqual(hits('assert_eq!(status, 200, "saw {}", first);'), ["first"])
        self.assertEqual(hits('assert_eq!(status, 200, "{other}");'), [])
        self.assertEqual(hits('assert_eq!(status, 200, "the first admin save");'), [])
        self.assertEqual(hits('let line = format!("{first}");'), [])
        self.assertEqual(hits('assert!(token.len() > 8, "{token}");'), ["token"])
        allowed = 'assert_eq!(status, 200, "{first}"); // lint-diff: allow CG011 until 2099-01-01 approved by the operator: fixture'
        self.assertEqual(module.message_leaks([allowed], 0, bound, today), [])
        expired = allowed.replace("2099-01-01", "2001-01-01")
        self.assertEqual(module.message_leaks([expired], 0, bound, today), ["first"])
        unapproved = 'assert_eq!(status, 200, "{first}"); // lint-diff: allow CG011 until 2099-01-01'
        self.assertEqual(module.message_leaks([unapproved], 0, bound, today), ["first"])

    def test_reference_needle_names_shared_basenames_by_folder(self):
        tracked = ["a/SKILL.md", "b/SKILL.md", "docs/GUIDE.md"]
        self.assertEqual(module.reference_needle("a/SKILL.md", tracked), "a/")
        self.assertEqual(module.reference_needle("docs/x/SKILL.md", tracked), "x/")
        self.assertEqual(module.reference_needle("docs/OLD.md", tracked), "OLD.md")
        self.assertEqual(module.reference_needle("docs/x/README.md", tracked), "docs/x/")
        self.assertEqual(module.reference_needle("README.md", tracked), "")
        self.assertEqual(module.reference_needle("SKILL.md", tracked), "")

    def test_owner_exempt_paths_are_read_from_the_guard(self):
        guard = 'const DOCS_BUDGET_OWNER_EXEMPT: &[(&str, &str)] = &[\n    ("docs/a.md", "why"),\n    // ("docs/b.md", "old"),\n];\n'
        self.assertEqual(module.owner_exempt_paths(guard), {"docs/a.md"})
        self.assertEqual(module.owner_exempt_paths("const DOCS_BUDGET_OWNER_EXEMPT: &[(&str, &str)] = &[];\n"), set())

    def test_the_real_exempt_table_is_found(self):
        guard = os.path.join(os.path.dirname(SCRIPT), os.pardir, "tests", "invariant_guard.rs")
        with open(guard, encoding="utf-8") as handle:
            self.assertIsNotNone(module.EXEMPT_TABLE.search(handle.read()))

    def test_self_manual_inputs_are_read_from_the_builder(self):
        self.assertEqual(module.self_manual_inputs(BUILDER), {"AGENTS.md"})
        self.assertEqual(module.self_manual_inputs("no inputs here"), set())

    def test_timing_bound_pattern(self):
        hit = module.ELAPSED_BOUND.search("assert!(start.elapsed() < std::time::Duration::from_millis(500));")
        self.assertEqual((hit.group(1), hit.group(2)), ("millis", "500"))
        self.assertIsNone(module.ELAPSED_BOUND.search("let waited = start.elapsed();"))


    def test_shipped_values_read_block_scalars_only(self):
        text = "\n".join([
            "# top comment",
            "web:",
            "  enabled: true # flag_is_true",
            "  name: 'a # not a comment'",
            "  limit: 30",
            "  ratio: 0.5",
            "  endpoint: null",
            "  hosts: []",
            "  search:",
            "    fallback: false",
            "  patterns:",
            "    - 'x'",
            "browser:",
            "  enabled: false",
            "",
        ])
        self.assertEqual(module.shipped_values(text), {
            "web.enabled": True, "web.name": "a # not a comment", "web.limit": 30, "web.ratio": 0.5,
            "web.endpoint": None, "web.search.fallback": False, "browser.enabled": False,
        })

    @unittest.skipUnless(importlib.util.find_spec("yaml"), "PyYAML is not installed")
    def test_shipped_values_agree_with_a_yaml_parser_on_the_real_config(self):
        import yaml

        config = os.path.join(os.path.dirname(SCRIPT), os.pardir, "assets", "config.default.yaml")
        with open(config, encoding="utf-8") as handle:
            text = handle.read()

        def flat(node, prefix=""):
            for key, value in node.items():
                path = f"{prefix}.{key}" if prefix else str(key)
                if isinstance(value, dict):
                    yield from flat(value, path)
                elif not isinstance(value, list):
                    yield path, value

        self.assertEqual(module.shipped_values(text), dict(flat(yaml.safe_load(text))))

    def test_shipped_claims_read_the_phrasings_the_docs_use(self):
        text = "\n".join([
            "`browser.enabled`, ships true; administrators only.",
            "",
            "Reads use `web.firecrawl.enabled` (ships off; opt in).",
            "",
            "`diagnostics.logs_enabled` ships as the literal boolean true for admins.",
            "",
            "`web.goal_web_default` (ships `all`) picks the mode.",
            "",
            "`agentic.gh_timeout_sec` (ships 30) bounds each call.",
            "",
            "Check `notifications.enabled` and `agentic.deepagent_github.retrieval.enabled`",
            "remain false in shipped YAML.",
            "",
            "Writes ship closed: `agentic.enabled` stays shut.",
            "",
            "`netconnect.enabled` and every tier flag ship false.",
            "",
        ])
        self.assertEqual(list(module.shipped_claims(text)), [
            (1, "browser.enabled", "true"),
            (3, "web.firecrawl.enabled", "off"),
            (5, "diagnostics.logs_enabled", "true"),
            (7, "web.goal_web_default", "`all`"),
            (9, "agentic.gh_timeout_sec", "30"),
            # Wrapped across lines; only the key nearest the verb is claimed.
            (12, "agentic.deepagent_github.retrieval.enabled", "false"),
            (16, "netconnect.enabled", "false"),
        ])

    def test_claims_compare_only_what_the_config_confirms(self):
        values = {"web.search.result_reads.arxiv_pdf": True, "a.enabled": True, "b.enabled": False,
                  "web.carry_forward_sources": 0, "web.mode": "all", "x.endpoint": None}
        self.assertEqual(module.resolve_key("result_reads.arxiv_pdf", values), ("web.search.result_reads.arxiv_pdf", False))
        self.assertEqual(module.resolve_key("enabled", values), (None, False))
        self.assertEqual(module.resolve_key("a.enabled", values), ("a.enabled", True))
        self.assertIs(module.agrees(module.claimed("off"), False), True)
        self.assertIs(module.agrees(module.claimed("closed"), True), False)
        self.assertIs(module.agrees(module.claimed("`all`"), "all"), True)
        self.assertIs(module.agrees(module.claimed("30"), 30), True)
        self.assertIsNone(module.agrees(module.claimed("off"), 0))
        self.assertIsNone(module.agrees(module.claimed("true"), None))

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

    def test_a_multiline_row_is_checked_like_any_other(self):
        self.write("tests/invariant_guard.rs", GUARD.format(agents=5, extra=WRAPPED.format(cap=3)))
        self.write("docs/long/SKILL.md", "one two three four\n")
        code, out = self.lint()
        self.assertEqual(code, 1, out)
        self.assertIn("docs/long/SKILL.md: error CG003: 4 words, cap 3", out)
        self.assertIn("tests/invariant_guard.rs:4: error CG004: `docs/long/SKILL.md` cap new row at 3", out)

    def test_an_unreadable_table_fails_instead_of_passing(self):
        self.write("tests/invariant_guard.rs", GUARD.format(agents=5, extra='    ("docs/A.md", Guide, 90 + 1),\n'))
        code, out = self.lint()
        self.assertEqual(code, 1, out)
        self.assertIn("read 1 DOCS_BUDGET rows and could not read 1", out)

    def test_staged_scope_ignores_unstaged_edits(self):
        self.write("src/lib.rs", 'fn main() { let _ = "http://site.fixture/a"; }\n')  # DevSkim: ignore DS137138 because this is a lint fixture, never fetched.
        self.assertEqual(self.lint("--staged")[0], 0)
        self.git("add", "-A")
        code, out = self.lint("--staged")
        self.assertEqual(code, 1, out)
        self.assertIn("CG001", out)

    def test_staged_scope_reads_the_index_not_the_working_tree(self):
        # Staged: over its cap, with the manual to match. Working tree: restored. The commit is still wrong.
        self.write("AGENTS.md", "one two three four five six\n")
        self.write("assets/self/MANUAL.md", "manual:one two three four five six\n")
        self.git("add", "-A")
        self.write("AGENTS.md", "one two three\n")
        self.write("assets/self/MANUAL.md", "manual:one two three\n")
        code, out = self.lint("--staged")
        self.assertEqual(code, 1, out)
        self.assertIn("AGENTS.md: error CG003: 6 words, cap 5", out)
        self.assertNotIn("CG002", out)

    def test_staged_scope_is_not_failed_by_unstaged_edits(self):
        self.write("AGENTS.md", "one two four\n")
        self.write("assets/self/MANUAL.md", "manual:one two four\n")
        self.git("add", "-A")
        self.write("AGENTS.md", "one two three four five six\n")
        self.write("tests/invariant_guard.rs", "not a table\n")
        code, out = self.lint("--staged")
        self.assertEqual(code, 0, out)
        self.assertIn("0 error(s), 0 warning(s)", out)

    def test_staged_scope_sees_a_stale_staged_manual(self):
        self.write("AGENTS.md", "one two four\n")
        self.git("add", "-A")
        self.write("assets/self/MANUAL.md", "manual:one two four\n")
        code, out = self.lint("--staged")
        self.assertEqual(code, 1, out)
        self.assertIn("assets/self/MANUAL.md: error CG002", out)

    def test_a_weak_hash_name_in_a_shell_script_is_a_warning(self):
        self.write("scripts/new.sh", "printf '%s\\n' \"" + "SHA" + "-1 label\"\n")
        code, out = self.lint()
        self.assertEqual(code, 0, out)
        self.assertIn("scripts/new.sh:1: warning CG006", out)

    def test_a_deleted_doc_must_leave_the_exempt_list_budget_and_links(self):
        # The #881 shape: an owner-exempt path deleted while its entry stays.
        exempt = 'const DOCS_BUDGET_OWNER_EXEMPT: &[(&str, &str)] = &[\n    ("docs/OLD.md", "report"),\n];\n'
        self.write("docs/OLD.md", "old words\n")
        self.write("docs/README.md", "See [old](OLD.md).\n")
        self.write("tests/invariant_guard.rs", GUARD.format(agents=5, extra='    ("docs/OLD.md", Guide, 10),\n') + exempt)
        self.commit("add a doc")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        os.remove(os.path.join(self.root, "docs/OLD.md"))
        code, out = self.lint()
        self.assertEqual(code, 1, out)
        self.assertIn("tests/invariant_guard.rs: error CG008: `docs/OLD.md` was deleted; remove it from DOCS_BUDGET_OWNER_EXEMPT", out)
        self.assertIn("tests/invariant_guard.rs:4: warning CG008: `docs/OLD.md` was deleted; remove its DOCS_BUDGET row", out)
        self.assertIn("docs/README.md: warning CG008: still names `OLD.md`", out)
        # Dropping the exempt entry clears the error; the dead row and the link stay warnings.
        self.write("tests/invariant_guard.rs", GUARD.format(agents=5, extra='    ("docs/OLD.md", Guide, 10),\n'))
        code, out = self.lint()
        self.assertEqual(code, 0, out)
        self.assertEqual(out.count("warning CG008"), 2, out)
        self.write("tests/invariant_guard.rs", GUARD.format(agents=5, extra=""))
        self.write("docs/README.md", "Nothing old.\n")
        code, out = self.lint()
        self.assertNotIn("CG008", out)

    def test_a_deleted_skill_is_named_by_its_folder_not_its_shared_basename(self):
        # The #1164 shape: deleting one SKILL.md flagged every file naming any skill.
        self.write(".claude/skills/doc-sync/SKILL.md", "doc sync\n")
        self.write(".claude/skills/other/SKILL.md", "other\n")
        self.write("CLAUDE.md", "Each `.claude/skills/<slug>/SKILL.md` runs as a skill.\n")
        self.write(".claude/skills/README.md", "- [`doc-sync/`](doc-sync/SKILL.md): sync.\n")
        self.commit("add skills")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.git("rm", "-q", ".claude/skills/doc-sync/SKILL.md")
        code, out = self.lint("--staged")
        self.assertEqual(code, 0, out)
        self.assertIn(".claude/skills/README.md: warning CG008: still names `doc-sync/`", out)
        self.assertNotIn("CLAUDE.md: warning CG008", out)
        self.assertNotIn("other/SKILL.md: warning CG008", out)
        self.assertNotIn("`SKILL.md`", out)
        self.assertEqual(out.count("warning CG008"), 1, out)

    def test_a_deleted_self_manual_input_is_an_error(self):
        self.write("docs/IN.md", "in\n")
        self.write("scripts/build-self-manual.py", BUILDER.replace('"AGENTS.md",\n]', '"AGENTS.md",\n    "docs/IN.md",\n]'))
        self.commit("add an input")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.git("rm", "-q", "docs/IN.md")
        code, out = self.lint("--staged")
        self.assertEqual(code, 1, out)
        self.assertIn("scripts/build-self-manual.py: error CG008: `docs/IN.md` was deleted", out)

    def test_a_missing_base_is_a_usage_error(self):
        self.assertEqual(self.lint("--base", "origin/absent")[0], 2)

    def config(self, browser="false", retrieval="true"):
        self.write("assets/config.default.yaml", "\n".join([
            "browser:",
            f"  enabled: {browser}",
            "agentic:",
            "  deepagent_github:",
            "    retrieval:",
            f"      enabled: {retrieval}",
            "",
        ]))

    def test_a_flipped_shipped_value_flags_prose_that_still_states_the_old_one(self):
        # The c8ae0f0 shape: the config flips; docs elsewhere keep the old value.
        self.config()
        self.write("docs/DESKTOP.md", "The browser (`browser.enabled`, ships false; admins only).\n")
        self.write("docs/OTHER.md", "Retrieval (`agentic.deepagent_github.retrieval.enabled` ships true).\n")
        self.commit("config and docs")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.config(browser="true")
        code, out = self.lint()
        self.assertEqual(code, 1, out)
        self.assertIn("docs/DESKTOP.md:1: error CG009: says `browser.enabled` ships false", out)
        self.assertNotIn("OTHER.md", out)
        self.write("docs/DESKTOP.md", "The browser (`browser.enabled`, ships true; admins only).\n")
        self.assertEqual(self.lint()[0], 0)

    def test_an_added_line_must_state_the_shipped_value(self):
        self.config()
        self.commit("config")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        # The #960 shape: a skill says retrieval ships false; it ships true.
        self.write(".claude/skills/guard/SKILL.md", "Check `agentic.deepagent_github.retrieval.enabled`\nremains false in shipped YAML.\n")
        self.write("docs/SUFFIX.md", "Use `retrieval.enabled` (ships off).\n")
        code, out = self.lint()
        self.assertEqual(code, 1, out)
        self.assertIn(".claude/skills/guard/SKILL.md:2: error CG009", out)
        self.assertIn("docs/SUFFIX.md:1: warning CG009: says `retrieval.enabled` ships off", out)

    def test_an_untouched_wrong_line_is_not_this_diffs_finding(self):
        self.config()
        self.write("docs/OLD.md", "Line one.\n\n`browser.enabled` ships true.\n")
        self.commit("old prose")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.write("docs/OLD.md", "Line one, edited.\n\n`browser.enabled` ships true.\n")
        code, out = self.lint()
        self.assertNotIn("CG009", out)

    def test_staged_scope_reads_the_staged_config_and_docs(self):
        self.config()
        self.write("docs/DESKTOP.md", "`browser.enabled` ships false.\n")
        self.commit("base")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.config(browser="true")
        self.git("add", "-A")
        self.config()
        code, out = self.lint("--staged")
        self.assertEqual(code, 1, out)
        self.assertIn("docs/DESKTOP.md:1: error CG009", out)


if __name__ == "__main__":
    unittest.main()
