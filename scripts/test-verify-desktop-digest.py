#!/usr/bin/env python3
"""The post-codesign sidecar digest must still be the bytes compiled into the shell."""
import hashlib
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


CHECK = (
    "import pathlib, sys; d = sys.argv[1].encode(); "
    "b = pathlib.Path(sys.argv[2]).read_bytes(); "
    "raise SystemExit(0 if d in b else 1)"
)


def digest_in_shell(sidecar: Path, shell: Path) -> int:
    digest = hashlib.sha256(sidecar.read_bytes()).hexdigest()
    return subprocess.run(
        [sys.executable, "-c", CHECK, digest, str(shell)],
        check=False,
    ).returncode


class SidecarDigestTests(unittest.TestCase):
    def test_matching_digest_is_present(self):
        with tempfile.TemporaryDirectory() as tmp:
            sidecar = Path(tmp) / "cgagentharness"
            shell = Path(tmp) / "cg-agent-harness-desktop"
            payload = b"signed-sidecar-bytes"
            sidecar.write_bytes(payload)
            digest = hashlib.sha256(payload).hexdigest()
            shell.write_bytes(b"prefix " + digest.encode() + b" suffix")
            self.assertEqual(digest_in_shell(sidecar, shell), 0)

    def test_mutated_sidecar_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            sidecar = Path(tmp) / "cgagentharness"
            shell = Path(tmp) / "cg-agent-harness-desktop"
            original = b"signed-sidecar-bytes"
            sidecar.write_bytes(b"mutated-after-app-codesign")
            digest = hashlib.sha256(original).hexdigest()
            shell.write_bytes(b"prefix " + digest.encode() + b" suffix")
            self.assertEqual(digest_in_shell(sidecar, shell), 1)


if __name__ == "__main__":
    unittest.main()
