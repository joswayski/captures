#!/usr/bin/env python3
"""Exercise real native-process forwarding with disposable History on any host.

Requires a graphical session; exercises media opening, not desktop capture.
Linux callers supply a private X11/Wayland session. This is not physical acceptance.
"""
import argparse
import json
import os
from pathlib import Path
import struct
import subprocess
import time
import zlib


def png(width, height):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    pixels = b"".join(b"\0" + b"".join(bytes((x * 13 % 256, y * 29 % 256, 91))
                                    for x in range(width)) for y in range(height))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(pixels)) + chunk(b"IEND", b""))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    history = output / "history"
    sender = output / "sender"
    sender.mkdir()
    source = sender / "Capture é with spaces.png"
    source.write_bytes(png(13, 7))
    original = source.read_bytes()
    settings = output / "settings.json"
    settings.write_text(json.dumps({"settings_schema_version": 5,
        "appearance": "dark", "theme": "mustard", "output_directory": str(output / "exports"),
        "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
        "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10",
        "launch_at_login": False, "show_mini_previews": False, "auto_copy_to_clipboard": False}))
    common = [str(binary), "--live", "--history-root", str(history)]
    env = os.environ.copy()
    # The application's normal stdout logger must be visible before normal quit.
    env["NSUnbufferedIO"] = "YES"

    def wait(predicate, description, seconds=20):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            assert primary.poll() is None, f"primary exited early: {log.read_text(encoding='utf-8', errors='replace')}"
            value = predicate()
            if value:
                return value
            time.sleep(.05)
        raise AssertionError(f"Timed out: {description}; {log.read_text(encoding='utf-8', errors='replace')}")

    def events():
        values = []
        for line in log.read_text(encoding="utf-8", errors="replace").splitlines():
            try:
                values.append(json.loads(line))
            except json.JSONDecodeError:
                pass
        return values

    def forward(*paths):
        unused = output / "must-not-create-settings.json"
        command = common + ["--settings-file", str(unused)]
        for path in paths:
            command += ["--open-media", path]
        result = subprocess.run(command, cwd=sender, env=env, capture_output=True,
                                encoding="utf-8", timeout=10)
        assert result.returncode == 0, result.stderr
        assert not unused.exists(), "secondary wrote settings"
        for line in result.stdout.splitlines():
            try:
                assert json.loads(line).get("event") not in ("ready", "starting"), "secondary initialized UI"
            except json.JSONDecodeError:
                pass

    log = output / "primary.log"
    with log.open("w") as stream:
        primary = subprocess.Popen(common + ["--settings-file", str(settings), "--quit-after", "30"],
                                   env=env, stdout=stream, stderr=subprocess.STDOUT)
        try:
            wait(lambda: any(event.get("event") == "ready" for event in events()), "native readiness")
            forward(source.name)

            def imported():
                entries = list(history.glob("*/metadata.json"))
                if len(entries) == 1:
                    try:
                        return entries[0], json.loads(entries[0].read_text(encoding="utf-8"))
                    except (OSError, json.JSONDecodeError):
                        pass
                return None

            metadata, entry = wait(imported, "forwarded media imported by resident host")
            assert (entry["width"], entry["height"]) == (13, 7)
            assert Path(entry["saved_path"]).samefile(source)
            assert (metadata.parent / "capture.png").is_file()
            forward("./" + source.name)  # canonical alias, no second History item
            forward()  # focus-only relaunch
            wait(lambda: any(event.get("event") == "instance-relaunch" for event in events()),
                 "resident root handled relaunch after editor opened")
            assert len(list(history.glob("*/metadata.json"))) == 1
            assert source.read_bytes() == original
            assert primary.wait(timeout=35) == 0, log.read_text(encoding="utf-8", errors="replace")
        finally:
            if primary.poll() is None:
                primary.kill()
                primary.wait()

    # Normal shutdown releases election and endpoint before a fresh process starts.
    result = subprocess.run(common + ["--settings-file", str(settings), "--quit-after", "3"],
                            env=env, capture_output=True, encoding="utf-8", timeout=20)
    (output / "restart.log").write_text(result.stdout + result.stderr, encoding="utf-8")
    assert result.returncode == 0, result.stderr
    assert any(json.loads(line).get("event") == "ready" for line in result.stdout.splitlines()
               if line.startswith("{")), "restart did not become the primary"
    checks = ["secondary-exits-before-ui", "secondary-does-not-write-settings", "sender-relative-path",
              "resident-history-import", "canonical-alias-no-duplicate", "source-bytes-unchanged",
              "focus-only-request-handled-by-host", "normal-quit-and-primary-restart"]
    (output / "result.json").write_text(json.dumps({"passed": True, "checks": checks}, indent=2) + "\n")
    print(f"PASS native instance forwarding: {len(checks)} checks")


if __name__ == "__main__":
    main()
