#!/usr/bin/env python3
import importlib.util
import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("stamp_version", HERE / "stamp-version.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class StampVersionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        (self.tmp / "desktop").mkdir()
        for name in ("Cargo.toml", "Cargo.lock", "desktop/Info.plist"):
            shutil.copy(HERE.parent / name, self.tmp / name)

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def read(self, name):
        return (self.tmp / name).read_text(encoding="utf-8")

    def test_stamps_only_the_root_package_and_bundle_keys(self):
        lock_before = self.read("Cargo.lock").splitlines()
        self.assertEqual(module.stamp(self.tmp, "v12.3.45"), "12.3.45")
        manifest = self.read("Cargo.toml")
        self.assertIn('\nversion = "12.3.45"\n', manifest)
        # The self dev-dependency's exact pin must follow, or resolution fails.
        self.assertIn('cgagentharness = { path = ".", version = "=12.3.45",', manifest)
        self.assertNotIn('version = "=0.1.0"', manifest)
        lock_after = self.read("Cargo.lock").splitlines()
        diff = [(a, b) for a, b in zip(lock_before, lock_after) if a != b]
        self.assertEqual(len(lock_before), len(lock_after))
        self.assertEqual(len(diff), 1)
        self.assertEqual(diff[0][1], 'version = "12.3.45"')
        plist = self.read("desktop/Info.plist")
        self.assertIn("<key>CFBundleShortVersionString</key><string>12.3.45</string>", plist)
        self.assertIn("<key>CFBundleVersion</key><string>12.3.45</string>", plist)
        self.assertIn("<key>LSMinimumSystemVersion</key><string>12.0</string>", plist)

    def test_rejects_non_stable_tags_without_writing(self):
        before = {n: self.read(n) for n in ("Cargo.toml", "Cargo.lock", "desktop/Info.plist")}
        for tag in ("0.1.40", "v0.1", "v01.2.3", "v1.2.3-rc.1", "v1.2.3+meta", "v1.2.3\n",
                    "v1.2.3 ", "vv1.2.3", "", "v1.2.3.4", "v1.2.03"):
            with self.subTest(tag=tag):
                with self.assertRaises(ValueError):
                    module.stamp(self.tmp, tag)
        self.assertEqual(before, {n: self.read(n) for n in before})

    def test_fails_closed_without_partial_writes(self):
        plist = self.tmp / "desktop/Info.plist"
        plist.write_text(plist.read_text().replace("CFBundleVersion", "CFBundleVersionX"))
        manifest_before = self.read("Cargo.toml")
        with self.assertRaises(ValueError):
            module.stamp(self.tmp, "v1.2.3")
        self.assertEqual(self.read("Cargo.toml"), manifest_before)

    def test_refuses_a_manifest_without_the_self_pin(self):
        manifest = self.tmp / "Cargo.toml"
        manifest.write_text(manifest.read_text().replace('path = ".", version = "=', 'path = ".", version = "^'))
        lock_before = self.read("Cargo.lock")
        with self.assertRaises(ValueError):
            module.stamp(self.tmp, "v1.2.3")
        self.assertEqual(self.read("Cargo.lock"), lock_before)

    def test_refuses_duplicate_targets(self):
        plist = self.tmp / "desktop/Info.plist"
        text = plist.read_text()
        plist.write_text(text.replace("</dict></plist>",
                                      "<key>CFBundleVersion</key><string>9.9.9</string>\n</dict></plist>"))
        with self.assertRaises(ValueError):
            module.stamp(self.tmp, "v1.2.3")

    def test_check_version_requires_exact_match(self):
        module.check_version("cgagentharness 0.1.40\n", "v0.1.40")
        for output in ("cgagentharness 0.1.0", "cgagentharness 0.1.40-dirty", "other 0.1.40", ""):
            with self.subTest(output=output):
                with self.assertRaises(ValueError):
                    module.check_version(output, "v0.1.40")

    def app_zip(self, plist_text, extra=()):
        path = self.tmp / "app.zip"
        import zipfile
        with zipfile.ZipFile(path, "w") as archive:
            archive.writestr("CG Agent Harness.app/Contents/Info.plist", plist_text)
            for name, data in extra:
                archive.writestr(name, data)
        return path

    def test_check_app_zip_requires_both_bundle_keys(self):
        module.stamp(self.tmp, "v0.1.40")
        stamped = self.read("desktop/Info.plist")
        module.check_app_zip(self.app_zip(stamped), "v0.1.40")
        with self.assertRaises(ValueError):
            module.check_app_zip(self.app_zip(stamped), "v0.1.41")
        half = stamped.replace("<key>CFBundleVersion</key><string>0.1.40</string>",
                               "<key>CFBundleVersion</key><string>0.1.0</string>")
        with self.assertRaises(ValueError):
            module.check_app_zip(self.app_zip(half), "v0.1.40")
        original = (HERE.parent / "desktop/Info.plist").read_text()
        with self.assertRaises(ValueError):
            module.check_app_zip(self.app_zip(original), "v0.1.40")

    def test_check_app_zip_fails_closed_on_missing_plist(self):
        path = self.tmp / "empty.zip"
        import zipfile
        with zipfile.ZipFile(path, "w") as archive:
            archive.writestr("other.txt", "x")
        self.assertEqual(module.main(["--tag", "v0.1.40", "--check-app-zip", str(path)]), 1)
        (self.tmp / "junk.zip").write_bytes(b"not a zip")
        self.assertEqual(module.main(["--tag", "v0.1.40", "--check-app-zip", str(self.tmp / "junk.zip")]), 1)

    def test_cli_exit_codes(self):
        self.assertEqual(module.main(["--tag", "v1.2.3", "--root", str(self.tmp)]), 0)
        self.assertEqual(module.main(["--tag", "1.2.3", "--root", str(self.tmp)]), 1)
        self.assertEqual(module.main(["--tag", "v1.2.3", "--check-version", "cgagentharness 1.2.4"]), 1)


if __name__ == "__main__":
    unittest.main()
