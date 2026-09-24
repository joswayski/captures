import json
import os
from pathlib import Path, PureWindowsPath
import plistlib
import shutil
import subprocess
import tempfile
import time
import unittest

import package


class DevelopmentPackageTests(unittest.TestCase):
    def test_formats_match_shipping_without_claiming_defaults(self):
        shipping = json.loads((package.ROOT / "apps/desktop/src-tauri/tauri.conf.json").read_text())
        expected = {(tuple(item["ext"]), item["mimeType"]) for item in shipping["bundle"]["fileAssociations"]}
        self.assertEqual({(ext, mime) for _, ext, mime, _ in package.FORMATS}, expected)
        info = package.mac_info()
        self.assertNotEqual(info["CFBundleIdentifier"], shipping["identifier"])
        self.assertTrue(info["CapturesNativeLive"])
        for entry in info["CFBundleDocumentTypes"]:
            self.assertEqual(entry["CFBundleTypeRole"], "Editor")
            self.assertEqual(entry["LSHandlerRank"], "Alternate")
        self.assertIn("NSMicrophoneUsageDescription", info)

    def test_windows_alternates_and_selective_removal_quote_unicode_paths(self):
        registration, removal = package.windows_registry(PureWindowsPath(r"C:\Native é space\CapturesNative.exe"))
        self.assertIn(r'@="\"C:\\Native é space\\CapturesNative.exe\" --live -- \"%1\""', registration)
        self.assertEqual(registration.count('"MultiSelectModel"="Document"'), 2)
        self.assertNotIn("HKEY_LOCAL_MACHINE", registration)
        self.assertNotIn("UserChoice", registration)
        self.assertNotIn("UserChoice", removal)
        self.assertEqual(removal.count('"CapturesNativeDevelopment.Media"=-'), 7)
        self.assertEqual(removal.count("[-"), 2)
        for _, extensions, _, _ in package.FORMATS:
            for ext in extensions:
                self.assertNotIn(f"\\.{ext}]", registration)  # never set extension defaults
                self.assertIn(f"\\.{ext}\\OpenWithProgids]", registration)

    def test_stages_self_contained_resources_and_never_overwrites_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "binary"
            binary.write_bytes(b"test executable")
            resources = root / "resources.bundle"
            resources.mkdir()
            (resources / "tokens.json").write_text("{}")
            for platform in ("macos", "windows", "linux"):
                output = root / platform
                launcher, executable = package.stage(platform, binary, output, resources)
                self.assertEqual(executable.read_bytes(), binary.read_bytes())
                self.assertTrue(launcher.exists())
                with self.assertRaises(FileExistsError):
                    package.stage(platform, binary, output, resources)
                if platform == "macos":
                    self.assertEqual(plistlib.loads((launcher / "Contents/Info.plist").read_bytes()), package.mac_info())
                    self.assertEqual((launcher / "Contents/Resources/CapturesNative_CapturesNative.bundle/tokens.json").read_text(), "{}")
                elif platform == "windows":
                    self.assertTrue(launcher.read_bytes().startswith(b"\xff\xfe"))

    @unittest.skipUnless(os.name == "posix" and shutil.which("gio"), "requires gio desktop-entry parser")
    def test_gio_preserves_exec_quoting_and_multiple_paths_without_a_shell(self):
        # Exercise an independent desktop-entry parser, including shell syntax
        # in executable and media filenames and literal field codes in media.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / 'é space $HOME `false` "quote" \\slash'
            with self.assertRaises(ValueError):
                package.desktop_entry(root / "%f binary")
            result = root / "argv.json"
            binary.write_text("#!/usr/bin/env python3\nimport json, os, sys\n"
                              "open(os.environ['CAPTURES_ARGV'], 'w').write(json.dumps(sys.argv[1:]))\n")
            binary.chmod(0o755)
            desktop = root / "test.desktop"
            desktop.write_text(package.desktop_entry(binary))
            paths = [root / "first é space.png", root / "second %F $HOME.webm"]
            for path in paths:
                path.touch()
            launched = subprocess.run(["gio", "launch", str(desktop), *map(str, paths)],
                                      env={**os.environ, "CAPTURES_ARGV": str(result)},
                                      capture_output=True, timeout=10)
            self.assertEqual(launched.returncode, 0, launched.stderr)
            deadline = time.monotonic() + 5
            while not result.exists() and time.monotonic() < deadline:
                time.sleep(.02)
            self.assertEqual(json.loads(result.read_text()), ["--live", "--", *map(str, paths)])


if __name__ == "__main__":
    unittest.main()
