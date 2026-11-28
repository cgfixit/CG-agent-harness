"""Exercise the operator's re-score command on a disposable saved run."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


RUNNER = Path(__file__).with_name("bakeoff10.py")


def rescore(folder):
    return subprocess.run([sys.executable, str(RUNNER), "--score", str(folder)],
                          capture_output=True, text=True)


def evidence(folder):
    return {p.name: hashlib.sha256(p.read_bytes()).digest() for p in folder.iterdir()
            if p.name != "rescore.json"}


class RescoreTest(unittest.TestCase):
    def test_same_evidence_survives_label_and_root_rescores(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            label = root / "A"
            label.mkdir()
            (label / "C0.html").write_text("<html>fixture</html>")
            (label / "C0.raw.json").write_text('{"choices":[]}')
            (label / "C0.score.json").write_text(json.dumps({
                "model": "local:fixture", "wall_s": 2.5, "usage": {"total_tokens": 16}, "err": None}))
            (label / "C2.transcript.json").write_text(json.dumps({
                "finish_args": None, "tools_used": [], "wall_s": 1.5, "err": "fixture failure"}))
            (label / "summary.json").write_text(json.dumps({
                "label": "A", "model": "local:fixture", "n": 2, "cases": {}, "ollama_ps": "fixture"}))
            original = evidence(label)
            first = None
            for folder in [label, root, label]:
                run = rescore(folder)
                self.assertEqual(run.returncode, 0, run.stderr)
                self.assertEqual(evidence(label), original)
                encoded = (label / "rescore.json").read_bytes()
                report = json.loads(encoded)
                self.assertEqual(report["model"], "local:fixture")
                self.assertEqual(report["cases"]["C0"]["wall_s"], 2.5)
                self.assertEqual(report["cases"]["C0"]["usage"], {"total_tokens": 16})
                self.assertEqual(report["cases"]["C2"]["err"], "fixture failure")
                if first is None:
                    first = encoded
                self.assertEqual(encoded, first)

            (label / "C2.transcript.json").write_text("incomplete")
            original = evidence(label)
            self.assertNotEqual(rescore(root).returncode, 0)
            self.assertEqual(evidence(label), original)
            self.assertEqual((label / "rescore.json").read_bytes(), first)

    def test_linked_sibling_run_is_left_alone(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "run"
            other = Path(directory) / "other"
            root.mkdir()
            other.mkdir()
            (other / "C0.html").write_text("<html>different run</html>")
            (root / "linked-label").symlink_to(other, target_is_directory=True)
            self.assertEqual(rescore(root).returncode, 0)
            self.assertFalse((other / "rescore.json").exists())

    def test_legacy_html_does_not_require_run_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            label = Path(directory)
            (label / "C0.html").write_text("<html>fixture</html>")
            original = evidence(label)
            self.assertEqual(rescore(label).returncode, 0)
            report = json.loads((label / "rescore.json").read_text())
            self.assertIsNone(report["model"])
            self.assertIsNone(report["cases"]["C0"]["wall_s"])
            self.assertEqual(evidence(label), original)


if __name__ == "__main__":
    unittest.main()
