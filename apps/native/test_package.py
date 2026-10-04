import json
import os
from pathlib import Path, PureWindowsPath
import plistlib
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

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

    def test_media_packages_keep_target_pair_source_and_licenses_and_reject_incomplete_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            inputs = {
                "scripts/build-ffmpeg-sidecars.sh": b'FFMPEG_VERSION="9.8.7"\n',
                "target/ffmpeg-dist/ffmpeg-9.8.7.tar.xz": b"corresponding source",
                "target/ffmpeg-dist/ffmpeg-9.8.7.tar.xz.asc": b"detached signature",
                "target/ffmpeg-dist/ffmpeg-9.8.7-BUILD_CONFIG.txt": b"exact configuration",
                "target/ffmpeg-dist/ffmpeg-9.8.7-COPYING.LGPLv2.1": b"LGPL license",
                "target/ffmpeg-dist/ffmpeg-9.8.7-NOTICE.md": b"FFmpeg notice",
                "apps/desktop/src-tauri/openh264/LICENSE": b"OpenH264 license",
                "apps/desktop/src-tauri/openh264/NOTICE.md": b"OpenH264 notice",
                "apps/desktop/src-tauri/Info.plist": plistlib.dumps({}),
                "apps/desktop/src-tauri/icons/icon.icns": b"icon",
                "LICENSE": b"Captures license",
                "TRADEMARKS.md": b"Captures trademarks",
            }
            for name, data in inputs.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
            binary = root / "native"
            binary.write_bytes(b"native executable")
            resources = root / "resources.bundle"
            resources.mkdir()
            targets = (
                ("macos", "aarch64-apple-darwin", "ffmpeg-aarch64-apple-darwin", "ffprobe-aarch64-apple-darwin"),
                ("macos", "x86_64-apple-darwin", "ffmpeg-x86_64-apple-darwin", "ffprobe-x86_64-apple-darwin"),
                ("windows", "x86_64-pc-windows-msvc", "ffmpeg-x86_64-pc-windows-msvc.exe", "ffprobe-x86_64-pc-windows-msvc.exe"),
                ("linux", "x86_64-unknown-linux-gnu", "ffmpeg-x86_64-unknown-linux-gnu", "ffprobe-x86_64-unknown-linux-gnu"),
            )
            with patch.object(package, "ROOT", root):
                for platform, target, ffmpeg, ffprobe in targets:
                    tools = root / "apps/desktop/src-tauri/binaries"
                    tools.mkdir(exist_ok=True)
                    for name in (ffmpeg, ffprobe):
                        (tools / name).write_bytes(name.encode())
                    output = root / f"package é {target}"
                    launcher, executable = package.stage(platform, binary, output, resources, target)
                    for name in (ffmpeg, ffprobe):
                        staged = executable.parent / "binaries" / name
                        self.assertEqual(staged.read_bytes(), name.encode())
                        if os.name != "nt":
                            self.assertTrue(staged.stat().st_mode & 0o111)
                    self.assertEqual(len(list((executable.parent / "binaries").iterdir())), 2)
                    licenses = (launcher / "Contents/Resources" if platform == "macos" else output) / "media-licenses"
                    for name, data in inputs.items():
                        if name.startswith("target/ffmpeg-dist/"):
                            self.assertEqual((licenses / "ffmpeg" / Path(name).name).read_bytes(), data)
                        elif name.startswith("apps/desktop/src-tauri/openh264/"):
                            self.assertEqual((licenses / "openh264" / Path(name).name).read_bytes(), data)
                output = root / "invalid-package"
                with self.assertRaisesRegex(ValueError, "does not match"):
                    package.stage("linux", binary, output, media_target="aarch64-apple-darwin")
                self.assertFalse(output.exists())
                for missing in ("apps/desktop/src-tauri/binaries/ffprobe-x86_64-unknown-linux-gnu",
                                "target/ffmpeg-dist/ffmpeg-9.8.7.tar.xz"):
                    path = root / missing
                    data = path.read_bytes()
                    path.unlink()
                    with self.assertRaisesRegex(ValueError, "Missing media package input"):
                        package.stage("linux", binary, output, media_target="x86_64-unknown-linux-gnu")
                    self.assertFalse(output.exists(), "missing tool/source must fail before staging")
                    path.write_bytes(data)

    @unittest.skipUnless(sys.platform.startswith("linux") and shutil.which("gio"),
                         "requires Linux GIO desktop-entry launch support")
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
