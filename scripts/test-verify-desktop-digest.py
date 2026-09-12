#!/usr/bin/env python3
"""Read the built shell's digest without starting the GUI or accessing a home."""
import hashlib
import os
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SHELL = ROOT / "desktop/target/release/cg-agent-harness-desktop"
SIDECAR = ROOT / "target/release/cgagentharness"

class SidecarDigestTests(unittest.TestCase):
    def test_built_shell_reports_the_signed_sidecar_digest(self):
        output = subprocess.check_output([str(SHELL), "--bundled-backend-sha256"],
            env={"PATH": "/usr/bin:/bin", "CGAGENTHARNESS_HOME": "/nonexistent/never-opened"}, timeout=10).decode().strip()
        payload = SIDECAR.read_bytes()
        self.assertEqual(output, hashlib.sha256(payload).hexdigest())
        self.assertNotEqual(output, hashlib.sha256(payload + b"mutated after signing").hexdigest())

if __name__ == "__main__":
    unittest.main()
