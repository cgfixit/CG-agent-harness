#!/usr/bin/env python3
"""uninstall-macos.sh contract with portable command doubles (no real macOS tools).

The script deletes the clone it runs from, so every test copies it into a
disposable fixture clone under a fixture $HOME. PATH is closed: macOS tools are
doubles, `rm` refuses any path outside the sandbox, and only a few real text
tools are linked in, so a regression cannot reach the operator's Keychain,
apps, Homebrew or files.
"""
import hashlib
import json
import os
from pathlib import Path
import pty
import select
import shutil
import subprocess
import sys
import tempfile
import time
import unittest


REPO = Path(__file__).resolve().parent.parent
BUNDLE = "com.cgfixit.agent-harness"
APP = "CG Agent Harness.app"
MARKER = "# Added by CG-agent-harness scripts/init.sh"
BREW_LINE = 'eval "$(/opt/homebrew/bin/brew shellenv)"'
CARGO_LINE = '. "$HOME/.cargo/env"'
# The script keeps the Homebrew PATH line while this exists; it is outside the sandbox.
HOST_HAS_BREW = os.access("/opt/homebrew/bin/brew", os.X_OK)
REAL_TOOLS = ("bash", "sed", "grep", "awk", "tr", "wc", "cp", "cat", "dirname", "basename",
              "head", "seq", "sleep", "git", "sort")
DOUBLES = ("uname", "id", "defaults", "launchctl", "security", "pgrep", "osascript", "brew",
           "rustup", "du", "lsof", "rm")


def snapshot(root):
    skip = {"bin", "state.json", "calls.jsonl"}
    return {str(p.relative_to(root)): (os.readlink(p) if p.is_symlink() else
            hashlib.sha256(p.read_bytes()).hexdigest() if p.is_file() else None)
            for p in root.rglob("*") if p.relative_to(root).parts[0] not in skip
            and ".git" not in p.relative_to(root).parts}


def services(home):
    return [f"CGagentHarness ({home})", f"CGagentHarness admin ({home})"]


DOUBLE = '''#!/usr/bin/env python3
import json, os, pathlib, shutil, sys
name, args = pathlib.Path(sys.argv[0]).name, sys.argv[1:]
state_path = pathlib.Path(os.environ["UNINSTALL_STATE"])
state = json.loads(state_path.read_text())
with open(os.environ["UNINSTALL_CALLS"], "a") as log:
    log.write(json.dumps([name, *args]) + "\\n")
def save():
    state_path.write_text(json.dumps(state))
if name == "uname":
    print(state.get("platform", "Darwin"))
elif name == "id":
    assert args == ["-u"], args
    print(state.get("uid", 501))
elif name == "defaults":
    assert args[0] == "read" and args[2] == "CFBundleIdentifier", args
    assert args[1].endswith("/Contents/Info"), args
    bundle = state["apps"].get(args[1][:-len("/Contents/Info")])
    if bundle is None:
        sys.exit(1)
    print(bundle)
elif name == "launchctl":
    if args[0] == "getenv":
        value = state["launchd"].get(args[1])
        if value is not None:
            print(value)
    elif args[0] == "unsetenv":
        state["launchd"].pop(args[1], None); save()
    else:
        raise AssertionError(args)
elif name == "pgrep":
    if args[:1] == ["-xq"]:
        sys.exit(0 if state.get("ollama_running") else 1)
    assert args[:3] == ["-u", str(state.get("uid", 501)), "-f"], args
    sys.exit(0 if state.get("app_running") else 1)
elif name == "osascript":
    if 'quit app "CG Agent Harness"' in args[-1] and not state.get("app_stubborn"):
        state["app_running"] = False
    if 'quit app "Ollama"' in args[-1]:
        state["ollama_running"] = False
    save()
elif name == "security":
    action = args[0]
    if action == "find-certificate":
        assert args[1:4] == ["-a", "-c", "CG Agent Harness local portal"] and args[4] == "-Z", args
        for h in state["certs"]:
            print("SHA-256 hash: " + h * 2 + "\\nSHA-1 hash: " + h)  # DevSkim: ignore DS126858 because this double mimics the SHA-1 item-id label security prints; nothing is hashed.
    elif action == "delete-certificate":
        assert args[1:3] == ["-t", "-Z"], args
        state["certs"].remove(args[3]); save()
    else:
        assert action in ("find-generic-password", "delete-generic-password") and args[1] == "-s", args
        entries = state["entries"].get(args[2], [])
        if not entries:
            sys.exit(44)
        if action == "delete-generic-password":
            entries.pop(); save()
elif name == "brew":
    kind = "casks" if "--cask" in args else "formulae"
    item = args[-1]
    if args[0] == "list":
        sys.exit(0 if item in state["brew"][kind] else 1)
    assert args[0] == "uninstall", args
    state["brew"][kind].remove(item); save()
elif name == "rustup":
    if args[:1] == ["run"]:
        sys.exit(0 if args[1] in state["toolchains"] else 1)
    if args[:2] == ["toolchain", "uninstall"]:
        state["toolchains"].remove(args[2]); save()
    elif args == ["self", "uninstall", "-y"]:
        state["toolchains"] = []; save()
        home = pathlib.Path(os.environ["HOME"])
        for d in (".cargo", ".rustup"):
            shutil.rmtree(home / d, ignore_errors=True)
    else:
        raise AssertionError(args)
elif name == "du":
    print("4.0G\\t" + args[-1])
elif name == "lsof":
    code, output = state.get("open", {}).get(args[-1], [1, ""])
    sys.stdout.write(output)
    sys.exit(code)
elif name == "rm":
    flags, sep, path = args[0], args[1], args[2]
    assert flags in ("-rf", "-f") and sep == "--" and len(args) == 3, args
    target = pathlib.Path(path)
    sandbox = pathlib.Path(os.environ["UNINSTALL_SANDBOX"])
    # Never outside the sandbox, never the fixture $HOME itself.
    assert sandbox in target.resolve().parents, path
    assert target.resolve() != pathlib.Path(os.environ["HOME"]).resolve(), path
    if target.is_dir() and not target.is_symlink():
        shutil.rmtree(target)
    elif target.exists() or target.is_symlink():
        target.unlink()
else:
    raise AssertionError(name)
'''


class UninstallContract(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix="cgah-uninstall-contract-")
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()
        self.user = self.root / "Users" / "me"
        self.home = self.user / ".CGagentHarness"
        self.clone = self.user / "src" / "CG-agent-harness"
        (self.clone / "scripts" / "logs").mkdir(parents=True)
        (self.clone / "target" / "debug").mkdir(parents=True)
        for name in ("Cargo.toml", "init.command", "uninstall.command", "rust-toolchain.toml"):
            shutil.copy2(REPO / name, self.clone / name)
        for name in ("uninstall-macos.sh", "reset-macos.sh"):
            shutil.copy2(REPO / "scripts" / name, self.clone / "scripts" / name)
        self.toolchain = next(line.split('"')[1] for line in (REPO / "rust-toolchain.toml").read_text().splitlines()
                              if line.startswith("channel"))
        self.write(self.clone / "scripts/logs/install.log", "log\n")
        self.write(self.home / "config.yaml", "fixture\n")
        self.write(self.home / "sessions/keep.json", "{}\n")
        self.ours = self.user / "Applications" / APP
        self.foreign = self.root / "Applications" / APP
        for app in (self.ours, self.foreign):
            (app / "Contents").mkdir(parents=True)
        self.ours_data = [self.user / "Library" / p for p in (
            f"Caches/{BUNDLE}", f"WebKit/{BUNDLE}", f"HTTPStorages/{BUNDLE}",
            f"Saved Application State/{BUNDLE}.savedState", "Caches/CG-agent-harness-installer/previous")]
        for path in self.ours_data:
            path.mkdir(parents=True)
        self.write(self.user / f"Library/Preferences/{BUNDLE}.plist", "plist\n")
        self.ours_data.append(self.user / f"Library/Preferences/{BUNDLE}.plist")
        self.shared = [self.user / p for p in ("Library/Caches/com.other.app/keep", ".ollama/models/m",
                                               ".cargo/env", ".rustup/keep")]
        for path in self.shared:
            self.write(path, "shared fixture\n")
        self.write(self.user / "Library/Logs/ollama-serve.log", "log\n")
        self.profile = self.user / ".zprofile"
        self.write(self.profile, f"export FOO=1\n\n{MARKER}\n{BREW_LINE}\n\n{MARKER}\n{CARGO_LINE}\nalias ll=ls\n")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        for name in REAL_TOOLS:
            (self.bin / name).symlink_to(shutil.which(name))
        (self.bin / "python3").symlink_to(sys.executable)
        for name in DOUBLES:
            path = self.bin / name
            path.write_text(DOUBLE)
            path.chmod(0o700)
        self.state_path = self.root / "state.json"
        self.calls_path = self.root / "calls.jsonl"
        self.state = {
            "apps": {str(self.ours): BUNDLE, str(self.foreign): "com.other.thing"},
            "launchd": {"OLLAMA_CONTEXT_LENGTH": "32768"},
            "certs": ["AB12"],
            "entries": {**{s: ["key"] for s in services(self.home)}, "unrelated service": ["keep"]},
            "brew": {"casks": ["ollama-app"], "formulae": ["gh"]},
            "toolchains": [self.toolchain],
        }
        self.env = {"HOME": str(self.user), "PATH": str(self.bin), "UNINSTALL_SANDBOX": str(self.root),
                    "UNINSTALL_STATE": str(self.state_path), "UNINSTALL_CALLS": str(self.calls_path)}

    @staticmethod
    def write(path, text):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def git(self, *args):
        subprocess.run(["git", "-C", str(self.clone), "-c", "user.email=t@t", "-c", "user.name=t", *args],
                       check=True, capture_output=True)

    def run_uninstall(self, expected, message, *args):
        self.state_path.write_text(json.dumps(self.state))
        self.calls_path.write_text("")
        result = subprocess.run(["/bin/bash", str(self.clone / "scripts/uninstall-macos.sh"), *args],
                                env=self.env, cwd=self.root, stdin=subprocess.DEVNULL,
                                capture_output=True, text=True, timeout=30)
        return self.finish(result.returncode, result.stdout + result.stderr, expected, message)

    def run_tty(self, expected, message, answers, *args):
        """Drive the Finder entry point (uninstall.command) through a real pty."""
        self.state_path.write_text(json.dumps(self.state))
        self.calls_path.write_text("")
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(self.root)
            os.execve("/bin/bash", ["bash", str(self.clone / "uninstall.command"), *args], self.env)
        out, pending, deadline = b"", list(answers), time.time() + 30
        while time.time() < deadline:
            if not select.select([fd], [], [], 0.5)[0]:
                continue
            try:
                chunk = os.read(fd, 4096)
            except OSError:
                break
            if not chunk:
                break
            out += chunk
            if pending and pending[0][0].encode() in out.rsplit(b"\n", 1)[-1]:
                os.write(fd, pending.pop(0)[1].encode() + b"\n")
        _, status = os.waitpid(pid, 0)
        self.assertEqual(pending, [], out.decode(errors="replace"))
        return self.finish(os.waitstatus_to_exitcode(status), out.decode(errors="replace"), expected, message)

    def finish(self, code, output, expected, message):
        self.output = output
        self.assertEqual(code, expected, output)
        self.assertIn(message, output)
        self.calls = [json.loads(line) for line in self.calls_path.read_text().splitlines()]
        after = json.loads(self.state_path.read_text())
        # Never touched, whatever the outcome.
        self.assertTrue((self.foreign / "Contents").is_dir())
        self.assertEqual(after["entries"]["unrelated service"], ["keep"])
        self.assertEqual((self.user / "Library/Caches/com.other.app/keep").read_text(), "shared fixture\n")
        return after

    def destructive(self):
        return [c for c in self.calls if c[0] == "rm" or c[:2] in (
            ["security", "delete-certificate"], ["security", "delete-generic-password"],
            ["brew", "uninstall"], ["launchctl", "unsetenv"])
            or (c[0] == "rustup" and c[1] in ("toolchain", "self"))]

    def assert_nothing_removed(self, before):
        self.assertEqual(snapshot(self.root), before)
        self.assertEqual(self.destructive(), [])

    def test_dry_run_changes_nothing(self):
        before = snapshot(self.root)
        after = self.run_uninstall(0, "dry run finished", "--dry-run", "--yes", "--remove-ollama",
                                   "--remove-models", "--remove-gh", "--remove-rustup")
        self.assert_nothing_removed(before)
        self.assertEqual(after, self.state)
        for line in (f"app: {self.ours}", "1 trusted", "Ollama,", "~/.ollama (models)", "GitHub CLI",
                     "rustup, ~/.cargo", f"this clone: {self.clone}", f"rm -rf -- {self.clone}",
                     "dry run; nothing was changed"):
            self.assertIn(line, self.output)
        self.assertNotIn(str(self.foreign), self.output)

    def test_no_terminal_without_yes_refuses(self):
        before = snapshot(self.root)
        self.run_uninstall(4, "no terminal to confirm in")
        self.assert_nothing_removed(before)

    def test_yes_removes_harness_items_and_keeps_shared_tools(self):
        after = self.run_uninstall(0, "uninstalled", "--yes", "--app-dir", str(self.root / "Applications"))
        self.assertFalse(self.home.exists())
        for service in services(self.home):
            self.assertEqual(after["entries"][service], [])
        self.assertEqual(after["certs"], [])
        self.assertFalse(self.ours.exists())
        for path in self.ours_data:
            self.assertFalse(path.exists(), path)
        self.assertFalse(self.clone.exists())
        for path in (".ollama/models/m", ".cargo/env", ".rustup/keep"):
            self.assertTrue((self.user / path).exists(), path)
        self.assertEqual(after["brew"], self.state["brew"])
        self.assertEqual(after["toolchains"], self.state["toolchains"])
        self.assertEqual(after["launchd"], self.state["launchd"])
        # The brew pair goes only when Homebrew is gone; the cargo pair stays while ~/.cargo/env does.
        kept_brew = f"{MARKER}\n{BREW_LINE}\n\n" if HOST_HAS_BREW else ""
        self.assertEqual(self.profile.read_text(), f"export FOO=1\n\n{kept_brew}{MARKER}\n{CARGO_LINE}\nalias ll=ls\n")
        if not HOST_HAS_BREW:
            self.assertTrue((self.user / ".zprofile.cgah-uninstall.bak").is_file())
        self.assertIn("Command Line Tools: sudo rm -rf", self.output)

    def test_shared_tool_flags_remove_only_what_they_name(self):
        after = self.run_uninstall(0, "uninstalled", "--yes", "--keep-clone", "--remove-rustup",
                                   "--remove-ollama", "--remove-models", "--remove-gh")
        self.assertEqual(after["brew"], {"casks": [], "formulae": []})
        self.assertEqual(after["toolchains"], [])
        self.assertNotIn("OLLAMA_CONTEXT_LENGTH", after["launchd"])
        for path in (".ollama", ".cargo", ".rustup", "Library/Logs/ollama-serve.log"):
            self.assertFalse((self.user / path).exists(), path)
        self.assertIn(["rustup", "self", "uninstall", "-y"], self.calls)
        self.assertFalse(any(c[:2] == ["rustup", "toolchain"] for c in self.calls))
        kept_brew = f"\n{MARKER}\n{BREW_LINE}\n" if HOST_HAS_BREW else ""
        self.assertEqual(self.profile.read_text(), f"export FOO=1\n{kept_brew}alias ll=ls\n")
        # --keep-clone still clears the build tree and install log.
        self.assertTrue((self.clone / "scripts/uninstall-macos.sh").is_file())
        self.assertFalse((self.clone / "target").exists())
        self.assertFalse((self.clone / "scripts/logs").exists())

    def test_toolchain_flag_keeps_rustup(self):
        after = self.run_uninstall(0, "uninstalled", "--yes", "--keep-clone", "--remove-toolchain")
        self.assertIn(["rustup", "toolchain", "uninstall", self.toolchain], self.calls)
        self.assertEqual(after["toolchains"], [])
        self.assertTrue((self.user / ".cargo/env").is_file())
        self.assertIn(CARGO_LINE, self.profile.read_text())

    def test_unsaved_work_keeps_the_clone(self):
        # As in the real repository, the build tree and install log are ignored.
        self.write(self.clone / ".gitignore", "target/\n/scripts/logs/\n")
        self.git("init", "-q")
        self.git("add", "-A")
        self.git("commit", "-qm", "fixture")
        for label, prepare in (("unpushed commit", lambda: None),
                               ("uncommitted change", lambda: (
                                   self.git("update-ref", "refs/remotes/origin/main", "HEAD"),
                                   self.write(self.clone / "new.txt", "work\n")))):
            with self.subTest(label):
                prepare()
                self.run_uninstall(0, "keeping the clone", "--yes")
                self.assertTrue((self.clone / "scripts/uninstall-macos.sh").is_file())
                self.assertFalse((self.clone / "target").exists())
        # Everything pushed and committed: the clone goes.
        (self.clone / "new.txt").unlink()
        self.run_uninstall(0, "uninstalled", "--yes")
        self.assertFalse(self.clone.exists())

    def test_folder_that_is_not_a_clone_is_kept(self):
        (self.clone / "Cargo.toml").write_text('[package]\nname = "something-else"\n')
        self.run_uninstall(0, "does not look like a CG-agent-harness clone", "--yes")
        self.assertTrue((self.clone / "Cargo.toml").is_file())

    def test_failed_reset_stops_before_anything_else(self):
        self.write(self.home / ".server.lock", "")
        self.state["open"] = {str(self.home / ".server.lock"): [0, "4242\n"]}
        before = snapshot(self.root)
        after = self.run_uninstall(2, "nothing after it was removed", "--yes")
        self.assert_nothing_removed(before)
        self.assertEqual(after["certs"], self.state["certs"])

    def test_running_app_refuses_before_anything(self):
        self.state["app_running"] = True
        before = snapshot(self.root)
        self.run_uninstall(4, "quit the app (Cmd-Q) and rerun", "--yes")
        self.assert_nothing_removed(before)

    def test_launchd_home_is_reset_too(self):
        other = self.user / "Other Home"
        self.write(other / "config.yaml", "fixture\n")
        self.state["launchd"]["CGAGENTHARNESS_HOME"] = str(other)
        self.state["entries"].update({s: ["key"] for s in services(other)})
        after = self.run_uninstall(0, "uninstalled", "--yes", "--keep-clone")
        self.assertFalse(self.home.exists())
        self.assertFalse(other.exists())
        for service in services(self.home) + services(other):
            self.assertEqual(after["entries"][service], [])
        self.assertNotIn("CGAGENTHARNESS_HOME", after["launchd"])

    def test_environment_and_arguments_refuse(self):
        before = snapshot(self.root)
        for state, args, message in ((
                {"platform": "Linux"}, ["--yes"], "macOS only"), ({"uid": 0}, ["--yes"], "not root"),
                ({}, ["--bogus"], "unknown option"), ({}, ["--app-dir"], "--app-dir needs a directory")):
            with self.subTest(args=args, state=state):
                self.state.update(state)
                self.run_uninstall(3, message, *args)
                self.assert_nothing_removed(before)
                self.state.pop("platform", None)
                self.state.pop("uid", None)

    def test_terminal_decline_removes_nothing(self):
        before = snapshot(self.root)
        self.run_tty(4, "nothing was removed", [
            ("toolchain", "y"), ("rustup entirely", ""), ("Ollama?", "y"), ("model", ""),
            ("GitHub", ""), ("clone", ""), ("Type UNINSTALL", "uninstall"), ("Return", "")])
        self.assert_nothing_removed(before)

    def test_terminal_answers_select_what_goes(self):
        after = self.run_tty(0, "uninstalled", [
            ("toolchain", "n"), ("rustup entirely", "n"), ("Ollama?", "y"), ("model", "n"),
            ("GitHub", "n"), ("clone", "n"), ("Type UNINSTALL", "UNINSTALL"), ("Return", "")])
        self.assertFalse(self.home.exists())
        self.assertEqual(after["brew"], {"casks": [], "formulae": ["gh"]})
        self.assertTrue((self.user / ".ollama/models/m").is_file())
        self.assertEqual(after["toolchains"], self.state["toolchains"])
        self.assertTrue((self.clone / "scripts/uninstall-macos.sh").is_file())


if __name__ == "__main__":
    unittest.main()
