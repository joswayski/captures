#!/usr/bin/python3
"""Exercise the native screenshot editor on disposable private X11/software GL.

Uses an asymmetric History fixture and real pointer/keyboard input, not app hooks.
Requires the same system Python dbus/gi and X11 tools as x11_preview_smoke.py.
"""
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
    parser.add_argument("--appearance", choices=("dark", "light"), default="dark")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env.update(WGPU_BACKEND="gl", WINIT_X11_SCALE_FACTOR="1", XDG_SESSION_TYPE="x11")
    for variable, name in (("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data"),
                           ("XDG_CACHE_HOME", "cache"), ("XDG_RUNTIME_DIR", "runtime")):
        path = output / name
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
        process = subprocess.Popen(command, env=env, stdout=stdout, stderr=stderr)
        children.append(process)
        if announce:
            assert select.select([process.stdout], [], [], 10)[0], f"{name} did not start"
            endpoint = process.stdout.readline().decode().strip()
            assert endpoint, f"{name} failed: inspect its stderr"
            return endpoint
        return process

    def run(*command):
        return subprocess.check_output(command, env=env, timeout=30)

    def windows(title):
        result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name",
                                 f"^{re.escape(title)}"], env=env, capture_output=True,
                                text=True, timeout=5)
        assert result.returncode in (0, 1), result.stderr
        return result.stdout.split()

    def shot(window, name):
        time.sleep(.5)
        run("import", "-window", window, str(output / f"{name}.png"))

    def pixel(name, x, y, expected):
        actual = run("convert", str(output / f"{name}.png"), "-crop", f"1x1+{x}+{y}",
                     "-depth", "8", "rgb:-")
        assert actual == bytes(expected), (name, x, y, actual, expected)

    def wait(predicate, description):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if value := predicate():
                return value
            time.sleep(.05)
        shot("root", "timeout-desktop")
        raise AssertionError(f"Timed out: {description}")

    def click(window, x, y):
        run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
            "sleep", ".2",
            "mousemove", "--sync", "--window", window, str(x - 1), str(y),
            "mousemove_relative", "--sync", "1", "0", "sleep", ".15", "mousedown", "1",
            "sleep", ".15", "mouseup", "1", "sleep", ".2")

    try:
        env["DISPLAY"] = ":" + spawn("xvfb", ["Xvfb", "-displayfd", "1", "-screen", "0",
            "1280x900x24", "-dpi", "96", "-nolisten", "tcp"], True)
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
        time.sleep(.5)
        history = output / "history"
        artifact_id = "10d309cf-83ec-4d45-aab8-87e70948cdea"
        artifact = history / artifact_id
        artifact.mkdir(parents=True)
        run("convert", "-size", "640x360", "xc:#286ea6", "-fill", "#e5b344",
            "-draw", "rectangle 80,60 220,200", "-fill", "#2e9e71",
            "-draw", "rectangle 400,150 620,340", "PNG32:" + str(artifact / "capture.png"))
        original = (artifact / "capture.png").read_bytes()
        (artifact / "preview.png").write_bytes(original)
        (artifact / "metadata.json").write_text(json.dumps({
            "id": artifact_id, "kind": "screenshot", "preview_url": "", "full_url": "",
            "width": 640, "height": 360, "size_bytes": len(original),
            "created_at": datetime.now(timezone.utc).isoformat(), "mode": "region",
            "saved_path": None, "mime_type": "image/png",
        }))
        settings = output / "settings.json"
        settings.write_text(json.dumps({
            "settings_schema_version": 5, "appearance": args.appearance, "theme": "mustard",
            "output_directory": str(output / "exports"), "launch_at_login": False,
            "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
            "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10",
            "auto_copy_to_clipboard": False, "show_mini_previews": False,
        }))
        app = spawn("app", [str(binary), "--live", "--history-root", str(history),
                    "--settings-file", str(settings), "--quit-after", "240"])
        root = wait(lambda: windows("Captures"), "History workspace")[0]
        run("xdotool", "windowmove", "--sync", root, "0", "0")
        time.sleep(1)
        shot(root, "history")
        click(root, 810, 191)
        editor = wait(lambda: windows("Screenshot editor"), "screenshot editor")[0]
        run("xdotool", "windowmove", "--sync", editor, "100", "80")
        time.sleep(1)
        shot(editor, "editor-original")
        pixel("editor-original", 10, 690,
              (245, 245, 247) if args.appearance == "light" else (16, 16, 20))

        def field(y, value):
            click(editor, 78, y)
            run("xdotool", "key", "ctrl+a")
            run("xdotool", "type", "--clearmodifiers", "--delay", "60", str(value))
            run("xdotool", "key", "Return", "sleep", ".2")

        draft = output / "editor-drafts" / artifact_id / "manifest.json"

        def saved(width, height, x, y):
            if not draft.exists():
                return False
            document = json.loads(draft.read_text())["document"]
            layer = document["elements"][0]
            return (document["width"], document["height"], layer["x"], layer["y"]) == (width, height, x, y)

        def save(width, height, x, y):
            click(editor, 170, 62)
            wait(lambda: saved(width, height, x, y), f"saved {width}x{height} at {x},{y}")

        for y, value in ((159, 40), (203, 30), (247, 360), (291, 240)):
            field(y, value)
        click(editor, 50, 335)
        save(360, 240, -40, -30)
        shot(editor, "editor-cropped")
        pixel("editor-cropped", 900, 400, (40, 110, 166))
        pixel("editor-cropped", 350, 200, (229, 179, 68))
        click(editor, 35, 62)  # Undo
        save(640, 360, 0, 0)
        click(editor, 98, 62)  # Redo
        save(360, 240, -40, -30)
        field(428, 480)
        field(472, 300)
        click(editor, 58, 516)
        save(480, 300, -40, -30)
        shot(editor, "editor-resized")
        pixel("editor-resized", 900, 400, (46, 158, 113))

        def close(window):
            run("xdotool", "windowactivate", "--sync", window, "key", "alt+F4", "sleep", ".4")

        def reopen():
            click(root, 810, 191)
            window = wait(lambda: windows("Screenshot editor"), "reopened editor")[0]
            run("xdotool", "windowmove", "--sync", window, "100", "80")
            time.sleep(.6)
            return window

        close(editor)
        wait(lambda: not windows("Screenshot editor"), "saved editor closes")
        editor = reopen()
        shot(editor, "editor-reopened")
        pixel("editor-reopened", 900, 400, (46, 158, 113))
        field(428, 510)
        click(editor, 58, 516)
        close(editor)
        shot(editor, "editor-unsaved-close")
        click(editor, 200, 128)  # Close without saving retains the previous draft.
        wait(lambda: not windows("Screenshot editor"), "unsaved editor closes")
        assert saved(480, 300, -40, -30)
        editor = reopen()
        click(editor, 275, 62)
        shot(editor, "editor-discard-confirmation")
        click(editor, 55, 128)
        wait(lambda: not draft.exists(), "explicit discard removes the saved draft")
        shot(editor, "editor-discarded")

        # Force a genuine filesystem failure without replacing a user's data.
        drafts = output / "editor-drafts"
        drafts.rename(output / "previous-drafts")
        drafts.write_text("blocks directory creation")
        field(428, 500)
        click(editor, 58, 516)
        click(editor, 170, 62)
        shot(editor, "editor-save-error")
        assert app.poll() is None and windows("Screenshot editor")
        close(root)  # With no tray host, normal root close must flush or cancel.
        assert app.poll() is None and windows("Screenshot editor"), "failed flush must cancel quit"
        shot(editor, "editor-quit-error")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "editor-small-error")
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "8", "5")
        shot(editor, "editor-small-scrolled")
        drafts.unlink()
        close(root)
        wait(lambda: app.poll() is not None, "retry quit saves draft and exits")
        assert app.returncode == 0
        assert saved(500, 360, 0, 0)
        assert (artifact / "capture.png").read_bytes() == original
        (output / "result.json").write_text(json.dumps({
            "passed": True, "appearance": args.appearance,
            "checks": ["crop", "canvas", "undo-redo", "draft-reopen", "close-preserves-draft",
                       "explicit-discard", "save-error", "quit-error-retry", "original-unchanged"],
            "originalSha256": hashlib.sha256(original).hexdigest(),
        }, indent=2) + "\n")
        print("PASS native editor: crop, canvas, undo/redo, draft reopen, close/discard, save/quit recovery, original unchanged")
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
