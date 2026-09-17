#!/usr/bin/env python3
import asyncio
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "grok_acp_probe", Path(__file__).with_name("grok-acp-probe.py"))
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


@unittest.skipUnless(os.name == "posix", "probe requires POSIX process groups")
class GrokAcpProbeTests(unittest.TestCase):
    def test_profiles_tools_and_funding_modes_are_isolated(self):
        ambient = {"XAI_API_KEY": "ambient-key", "GROK_API_KEY": "ambient-harness-key",
                   "GH_TOKEN": "ambient-github-key"}
        with tempfile.TemporaryDirectory() as tmp, patch.dict(os.environ, ambient):
            subscription = asyncio.run(
                probe.probe_profile(Path(tmp), "subscription", "subscription"))
            api_key = asyncio.run(probe.probe_profile(Path(tmp), "api-key", "api_key"))
            self.assertEqual(subscription["answer"], "fixture:subscription")
            self.assertEqual(api_key["answer"], "fixture:api_key")
            self.assertEqual(set(subscription["denied_methods"]), probe.DENIED_METHODS)
            self.assertEqual(set(api_key["denied_methods"]), probe.DENIED_METHODS)
            self.assertEqual(subscription["empty_mcp_updates"], 1)
            self.assertEqual(api_key["empty_mcp_updates"], 1)
            self.assertEqual(
                json.loads((Path(tmp) / "subscription/grok/state.json").read_text())["auth_mode"],
                "subscription")
            self.assertEqual(
                json.loads((Path(tmp) / "api-key/grok/state.json").read_text())["auth_mode"],
                "api_key")

    def test_argv_locks_deny_all_and_disallowed_tools(self):
        argv = probe._argv(["runtime"], {"work": Path("/w"), "grok": Path("/g")})
        self.assertEqual(argv[argv.index("--deny") + 1], probe.DENY_ALL)
        self.assertEqual(argv[argv.index("--disallowed-tools") + 1], probe.DISALLOWED_TOOLS)

    def test_fake_runtime_rejects_missing_deny_or_disallowed_tools(self):
        script = Path(__file__).with_name("grok-acp-probe.py")
        for flag in ("--deny", "--disallowed-tools"):
            with tempfile.TemporaryDirectory() as tmp:
                grok = Path(tmp) / "grok"
                work = Path(tmp) / "work"
                grok.mkdir()
                work.mkdir()
                (grok / "requirements.toml").write_text(
                    "[grok_com_config]\ndisable_api_key_auth = true\n")
                env = {
                    "HOME": tmp,
                    "GROK_HOME": str(grok),
                    "PATH": "/usr/bin:/bin",
                    "TMPDIR": tmp,
                    **probe.OFF_ENV,
                }
                argv = probe._argv([sys.executable, str(script), "--fake-runtime"],
                                   {"work": work, "grok": grok})
                idx = argv.index(flag)
                del argv[idx:idx + 2]
                completed = subprocess.run(
                    argv, cwd=work, env=env, input=b"",
                    capture_output=True, timeout=5)
                self.assertNotEqual(completed.returncode, 0, flag)
                self.assertIn(b"unsafe fake-runtime launch contract", completed.stderr)

    def test_running_treats_exited_pid_as_not_live(self):
        child = subprocess.Popen(
            [sys.executable, "-c", "pass"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        self.assertEqual(child.wait(timeout=2), 0)
        self.assertFalse(probe._running(child.pid))

    def test_wrong_auth_surface_fails_closed(self):
        with self.assertRaisesRegex(RuntimeError, "grok.com"):
            probe._select_auth("subscription", {"authMethods": [{"id": "cached_token"}]}, {})
        with self.assertRaisesRegex(RuntimeError, "funding mode"):
            probe._select_auth("api_key", {"authMethods": [{"id": "grok.com"}]}, {})
        self.assertIsNone(probe._select_auth(
            "api_key", {"authMethods": [{"id": "grok.com"}]},
            {"XAI_API_KEY": probe.FIXTURE_KEY}))

    def test_cancellation_reaps_the_runtime_descendant(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = asyncio.run(
                probe.probe_profile(Path(tmp), "cancel", "subscription", cancel=True))
        self.assertEqual(result, {"cancel_notification": True, "descendant_reaped": True})


if __name__ == "__main__":
    unittest.main(verbosity=2)
