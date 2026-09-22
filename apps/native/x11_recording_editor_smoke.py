#!/usr/bin/python3
"""Real frame/trim/export input on a disposable private X11 desktop."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import select
import subprocess
import threading
import time

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

from x11_capture_smoke import ScreenSaver


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--appearance", choices=("light", "dark"), default="dark")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env.update(WGPU_BACKEND="gl", WINIT_X11_SCALE_FACTOR="1", XDG_SESSION_TYPE="x11")
    for variable in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR"):
        path = output / variable.lower()
        path.mkdir(mode=0o700)
        env[variable] = str(path)
    children, logs = [], []
    loop = None

    def spawn(name, command, announce=False):
        stdout = subprocess.PIPE if announce else (output / f"{name}.jsonl").open("w")
        stderr = (output / f"{name}.stderr.txt").open("w")
        logs.append(stderr)
        if not announce:
            logs.append(stdout)
        child = subprocess.Popen(command, env=env, stdout=stdout, stderr=stderr)
        children.append(child)
        if announce:
            assert select.select([child.stdout], [], [], 10)[0], name
            return child.stdout.readline().decode().strip()
        return child

    def run(*command):
        return subprocess.check_output(command, env=env, timeout=40)

    def windows(title):
        found = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", f"^{re.escape(title)}"],
                               env=env, capture_output=True, text=True, timeout=5)
        assert found.returncode in (0, 1), found.stderr
        return found.stdout.split()

    def shot(window, name):
        time.sleep(.5)
        run("import", "-window", window, str(output / f"{name}.png"))

    def wait(predicate, message):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if value := predicate():
                return value
            time.sleep(.1)
        shot("root", "timeout-desktop")
        raise AssertionError(message)

    def click(window, x, y):
        wait(lambda: "Working…" not in run("xdotool", "getwindowname", window).decode(), "worker idle")
        run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
            "sleep", ".2", "mousemove", "--sync", "--window", window, str(x), str(y),
            "sleep", ".2", "mousedown", "1", "sleep", ".15", "mouseup", "1", "sleep", ".3")

    def field(window, x, y, value):
        click(window, x, y)
        run("xdotool", "key", "ctrl+a")
        run("xdotool", "type", "--clearmodifiers", "--delay", "35", "--", str(value))
        run("xdotool", "key", "Return", "sleep", ".3")

    def close(window):
        run("xdotool", "windowactivate", "--sync", window, "key", "alt+F4", "sleep", ".5")

    def dominant(path, channel, at=None):
        if at is None:
            pixel = run("convert", str(path), "-crop", "1x1+480+220", "-depth", "8", "rgb:-")
        else:
            pixel = run("ffmpeg", "-v", "error", "-ss", str(at), "-i", str(path),
                        "-frames:v", "1", "-vf", "crop=2:2:160:90", "-f", "rawvideo", "-pix_fmt", "rgb24", "-")[:3]
        assert len(pixel) == 3 and pixel[channel] > 90, (path, pixel)
        assert all(pixel[channel] > pixel[i] + 40 for i in range(3) if i != channel), (path, pixel)

    try:
        env["DISPLAY"] = ":" + spawn("xvfb", ["Xvfb", "-displayfd", "1", "-screen", "0", "1280x1000x24", "-dpi", "96", "-nolisten", "tcp"], True)
        address = spawn("dbus", ["dbus-daemon", "--session", "--nofork", "--print-address=1"], True)
        env["DBUS_SESSION_BUS_ADDRESS"] = env["DBUS_SYSTEM_BUS_ADDRESS"] = address
        DBusGMainLoop(set_as_default=True)
        bus = dbus.bus.BusConnection(address)
        name = dbus.service.BusName("org.freedesktop.ScreenSaver", bus=bus, do_not_queue=True)
        saver = ScreenSaver(name, "/org/freedesktop/ScreenSaver")
        loop = GLib.MainLoop()
        thread = threading.Thread(target=loop.run, daemon=True)
        thread.start()
        spawn("openbox", ["openbox", "--sm-disable"])
        history = output / "history"
        artifact_id = "032135f1-11e4-4a47-893d-2368c079a6ba"
        artifact = history / artifact_id
        artifact.mkdir(parents=True)
        source = output / "source.mp4"
        run("ffmpeg", "-v", "error", "-f", "lavfi", "-i", "color=red:s=320x180:r=10:d=1",
            "-f", "lavfi", "-i", "color=green:s=320x180:r=10:d=1",
            "-f", "lavfi", "-i", "color=blue:s=320x180:r=10:d=1",
            "-filter_complex", "[0:v][1:v][2:v]concat=n=3:v=1:a=0,drawbox=x=15:y=10:w=45:h=25:color=white:t=fill[v]",
            "-map", "[v]", "-c:v", "mpeg4", "-q:v", "2", str(source))
        run("ffmpeg", "-v", "error", "-i", str(source), "-frames:v", "1", str(artifact / "preview.png"))
        metadata = artifact / "metadata.json"
        metadata.write_text(json.dumps({
            "id": artifact_id, "kind": "video", "preview_url": "", "full_url": "",
            "width": 320, "height": 180, "size_bytes": source.stat().st_size,
            "created_at": datetime.now(timezone.utc).isoformat(), "mode": None,
            "saved_path": str(source), "mime_type": "video/mp4", "duration_ms": 3000,
            "target": {"type": "display", "display_id": "fixture"},
            "has_system_audio": False, "has_microphone_audio": False, "dropped_frames": 0,
        }))
        original, original_metadata = source.read_bytes(), metadata.read_bytes()
        exports = output / "exports"
        exports.mkdir()
        settings = output / "settings.json"
        settings.write_text(json.dumps({"settings_schema_version": 5, "appearance": args.appearance,
            "theme": "mustard", "output_directory": str(exports), "launch_at_login": False,
            "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
            "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10",
            "auto_copy_to_clipboard": False, "show_mini_previews": False}))
        app = spawn("app", [str(binary), "--live", "--history-root", str(history),
                    "--settings-file", str(settings), "--quit-after", "600"])
        root = wait(lambda: windows("Captures"), "History")[0]
        time.sleep(1)
        shot(root, "history")
        click(root, 795, 191)
        editor = wait(lambda: windows("Recording editor"), "recording editor opens")[0]
        run("xdotool", "windowmove", "--sync", editor, "80", "60")
        run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
        wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(), "decode")
        shot(editor, "original")
        dominant(output / "original.png", 0)
        run("xdotool", "windowminimize", root, "sleep", ".5")
        # Numeric fields exercise exact source-relative times, independently of
        # slider geometry and the trim start.
        field(editor, 136, 520, 1500)
        click(editor, 222, 520)
        shot(editor, "seek-green")
        dominant(output / "seek-green.png", 1)
        field(editor, 211, 598, 1100)
        field(editor, 98, 598, 2600)
        click(editor, 400, 598)
        shot(editor, "invalid-trim")
        dominant(output / "invalid-trim.png", 1)
        run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
        shot(editor, "minimum-error")
        run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
        field(editor, 98, 598, 1100)
        field(editor, 227, 598, 2300)
        click(editor, 400, 598)
        shot(editor, "trimmed")
        dominant(output / "trimmed.png", 1)
        close(root)
        assert app.poll() is None and windows("Recording editor"), "dirty editor blocks quit"
        shot(editor, "quit-guard")
        close(editor)
        assert windows("Recording editor"), "dirty editor requires confirmation"
        shot(editor, "close-confirmation")
        click(editor, 55, 794)  # Keep editing, immediately above the fixed destination row.
        run("xdotool", "windowminimize", root, "sleep", ".5")
        destination = exports / "trimmed.mp4"
        field(editor, 360, 838, destination)
        click(editor, 899, 882)
        wait(lambda: len(list(history.glob("*/metadata.json"))) == 2, "export published in History")
        info = json.loads(run("ffprobe", "-v", "error", "-show_format", "-show_streams", "-of", "json", str(destination)))
        assert abs(float(info["format"]["duration"]) - 1.2) < .15, info
        assert (info["streams"][0]["width"], info["streams"][0]["height"]) == (320, 180)
        dominant(destination, 1, .2)
        dominant(destination, 2, 1.0)
        shot(editor, "saved")
        saved_bytes = destination.read_bytes()
        click(editor, 899, 882)
        shot(editor, "collision")
        assert destination.read_bytes() == saved_bytes and len(list(history.glob("*/metadata.json"))) == 2
        click(editor, 87, 882)  # GIF changes only the export format/path, not the edit.
        click(editor, 899, 882)
        wait(lambda: len(list(history.glob("*/metadata.json"))) == 3, "GIF published in History")
        gif = destination.with_suffix(".gif")
        dominant(gif, 1, .2)
        dominant(gif, 2, 1.0)
        shot(editor, "gif-saved")
        run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
        shot(editor, "minimum-saved")
        assert source.read_bytes() == original and metadata.read_bytes() == original_metadata
        close(editor)
        wait(lambda: not windows("Recording editor"), "saved editor closes")
        close(root)
        wait(lambda: app.poll() is not None, "quit")
        assert app.returncode == 0
        (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
            "checks": ["history-open", "decoded-frame", "source-relative-seek", "trim-preview",
                "failed-trim-retains-frame", "minimum-error", "dirty-quit-guard", "close-confirmation",
                "save-new", "duration", "dimensions", "export-green", "export-blue", "collision",
                "gif-green", "gif-blue", "minimum-saved", "immutable-source", "saved-close-and-quit",
                "worker-completion-with-minimized-root"],
            "source_sha256": hashlib.sha256(original).hexdigest()}, indent=2) + "\n")
        print("PASS recording editor: decoded seeks, trim, MP4 duration/content, History, immutable source")
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
        if loop:
            loop.quit()
            thread.join(timeout=5)
        for log in logs:
            log.close()


if __name__ == "__main__":
    main()
