#!/usr/bin/env python3
"""K19: portable command doubles by default; --native-home PATH uses real macOS tools.

Native mode accepts only the marked, stopped standalone home handed off by
install-macos.yml. It uses real synthetic Keychain entries, with injected faults
for the preflight and retry checks.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


RESET = Path(__file__).with_name("reset-macos.sh").resolve()
OPEN_FILES = (".server.lock", "auth.sqlite3", "memory/structured.sqlite3")


def seed_home(home):
    home.mkdir(parents=True)
    for name in ("config.yaml", "sessions/keep.json", *OPEN_FILES):
        path = home / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("fixture\n")


def snapshot(root):
    return {str(p.relative_to(root)): (os.readlink(p) if p.is_symlink() else
            hashlib.sha256(p.read_bytes()).hexdigest() if p.is_file() else None)
            for p in root.rglob("*")}


def services(home):
    return [f"CGagentHarness ({home})", f"CGagentHarness admin ({home})"]


# PATH is closed: neither a missing double nor a refusal regression can reach
# the operator's Keychain or remove anything outside the selected fixture home.
DOUBLE = '''#!/usr/bin/env python3
import json, os, pathlib, shutil, sys
name, args = pathlib.Path(sys.argv[0]).name, sys.argv[1:]
state_path = pathlib.Path(os.environ["RESET_STATE"])
state = json.loads(state_path.read_text())
with open(os.environ["RESET_CALLS"], "a") as log:
    log.write(json.dumps([name, *args]) + "\\n")
if name == "uname":
    assert args == ["-s"]
    print(state.get("platform", "Darwin"))
elif name == "lsof":
    assert args[:2] == ["-t", "--"] and len(args) == 3
    code, output = state.get("open", {}).get(args[2], [1, ""])
    sys.stdout.write(output)
    sys.exit(code)
elif name == "security":
    assert len(args) == 3 and args[1] == "-s"
    action, service = args[0], args[2]
    assert action in ("find-generic-password", "delete-generic-password")
    code = state.get("errors", {}).get(action, {}).get(service, 0)
    if isinstance(code, list):
        code = code.pop(0) if code else 0
        state_path.write_text(json.dumps(state))
    if code:
        sys.exit(code)
    entries = state["entries"].get(service, [])
    if not entries:
        sys.exit(44)
    if action == "delete-generic-password" and not state.get("stuck"):
        entries.pop()
        state_path.write_text(json.dumps(state))
elif name == "rm":
    assert args == ["-rf", "--", state["removable"]], args
    shutil.rmtree(state["removable"])
else:
    raise AssertionError(name)
'''


class ResetContract(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix="cgah-reset-contract-")
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()
        self.user_home = self.root / "user"
        self.home = self.user_home / ".CGagentHarness"
        self.other = self.root / "other home"
        seed_home(self.home)
        seed_home(self.other)
        for name in (".ollama/models/model", ".cargo/bin/tool", "runtime/keep"):
            path = self.user_home / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("shared fixture\n")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        for name in ("tr", "wc", "dirname", "basename"):
            (self.bin / name).symlink_to(shutil.which(name))
        (self.bin / "python3").symlink_to(sys.executable)
        for name in ("uname", "lsof", "security", "rm"):
            path = self.bin / name
            path.write_text(DOUBLE)
            path.chmod(0o700)
        self.state_path = self.root / "state.json"
        self.calls_path = self.root / "calls.jsonl"
        self.state = {"removable": str(self.home), "entries": {
            **{s: ["key-a", "key-b"] for s in services(self.home)},
            **{s: ["unrelated-key"] for s in services(self.other)},
            "unrelated service": ["keep"],
        }}
        self.env = {"HOME": str(self.user_home), "PATH": str(self.bin),
                    "CGAGENTHARNESS_HOME": str(self.home),
                    "RESET_STATE": str(self.state_path), "RESET_CALLS": str(self.calls_path)}

    def run_reset(self, expected, message, *args, home=None):
        self.state_path.write_text(json.dumps(self.state))
        self.calls_path.write_text("")
        env = dict(self.env)
        if home is not None:
            env["CGAGENTHARNESS_HOME"] = str(home)
        before = snapshot(self.other)
        result = subprocess.run(["/bin/bash", str(RESET), *args], env=env,
                                cwd=self.root, capture_output=True, text=True, timeout=15)
        output = result.stdout + result.stderr
        self.output = output
        self.assertEqual(result.returncode, expected, output)
        self.assertIn(message, output)
        self.calls = [json.loads(line) for line in self.calls_path.read_text().splitlines()]
        after = json.loads(self.state_path.read_text())
        for service in (*services(self.other), "unrelated service"):
            self.assertEqual(after["entries"][service], self.state["entries"][service])
            self.assertFalse(any(c[0] == "security" and c[-1] == service for c in self.calls))
        self.assertEqual(snapshot(self.other), before)
        for name in (".ollama/models/model", ".cargo/bin/tool", "runtime/keep"):
            self.assertEqual((self.user_home / name).read_text(), "shared fixture\n")
        if expected != 0 or not args:
            self.assertFalse(any(c[0] == "rm" for c in self.calls))
        return after

    def test_dry_run_preserves_all_files_and_credentials(self):
        before = snapshot(self.home)
        after = self.run_reset(0, "dry run; nothing was changed")
        self.assertEqual(snapshot(self.home), before)
        self.assertEqual(after, self.state)
        self.assertFalse(any(c[0:2] == ["security", "delete-generic-password"] for c in self.calls))

    def test_unsafe_and_non_harness_paths_refuse_before_keychain(self):
        empty = self.root / "not a harness"
        empty.mkdir()
        (empty / "keep").write_text("keep")
        (self.user_home / "config.yaml").write_text("even a marked user home is unsafe")
        paths = [("relative", "home must be an absolute path"), ("/", "home must be an absolute path"),
                 ("/usr", "is too shallow"), (self.user_home, "is not a harness home"),
                 (self.user_home / ".." / "user", "is not a harness home"),
                 (empty, "has no config.yaml or harness.json")]
        paths += [(p, message) for p, message in ((Path("/Users"), "is not a harness home"),
                  (Path("/Users") / "Shared", "is a direct child of /Users")) if p.is_dir()]
        for home, message in paths:
            with self.subTest(home=home):
                before = snapshot(self.user_home)
                self.run_reset(2, message, "--yes", home=home)
                self.assertEqual(snapshot(self.user_home), before)
                self.assertEqual((empty / "keep").read_text(), "keep")
                self.assertFalse(any(c[0] == "security" for c in self.calls))

    def test_symlinks_refuse_without_touching_the_target(self):
        for target in (self.home, self.root / "missing"):
            link = self.root / "link"
            link.symlink_to(target, target_is_directory=True)
            for home in (str(link) + suffix for suffix in ("", "/", "//", "///")):
                with self.subTest(home=home, target=target):
                    before = snapshot(self.home)
                    self.run_reset(2, "is a symlink", "--yes", home=home)
                    self.assertTrue(link.is_symlink())
                    self.assertTrue(self.home.is_dir())
                    self.assertEqual(snapshot(self.home), before)
            link.unlink()

    def test_open_files_and_lsof_errors_refuse_before_keychain(self):
        for name in OPEN_FILES:
            for code, output in ((0, "123\n"), (1, "123\n"), (2, "")):
                with self.subTest(name=name, code=code):
                    self.state["open"] = {str(self.home / name): [code, output]}
                    before = snapshot(self.home)
                    self.run_reset(2, "is open or could not be checked", "--yes")
                    self.assertEqual(snapshot(self.home), before)
                    self.assertFalse(any(c[0] == "security" for c in self.calls))

    def test_missing_tools_fail_closed(self):
        for name in ("lsof", "security"):
            with self.subTest(name=name):
                path = self.bin / name
                path.rename(self.bin / (name + ".hidden"))
                self.run_reset(2, name + (" not found" if name == "lsof" else " tool not found"), "--yes")
                (self.bin / (name + ".hidden")).rename(path)

    def test_all_service_lookups_precede_any_deletion(self):
        given = str(self.home) + "/"
        for service in services(given):
            self.state["entries"][service] = ["given-key"]
        for service in services(self.home) + services(given):
            for args in ((), ("--yes",)):
                with self.subTest(service=service, args=args):
                    self.state["errors"] = {"find-generic-password": {service: 36}}
                    before = snapshot(self.home)
                    after = self.run_reset(1, "Keychain lookup", *args, home=given)
                    self.assertEqual(snapshot(self.home), before)
                    self.assertEqual(after["entries"], self.state["entries"])
                    self.assertFalse(any(c[0:2] == ["security", "delete-generic-password"] for c in self.calls))

    def test_first_delete_failure_preserves_home_and_credentials(self):
        self.state["errors"] = {"delete-generic-password": {services(self.home)[0]: 36}}
        before = snapshot(self.home)
        after = self.run_reset(1, "could not delete a Keychain entry", "--yes")
        self.assertEqual(snapshot(self.home), before)
        self.assertEqual(after["entries"], self.state["entries"])

    def test_mid_deletion_failures_report_progress_and_resume(self):
        initial = json.dumps(self.state)
        for action, service_index, errors, removed in (
                ("delete-generic-password", 0, [0, 36], 1),
                ("find-generic-password", 0, [0, 0, 36], 1),
                ("find-generic-password", 1, [0, 36], 2)):
            with self.subTest(action=action, service_index=service_index):
                if not self.home.exists():
                    seed_home(self.home)
                self.state = json.loads(initial)
                self.state["errors"] = {action: {services(self.home)[service_index]: errors}}
                before = snapshot(self.home)
                after = self.run_reset(1, f"{removed} successful delete operations", "--yes")
                self.assertIn("home removal was not attempted", self.output)
                self.assertIn("same CGAGENTHARNESS_HOME", self.output)
                self.assertEqual(snapshot(self.home), before)
                self.assertEqual(after["entries"][services(self.home)[0]], ["key-a"] if removed == 1 else [])
                self.assertEqual(after["entries"][services(self.home)[1]], ["key-a", "key-b"])
                # Resume from the actual partially changed state, not a new fixture.
                self.state = after
                self.state.pop("errors")
                finished = self.run_reset(0, f"removed {self.home}", "--yes")
                self.assertFalse(self.home.exists())
                for service in services(self.home):
                    self.assertEqual(finished["entries"][service], [])

    def test_keychain_nonprogress_is_bounded(self):
        self.state["stuck"] = True
        self.run_reset(1, "more than 100 Keychain entries", "--yes")
        self.assertEqual(sum(c[0:2] == ["security", "delete-generic-password"] for c in self.calls), 100)

    def test_yes_removes_only_selected_home_and_all_its_service_variants(self):
        (self.home / "external").symlink_to(self.other, target_is_directory=True)
        alias = self.root / "parent alias"
        alias.symlink_to(self.user_home, target_is_directory=True)
        given = str(alias / self.home.name) + "/"
        for service in services(given):
            self.state["entries"][service] = ["given-key"]
        after = self.run_reset(0, f"removed {self.home}", "--yes", home=given)
        self.assertFalse(self.home.exists())
        for service in (*services(self.home), *services(given)):
            self.assertEqual(after["entries"][service], [])
        self.assertEqual([c for c in self.calls if c[0] == "rm"], [["rm", "-rf", "--", str(self.home)]])

    def test_legacy_home_and_default_home_selection(self):
        (self.home / "config.yaml").rename(self.home / "harness.json")
        self.env.pop("CGAGENTHARNESS_HOME")
        self.run_reset(0, f"removed {self.home}", "--yes")
        self.assertFalse(self.home.exists())

    def test_missing_home_removes_only_matching_credentials(self):
        shutil.rmtree(self.home)
        alias = self.root / "parent alias"
        alias.symlink_to(self.user_home, target_is_directory=True)
        given = alias / self.home.name
        for service in services(given):
            self.state["entries"][service] = ["given-key"]
        after = self.run_reset(0, "nothing to delete", "--yes", home=given)
        for service in (*services(self.home), *services(given)):
            self.assertEqual(after["entries"][service], [])
        self.assertFalse(any(c[0] == "rm" for c in self.calls))

    def test_platform_and_arguments_refuse(self):
        self.state["platform"] = "Linux"
        self.run_reset(2, "macOS only", "--yes")
        self.state["platform"] = "Darwin"
        for args in (("--no",), ("--yes", "extra")):
            self.run_reset(2, "usage:", *args)


def native_reset(given):
    """Real lsof, Keychain and rm; never change Keychain settings or shared tools."""
    if not __debug__:
        raise RuntimeError("native acceptance requires Python assertions enabled")
    assert sys.platform == "darwin", "native acceptance requires macOS"
    home = Path(given).resolve()
    assert not Path(given).is_symlink()
    assert home.parent == Path(os.environ["RUNNER_TEMP"]).resolve()
    assert home.name.startswith("cgah-reset.")
    assert (home / ".k19-owned").read_text().strip() == str(home)
    assert (home / "config.yaml").is_file() and (home / ".server.lock").is_file()
    env = {"HOME": os.environ["HOME"], "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
           "CGAGENTHARNESS_HOME": str(home) + "/"}

    def run(argv, code=0):
        result = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=30)
        assert result.returncode == code, (argv, result.returncode, result.stdout, result.stderr)
        return result.stdout + result.stderr

    # The workflow has reaped its launcher and checked the backend PID. Require
    # lsof's explicit no-holder status too; an inspection error is not shutdown.
    for name in OPEN_FILES:
        if (home / name).exists():
            assert not run(["/usr/sbin/lsof", "-t", "--", str(home / name)], 1).strip()

    created = []
    with tempfile.TemporaryDirectory(prefix="cgah-reset-control.", dir=home.parent) as directory, \
            tempfile.TemporaryDirectory(prefix="cgah-reset-fault.", dir=home.parent) as fault_dir:
        other = Path(directory)
        seed_home(other / "home")
        (home / "external").symlink_to(other, target_is_directory=True)
        link = other / "home-link"
        link.symlink_to(home, target_is_directory=True)
        for name in ("models/model", "runtime/tool", "tools/keep"):
            path = other / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("shared fixture\n")
        selected = [(s, a) for s in services(home) + services(str(home) + "/")
                    for a in ("k19-provider-a", "k19-provider-b")]
        unrelated = [(s, "k19-control") for s in services(other / "home")]
        try:
            for service, account in selected + unrelated:
                run(["/usr/bin/security", "add-generic-password", "-s", service,
                     "-a", account, "-w", "k19-disposable-fixture"])
                created.append((service, account))
            before, control = snapshot(home), snapshot(other)
            assert "dry run; nothing was changed" in run(["/bin/bash", str(RESET)])
            for suffix in ("", "/", "//", "///"):
                assert "is a symlink" in run(["/usr/bin/env", f"CGAGENTHARNESS_HOME={link}{suffix}",
                                             "/bin/bash", str(RESET), "--yes"], 2)
            assert snapshot(home) == before and snapshot(other) == control
            for service, account in selected + unrelated:
                assert run(["/usr/bin/security", "find-generic-password", "-s", service,
                            "-a", account, "-w"]).strip() == "k19-disposable-fixture"
            for name in OPEN_FILES:
                path = home / name
                path.parent.mkdir(parents=True, exist_ok=True)
                with path.open("ab"):
                    before = snapshot(home)
                    assert "is open or could not be checked" in run(["/bin/bash", str(RESET), "--yes"], 2)
                assert snapshot(home) == before and snapshot(other) == control
            for service, account in selected + unrelated:
                assert run(["/usr/bin/security", "find-generic-password", "-s", service,
                            "-a", account, "-w"]).strip() == "k19-disposable-fixture"
            # Inject failures without locking or changing the operator's Keychain.
            wrapper = Path(fault_dir) / "security"
            wrapper.write_text(f"#!{sys.executable}\n" + '''import os, sys
if sys.argv[1:] == [os.environ["K19_FAIL_ACTION"], "-s", os.environ["K19_FAIL_SERVICE"]]:
    sys.exit(36)
os.execv("/usr/bin/security", ["security", *sys.argv[1:]])
''')
            wrapper.chmod(0o700)
            system_path = env["PATH"]
            env["PATH"] = fault_dir + ":" + system_path
            for action, service, removed in (
                    ("find-generic-password", services(str(home) + "/")[1], 0),
                    ("delete-generic-password", services(home)[1], 2)):
                env.update(K19_FAIL_ACTION=action, K19_FAIL_SERVICE=service)
                output = run(["/bin/bash", str(RESET), "--yes"], 1)
                assert f"{removed} successful delete operations" in output
                assert "home removal was not attempted" in output
                assert "same CGAGENTHARNESS_HOME" in output
                assert snapshot(home) == before and snapshot(other) == control
                for entry_service, account in selected + unrelated:
                    argv = ["/usr/bin/security", "find-generic-password", "-s", entry_service,
                            "-a", account, "-w"]
                    if removed and entry_service == services(home)[0]:
                        run(argv, 44)
                    else:
                        assert run(argv).strip() == "k19-disposable-fixture"
            env["PATH"] = system_path
            del env["K19_FAIL_ACTION"], env["K19_FAIL_SERVICE"]
            assert f"removed {home}" in run(["/bin/bash", str(RESET), "--yes"])
            assert not home.exists() and snapshot(other) == control
            for service, account in selected:
                run(["/usr/bin/security", "find-generic-password", "-s", service, "-a", account], 44)
            for service, account in unrelated:
                assert run(["/usr/bin/security", "find-generic-password", "-s", service,
                            "-a", account, "-w"]).strip() == "k19-disposable-fixture"
            print("PASS native reset: dry-run, symlink/open-file refusal, owned removal, credential and shared-fixture isolation")
            print("PASS real Keychain with injected failures: preflight preservation, partial-progress report, retry to completion")
        finally:
            cleanup_errors = []
            for service, account in created:
                result = subprocess.run(["/usr/bin/security", "delete-generic-password", "-s", service,
                                         "-a", account], env=env, capture_output=True, timeout=30)
                if result.returncode not in (0, 44):
                    cleanup_errors.append((service, result.returncode))
            assert not cleanup_errors, ("fixture credential cleanup failed", cleanup_errors)


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--native-home":
        native_reset(sys.argv[2])
    else:
        unittest.main()
