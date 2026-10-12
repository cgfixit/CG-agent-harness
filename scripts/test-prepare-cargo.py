#!/usr/bin/env python3
"""Exercise real offline preparation without trusting candidate toolchain files."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class PreparationToolchain(unittest.TestCase):
    @unittest.skipUnless(os.name == "posix", "fixture compiler is a POSIX script")
    def test_candidate_cannot_select_the_preparation_compiler(self):
        helper = Path(__file__).with_name("prepare-cargo.py").resolve()
        env = dict(os.environ, RUSTUP_AUTO_INSTALL="0", CARGO_NET_OFFLINE="true")
        # Exercise rustup's candidate-file selection, not an ambient override
        # that would hide the original bug. PATH must contain a rustup proxy.
        env.pop("RUSTUP_TOOLCHAIN", None)
        with tempfile.TemporaryDirectory(prefix="cargo-preparation-") as directory:
            root = Path(directory)
            repo = root / "candidate"
            (repo / "src").mkdir(parents=True)
            (repo / "Cargo.toml").write_text('[package]\nname="fixture"\nversion="0.1.0"\nedition="2021"\n')
            (repo / "Cargo.lock").write_text('version = 3\n[[package]]\nname="fixture"\nversion="0.1.0"\n')
            (repo / "src/lib.rs").write_text("pub fn value() -> u8 { 1 }\n")
            compiler = root / "untrusted-toolchain/bin/rustc"
            compiler.parent.mkdir(parents=True)
            marker = root / "candidate-compiler-executed"
            compiler.write_text('#!/bin/sh\nprintf invoked > "$CGAH_PROBE_MARKER"\nexit 42\n')
            compiler.chmod(0o700)
            env["CGAH_PROBE_MARKER"] = str(marker)
            toolchain = repo / "rust-toolchain.toml"
            toolchain.write_text("[toolchain]\npath = " + json.dumps(str(compiler.parent.parent)) + "\n")
            # Prove the fixture activates the original rustup boundary.
            trigger = subprocess.run(["rustc", "--print", "sysroot"], cwd=repo, env=env,
                                     capture_output=True, text=True)
            self.assertTrue(marker.exists(), "test requires rustup proxies on PATH: " + trigger.stderr)
            marker.unlink()
            for index, cwd in enumerate([root, repo]):
                home = root / f"home-{index}"
                result = subprocess.run(["python3", str(helper), str(repo), str(home)],
                                        cwd=cwd, env=env, capture_output=True, text=True)
                self.assertFalse(marker.exists(), "candidate selected executable before confinement")
                self.assertEqual(result.returncode, 0, result.stderr)
                records = list(home.glob("data/agentic/cargo-prepared/*/prepared.json"))
                self.assertEqual(len(records), 1)
                record = json.loads(records[0].read_text())
                self.assertNotEqual(Path(record["toolchain_root"]), compiler.parent.parent)
                self.assertTrue(record["rustc_version"].startswith("rustc "))
            # A normal repository and the operator's installed default compiler
            # still produce an offline snapshot without compiling candidate code.
            toolchain.unlink()
            result = subprocess.run(["python3", str(helper), str(repo), str(root / "control-home")],
                                    cwd=repo, env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse((repo / "target").exists())


if __name__ == "__main__":
    unittest.main()
