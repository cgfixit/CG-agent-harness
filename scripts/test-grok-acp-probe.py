#!/usr/bin/env python3
import asyncio
import importlib.util
import json
import os
from pathlib import Path
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
