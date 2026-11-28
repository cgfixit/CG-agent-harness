"""Exercise the runner's Ollama lifecycle without contacting a model service."""
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import bakeoff10


class ModelUnloadTest(unittest.TestCase):
    def run_bakeoff(self, output, *args):
        loaded = {"shared", "unrelated"}
        commands = []

        def ollama(argv, **kwargs):
            commands.append(argv)
            if argv[:2] == ["ollama", "stop"]:
                loaded.remove(argv[2])
            return bakeoff10.subprocess.CompletedProcess(argv, 0, stdout=" ".join(sorted(loaded)))

        def chat(model, *_):
            loaded.add(model)
            return {}, "<html>fixture</html>"

        argv = ["bakeoff10.py", "--out", str(output), "--cases", "C0", "--models", "shared,fresh", *args]
        with patch.object(sys, "argv", argv), patch.object(bakeoff10, "chat", side_effect=chat), \
                patch.object(bakeoff10.subprocess, "run", side_effect=ollama), contextlib.redirect_stdout(io.StringIO()):
            bakeoff10.main()
        return loaded, commands

    def test_default_leaves_preloaded_and_newly_loaded_models_available(self):
        with tempfile.TemporaryDirectory() as folder:
            loaded, commands = self.run_bakeoff(Path(folder))
            self.assertEqual(loaded, {"shared", "fresh", "unrelated"})
            self.assertEqual(commands, [["ollama", "ps"], ["ollama", "ps"]])
            summary = json.loads((Path(folder) / "B" / "summary.json").read_text())
            self.assertEqual(summary["model"], "fresh")
            self.assertIn("shared", summary["ollama_ps"])

    def test_opt_in_stops_only_each_tested_model_after_recording_its_inventory(self):
        with tempfile.TemporaryDirectory() as folder:
            loaded, commands = self.run_bakeoff(Path(folder), "--unload")
            self.assertEqual(loaded, {"unrelated"})
            self.assertEqual(commands, [["ollama", "ps"], ["ollama", "stop", "shared"],
                                        ["ollama", "ps"], ["ollama", "stop", "fresh"]])
            for label, model in [("A", "shared"), ("B", "fresh")]:
                summary = json.loads((Path(folder) / label / "summary.json").read_text())
                self.assertIn(model, summary["ollama_ps"].split())

    def test_nonrunning_modes_never_unload(self):
        with tempfile.TemporaryDirectory() as folder:
            for args in [("--dry-run",), ("--score", folder), ("--cases", "unknown")]:
                with self.subTest(args=args):
                    loaded, commands = self.run_bakeoff(Path(folder), "--unload", *args)
                    self.assertEqual(loaded, {"shared", "unrelated"})
                    self.assertFalse(any(cmd[:2] == ["ollama", "stop"] for cmd in commands))
