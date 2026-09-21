#!/usr/bin/python3
"""Exercise the native screenshot editor on disposable private X11/software GL.

Uses an asymmetric History fixture and real pointer/keyboard input, not app hooks.
Requires the same system Python dbus/gi and X11 tools as x11_preview_smoke.py.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
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


class FileRequest(dbus.service.Object):
    @dbus.service.signal("org.freedesktop.portal.Request", signature="ua{sv}")
    def Response(self, code, results):
        pass


class FileChooser(dbus.service.Object):
    """Disposable file/folder portal transport fixture, not a physical dialog."""

    def __init__(self, bus, selected):
        self.name = dbus.service.BusName("org.freedesktop.portal.Desktop", bus=bus)
        super().__init__(self.name, "/org/freedesktop/portal/desktop")
        self.selected = selected
        self.calls, self.requests = [], []
        self.pending = None

    @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="ss", out_signature="v")
    def Get(self, interface, property_name):
        assert interface == "org.freedesktop.portal.FileChooser" and property_name == "version"
        return dbus.UInt32(3)

    @dbus.service.method("org.freedesktop.portal.FileChooser", in_signature="ssa{sv}",
                         out_signature="o", sender_keyword="sender")
    def OpenFile(self, parent, title, options, sender):
        self.calls.append((title, options))
        owner = sender.removeprefix(":").replace(".", "_")
        path = f"/org/freedesktop/portal/desktop/request/{owner}/{options['handle_token']}"
        request = FileRequest(self.name, path)
        self.requests.append(request)
        self.pending = request
        return dbus.ObjectPath(path)

    def respond(self, cancel):
        self.pending.Response(1 if cancel else 0, {} if cancel else {
            "uris": dbus.Array([self.selected.as_uri()], signature="s"),
        })
        self.pending = None
        return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--appearance", choices=("dark", "light"), default="dark")
    parser.add_argument("--background-only", action="store_true",
                        help="Exercise canvas background controls, alpha and draft reopen only")
    parser.add_argument("--wand-only", action="store_true",
                        help="Exercise image-background removal, undo, draft and clipboard alpha")
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

    def pixel(name, x, y, expected, tolerance=0):
        actual = run("convert", str(output / f"{name}.png"), "-crop", f"1x1+{x}+{y}",
                     "-depth", "8", "rgb:-")
        assert len(actual) == 3 and all(abs(a - b) <= tolerance for a, b in zip(actual, expected)), (name, x, y, actual, expected)

    def wait(predicate, description):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if value := predicate():
                return value
            time.sleep(.05)
        shot("root", "timeout-desktop")
        raise AssertionError(f"Timed out: {description}")

    def click(window, x, y):
        wait(lambda: "Working…" not in run("xdotool", "getwindowname", window).decode(),
             "window ready for input")
        run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
            "sleep", ".2",
            "mousemove", "--sync", "--window", window, str(x - 1), str(y),
            "mousemove_relative", "--sync", "1", "0", "sleep", ".15", "mousedown", "1",
            "sleep", ".15", "mouseup", "1", "sleep", ".2")

    def drag(start, end, shift=False):
        run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, start), "sleep", ".2")
        if shift:
            run("xdotool", "keydown", "Shift_L", "sleep", ".1")
        run("xdotool", "mousedown", "1", "sleep", ".2", "mousemove", "--sync",
            "--window", editor, *map(str, end), "sleep", ".3", "mouseup", "1", "sleep", ".2")
        if shift:
            run("xdotool", "keyup", "Shift_L")

    try:
        env["DISPLAY"] = ":" + spawn("xvfb", ["Xvfb", "-displayfd", "1", "-screen", "0",
            "1280x900x24", "-dpi", "96", "-nolisten", "tcp"], True)
        address = spawn("dbus", ["dbus-daemon", "--session", "--nofork", "--print-address=1"], True)
        env["DBUS_SESSION_BUS_ADDRESS"] = env["DBUS_SYSTEM_BUS_ADDRESS"] = address
        DBusGMainLoop(set_as_default=True)
        bus = dbus.bus.BusConnection(address)
        name = dbus.service.BusName("org.freedesktop.ScreenSaver", bus=bus, do_not_queue=True)
        saver = ScreenSaver(name, "/org/freedesktop/ScreenSaver")
        imported_path = output / "Imported sample é.png"
        chooser = FileChooser(bus, imported_path)
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
        if args.wand_only:
            run("convert", str(artifact / "capture.png"), "-fill", "#e5b344",
                "-draw", "rectangle 300,50 320,70", "PNG32:" + str(artifact / "capture.png"))
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
                    "--settings-file", str(settings), "--quit-after", "600"])
        root = wait(lambda: windows("Captures"), "History workspace")[0]
        run("xdotool", "windowmove", "--sync", root, "0", "0")
        time.sleep(1)
        shot(root, "history")
        click(root, 810, 191)
        editor = wait(lambda: windows("Screenshot editor"), "screenshot editor")[0]
        run("xdotool", "windowmove", "--sync", editor, "100", "80")
        time.sleep(1)
        shot(editor, "editor-original")
        pixel("editor-original", 750, 690,
              (245, 245, 247) if args.appearance == "light" else (16, 16, 20))

        def field(y, value, x=78):
            click(editor, x, y)
            run("xdotool", "key", "ctrl+a")
            run("xdotool", "type", "--clearmodifiers", "--delay", "60", "--", str(value))
            run("xdotool", "key", "Return", "sleep", ".2")

        draft = output / "editor-drafts" / artifact_id / "manifest.json"

        def saved(width, height, x, y):
            if not draft.exists():
                return False
            document = json.loads(draft.read_text())["document"]
            layer = document["elements"][0]
            return (document["width"], document["height"], layer["x"], layer["y"]) == (width, height, x, y)

        def save_until(predicate, description):
            def attempt():
                # Save is idempotent; an edit can still be queued while X11
                # exposes the prior idle title. Never retry a non-idempotent edit.
                click(editor, 170, 62)
                return predicate()
            wait(attempt, description)
            # The manifest is written before the worker's UI snapshot arrives.
            # Do not type into fields that that snapshot is about to repopulate.
            wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
                 "saved snapshot presented")

        def save(width, height, x, y):
            save_until(lambda: saved(width, height, x, y), f"saved {width}x{height} at {x},{y}")

        def close(window):
            run("xdotool", "windowactivate", "--sync", window, "key", "alt+F4", "sleep", ".4")

        def reopen():
            click(root, 810, 191)
            window = wait(lambda: windows("Screenshot editor"), "reopened editor")[0]
            run("xdotool", "windowmove", "--sync", window, "100", "80")
            time.sleep(.6)
            return window

        def layers():
            return json.loads(draft.read_text())["document"]["elements"] if draft.exists() else []

        def save_layers(predicate, description):
            save_until(lambda: predicate(layers()), description)
            return layers()

        if args.wand_only:
            run("xdotool", "windowsize", "--sync", editor, "886", "700")
            save(640, 360, 0, 0)
            source = layers()[0]["src"]
            click(editor, 736, 62)
            click(editor, 150, 182)
            shot(editor, "wand-controls")
            # Fitted image origin (238,89), scale 1. Original capture stays locked.
            click(editor, 338, 189)
            edited = save_layers(lambda values: values[0]["src"] != source, "wand edit")[0]
            assert edited["locked"] and edited["originalSrc"] == source

            def asset_pixel(layer, x, y, expected):
                asset = draft.parent / "assets" / (layer["src"].split(":", 1)[1] + ".png")
                assert run("convert", str(asset), "-crop", f"1x1+{x}+{y}",
                           "-depth", "8", "rgba:-") == bytes(expected)

            asset_pixel(edited, 100, 100, (0, 0, 0, 0))
            asset_pixel(edited, 310, 60, (229, 179, 68, 255))
            asset_pixel(edited, 2, 1, (40, 110, 166, 255))
            shot(editor, "wand-contiguous")
            click(editor, 338, 189)  # Transparent seed fails; must not create a new asset.
            shot(editor, "wand-no-match")
            save_layers(lambda values: values[0]["src"] == edited["src"], "no-match preserves pixels")
            click(editor, 35, 62)
            save_layers(lambda values: values[0]["src"] == source, "wand undo")
            click(editor, 98, 62)
            save_layers(lambda values: values[0]["src"] == edited["src"], "wand redo")
            click(editor, 35, 62)
            save_layers(lambda values: values[0]["src"] == source, "undo before global removal")
            click(editor, 128, 220)  # Disable Contiguous.
            click(editor, 338, 189)
            global_edit = save_layers(lambda values: values[0]["src"] != source, "global wand")[0]
            asset_pixel(global_edit, 100, 100, (0, 0, 0, 0))
            asset_pixel(global_edit, 310, 60, (0, 0, 0, 0))
            assert global_edit["originalSrc"] == source
            shot(editor, "wand-global")
            close(editor)
            wait(lambda: not windows("Screenshot editor"), "wand draft closes")
            editor = reopen()
            asset_pixel(layers()[0], 310, 60, (0, 0, 0, 0))
            run("xdotool", "windowsize", "--sync", editor, "886", "700")
            click(editor, 736, 62)
            click(editor, 150, 182)
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, "wand-minimum-reopened")
            run("xdotool", "windowsize", "--sync", editor, "1000", "800")
            click(editor, 535, 62)
            click(editor, 170, 657)
            wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
                 "wand clipboard copy")
            png = output / "clipboard-wand.png"
            png.write_bytes(run("xclip", "-selection", "clipboard", "-t", "image/png", "-o"))
            assert run("identify", "-format", "%wx%h", str(png)) == b"640x360"
            assert run("convert", str(png), "-crop", "1x1+310+60", "-depth", "8", "rgba:-") == bytes((0, 0, 0, 0))
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "wand suite quits")
            assert app.returncode == 0
            checks = ["wand-locked-contiguous-exact-pixels", "wand-no-match-preserves-state",
                      "wand-undo-redo", "wand-global-disconnected-pixels", "wand-retains-original",
                      "wand-minimum-draft-reopen", "wand-clipboard-alpha-original-unchanged"]
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance, "checks": checks,
            }, indent=2) + "\n")
            print("PASS native Wand: locked image, contiguous/global, rollback, undo/redo, draft, clipboard alpha")
            return

        if args.background_only:
            run("xdotool", "windowsize", "--sync", editor, "1000", "800")
            field(428, 720)
            field(472, 420)
            click(editor, 58, 516)
            save(720, 420, 0, 0)
            before = draft.read_bytes()
            shot(editor, "background-controls")
            field(633, "#214365", x=95)
            assert draft.read_bytes() == before, "unapplied fields must not edit the draft"
            click(editor, 75, 670)

            def background_is(color):
                return json.loads(draft.read_text())["document"]["background"] == color

            save_until(lambda: background_is("#214365"), "solid canvas background")
            shot(editor, "background-solid")
            pixel("background-solid", 960, 500, (33, 67, 101))
            pixel("background-solid", 270, 120, (40, 110, 166))
            field(633, "invalid", x=95)
            click(editor, 75, 670)
            shot(editor, "background-error")
            assert background_is("#214365")
            pixel("background-error", 960, 500, (33, 67, 101))
            # The error row adds 27px below the toolbar until the next command.
            click(editor, 20, 595 + 27)
            click(editor, 75, 670 + 27)
            save_until(lambda: background_is(None), "transparent canvas background")
            shot(editor, "background-transparent")
            click(editor, 35, 62)
            save_until(lambda: background_is("#214365"), "undo canvas background")
            click(editor, 98, 62)
            save_until(lambda: background_is(None), "redo canvas background")
            close(editor)
            wait(lambda: not windows("Screenshot editor"), "background draft closes")
            editor = reopen()
            assert background_is(None)
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "12", "5")
            shot(editor, "background-transparent-minimum-reopened")
            run("xdotool", "windowsize", "--sync", editor, "1000", "800")
            click(editor, 535, 62)
            click(editor, 65, 366)  # Preview PNG, then copy the edited frame.
            click(editor, 170, 657)
            wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
                 "transparent clipboard copy completes")
            shot(editor, "background-output-copied")
            png = output / "clipboard-background.png"
            png.write_bytes(run("xclip", "-selection", "clipboard", "-t", "image/png", "-o"))
            assert run("identify", "-format", "%wx%h", str(png)) == b"720x420"
            for x, y, expected in ((700, 400, (0, 0, 0, 0)), (2, 1, (40, 110, 166, 255))):
                assert run("convert", str(png), "-crop", f"1x1+{x}+{y}", "-depth", "8", "rgba:-") == bytes(expected)
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "background suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["background-unapplied-no-write", "background-solid-pixels",
                           "background-invalid-rollback", "background-transparent-undo-redo",
                           "background-minimum-draft-reopen", "background-clipboard-alpha-original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native canvas backgrounds: color, rollback, transparency, undo/redo, draft, clipboard alpha")
            return

        run("xdotool", "windowsize", "--sync", editor, "886", "700")
        # Viewport state is host-only. Exercise anchored wheel zoom and an
        # ordered middle-button pan before the coordinate-sensitive fixtures.
        shot(editor, f"viewport-before-{args.appearance}")
        pixel(f"viewport-before-{args.appearance}", 310, 200, (40, 110, 166))
        run("xdotool", "mousemove", "--sync", "--window", editor, "520", "250",
            "keydown", "ctrl", "click", "4", "click", "4", "keyup", "ctrl", "sleep", ".3")
        shot(editor, f"viewport-zoom-{args.appearance}")
        pixel(f"viewport-zoom-{args.appearance}", 310, 200, (229, 179, 68))
        assert not draft.exists(), "zoom must not create a draft"
        run("xdotool", "mousemove", "--sync", "--window", editor, "520", "250",
            "mousedown", "2", "mousemove", "--sync", "--window", editor, "585", "290",
            "sleep", ".3")
        shot(editor, f"viewport-pan-active-{args.appearance}")
        assert not draft.exists(), "active pan must not enqueue an edit"
        run("xdotool", "mouseup", "2", "sleep", ".3")
        shot(editor, f"viewport-pan-settled-{args.appearance}")
        pixel(f"viewport-pan-settled-{args.appearance}", 310, 200, (40, 110, 166))
        pixel(f"viewport-pan-settled-{args.appearance}", 490, 200, (229, 179, 68))
        assert not draft.exists(), "settled pan must not enqueue an edit"
        # Recenter keeps zoom; Fit restores the historical fixture geometry.
        click(editor, 840, 25)
        shot(editor, f"viewport-recenter-{args.appearance}")
        click(editor, 630, 18)
        shot(editor, f"viewport-fit-{args.appearance}")
        pixel(f"viewport-fit-{args.appearance}", 340, 200, (229, 179, 68))
        pixel(f"viewport-fit-{args.appearance}", 310, 200, (40, 110, 166))
        click(editor, 686, 18)  # 100%
        click(editor, 778, 18)  # + (1.25x)
        assert not draft.exists(), "toolbar zoom must remain outside draft state"
        click(editor, 630, 18)  # Fit also cancels any viewport gesture and restores coordinates.
        click(editor, 736, 62)
        click(editor, 95, 176)  # Pen follows Arrow on the second tool row.
        run("xdotool", "mousemove", "--window", editor, "318", "329", "mousedown", "1", "sleep", ".2")
        for x, y in [(378, 209), (438, 329), (518, 249)]:
            run("xdotool", "mousemove", "--sync", "--window", editor, str(x), str(y), "sleep", ".2")
        shot(editor, "freehand-transient")
        assert not draft.exists()
        # First quadratic at t=1/2: (132.5,165) document pixels. The control
        # point (140,120) must remain unpainted, unlike an unsmoothed polyline.
        pixel("freehand-transient", 371, 254, (255, 59, 92))
        pixel("freehand-transient", 378, 209, (229, 179, 68))
        pixel("freehand-transient", 316, 331, (255, 59, 92))  # round start cap
        pixel("freehand-transient", 520, 247, (255, 59, 92))  # round end cap
        run("xdotool", "mouseup", "1", "sleep", ".3")
        curve = save_layers(lambda values: len(values) == 2, "freehand curve")[-1]
        assert curve["kind"] == "path" and curve["style"]["fill"] is None
        assert len(curve["points"]) == 4
        for point, expected in zip(curve["points"], [(80, 240), (140, 120), (200, 240), (280, 160)]):
            assert abs(point["x"] - expected[0]) < 1e-12 and abs(point["y"] - expected[1]) < 1e-12
        shot(editor, "freehand-curve")
        pixel("freehand-curve", 371, 254, (255, 59, 92))
        pixel("freehand-curve", 378, 209, (229, 179, 68))
        pixel("freehand-curve", 316, 331, (255, 59, 92))
        pixel("freehand-curve", 520, 247, (255, 59, 92))
        drag((700, 420), (701, 420))  # Under 1.5 screen pixels: keep only the press sample.
        dot = save_layers(lambda values: len(values) == 3, "freehand one-point dot")[-1]
        assert dot["kind"] == "path" and len(dot["points"]) == 1
        shot(editor, "freehand-dot")
        pixel("freehand-dot", 700, 420, (255, 59, 92))
        click(editor, 35, 62)
        save_layers(lambda values: len(values) == 2, "freehand dot undo")
        before_cancel = draft.read_bytes()
        run("xdotool", "mousemove", "--window", editor, "400", "400", "mousedown", "1",
            "sleep", ".2", "mousemove", "--sync", "--window", editor, "460", "410", "sleep", ".2",
            "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
        assert draft.read_bytes() == before_cancel
        click(editor, 98, 62)
        assert save_layers(lambda values: len(values) == 3, "cancel preserves freehand redo")[-1]["id"] == dot["id"]
        drag((320, 500), (420, 560))
        save_layers(lambda values: len(values) == 4, "outside freehand stroke")
        assert saved(640, 479, 0, 0)  # authored sample max y=471 plus shipping 8px padding
        shot(editor, "freehand-outside")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "freehand-minimum")
        close(editor)
        wait(lambda: not windows("Screenshot editor"), "freehand editor closes")
        editor = reopen()
        run("xdotool", "windowsize", "--sync", editor, "886", "700")
        shot(editor, "freehand-reopened")
        pixel("freehand-reopened", 371, 254, (255, 59, 92))
        assert layers()[1]["id"] == curve["id"] and layers()[1]["points"] == curve["points"]
        assert (artifact / "capture.png").read_bytes() == original
        click(editor, 275, 62)
        click(editor, 55, 128)
        wait(lambda: not draft.exists(), "discard freehand edits")

        run("xdotool", "windowsize", "--sync", editor, "886", "700")
        click(editor, 736, 62)
        shot(editor, "open-shape-tools")
        click(editor, 190, 132)  # Line follows Rectangle and Ellipse.
        drag((320, 310), (500, 310))
        horizontal = save_layers(lambda values: len(values) == 2, "horizontal line")[-1]
        assert horizontal["shape"] == "line" and horizontal["style"]["fill"] is None
        assert (horizontal["x"], horizontal["y"], horizontal["endX"], horizontal["endY"]) == (82, 221, 262, 221)
        shot(editor, "open-shape-horizontal")
        pixel("open-shape-horizontal", 400, 310, (255, 59, 92))
        pixel("open-shape-horizontal", 400, 300, (40, 110, 166))
        drag((540, 360), (540, 200))
        vertical = save_layers(lambda values: len(values) == 3, "reverse vertical line")[-1]
        assert (vertical["x"], vertical["y"], vertical["endX"], vertical["endY"]) == (302, 271, 302, 111)
        click(editor, 700, 420)
        point_line = save_layers(lambda values: len(values) == 4, "zero-length line click")[-1]
        assert (point_line["x"], point_line["y"]) == (point_line["endX"], point_line["endY"])
        click(editor, 40, 176)  # Arrow wraps onto the next row.
        before_arrow = draft.read_bytes()
        run("xdotool", "mousemove", "--window", editor, "750", "320", "mousedown", "1",
            "sleep", ".2", "mousemove", "--sync", "--window", editor, "580", "190", "sleep", ".3")
        shot(editor, "open-shape-arrow-transient")
        pixel("open-shape-arrow-transient", 665, 255, (255, 59, 92))
        assert draft.read_bytes() == before_arrow
        run("xdotool", "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
        save_layers(lambda values: len(values) == 4, "arrow Escape cancellation")
        drag((750, 320), (580, 190))
        shot(editor, "open-shape-arrow-result")
        arrow = save_layers(lambda values: len(values) == 5, "reverse diagonal arrow")[-1]
        assert arrow["shape"] == "arrow" and arrow["controls"] == []
        assert all(abs(actual - expected) < 1e-12 for actual, expected in zip(
            (arrow["x"], arrow["y"], arrow["endX"], arrow["endY"]), (512, 231, 342, 101)))
        shot(editor, "open-shape-arrow")
        pixel("open-shape-arrow", 665, 255, (255, 59, 92))
        pixel("open-shape-arrow", 540, 260, (255, 59, 92))
        drag((500, 400), (502, 400))  # Two screen/document pixels is below the 3px gesture threshold.
        save_layers(lambda values: len(values) == 5, "short arrow cancellation")
        click(editor, 35, 62)
        save_layers(lambda values: len(values) == 4, "arrow single undo")
        click(editor, 98, 62)
        assert save_layers(lambda values: len(values) == 5, "arrow redo")[-1]["id"] == arrow["id"]
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "open-shape-minimum")
        close(editor)
        wait(lambda: not windows("Screenshot editor"), "open-shape editor closes")
        editor = reopen()
        run("xdotool", "windowsize", "--sync", editor, "886", "700")
        shot(editor, "open-shape-reopened")
        pixel("open-shape-reopened", 665, 255, (255, 59, 92))
        assert layers()[-1]["id"] == arrow["id"]
        assert (artifact / "capture.png").read_bytes() == original
        click(editor, 275, 62)
        click(editor, 55, 128)
        wait(lambda: not draft.exists(), "discard open-shape edits")

        run("xdotool", "windowsize", "--sync", editor, "886", "700")
        click(editor, 736, 62)
        click(editor, 50, 132)
        drag((320, 250), (480, 370))
        annotation = save_layers(lambda values: len(values) == 2, "annotation fixture")[-1]
        click(editor, 463, 62)
        click(editor, 79, 300)
        save_layers(lambda values: values[-1]["locked"], "locked annotation remains style editable")
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "20", "5")
        shot(editor, "annotation-fields")
        unchanged = draft.read_bytes()
        click(editor, 50, 503)  # Unchanged Apply is disabled.
        field(415, "#23b5a9")
        assert draft.read_bytes() == unchanged
        shot(editor, "annotation-unapplied")
        pixel("annotation-unapplied", 400, 310, (255, 59, 92))
        click(editor, 150, 503)  # Reset does not mutate the document.
        assert draft.read_bytes() == unchanged
        click(editor, 50, 503)  # A broken Reset would apply the staged cyan here.
        save_layers(lambda values: values[-1]["style"]["fill"] == "#ff3b5c", "reset cleared staged fill")
        field(415, "#23b5a9")
        click(editor, 50, 503)
        save_layers(lambda values: values[-1]["style"]["fill"] == "#23b5a9", "annotation fill")
        shot(editor, "annotation-fill")
        pixel("annotation-fill", 400, 310, (35, 181, 169))
        click(editor, 35, 62)
        save_layers(lambda values: values[-1]["style"]["fill"] == "#ff3b5c", "one-step style undo")
        click(editor, 98, 62)
        save_layers(lambda values: values[-1]["style"]["fill"] == "#23b5a9", "style redo")
        click(editor, 15, 300)  # Enable stroke, then keep the final controls in view.
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "20", "5")
        field(256, "#3269d6")
        field(300, 12, 130)
        click(editor, 15, 344)  # Clear fill.
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "20", "5")
        click(editor, 50, 503)
        save_layers(lambda values: values[-1]["style"]["fill"] is None and values[-1]["style"]["strokeWidth"] == 12, "annotation outline")
        shot(editor, "annotation-outline")
        pixel("annotation-outline", 400, 310, (40, 110, 166))
        pixel("annotation-outline", 323, 310, (50, 105, 214))
        click(editor, 15, 415)  # Restore fill; enter a different color from the stroke.
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "20", "5")
        field(415, "#23b5a9")
        click(editor, 15, 459)  # Enable custom shadow controls.
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "25", "5")
        shot(editor, "annotation-shadow-fields")
        field(283, "#ff8800")
        field(327, 80, 125)
        field(371, 0)
        field(415, 25, 90)
        field(459, -12, 90)
        click(editor, 50, 503)
        styled = save_layers(lambda values: values[-1]["style"].get("dropShadowStyle", {}).get("offsetX") == 25, "custom annotation shadow")[-1]
        assert styled["id"] == annotation["id"] and styled["locked"]
        assert styled["style"]["dropShadowStyle"] == {"color": "#ff8800", "opacity": 80, "blur": 0, "offsetX": 25, "offsetY": -12}
        shot(editor, "annotation-shadow")
        pixel("annotation-shadow", 509, 310, (212, 131, 33), tolerance=1)
        click(editor, 28, 283)
        shot(editor, "annotation-color-picker")
        run("xdotool", "key", "Escape", "sleep", ".2")
        click(editor, 15, 212)  # Disable shadow without losing custom knobs.
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "20", "5")
        click(editor, 50, 503)
        disabled = save_layers(lambda values: values[-1]["style"]["dropShadow"] is False, "shadow off")[-1]
        assert disabled["style"]["dropShadowStyle"] == styled["style"]["dropShadowStyle"]
        shot(editor, "annotation-shadow-off")
        pixel("annotation-shadow-off", 509, 310, (40, 110, 166))
        click(editor, 15, 459)
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "25", "5")
        click(editor, 50, 503)
        save_layers(lambda values: values[-1]["style"] == styled["style"], "shadow settings restored")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "25", "5")
        shot(editor, "annotation-minimum")
        close(editor)
        wait(lambda: not windows("Screenshot editor"), "styled editor closes")
        editor = reopen()
        run("xdotool", "windowsize", "--sync", editor, "886", "700")
        shot(editor, "annotation-reopened")
        pixel("annotation-reopened", 509, 310, (212, 131, 33), tolerance=1)
        assert layers()[-1]["style"] == styled["style"]
        assert (artifact / "capture.png").read_bytes() == original
        click(editor, 275, 62)
        click(editor, 55, 128)
        wait(lambda: not draft.exists(), "discard annotation edits")
        run("xdotool", "windowsize", "--sync", editor, "1000", "700")

        # These synthetic hex colors are sRGB. Keep the fixture untagged rather
        # than ImageMagick's gamma/chromaticity-only PNG; profiles have unit coverage.
        run("convert", "-size", "120x80", "xc:#d53e55", "-fill", "#3cb371",
            "-draw", "rectangle 10,9 39,29", "-fill", "#2d64bd",
            "-draw", "rectangle 88,51 119,79", "-strip", "PNG32:" + str(imported_path))
        imported_bytes = imported_path.read_bytes()
        click(editor, 640, 62)
        wait(lambda: chooser.pending, "image file picker opened")
        title, options = chooser.calls[-1]
        assert title == "Import image" and not options.get("directory", False)
        assert not options.get("multiple", False)
        shot(editor, "import-picker-pending")
        save(640, 360, 0, 0)  # A waiting picker must not occupy the session worker.
        before_import = draft.read_bytes()
        GLib.idle_add(chooser.respond, True)
        time.sleep(.4)
        assert draft.read_bytes() == before_import

        invalid = output / "broken.png"
        invalid.write_text("not an image")
        chooser.selected = invalid
        click(editor, 640, 62)
        wait(lambda: chooser.pending, "picker after cancellation")
        GLib.idle_add(chooser.respond, False)
        time.sleep(.8)
        shot(editor, "import-decode-error")
        assert draft.read_bytes() == before_import
        chooser.selected = imported_path
        click(editor, 640, 62)
        wait(lambda: chooser.pending, "retry import")
        GLib.idle_add(chooser.respond, False)
        imported_layers = save_layers(lambda values: len(values) == 2, "imported owned layer")
        imported_layer = imported_layers[-1]
        imported_id = imported_layer["id"]
        assert (imported_layer["name"], imported_layer["x"], imported_layer["y"],
                imported_layer["width"], imported_layer["height"]) == (imported_path.name, 260, 360, 120, 80)
        assert imported_layer["source"] == "imported" and imported_layer["src"].startswith("draft-asset:")
        assert not imported_layer["locked"] and imported_layer["visible"] and imported_layer["opacity"] == 100
        assert saved(640, 440, 0, 0)
        shot(editor, "imported-canvas")
        pixel("imported-canvas", 568, 537, (60, 179, 113))
        pixel("imported-canvas", 674, 584, (45, 100, 189))
        click(editor, 463, 62)
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "25", "4")
        shot(editor, "imported-selected-layer")
        click(editor, 78, 371)
        run("xdotool", "key", "ctrl+a", "ctrl+c", "sleep", ".2")
        assert run("xclip", "-selection", "clipboard", "-o").decode() == imported_path.name
        run("xdotool", "key", "Escape")  # The single-line name field scrolls; its value is intact.
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "imported-minimum")
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "8", "5")
        shot(editor, "imported-minimum-scrolled")
        run("xdotool", "windowsize", "--sync", editor, "1000", "700")
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "12", "4")
        assert imported_path.read_bytes() == imported_bytes
        imported_path.unlink()  # A saved import must no longer depend on its source file.
        click(editor, 35, 62)
        save(640, 360, 0, 0)
        assert len(layers()) == 1
        click(editor, 98, 62)
        save(640, 440, 0, 0)
        assert layers()[-1]["id"] == imported_id

        click(editor, 100, 158)  # Redo retained the original's selection; pick the imported row.
        resize_before = draft.read_bytes()
        run("xdotool", "mousemove", "--sync", "--window", editor, "685", "560",
            "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
            "720", "560", "sleep", ".3")
        shot(editor, "layer-resize-active-guides")
        assert draft.read_bytes() == resize_before, "resize preview must remain transient"
        run("xdotool", "mouseup", "1", "sleep", ".3")
        # Resize follows the final absolute pointer, not the rounded press pixel.
        resized_width = (720 - 238) * 640 / 754 - 260
        resized = save_layers(
            lambda values: math.isclose(values[-1]["width"], resized_width, abs_tol=1e-5)
            and values[-1]["height"] == 80 and values[-1]["x"] == 260,
            "imported image resized from east grip")[-1]
        assert resized["id"] == imported_id
        shot(editor, "layer-resize-committed")
        # Independently scale the fixture's green center (25,19) from its left edge.
        resized_green = (round(238 + (260 + 25 * resized_width / 120) * 754 / 640),
                         round(89 + (360 + 19) * 754 / 640))
        pixel("layer-resize-committed", *resized_green, (60, 179, 113))
        click(editor, 35, 62)
        save_layers(lambda values: values[-1]["width"] == 120 and values[-1]["height"] == 80,
                    "undo imported image resize")
        shot(editor, "layer-resize-undone")
        pixel("layer-resize-undone", 568, 537, (60, 179, 113))
        click(editor, 98, 62)
        save_layers(lambda values: math.isclose(values[-1]["width"], resized_width, abs_tol=1e-5),
                    "redo imported image resize")
        close(editor)
        wait(lambda: not windows("Screenshot editor"), "resized imported draft closes")
        editor = reopen()
        shot(editor, "layer-resize-reopened")
        reopened_resize = layers()[-1]
        assert reopened_resize["id"] == imported_id
        assert math.isclose(reopened_resize["width"], resized_width, abs_tol=1e-5)
        pixel("layer-resize-reopened", *resized_green, (60, 179, 113))
        # Undo history is session-local. Restore through a fresh east-grip resize
        # at 1:1 scale, where the desired edge lands on an exact pointer pixel.
        click(editor, 463, 62)  # Reopened editors start in Geometry, not Layers.
        click(editor, 100, 158)
        run("xdotool", "windowsize", "--sync", editor, "886", "700", "sleep", ".3")
        drag((round(238 + 260 + resized_width), 489), (618, 489))
        save_layers(lambda values: values[-1]["width"] == 120 and values[-1]["height"] == 80,
                    "restore imported size after draft reopen")
        run("xdotool", "windowsize", "--sync", editor, "1000", "700", "sleep", ".3")

        # At 1.178125 scale the 120x80 image's grip is near (615,485), around
        # pivot (615,560). Start five points above the grip, within its hit radius.
        # Exercise a free-angle transient first; Escape must leave draft/pixels intact.
        rotation_before = draft.read_bytes()
        run("xdotool", "mousemove", "--sync", "--window", editor, "615", "480",
            "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
            "650", "460", "sleep", ".3")
        shot(editor, "layer-rotation-free-transient")
        run("xdotool", "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
        assert draft.read_bytes() == rotation_before
        # Vector (0,-80) to (45,-95) is 25.35 degrees, snapping to 30 degrees.
        drag((615, 480), (660, 465), shift=True)
        rotated = save_layers(
            lambda values: math.isclose(values[-1].get("rotation", 0), math.pi / 6,
                                        abs_tol=1e-12),
            "Shift-snapped imported image rotation")[-1]
        assert rotated["id"] == imported_id
        shot(editor, "layer-rotation-shift-result")
        # Independently rotate the green fixture center (25,19) about (60,40).
        angle = math.pi / 6
        expected_x = 260 + 60 + (-35 * math.cos(angle) - -21 * math.sin(angle))
        expected_y = 360 + 40 + (-35 * math.sin(angle) + -21 * math.cos(angle))
        expected_screen = (round(238 + expected_x * 754 / 640),
                           round(89 + expected_y * 754 / 640))
        assert expected_screen == (592, 518)
        pixel("layer-rotation-shift-result", *expected_screen, (60, 179, 113))
        click(editor, 35, 62)
        save_layers(lambda values: "rotation" not in values[-1], "undo imported image rotation")
        shot(editor, "layer-rotation-undone")
        pixel("layer-rotation-undone", 568, 537, (60, 179, 113))
        click(editor, 98, 62)
        save_layers(lambda values: math.isclose(values[-1].get("rotation", 0), math.pi / 6,
                                                abs_tol=1e-12),
                    "redo imported image rotation")
        close(editor)
        wait(lambda: not windows("Screenshot editor"), "imported draft closes")
        editor = reopen()
        shot(editor, "layer-rotation-reopened")
        pixel("layer-rotation-reopened", *expected_screen, (60, 179, 113))
        assert layers()[-1]["id"] == imported_id
        assert math.isclose(layers()[-1]["rotation"], math.pi / 6, abs_tol=1e-12)
        assert saved(640, 440, 0, 0)
        # Discard returns to the original capture, without deleting exports or source data.
        click(editor, 275, 62)
        click(editor, 55, 128)
        wait(lambda: not draft.exists(), "discard imported draft")
        chooser.selected = artifact / "capture.png"
        click(editor, 640, 62)
        wait(lambda: chooser.pending, "picker before close")
        close(editor)
        wait(lambda: not windows("Screenshot editor"), "picker does not prevent closing")
        editor = reopen()
        GLib.idle_add(chooser.respond, False)  # Late result belongs to the old editor only.
        time.sleep(.5)
        save(640, 360, 0, 0)
        assert len(layers()) == 1
        assert (artifact / "capture.png").read_bytes() == original

        click(editor, 463, 62)  # Layers, preserving the Geometry panel's scroll position.
        shot(editor, "layers-original-locked")
        click(editor, 88, 632)
        shot(editor, "layers-transform-menu")
        run("xdotool", "key", "Escape")

        def transform(index, orientation, width, height):
            click(editor, 88, 632)
            click(editor, 58, 464 + 44 * index)
            save_layers(lambda values: values[0].get("orientation") == orientation,
                        f"transform {orientation}")
            assert saved(width, height, 0, 0), "fresh photo rotates its canvas"
            assert layers()[0]["locked"], "transform must not unlock the original"

        def assert_transformed_pixels(name, gold_left, gold_above):
            shot(editor, name)
            pixels = run("convert", str(output / f"{name}.png"), "-crop", "754x610+238+89",
                         "-depth", "8", "rgb:-")

            def center(color):
                points = [(i // 3 % 754, i // 3 // 754)
                          for i in range(0, len(pixels), 3) if pixels[i:i + 3] == bytes(color)]
                assert len(points) > 100, (name, color, "missing painted region")
                xs, ys = zip(*points)
                return (min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2

            gold, green = center((229, 179, 68)), center((46, 158, 113))
            assert (gold[0] < green[0]) == gold_left, (name, gold, green)
            assert (gold[1] < green[1]) == gold_above, (name, gold, green)

        # The unequal, offset regions distinguish every menu action's direction.
        transform(1, "rotate-90", 360, 640)
        assert_transformed_pixels("layers-rotate-right", False, True)
        close(editor)
        editor = reopen()
        click(editor, 463, 62)
        assert_transformed_pixels("layers-transform-reopened", False, True)
        transform(2, "transpose", 360, 640)
        assert_transformed_pixels("layers-flip-horizontal", True, True)
        click(editor, 36, 62)
        save_layers(lambda values: values[0].get("orientation") == "rotate-90", "undo flip")
        # Reopening starts a new undo history, so rotate left explicitly restores the photo.
        transform(0, None, 640, 360)
        transform(0, "rotate-270", 360, 640)
        assert_transformed_pixels("layers-rotate-left", True, False)
        transform(3, "transpose", 360, 640)
        assert_transformed_pixels("layers-flip-vertical", True, True)
        click(editor, 36, 62)
        save_layers(lambda values: values[0].get("orientation") == "rotate-270", "undo vertical flip")
        click(editor, 36, 62)
        save_layers(lambda values: values[0].get("orientation") is None, "undo left rotation")
        assert saved(640, 360, 0, 0)
        click(editor, 17, 299)  # A hidden, locked image remains transformable.
        save_layers(lambda values: not values[0]["visible"], "hide original before transform")
        transform(2, "flip-horizontal", 640, 360)
        assert not layers()[0]["visible"]
        click(editor, 36, 62)
        save_layers(lambda values: values[0].get("orientation") is None, "undo hidden transform")
        click(editor, 36, 62)
        save_layers(lambda values: values[0]["visible"], "restore original visibility")
        assert_transformed_pixels("layers-transform-restored", True, True)
        click(editor, 47, 591)  # Duplicate the locked original, not delete or move it.
        first = save_layers(lambda values: len(values) == 2, "duplicate original")
        copy_id = first[1]["id"]
        assert first[0]["locked"] and first[1]["visible"] and not first[1]["locked"]
        assert (first[1]["x"], first[1]["y"]) == (24, 24)
        assert first[0]["src"] == first[1]["src"]
        long_name = "Layer with a deliberately long name to retain"
        field(371, long_name)
        click(editor, 182, 371)
        save_layers(lambda values: values[-1]["name"] == long_name, "renamed image")
        field(459, 190)
        field(503, 70)
        click(editor, 34, 547)
        save_layers(lambda values: (values[-1]["x"], values[-1]["y"]) == (190, 70), "moved duplicate")
        shot(editor, "layers-moved")
        pixel("layers-moved", 591, 277, (229, 179, 68))
        # Canvas picking is based on the rendered document, not the layer-list selection.
        # Escape cancels the translated outline without touching the saved draft.
        canvas_before = draft.read_bytes()
        run("xdotool", "mousemove", "--window", editor, "591", "277", "mousedown", "1",
            "sleep", ".2", "mousemove", "--sync", "--window", editor, "631", "307", "sleep", ".2")
        shot(editor, "layers-canvas-active-outline")
        run("xdotool", "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
        assert draft.read_bytes() == canvas_before
        click(editor, 260, 200)  # Empty point before the unlocked copy clears selection.
        assert draft.read_bytes() == canvas_before
        # The raw horizontal delta lands just inside the canvas edge's magnetic
        # range. The shared move geometry snaps the duplicate's left edge to the
        # canvas/background layer edge while retaining the asymmetric raw Y move.
        run("xdotool", "mousemove", "--sync", "--window", editor, "591", "277",
            "sleep", ".2", "mousedown", "1", "sleep", ".2", "mousemove", "--sync",
            "--window", editor, "367", "307", "sleep", ".3")
        shot(editor, "layers-canvas-snapped-guides")
        assert draft.read_bytes() == canvas_before
        run("xdotool", "mouseup", "1", "sleep", ".2")
        # A 1000-point window leaves 754 points after the 230-point sidebar
        # and the canvas's two 8-point margins. The 640px image fits that width.
        expected_position = (0, 70 + 30 * 640 / 754)
        moved_canvas = save_layers(
            lambda values: all(math.isclose(values[-1][axis], expected, abs_tol=1e-5)
                               for axis, expected in zip(("x", "y"), expected_position)),
            "canvas drag moved duplicate")[-1]
        assert moved_canvas["id"] == copy_id
        shot(editor, "layers-canvas-moved-selection")
        pixel("layers-canvas-moved-selection", 591, 277, (40, 110, 166))
        pixel("layers-canvas-moved-selection", 367, 307, (229, 179, 68))
        click(editor, 35, 62)
        save_layers(lambda values: (values[-1]["x"], values[-1]["y"]) == (190, 70),
                    "undo snapped canvas move")
        click(editor, 98, 62)
        save_layers(
            lambda values: all(math.isclose(values[-1][axis], expected, abs_tol=1e-5)
                               for axis, expected in zip(("x", "y"), expected_position)),
            "redo snapped canvas move")
        close(editor)
        wait(lambda: not windows("Screenshot editor"), "snapped layer draft closes")
        editor = reopen()
        reopened_move = layers()[-1]
        assert reopened_move["id"] == copy_id
        assert all(math.isclose(reopened_move[axis], expected, abs_tol=1e-5)
                   for axis, expected in zip(("x", "y"), expected_position))
        shot(editor, "layers-snapped-move-reopened")
        pixel("layers-snapped-move-reopened", 367, 307, (229, 179, 68))
        # Reopen starts in Geometry and undo history is intentionally not persisted.
        # Switch to Layers and restore explicitly so downstream fixtures stay stable.
        click(editor, 463, 62)
        click(editor, 100, 158)
        field(459, 190)
        field(503, 70)
        click(editor, 34, 547)
        save_layers(lambda values: (values[-1]["x"], values[-1]["y"]) == (190, 70),
                    "restore snapped move after reopen")
        field(415, 50)
        click(editor, 154, 415)
        save_layers(lambda values: values[-1]["opacity"] == 50, "half opacity")
        shot(editor, "layers-half-opacity")
        pixel("layers-half-opacity", 591, 277, (134, 144, 117), tolerance=1)
        click(editor, 15, 300)  # Hide the copy; the original blue pixel is restored.
        save_layers(lambda values: not values[-1]["visible"], "hidden duplicate")
        shot(editor, "layers-hidden")
        pixel("layers-hidden", 591, 277, (40, 110, 166))
        click(editor, 35, 62)  # Undo must restore the rendered half-opacity layer.
        save_layers(lambda values: values[-1]["visible"], "undo visibility")
        shot(editor, "layers-undo-visible")
        pixel("layers-undo-visible", 591, 277, (134, 144, 117), tolerance=1)
        click(editor, 79, 300)
        save_layers(lambda values: values[-1]["locked"], "lock duplicate")
        click(editor, 124, 591)  # Delete is disabled while locked.
        save_layers(lambda values: len(values) == 2 and values[-1]["locked"], "locked layer retained")
        shot(editor, "layers-locked")
        click(editor, 79, 300)
        save_layers(lambda values: not values[-1]["locked"], "unlock duplicate")
        click(editor, 47, 591)
        third = save_layers(lambda values: len(values) == 3, "second duplicate")[-1]["id"]
        assert (layers()[-1]["x"], layers()[-1]["y"]) == (214, 94)
        click(editor, 149, 547)  # Down: the third layer moves behind the first copy.
        save_layers(lambda values: [value["id"] for value in values] == ["capture-background", third, copy_id], "reordered down")
        shot(editor, "layers-reordered")
        click(editor, 92, 547)
        save_layers(lambda values: [value["id"] for value in values] == ["capture-background", copy_id, third], "reordered up")
        click(editor, 124, 591)
        save_layers(lambda values: len(values) == 2 and values[-1]["id"] == copy_id, "deleted selected copy")
        click(editor, 35, 62)
        save_layers(lambda values: len(values) == 3, "undo deletion")
        click(editor, 98, 62)
        save_layers(lambda values: len(values) == 2, "redo deletion")
        close(editor)
        wait(lambda: not windows("Screenshot editor"), "saved layers close")
        editor = reopen()
        click(editor, 463, 62)
        shot(editor, "layers-reopened")
        pixel("layers-reopened", 591, 277, (134, 144, 117), tolerance=1)
        assert layers()[-1]["name"] == long_name
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "8", "5")
        shot(editor, "layers-small-scrolled")
        run("xdotool", "windowsize", "--sync", editor, "1000", "700")
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "12", "4")
        click(editor, 100, 202)  # Select and explicitly unlock the original.
        click(editor, 79, 300)
        save_layers(lambda values: not values[0]["locked"], "unlock original")
        click(editor, 124, 591)
        save_layers(lambda values: [value["id"] for value in values] == [copy_id], "delete original layer")
        click(editor, 124, 591)
        save_layers(lambda values: len(values) == 0, "empty saved document")
        shot(editor, "layers-empty")
        click(editor, 35, 62)
        save_layers(lambda values: [value["id"] for value in values] == [copy_id], "undo empty document")
        click(editor, 275, 62)
        click(editor, 55, 128)
        wait(lambda: not draft.exists(), "discard layer edits")
        click(editor, 398, 62)  # Geometry has an independent scroll position.

        # At this size the preview is 1:1: image origin (238,89), size 640x360.
        run("xdotool", "windowsize", "--sync", editor, "886", "700")

        click(editor, 736, 62)  # Draw keeps the chosen shape active after each release.
        drag((658, 289), (538, 169))
        rectangle = save_layers(lambda values: len(values) == 2, "reverse rectangle")[-1]
        assert rectangle["kind"] == "shape" and rectangle["shape"] == "rectangle"
        assert (rectangle["x"], rectangle["y"], rectangle["endX"], rectangle["endY"]) == (420, 200, 300, 80)
        assert rectangle["style"]["fill"] == "#ff3b5c" and not rectangle["style"]["strokeEnabled"]
        shot(editor, "shape-rectangle")
        pixel("shape-rectangle", 600, 230, (255, 59, 92))
        click(editor, 35, 62)
        save_layers(lambda values: len(values) == 1, "single-step shape undo")
        shot(editor, "shape-undone")
        pixel("shape-undone", 600, 230, (40, 110, 166))
        click(editor, 98, 62)
        save_layers(lambda values: len(values) == 2 and values[-1]["id"] == rectangle["id"], "shape redo keeps id")
        click(editor, 138, 138)  # Ellipse.
        drag((608, 349), (778, 399))
        ellipse = save_layers(lambda values: len(values) == 3, "ellipse layer")[-1]
        assert ellipse["shape"] == "ellipse" and ellipse["id"] != rectangle["id"]
        assert (ellipse["x"], ellipse["y"], ellipse["endX"], ellipse["endY"]) == (370, 260, 540, 310)
        shot(editor, "shape-ellipse")
        pixel("shape-ellipse", 693, 374, (255, 59, 92))
        pixel("shape-ellipse", 610, 350, (40, 110, 166))
        before_draw = draft.read_bytes()
        run("xdotool", "mousemove", "--sync", "--window", editor, "320", "300", "mousedown", "1",
            "sleep", ".2", "mousemove", "--sync", "--window", editor, "420", "380", "sleep", ".3")
        shot(editor, "shape-transient")
        assert draft.read_bytes() == before_draw
        run("xdotool", "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
        save_layers(lambda values: len(values) == 3, "escape cancels shape")
        drag((320, 300), (320, 380))  # Degenerate zero-width gesture.
        save_layers(lambda values: len(values) == 3, "degenerate shape has no layer")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "shape-minimum")
        close(editor)
        wait(lambda: not windows("Screenshot editor"), "saved shapes close")
        editor = reopen()
        run("xdotool", "windowsize", "--sync", editor, "886", "700")
        shot(editor, "shape-reopened")
        pixel("shape-reopened", 600, 230, (255, 59, 92))
        pixel("shape-reopened", 693, 374, (255, 59, 92))
        assert layers()[-1]["id"] == ellipse["id"]
        click(editor, 736, 62)
        drag((298, 500), (358, 570))  # Fully outside the image grows the canvas.
        outside = save_layers(lambda values: len(values) == 4, "outside shape retained")[-1]
        assert outside["shape"] == "rectangle"
        assert saved(640, 486, 0, 0)
        shot(editor, "shape-outside-expanded")
        assert (artifact / "capture.png").read_bytes() == original
        click(editor, 275, 62)
        click(editor, 55, 128)
        wait(lambda: not draft.exists(), "discard shape edits")
        click(editor, 398, 62)

        click(editor, 159, 335)
        drag((638, 359), (278, 119))  # Reverse drag: 40,30 with size 360x240.
        shot(editor, "crop-selection")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "crop-selection-minimum")
        run("xdotool", "windowsize", "--sync", editor, "886", "700")
        assert not draft.exists(), "selection must not write a draft"
        assert (artifact / "capture.png").read_bytes() == original
        run("xdotool", "key", "Escape", "sleep", ".2")
        shot(editor, "crop-cancelled")
        save(640, 360, 0, 0)  # Escape must not crop or mutate the document.
        before_selection = draft.read_bytes()
        click(editor, 159, 335)
        drag((278, 119), (438, 219), shift=True)
        shot(editor, "crop-shift-square")
        assert draft.read_bytes() == before_selection
        click(editor, 50, 335)
        save(160, 160, -40, -30)
        click(editor, 35, 62)
        save(640, 360, 0, 0)
        click(editor, 159, 335)
        drag((638, 500), (278, 119))  # Starts below the image; clamps to y=360.
        shot(editor, "crop-outside-start")
        click(editor, 50, 335)
        save(360, 330, -40, -30)
        click(editor, 35, 62)
        save(640, 360, 0, 0)
        click(editor, 159, 335)
        click(editor, 125, 379)
        shot(editor, "crop-aspect-menu")
        run("xdotool", "key", "Escape", "sleep", ".2")
        click(editor, 125, 379)
        click(editor, 106, 507)  # 4:3 preset takes precedence over Shift's square.
        drag((278, 119), (438, 219), shift=True)
        shot(editor, "crop-preset-four-three")
        click(editor, 50, 335)
        save(160, 120, -40, -30)
        click(editor, 35, 62)
        save(640, 360, 0, 0)
        click(editor, 159, 335)
        click(editor, 125, 379)
        click(editor, 106, 419)  # Free for the following asymmetric crop.
        drag((638, 359), (278, 119))
        click(editor, 159, 335)  # Cancel restores numeric fields as well as pixels.
        click(editor, 50, 335)
        save(640, 360, 0, 0)
        click(editor, 159, 335)
        drag((638, 359), (278, 119))
        click(editor, 50, 335)
        run("xdotool", "windowsize", "--sync", editor, "1000", "700")
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

        saved_draft = draft.read_bytes()
        click(editor, 535, 62)  # Output: encode the edited frame, not History PNG.
        click(editor, 65, 366)
        shot(editor, "output-png")
        pixel("output-png", 500, 200, (229, 179, 68))
        pixel("output-png", 900, 400, (46, 158, 113))
        click(editor, 20, 274)  # PNG Compress with an explicit palette.
        click(editor, 20, 406)
        field(450, 4)
        click(editor, 65, 498)
        shot(editor, "output-png-palette")
        pixel("output-png-palette", 900, 400, (46, 158, 113))
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "8", "5")
        shot(editor, "output-palette-minimum-scrolled")
        run("xdotool", "windowsize", "--sync", editor, "1000", "700")
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "12", "4")
        click(editor, 85, 159)  # JPEG invalidates the PNG comparison.
        click(editor, 20, 274)  # Compress.
        click(editor, 65, 410)
        shot(editor, "output-jpeg")
        pixel("output-jpeg", 900, 400, (46, 158, 113), tolerance=4)
        click(editor, 65, 482)  # Edited canvas comparison.
        shot(editor, "output-edited-canvas")
        pixel("output-edited-canvas", 900, 400, (46, 158, 113))
        click(editor, 65, 525)  # Encoded output comparison.
        click(editor, 150, 159)  # WebP, still Compress.
        click(editor, 65, 410)
        shot(editor, "output-webp")
        pixel("output-webp", 900, 400, (46, 158, 113), tolerance=4)
        click(editor, 20, 318)  # Maximum file size enables the hard cap.
        field(362, 0)
        click(editor, 65, 410)
        shot(editor, "output-budget-error")
        assert app.poll() is None and windows("Screenshot editor")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "output-budget-error-minimum")
        run("xdotool", "windowsize", "--sync", editor, "1000", "700")
        click(editor, 20, 257)  # Preserve clears the failed budget (error adds 27px).
        click(editor, 65, 393)  # Retry clears error without persisting a draft.
        shot(editor, "output-retry")
        pixel("output-retry", 900, 400, (46, 158, 113))
        assert draft.read_bytes() == saved_draft, "preview must not write a draft"
        assert not (output / "exports").exists(), "preview must not publish files"

        exported = output / "exports" / "edited.webp"
        initial_directory = output / "unchosen folder"
        initial_directory.mkdir()
        exported.parent.mkdir()
        chooser.selected = exported.parent
        chooser.calls.clear()
        field(618, initial_directory / exported.name)
        click(editor, 149, 584)
        wait(lambda: len(chooser.calls) == 1, "folder dialog cancellation")
        shot(editor, "export-folder-pending")
        click(editor, 65, 657)  # Save is disabled until the folder choice completes.
        assert not list(initial_directory.iterdir()) and not list(exported.parent.iterdir())
        GLib.idle_add(chooser.respond, True)
        time.sleep(.5)
        click(editor, 149, 584)
        wait(lambda: len(chooser.calls) == 2, "folder dialog selection")
        GLib.idle_add(chooser.respond, False)
        time.sleep(.5)
        for title, options in chooser.calls:
            assert title == "Choose save location" and options["directory"] and not options.get("multiple", False)
            assert bytes(options["current_folder"]).rstrip(b"\0") == os.fsencode(initial_directory)
        assert not list(initial_directory.iterdir()) and not list(exported.parent.iterdir())
        assert draft.read_bytes() == saved_draft and len(list(history.glob("*/metadata.json"))) == 1
        shot(editor, "export-folder-selected")
        click(editor, 65, 657)
        wait(exported.exists, "new edited copy published")
        metadata = wait(lambda: [path for path in history.glob("*/metadata.json")
                               if path.parent != artifact], "new export in History")
        assert len(metadata) == 1
        entry = json.loads(metadata[0].read_text())
        assert (entry["width"], entry["height"], entry["mode"]) == (480, 300, "region")
        assert entry["saved_path"] == str(exported) and entry["mime_type"] == "image/webp"
        exported_bytes = exported.read_bytes()
        assert entry["size_bytes"] == len(exported_bytes)
        for path in (exported, metadata[0].parent / "capture.png"):
            assert run("identify", "-format", "%wx%h", str(path)) == b"480x300"
            assert run("convert", str(path), "-crop", "1x1+450+250", "-depth", "8", "rgb:-") == bytes((46, 158, 113))
        assert draft.read_bytes() == saved_draft, "export must not save the draft"
        shot(root, "export-history-refreshed")
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "12", "5")
        shot(editor, "export-saved-scrolled")
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "20", "4")
        click(editor, 65, 657)  # Same filename must fail rather than replace.
        shot(editor, "export-collision")
        assert exported.read_bytes() == exported_bytes
        assert len(list(history.glob("*/metadata.json"))) == 2

        # Retry after a real History failure must report the successfully saved file.
        history.rename(output / "previous-history")
        history.write_text("blocks History creation")
        recovered = output / "exports" / "recovered.webp"
        field(645, recovered)  # The collision error adds 27px above the panel.
        click(editor, 65, 657)  # Editing the destination clears the collision error.
        wait(recovered.exists, "file saved despite unavailable History")
        assert recovered.read_bytes() == exported_bytes
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "20", "5")
        shot(editor, "export-history-warning-scrolled")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "20", "5")
        shot(editor, "export-history-warning-minimum")
        run("xdotool", "windowsize", "--sync", editor, "1000", "700")
        history.unlink()
        (output / "previous-history").rename(history)
        assert draft.read_bytes() == saved_draft
        assert (artifact / "capture.png").read_bytes() == original

        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "20", "4")
        click(editor, 170, 657)  # Copy the edited canvas, not the History source.
        wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
             "edited clipboard copy completes")
        copied = run("xclip", "-selection", "clipboard", "-t", "image/png", "-o")
        clipboard_png = output / "clipboard-edited.png"
        clipboard_png.write_bytes(copied)
        assert run("identify", "-format", "%wx%h", str(clipboard_png)) == b"480x300"
        for x, y, expected in ((70, 80, (229, 179, 68)), (450, 250, (46, 158, 113))):
            assert run("convert", str(clipboard_png), "-crop", f"1x1+{x}+{y}", "-depth", "8", "rgb:-") == bytes(expected)
        assert draft.read_bytes() == saved_draft and len(list(history.glob("*/metadata.json"))) == 2
        assert len(list((output / "exports").iterdir())) == 2
        run("xdotool", "mousemove", "--window", editor, "180", "400", "click", "--repeat", "20", "5")
        shot(editor, "clipboard-copied")
        click(editor, 398, 62)  # Geometry restores its own scroll position.

        close(editor)
        wait(lambda: not windows("Screenshot editor"), "saved editor closes")
        assert run("xclip", "-selection", "clipboard", "-t", "image/png", "-o") == copied, "workspace retains clipboard after editor closes"
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
            "checks": ["viewport-wheel-anchor", "viewport-toolbar-fit-100-step-recenter",
                       "viewport-pan-active-settled-no-draft", "viewport-fit-pixels",
                       "crop", "crop-pointer-reverse", "crop-escape-no-mutation", "crop-transient-no-write",
                       "crop-shift-square", "crop-outside-start-clamping", "crop-preset-precedes-shift",
                       "crop-cancel-restores-fields", "crop-popup-escape",
                       "shape-reverse-rectangle-ellipse-pixels", "shape-single-undo-redo-stable-id",
                       "shape-transient-escape-degenerate", "shape-draft-reopen-outside-expansion",
                       "open-shape-horizontal-vertical-zero-lines", "open-shape-reverse-arrow-pixels",
                       "open-shape-transient-escape-short-cancel", "open-shape-undo-redo-minimum-reopen",
                       "freehand-quadratic-preview-and-render-pixels", "freehand-sampling-dot",
                       "freehand-cancel-retains-redo", "freehand-outside-expansion-minimum-reopen",
                       "annotation-unapplied-reset-noop", "annotation-locked-fill-stroke-pixels",
                       "annotation-style-undo-redo", "annotation-shadow-toggle-retains-custom",
                       "annotation-color-picker-minimum-reopen-pixels",
                       "canvas", "undo-redo", "draft-reopen", "close-preserves-draft",
                       "image-picker-pending-cancel-retry", "image-import-exact-pixels",
                       "image-import-owned-draft-reopen", "image-picker-stale-close-result",
                       "layer-resize-guides-commit-undo-draft-reopen-pixels",
                       "layer-rotation-free-cancel-shift-snap", "layer-rotation-undo-draft-reopen-pixels",
                       "explicit-discard", "save-error", "quit-error-retry", "original-unchanged",
                       "layer-duplicate-rename-move", "layer-canvas-click-drag-escape",
                       "layer-canvas-snap-edge-pixels-undo-reopen",
                       "layer-opacity-visibility-pixels",
                       "layer-lock-order-delete", "layer-draft-reopen", "layer-undo-redo",
                       "layer-empty-undo", "image-transform-four-actions-pixels",
                       "image-transform-locked-hidden", "image-transform-canvas-draft-undo",
                       "output-png-jpeg-webp", "output-comparison",
                       "output-budget-error-retry", "output-no-draft-or-file-write",
                       "output-png-palette-minimum-scroll", "export-new-copy-history",
                       "export-collision-original-protection", "export-history-warning-recovery",
                       "folder-portal-cancel-select-filename-no-persistence",
                       "clipboard-edited-pixels-no-persistence", "clipboard-survives-editor-close"],
            "originalSha256": hashlib.sha256(original).hexdigest(),
        }, indent=2) + "\n")
        print("PASS native editor: layers, crop, canvas, undo/redo, draft reopen, close/discard, save/quit recovery, original unchanged")
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
