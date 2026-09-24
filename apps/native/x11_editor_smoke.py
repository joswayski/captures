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
import random
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
    parser.add_argument("--brush-only", action="store_true",
                        help="Exercise erase/restore gestures, cancellation, draft and clipboard")
    parser.add_argument("--trim-only", action="store_true",
                        help="Exercise canvas trimming, undo/redo, draft and output dimensions")
    parser.add_argument("--zoom-only", action="store_true",
                        help="Exercise viewport gestures, toolbar and keyboard zoom without editing")
    parser.add_argument("--history-shortcuts-only", action="store_true",
                        help="Exercise tool and document keys without stealing text input")
    parser.add_argument("--text-draft-only", action="store_true",
                        help="Restore explicit-font text, save/reopen and copy pixels (no Text input UI)")
    parser.add_argument("--text-only", action="store_true",
                        help="Exercise the real Text tool UI, undo/redo and draft reopen")
    parser.add_argument("--text-defaults-only", action="store_true",
                        help="Exercise pre-placement Text style/size/color and centered box placement")
    parser.add_argument("--text-input-only", action="store_true",
                        help="Exercise on-canvas text composition, finish, blank discard, undo and quit")
    parser.add_argument("--polygon-only", action="store_true",
                        help="Exercise Triangle/Diamond/Star previews, cancellation and saved pixels")
    parser.add_argument("--rotation-snap-only", action="store_true",
                        help="Exercise custom Shift rotation stops without saving the UI setting")
    parser.add_argument("--output-presets-only", action="store_true",
                        help="Exercise compression presets and real saved PNG pixels")
    parser.add_argument("--output-size-only", action="store_true",
                        help="Exercise percentage/custom export dimensions without changing the document")
    parser.add_argument("--overwrite-only", action="store_true",
                        help="Exercise confirmed original replacement, History identity and retained drafts")
    parser.add_argument("--external-image-only", action="store_true",
                        help="Open external images, preserve per-file errors and safely reopen drafts/sources")
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
    editor = None
    draft = None

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

    shot_layouts = {}

    def shot(window, name):
        time.sleep(.5)
        run("import", "-window", window, str(output / f"{name}.png"))
        if editor is not None and str(window) == str(editor):
            shot_layouts[name] = (window_size(), document_size())

    def pixel(name, x, y, expected, tolerance=0):
        actual = run("convert", str(output / f"{name}.png"), "-crop", f"1x1+{x}+{y}",
                     "-depth", "8", "rgb:-")
        assert len(actual) == 3 and all(abs(a - b) <= tolerance for a, b in zip(actual, expected)), (name, x, y, actual, expected)

    def document_pixel(name, x, y, expected, tolerance=0):
        window, size = shot_layouts[name]
        left, top, scale = fit_geometry(size, window)
        pixel(name, round(left + x * scale), round(top + y * scale), expected, tolerance)

    def fixture_pixel(name, x, y, expected, tolerance=0):
        # Historical right-inspector fixtures used a document origin of (8, 89).
        document_pixel(name, x - 8, y - 89, expected, tolerance)

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

    # Keep the four coordinate spaces explicit. Toolbar coordinates are window-local,
    # inspector coordinates are local to the fixed 230px right panel, document points
    # are authored in image pixels, and exported-image pixels never pass through these
    # helpers. A few visual fixtures use the historical screenshot coordinates whose
    # document origin was (8, 89); fixture_to_document names that conversion directly.
    def window_size():
        geometry = run("xdotool", "getwindowgeometry", "--shell", editor).decode()
        return tuple(int(re.search(rf"^{axis}=(\d+)$", geometry, re.MULTILINE).group(1))
                     for axis in ("WIDTH", "HEIGHT"))

    def editor_width():
        return window_size()[0]

    def document_size():
        if draft is not None and draft.exists():
            document = json.loads(draft.read_text())["document"]
            return document["width"], document["height"]
        return 640, 360

    def fit_geometry(size=None, window=None):
        width, height = window or window_size()
        image_width, image_height = size or document_size()
        available = (64., 133. if width == 760 else 89., width - 238., height - 8.)
        scale = min(1., max(.02, (available[2] - available[0]) / image_width),
                    max(.02, (available[3] - available[1]) / image_height))
        center = ((available[0] + available[2]) / 2, (available[1] + available[3]) / 2)
        return (center[0] - image_width * scale / 2,
                center[1] - image_height * scale / 2, scale)

    def document_point(point, size=None):
        left, top, scale = fit_geometry(size)
        return round(left + point[0] * scale), round(top + point[1] * scale)

    def fixture_point(point, size=None):
        return document_point(fixture_to_document(point), size)

    def fixture_click(point, size=None):
        click(editor, *fixture_point(point, size))

    def fixture_move(point, *tail, size=None, sync=False):
        x, y = fixture_point(point, size)
        command = ["xdotool", "mousemove"]
        if sync:
            command.append("--sync")
        run(*command, "--window", editor, str(x), str(y), *tail)

    def resize_editor(width, height, *tail):
        # Full-size geometry fixtures use odd client heights so an even-height
        # document's centered Fit origin lands on an integer device pixel.
        run("xdotool", "windowsize", "--sync", editor, str(width), str(height), *map(str, tail))

    def fixture_to_document(point):
        return point[0] - 8, point[1] - 89

    def inspector_x(x):
        return editor_width() - 230 + x

    def inspector_click(x, y):
        click(editor, inspector_x(x), y)

    def export_click(action):
        # Pinned below the inspector: independent of section, scroll and output options.
        inspector_click({"copy": 44, "save": 134}[action], window_size()[1] - 63)

    def inspector_move(x, y, *tail):
        run("xdotool", "mousemove", "--window", editor, str(inspector_x(x)), str(y), *tail)

    def canvas_point(point):
        # Existing authored gestures describe points in the fixture screenshot.
        # Convert to document space first, then use the independently specified Fit.
        return document_point(fixture_to_document((point[0] - 230, point[1])))

    def drag(start, end, shift=False):
        start, end = canvas_point(start), canvas_point(end)
        run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, start), "sleep", ".2")
        if shift:
            run("xdotool", "keydown", "Shift_L", "sleep", ".1")
        run("xdotool", "mousedown", "1", "sleep", ".2", "mousemove", "--sync",
            "--window", editor, *map(str, end), "sleep", ".3", "mouseup", "1", "sleep", ".2")
        if shift:
            run("xdotool", "keyup", "Shift_L")

    try:
        env["DISPLAY"] = ":" + spawn("xvfb", ["Xvfb", "-displayfd", "1", "-screen", "0",
            "1280x1600x24" if args.text_only else "1280x1200x24",
            "-dpi", "96", "-nolisten", "tcp"], True)
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
        if args.output_presets_only:
            # A flat gradient can be smaller losslessly, correctly bypassing
            # quantization. Seeded RGB noise makes the palette path decisive.
            fixture = output / "preset-source.rgb"
            fixture.write_bytes(random.Random(739).randbytes(640 * 360 * 3))
            run("convert", "-size", "640x360", "-depth", "8", "rgb:" + str(fixture),
                "PNG32:" + str(artifact / "capture.png"))
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
        if args.overwrite_only:
            source_export = output / "original.png"
            source_export.write_bytes(original)
            metadata_path = artifact / "metadata.json"
            original_metadata = json.loads(metadata_path.read_text())
            original_metadata["saved_path"] = str(source_export)
            metadata_path.write_text(json.dumps(original_metadata))
        if args.text_draft_only:
            # Original test font, not a system font or a shipping font policy.
            draft_root = output / "editor-drafts" / artifact_id
            (draft_root / "assets").mkdir(parents=True)
            (draft_root / "fonts").mkdir()
            (draft_root / "assets/original.png").write_bytes(original)
            font = Path(__file__).resolve().parents[2] / "crates/captures-image/tests/shaping-regular.ttf"
            font_bytes = font.read_bytes()
            (draft_root / "fonts/regular.font").write_bytes(font_bytes)
            base = {"visible": True, "locked": False, "opacity": 100, "blendMode": "source-over"}
            (draft_root / "manifest.json").write_text(json.dumps({
                "schema_version": 1, "artifact_id": artifact_id, "updated_at_ms": 1,
                "fonts": {"families": {"sans": "Captures Shaping Test"}, "assets": ["regular"]},
                "document": {"width": 640, "height": 360, "background": "#f7f7f5", "elements": [
                    {**base, "kind": "image", "id": "capture-background", "x": 0, "y": 0,
                     "locked": True, "source": "background", "src": "draft-asset:original",
                     "originalSrc": None, "name": "Original screenshot", "width": 640,
                     "height": 360, "naturalWidth": 640, "naturalHeight": 360},
                    {**base, "kind": "text", "id": "label", "x": 250, "y": 40,
                     "text": "L\nfi", "fontSize": 80, "width": 180, "fontFamily": "sans",
                     "bold": False, "italic": False, "align": "right", "color": "#ff0000",
                     "background": "#f7f7f5", "outlined": False, "roundedBackground": True},
                ]},
            }))
        settings = output / "settings.json"
        settings.write_text(json.dumps({
            "settings_schema_version": 5, "appearance": args.appearance, "theme": "mustard",
            "output_directory": str(output / "exports"), "launch_at_login": False,
            "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
            "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10",
            "auto_copy_to_clipboard": False, "show_mini_previews": False,
        }))
        app_command = [str(binary), "--live", "--history-root", str(history),
                       "--settings-file", str(settings), "--quit-after", "600"]
        open_arguments = []
        if args.external_image_only:
            source_png = output / "External image é.png"
            source_jpeg = output / "Second image.jpg"
            source_webp = output / "Third image.webp"
            source_png.write_bytes(original)
            # Untagged sRGB fixture: ImageMagick otherwise emits unsupported
            # gAMA/cHRM-only metadata rather than an actual sRGB chunk.
            run("convert", str(source_png), "-strip", "PNG32:" + str(source_png))
            run("convert", "-size", "73x41", "xc:#872d46", "-strip", str(source_jpeg))
            run("convert", "-size", "81x53", "xc:#268752", "-strip",
                "-define", "webp:lossless=true", str(source_webp))
            source_bytes = {path: path.read_bytes() for path in [source_png, source_jpeg, source_webp]}
            bad_source = output / "Broken image.png"
            bad_source.write_bytes(b"not an image")
            alias = output / "Same image alias.png"
            alias.symlink_to(source_png)
            for path in artifact.iterdir():
                path.unlink()
            artifact.rmdir()  # Opening must populate an initially empty History.
            for path in [bad_source, source_jpeg, source_webp, source_png, alias]:
                open_arguments.extend(["--open-image", str(path)])

            def opened_entries():
                return [json.loads(path.read_text()) for path in history.glob("*/metadata.json")
                        if not path.parent.name.startswith(".")]

        app = spawn("app", app_command + open_arguments)
        root = wait(lambda: windows("Captures"), "History workspace")[0]
        run("xdotool", "windowmove", "--sync", root, "0", "0")
        time.sleep(1)
        if args.external_image_only:
            entries = wait(lambda: values if len(values := opened_entries()) == 3 else None,
                           "three imported History rows, not an alias duplicate")
            opened = next(value for value in entries if value["saved_path"] == str(source_png))
            artifact_id = opened["id"]
            artifact = history / artifact_id
            wait(lambda: len(windows("Screenshot editor")) == 3, "three native image editors")
            editor = wait(lambda: active if (active := run("xdotool", "getactivewindow").decode().strip())
                          in windows("Screenshot editor") else None, "last opened image focused")
            shot(root, "history")
        else:
            shot(root, "history")
            click(root, 810, 191)
            editor = wait(lambda: windows("Screenshot editor"), "screenshot editor")[0]
        run("xdotool", "windowmove", "--sync", editor, "100", "80")
        time.sleep(1)
        shot(editor, "editor-original")
        pixel("editor-original", 520, 690,
              (245, 245, 247) if args.appearance == "light" else (16, 16, 20))

        def type_text(value, delay=10):
            # xdotool type consumes all remaining arguments; do not chain keys after it.
            run("xdotool", "type", "--clearmodifiers", "--delay", str(delay), "--", str(value))

        def field(y, value, x=78):
            inspector_click(x, y)
            run("xdotool", "key", "ctrl+a")
            type_text(value, 60)
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
            run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
                "sleep", ".2")
            assert int(run("xdotool", "getwindowfocus")) == int(window), "close target must own focus"
            run("xdotool", "keydown", "Alt_L", "sleep", ".1", "key", "F4",
                "sleep", ".1", "keyup", "Alt_L", "sleep", ".4")

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

        def asset_pixel(layer, x, y, expected=None):
            asset = draft.parent / "assets" / (layer["src"].split(":", 1)[1] + ".png")
            actual = run("convert", str(asset), "-crop", f"1x1+{x}+{y}", "-depth", "8", "rgba:-")
            if expected is not None:
                assert actual == bytes(expected), (x, y, actual, expected)
            return actual

        if args.external_image_only:
            assert {item["saved_path"]: (item["width"], item["height"]) for item in entries} == {
                str(source_png): (640, 360), str(source_jpeg): (73, 41), str(source_webp): (81, 53),
            }
            for item in entries:
                saved = history / item["id"] / "capture.png"
                assert run("convert", str(saved), "-depth", "8", "rgba:-") == run(
                    "convert", item["saved_path"], "-depth", "8", "rgba:-"), "imported pixels match source decoding"
            assert len(windows("Screenshot editor")) == 3
            shot(editor, "external-image-opened")
            document_pixel("external-image-opened", 130, 100, (229, 179, 68))
            document_pixel("external-image-opened", 500, 250, (46, 158, 113))
            document_pixel("external-image-opened", 20, 20, (40, 110, 166))
            assert not draft.exists(), "opening/focusing must not create edit drafts"
            click(editor, 736, 62)
            inspector_click(105, 133)
            drag((320, 250), (480, 370))
            save_layers(lambda values: len(values) == 2, "external image edit is a real draft")
            preserved_draft = draft.read_bytes()
            assert all(path.read_bytes() == before for path, before in source_bytes.items())

            # A second executable must forward, not initialize another renderer,
            # settings writer or set of capture shortcuts. Sender CWD differs.
            run("xdotool", "windowactivate", "--sync", root)
            assert run("xdotool", "getactivewindow").decode().strip() == root
            forwarded = subprocess.run(
                [str(binary), "--live", "--history-root", str(history),
                 "--settings-file", str(output / "unused-secondary-settings.json"),
                 "--open-media", alias.name], cwd=output, env=env,
                capture_output=True, text=True, timeout=10)
            assert forwarded.returncode == 0, forwarded.stderr
            assert '"event":"forwarded"' in forwarded.stdout
            assert '"event":"ready"' not in forwarded.stdout
            assert not (output / "unused-secondary-settings.json").exists()
            wait(lambda: run("xdotool", "getactivewindow").decode().strip() == editor,
                 "forwarded canonical alias focuses existing edited window")
            assert len(windows("Screenshot editor")) == 3
            assert len(opened_entries()) == 3
            assert draft.read_bytes() == preserved_draft
            assert len(layers()) == 2

            run("xdotool", "windowminimize", root)
            relaunched = subprocess.run(app_command, cwd=output, env=env,
                                        capture_output=True, text=True, timeout=10)
            assert relaunched.returncode == 0, relaunched.stderr
            assert '"event":"forwarded"' in relaunched.stdout
            wait(lambda: run("xdotool", "getactivewindow").decode().strip() == root,
                 "empty relaunch restores and focuses Preferences")
            assert app.poll() is None
            close(root)
            wait(lambda: app.poll() is not None, "external image batch quits")
            assert app.returncode == 0

            # A closed edited source must not silently discard its saved work.
            app = spawn("app-draft", app_command + ["--open-image", str(source_png)])
            root = wait(lambda: windows("Captures"), "draft guard History")[0]
            time.sleep(2)
            assert not windows("Screenshot editor"), "saved draft blocks source reload"
            assert draft.read_bytes() == preserved_draft
            shot(root, "external-draft-blocked")
            editor = reopen()  # The existing History route still restores the saved edit.
            shot(editor, "external-draft-restored")
            assert len(layers()) == 2
            click(editor, 270, 62)
            click(editor, 55, 128)
            wait(lambda: not draft.exists(), "explicitly discard the saved draft")
            close(root)
            wait(lambda: app.poll() is not None, "draft-resolution process quits")
            assert app.returncode == 0

            run("convert", str(source_png), "-fill", "#1234ab", "-draw", "rectangle 8,8 50,50",
                "-strip", "PNG32:" + str(source_png))
            changed_source = source_png.read_bytes()
            app = spawn("app-reload", app_command + ["--open-image", str(source_png)])
            root = wait(lambda: windows("Captures"), "reloaded source History")[0]
            editor = wait(lambda: windows("Screenshot editor"), "reloaded external editor")[0]
            run("xdotool", "windowmove", "--sync", editor, "100", "80")
            time.sleep(1)
            shot(editor, "external-source-reloaded")
            document_pixel("external-source-reloaded", 20, 20, (18, 52, 171))
            reloaded = next(item for item in opened_entries() if item["saved_path"] == str(source_png))
            assert (reloaded["id"], reloaded["created_at"]) == (artifact_id, opened["created_at"])
            assert len(opened_entries()) == 3
            assert not draft.exists()
            assert source_png.read_bytes() == changed_source
            assert all(path.read_bytes() == before for path, before in source_bytes.items() if path != source_png)
            close(root)
            wait(lambda: app.poll() is not None, "external source suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["startup-files-with-spaces", "bad-file-does-not-stop-later-files",
                           "png-jpeg-webp-owned-pixels", "canonical-alias-no-duplicate",
                           "multiple-native-editors", "open-does-not-create-draft",
                           "edits-do-not-overwrite-source", "closed-draft-blocks-reload",
                           "history-restores-saved-draft", "explicit-discard-allows-source-reload",
                           "reload-preserves-history-identity", "reloaded-source-pixels",
                           "secondary-exits-before-renderer-and-settings",
                           "forwarded-relative-alias-preserves-edits",
                           "empty-relaunch-restores-preferences"],
            }, indent=2) + "\n")
            print("PASS native external images: batch, aliases, errors, pixels, drafts and safe reload")
            return

        if args.history_shortcuts_only:
            resize_editor(1000, 901, "sleep", ".3")  # Integer-pixel Fit origin for exact movement.
            click(editor, 28, 227)  # Persistent Shapes rail button.
            shot(editor, "tool-rail-shapes-menu")
            run("xdotool", "key", "Escape", "sleep", ".3")
            assert not draft.exists(), "opening/closing tool menus must not save edits"
            click(editor, 28, 269)  # Arrow, independent of the inspector section.
            drag((320, 250), (480, 370))
            arrow = save_layers(lambda values: len(values) == 2, "rail Arrow creates one layer")[-1]
            assert arrow["shape"] == "arrow", arrow
            resize_editor(760, 540, "sleep", ".3")
            click(editor, 28, 271)  # Shapes after the compact toolbar wraps.
            shot(editor, "tool-rail-minimum-menu")
            run("xdotool", "key", "Escape", "sleep", ".3")
            click(editor, 28, 313)
            shot(editor, "tool-rail-minimum-arrow")
            resize_editor(1000, 901, "sleep", ".3")
            click(editor, 270, 62)
            click(editor, 55, 128)
            wait(lambda: not draft.exists(), "discard rail fixture")
            click(editor, 300, 20)
            run("xdotool", "key", "s", "sleep", ".3")
            drag((320, 250), (480, 370))
            star = save_layers(lambda values: len(values) == 2, "S selects Star")[-1]
            assert star["shape"] == "star", star
            run("xdotool", "key", "v", "sleep", ".3")
            drag((400, 310), (421, 327))
            moved = save_layers(lambda values: len(values) == 2 and values[-1] != star,
                                "V selects and moves, not draws")[-1]
            assert moved == dict(star, x=star["x"] + 21, y=star["y"] + 17,
                                 endX=star["endX"] + 21, endY=star["endY"] + 17), moved
            before_crop = draft.read_bytes()
            run("xdotool", "key", "c", "sleep", ".3")
            drag((330, 270), (460, 350))
            run("xdotool", "key", "c", "sleep", ".3")
            shot(editor, "shortcut-crop-candidate")
            run("xdotool", "key", "v", "sleep", ".3")
            assert draft.read_bytes() == before_crop, "tool selection/cancellation must not save edits"
            assert save_layers(lambda values: len(values) == 2, "crop cancellation")[-1] == moved
            click(editor, 270, 62)
            click(editor, 55, 128)
            wait(lambda: not draft.exists(), "discard tool-key fixture")
            click(editor, 300, 20)  # Leave control focus before selecting a canvas tool.
            run("xdotool", "key", "r", "sleep", ".3")
            drag((320, 250), (480, 370))
            shape = save_layers(lambda values: len(values) == 2, "shortcut shape fixture")[-1]
            assert shape["shape"] == "rectangle", shape
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            save_layers(lambda values: len(values) == 1, "keyboard undo")
            click(editor, 396, 62)
            inspector_click(75, 428)
            run("xdotool", "key", "ctrl+shift+z", "sleep", ".3")
            save_layers(lambda values: len(values) == 1, "field focus does not redo document")
            click(editor, 736, 62)
            run("xdotool", "key", "ctrl+shift+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "keyboard redo")[-1] == shape

            click(editor, 396, 62)  # Geometry.
            inspector_click(75, 428)  # Focus canvas width, not a document action.
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "field focus keeps document")[-1] == shape
            click(editor, 270, 62)  # Open discard confirmation.
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            click(editor, 190, 128)  # Toolbar confirmation: cancel discard.
            assert save_layers(lambda values: len(values) == 2, "confirmation owns shortcuts")[-1] == shape
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            save_layers(lambda values: len(values) == 1, "document shortcut restored after dialog")
            run("xdotool", "key", "ctrl+shift+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "redo after dialog")[-1] == shape
            click(editor, 463, 62)  # Layers; select the restored shape, not the original.
            inspector_click(100, 158)
            run("xdotool", "key", "ctrl+d", "sleep", ".3")
            copied = save_layers(lambda values: len(values) == 3, "keyboard duplicate")[-1]
            assert copied["id"] != shape["id"]
            expected = dict(shape, id=copied["id"], x=shape["x"] + 24, y=shape["y"] + 24,
                            endX=shape["endX"] + 24, endY=shape["endY"] + 24,
                            visible=True, locked=False)
            assert copied == expected, (copied, expected)
            nudged = copied
            for key, dx, dy in [("Left", -1, 0), ("shift+Up", 0, -10),
                                ("shift+Right", 10, 0), ("Down", 0, 1)]:
                expected = dict(nudged, x=nudged["x"] + dx, y=nudged["y"] + dy,
                                endX=nudged["endX"] + dx, endY=nudged["endY"] + dy)
                run("xdotool", "key", key, "sleep", ".3")
                nudged = save_layers(lambda values: len(values) == 3 and values[-1] == expected,
                                     f"keyboard nudge {key}")[-1]
            for _ in range(4):
                run("xdotool", "key", "ctrl+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 3, "undo nudges exactly")[-1] == copied
            inspector_click(100, 158)  # Restore the copy selection after Undo.
            click(editor, 396, 62)
            inspector_click(75, 428)
            run("xdotool", "key", "ctrl+d", "Delete", "Left", "shift+Up", "sleep", ".3")
            run("xdotool", "key", "p", "c", "r", "sleep", ".3")
            shot(editor, "shortcut-field-keeps-tool-letters")
            assert save_layers(lambda values: len(values) == 3, "field protects layer shortcuts")[-1] == copied
            click(editor, 463, 62)
            run("xdotool", "key", "Delete", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "Delete removes selected copy")[-1] == shape
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 3, "undo keyboard deletion")[-1] == copied
            inspector_click(100, 158)  # Select restored copy explicitly after undo.
            run("xdotool", "key", "BackSpace", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "Backspace removes selected copy")[-1] == shape
            inspector_click(100, 202)  # Original image is locked.
            run("xdotool", "key", "Delete", "Right", "shift+Down", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "locked keyboard deletion")[-1] == shape
            # Layer snapshots must work even with an empty OS clipboard. Copy
            # then move/delete the source, so paste cannot just duplicate it.
            subprocess.run(["xclip", "-selection", "clipboard", "-i"], input=b"",
                           env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           check=True, timeout=10)
            inspector_click(100, 158)
            run("xdotool", "key", "ctrl+c", "sleep", ".3")
            assert run("xclip", "-selection", "clipboard", "-o") == b""
            run("xdotool", "key", "Right", "sleep", ".3", "key", "Delete", "sleep", ".3")
            save_layers(lambda values: len(values) == 1, "copied source deleted")
            run("xdotool", "key", "ctrl+v", "sleep", ".3")
            pasted = save_layers(lambda values: len(values) == 2, "paste without OS payload")[-1]
            expected = dict(shape, id=pasted["id"], x=shape["x"] + 24, y=shape["y"] + 24,
                            endX=shape["endX"] + 24, endY=shape["endY"] + 24,
                            visible=True, locked=False)
            assert pasted == expected and pasted["id"] != shape["id"], (pasted, expected)
            subprocess.run(["xclip", "-selection", "clipboard", "-i"], input=b"unrelated OS text",
                           env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           check=True, timeout=10)
            run("xdotool", "key", "ctrl+v", "sleep", ".3")
            pasted_twice = save_layers(lambda values: len(values) == 3, "one paste with OS payload")[-1]
            expected = dict(shape, id=pasted_twice["id"], x=shape["x"] + 48, y=shape["y"] + 48,
                            endX=shape["endX"] + 48, endY=shape["endY"] + 48,
                            visible=True, locked=False)
            assert pasted_twice == expected and pasted_twice["id"] != pasted["id"]
            assert run("xclip", "-selection", "clipboard", "-o") == b"unrelated OS text"
            shot(editor, "layer-clipboard-pasted")
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "paste single undo")[-1] == pasted
            run("xdotool", "key", "ctrl+shift+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 3, "paste redo")[-1] == pasted_twice
            close(editor)
            wait(lambda: not windows("Screenshot editor"), "copied editor closes")
            editor = reopen()
            run("xdotool", "key", "ctrl+v", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 3, "reopen has no layer clipboard")[-1] == pasted_twice
            click(editor, 463, 62)
            before_menu = draft.read_bytes()
            inspector_move(100, 202, "sleep", ".2", "mousedown", "3",
                           "sleep", ".15", "mouseup", "3", "sleep", ".3")
            shot(editor, "layer-context-menu")
            run("xdotool", "key", "Escape", "sleep", ".3")
            assert draft.read_bytes() == before_menu, "opening/cancelling a row menu must not edit"
            resize_editor(760, 540, "sleep", ".3")
            inspector_move(100, 202, "sleep", ".2", "mousedown", "3",
                           "sleep", ".15", "mouseup", "3", "sleep", ".3")
            shot(editor, "layer-context-menu-minimum")
            # Copy the first row through the actual popup. A missed opening must
            # fail this flow, not silently produce a menu-free review capture.
            inspector_click(130, 217)
            assert draft.read_bytes() == before_menu
            run("xdotool", "key", "ctrl+v", "sleep", ".3")
            menu_paste = save_layers(lambda values: len(values) == 4, "context-menu copy then paste")[-1]
            assert menu_paste == dict(pasted_twice, id=menu_paste["id"],
                                      x=pasted_twice["x"] + 24, y=pasted_twice["y"] + 24,
                                      endX=pasted_twice["endX"] + 24, endY=pasted_twice["endY"] + 24)
            assert menu_paste["id"] not in {shape["id"], pasted["id"], pasted_twice["id"]}
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "shortcut suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["keyboard-undo", "keyboard-redo-exact-layer", "field-undo-focus", "field-redo-focus",
                           "confirmation-focus", "shortcut-restored-after-dialog", "original-unchanged",
                           "duplicate-offset-fresh-id", "field-layer-shortcuts", "delete-selected-copy",
                           "backspace-selected-copy", "locked-delete-guard", "arrow-1px", "shift-arrow-10px",
                           "nudge-undo-exact", "field-nudge-focus", "locked-nudge-guard",
                           "S-star", "V-select-move", "C-crop-cancel", "R-rectangle", "field-tool-letters",
                           "rail-arrow-create", "rail-menu-escape", "rail-minimum",
                           "layer-copy-snapshot-after-delete", "layer-paste-empty-OS-clipboard",
                           "layer-paste-once-with-OS-text", "layer-paste-cumulative-offset",
                           "layer-paste-undo-redo", "layer-clipboard-session-local",
                           "layer-context-menu-cancel", "layer-context-menu-minimum",
                           "layer-context-menu-copy-paste"],
            }, indent=2) + "\n")
            print("PASS native editor shortcuts: tools, undo, redo, duplicate, delete, nudge, field/dialog focus and original unchanged")
            return

        if args.overwrite_only:
            run("xdotool", "windowsize", "--sync", editor, "1000", "1100")
            click(editor, 736, 62)
            inspector_click(105, 133)
            drag((320, 250), (480, 370))
            save_layers(lambda values: len(values) == 2, "edited original fixture")
            saved_draft = draft.read_bytes()
            click(editor, 550, 62)
            shot(editor, "overwrite-controls")
            inspector_click(85, 765)
            shot(editor, "overwrite-confirmation")
            assert source_export.read_bytes() == original
            assert json.loads(metadata_path.read_text()) == original_metadata
            assert draft.read_bytes() == saved_draft
            click(editor, 231, 190)  # Toolbar confirmation: cancel replacement.
            shot(editor, "overwrite-cancelled")
            assert source_export.read_bytes() == original
            inspector_click(85, 765)
            run("xdotool", "key", "Escape", "sleep", ".3")
            assert source_export.read_bytes() == original
            inspector_click(85, 765)
            click(editor, 75, 190)  # Toolbar confirmation: explicit Replace, not the copy-path field.
            wait(lambda: source_export.read_bytes() != original, "original file replaced")
            wait(lambda: (artifact / "capture.png").read_bytes() != original, "same History image replaced")
            shot(editor, "overwrite-saved")
            for path in [source_export, artifact / "capture.png"]:
                assert run("identify", "-format", "%wx%h", str(path)) == b"640x360"
                assert run("convert", str(path), "-crop", "1x1+162+221", "-depth", "8", "rgb:-") == bytes((255, 59, 92))
            updated = json.loads(metadata_path.read_text())
            assert (updated["id"], updated["created_at"], updated["saved_path"]) == (
                artifact_id, original_metadata["created_at"], str(source_export))
            assert len(list(history.glob("*/metadata.json"))) == 1
            assert draft.read_bytes() == saved_draft
            replaced = source_export.read_bytes()
            click(editor, 35, 62)
            save_layers(lambda values: len(values) == 1, "undo preserved after output")
            assert source_export.read_bytes() == replaced
            click(editor, 98, 62)
            save_layers(lambda values: len(values) == 2, "redo preserved after output")
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            inspector_move(180, 400, "click", "--repeat", "25", "5")
            shot(editor, "overwrite-minimum")
            close(editor)
            wait(lambda: not windows("Screenshot editor"), "overwritten editor closes")
            editor = reopen()
            assert len(layers()) == 2
            shot(editor, "overwrite-draft-reopened")
            assert source_export.read_bytes() == replaced
            close(root)
            wait(lambda: app.poll() is not None, "overwrite suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["confirmation-before-write", "cancel-and-escape", "exact-file-pixels",
                           "same-history-id-date", "draft-preserved", "undo-redo-preserved",
                           "minimum-controls", "editable-draft-reopen"],
            }, indent=2) + "\n")
            print("PASS native overwrite: confirmation, cancel, pixels, same History, draft and undo")
            return

        if args.rotation_snap_only:
            resize_editor(1000, 1001)
            click(editor, 463, 62)
            inspector_click(79, 300)  # Unlock the original image for canvas rotation.
            save_layers(lambda values: not values[0]["locked"], "unlocked original")
            shot(editor, "rotation-snap-controls")
            before = draft.read_bytes()
            field(743, 37, x=50)
            shot(editor, "rotation-snap-custom")
            assert draft.read_bytes() == before, "snap preference must not edit or save a draft"
            # Full-canvas image uses the inset top grip at (558,117), pivot (558,269).
            # Vector (0,-152) to (100,-110) is 42.27°, giving 37°, not default 45°.
            rotation_start = fixture_point((328, 117))
            rotation_end = fixture_point((428, 159))
            run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, rotation_start),
                "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
                *map(str, rotation_end), "keydown", "Shift_L", "sleep", ".3")
            shot(editor, "rotation-snap-transient")
            assert draft.read_bytes() == before
            run("xdotool", "key", "Escape", "sleep", ".2", "mouseup", "1", "keyup", "Shift_L")
            assert draft.read_bytes() == before
            drag((558, 117), (658, 159), shift=True)
            angle = 37 * math.pi / 180
            save_layers(lambda values: math.isclose(values[0].get("rotation", 0), angle, abs_tol=1e-12),
                        "custom 37-degree rotation")
            shot(editor, "rotation-snap-committed")
            # Independently rotate the original yellow rectangle's interior point (150,130).
            yellow_document = (320 - 170 * math.cos(angle) + 50 * math.sin(angle),
                               180 - 170 * math.sin(angle) - 50 * math.cos(angle))
            document_pixel("rotation-snap-committed", *yellow_document, (229, 179, 68))
            click(editor, 35, 62)
            save_layers(lambda values: values[0].get("rotation", 0) == 0, "custom rotation undo")
            click(editor, 98, 62)
            save_layers(lambda values: math.isclose(values[0].get("rotation", 0), angle, abs_tol=1e-12),
                        "custom rotation redo")
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            inspector_move(180, 400, "click", "--repeat", "14", "5")
            shot(editor, "rotation-snap-minimum")
            close(editor)
            wait(lambda: not windows("Screenshot editor"), "custom rotation closes")
            editor = reopen()
            resize_editor(1000, 1001)
            click(editor, 463, 62)
            shot(editor, "rotation-snap-reopened-default")
            assert math.isclose(layers()[0]["rotation"], angle, abs_tol=1e-12)
            document_pixel("rotation-snap-reopened-default", *yellow_document, (229, 179, 68))
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "rotation snap suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["setting-no-draft-write", "custom-shift-preview-cancel", "custom-37-not-default-45",
                           "independent-rotated-pixels", "single-undo-redo", "minimum-controls",
                           "saved-angle-reopen", "original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native rotation snap: custom angle, no preference edit, cancel, undo, draft, pixels")
            return

        if args.polygon_only:
            resize_editor(942, 701)
            save(640, 360, 0, 0)
            click(editor, 736, 62)
            shot(editor, "polygon-tools")
            ids = []
            for name, tool_x, start, end, center in [
                ("triangle", 45, (418, 239), (278, 119), (348, 179)),
                ("diamond", 125, (598, 289), (498, 129), (548, 209)),
                ("star", 192, (818, 419), (658, 299), (738, 359)),
            ]:
                inspector_click(tool_x, 265)
                before = draft.read_bytes()
                transient_start, transient_end = canvas_point(start), canvas_point(end)
                run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, transient_start),
                    "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
                    *map(str, transient_end), "sleep", ".3")
                shot(editor, f"polygon-{name}-transient")
                pixel(f"polygon-{name}-transient", *canvas_point(center), (255, 59, 92))
                assert draft.read_bytes() == before, "transient polygon cannot write the draft"
                run("xdotool", "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
                assert draft.read_bytes() == before
                drag(start, (start[0], end[1]))
                save_layers(lambda values: len(values) == len(ids) + 1, "zero-width polygon rejected")
                drag(start, end)
                created = save_layers(lambda values: len(values) == len(ids) + 2, f"{name} created")[-1]
                assert created["shape"] == name and created["id"] not in ids
                assert (created["x"], created["y"], created["endX"], created["endY"]) == (
                    start[0] - 238, start[1] - 89, end[0] - 238, end[1] - 89)
                shot(editor, f"polygon-{name}-committed")
                pixel(f"polygon-{name}-committed", *canvas_point(center), (255, 59, 92))
                click(editor, 35, 62)
                save_layers(lambda values: len(values) == len(ids) + 1, f"{name} undo")
                click(editor, 98, 62)
                save_layers(lambda values: values[-1]["id"] == created["id"], f"{name} redo stable ID")
                ids.append(created["id"])
            # A convex hull or fan triangulation would incorrectly fill this star notch.
            fixture_pixel("polygon-star-transient", 508, 409, (46, 158, 113))
            fixture_pixel("polygon-star-committed", 508, 409, (46, 158, 113))
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, "polygon-minimum")
            inspector_move(180, 400, "click", "--repeat", "5", "5")
            shot(editor, "polygon-minimum-scrolled")
            close(editor)
            wait(lambda: not windows("Screenshot editor"), "polygon draft closes")
            editor = reopen()
            resize_editor(942, 701)
            shot(editor, "polygon-reopened")
            for center in [(348, 179), (548, 209), (738, 359)]:
                pixel("polygon-reopened", *canvas_point(center), (255, 59, 92))
            fixture_pixel("polygon-reopened", 508, 409, (46, 158, 113))
            assert [layer["id"] for layer in layers()[1:]] == ids
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "polygon suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["three-shared-polygon-previews", "transient-no-draft-write", "escape-cancels",
                           "zero-width-no-layer", "reverse-coordinates", "three-committed-silhouettes",
                           "star-concave-notch", "single-undo-redo-stable-ids", "minimum-controls",
                           "draft-reopen-exact-pixels", "original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native polygons: previews, cancellation, silhouettes, undo/redo, minimum, draft")
            return

        if args.output_size_only:
            run("xdotool", "windowsize", "--sync", editor, "1000", "1000")
            click(editor, 535, 62)
            shot(editor, "output-size-original")
            exports = output / "exports"
            exports.mkdir()

            def size_export(name, expected, custom=False, section_x=535):
                inspector_click(65, 507 if custom else 463)
                wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
                     "resized output preview")
                shot(editor, f"output-size-{name}-preview")
                path = exports / f"{name}.png"
                field(759 if custom else 715, path)
                click(editor, section_x, 62)
                export_click("save")
                wait(path.exists, f"{name} saved")
                assert run("identify", "-format", "%wx%h", str(path)).decode() == expected
                entry_path = wait(lambda: next((p for p in history.glob("*/metadata.json")
                    if json.loads(p.read_text()).get("saved_path") == str(path)), None), "resized History item")
                entry = json.loads(entry_path.read_text())
                assert f"{entry['width']}x{entry['height']}" == expected
                assert run("identify", "-format", "%wx%h", str(entry_path.parent / "capture.png")).decode() == expected
                assert not draft.exists() and (artifact / "capture.png").read_bytes() == original
                shot(editor, f"output-size-{name}-saved")
                click(editor, 535, 62)

            inspector_click(65, 159)
            shot(editor, "output-size-menu")
            inspector_click(45, 247)  # 75%.
            size_export("75-percent", "480x270", section_x=389)
            inspector_click(65, 159)
            inspector_click(45, 291)  # 50%.
            size_export("50-percent", "320x180", section_x=475)
            inspector_click(65, 159)
            inspector_click(45, 335)  # Custom starts from 640x360, locked.
            field(203, 96, x=40)
            size_export("locked", "96x54", custom=True, section_x=735)
            inspector_click(170, 203)  # Unlock aspect.
            field(203, 31, x=112)
            size_export("unlocked", "96x31", custom=True)
            export_click("copy")  # Copy ignores output dimensions.
            copied = output / "clipboard-original-size.png"
            copied.write_bytes(run("xclip", "-selection", "clipboard", "-t", "image/png", "-o"))
            assert run("identify", "-format", "%wx%h", str(copied)) == b"640x360"
            field(203, 0, x=40)
            shot(editor, "output-size-invalid")
            assert len(list(exports.iterdir())) == 4
            field(203, 96, x=40)
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, "output-size-minimum")
            click(editor, 35, 106)  # Draw wraps at minimum width; no Output or scrolling.
            # xclip forks a selection owner; do not capture its inherited stdout pipe.
            subprocess.run(["xclip", "-selection", "clipboard", "/dev/null"], env=env,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True, timeout=10)
            export_click("copy")
            copied.write_bytes(run("xclip", "-selection", "clipboard", "-t", "image/png", "-o"))
            assert run("identify", "-format", "%wx%h", str(copied)) == b"640x360"
            shot(editor, "output-size-minimum-draw-copy")
            inspector_move(100, window_size()[1] - 27, "sleep", "1")
            shot(editor, "output-size-minimum-copy-detail")
            close(root)
            wait(lambda: app.poll() is not None, "output size suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["75-percent-preview-save-history", "50-percent-preview-save-history",
                           "custom-aspect-lock", "custom-independent-height", "copy-full-resolution",
                           "invalid-dimensions", "minimum-controls", "no-draft-or-original-write",
                           "save-from-all-sections", "copy-from-minimum-draw"],
            }, indent=2) + "\n")
            print("PASS native output sizing: percentages, custom lock/unlock, History, copy, no edits")
            return

        if args.output_presets_only:
            run("xdotool", "windowsize", "--sync", editor, "1000", "1000")
            click(editor, 535, 62)
            inspector_click(20, 371)  # Compress.
            inspector_click(65, 459)
            shot(editor, "output-preset-menu")
            inspector_click(45, 503)  # Tiny.
            inspector_click(65, 551)  # Preview PNG with automatic palette selection.
            wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
                 "Tiny preview encoded")
            shot(editor, "output-preset-tiny-preview")
            exports = output / "exports"
            exports.mkdir()
            tiny = exports / "tiny.png"
            field(803, tiny)
            inspector_click(78, 803)
            run("xdotool", "key", "ctrl+a", "ctrl+c", "sleep", ".2")
            assert run("xclip", "-selection", "clipboard", "-o").decode() == str(tiny)
            export_click("save")
            wait(tiny.exists, "Tiny PNG saved")
            shot(editor, "output-preset-tiny")
            assert int(run("identify", "-format", "%k", str(artifact / "capture.png"))) > 256
            assert int(run("identify", "-format", "%k", str(tiny))) <= 32
            inspector_click(20, 503)  # Explicit override; Highest must clear it.
            field(547, 2)
            inspector_click(65, 459)
            inspector_click(45, 679)  # Highest, not an arbitrary high numeric value.
            inspector_click(65, 551)
            wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
                 "Highest preview encoded")
            shot(editor, "output-preset-highest-preview")
            highest = exports / "highest.png"
            field(803, highest)
            inspector_click(78, 803)
            run("xdotool", "key", "ctrl+a", "ctrl+c", "sleep", ".2")
            assert run("xclip", "-selection", "clipboard", "-o").decode() == str(highest)
            export_click("save")
            wait(highest.exists, "Highest PNG saved")
            shot(editor, "output-preset-highest")
            assert run("convert", str(highest), "-depth", "8", "rgba:-") == run(
                "convert", str(artifact / "capture.png"), "-depth", "8", "rgba:-")
            assert not draft.exists(), "output controls and exports never save a draft"
            assert (artifact / "capture.png").read_bytes() == original
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, "output-preset-minimum")
            inspector_click(65, 503)
            shot(editor, "output-preset-minimum-menu")
            run("xdotool", "key", "Escape")
            close(root)
            wait(lambda: app.poll() is not None, "output preset suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["preset-menu", "tiny-saved-png-32-colors", "highest-clears-custom-palette",
                           "highest-saved-png-exact-pixels", "no-draft-or-original-write",
                           "minimum-controls-and-menu"],
            }, indent=2) + "\n")
            print("PASS native output presets: Tiny palette, Highest exact pixels, no edits, minimum")
            return

        if args.text_input_only:
            save_layers(lambda values: len(values) == 1, "composition baseline")

            def begin_input(point):
                click(editor, 736, 62)
                inspector_click(34, 128)
                click(editor, *document_point(point))

            before = draft.read_bytes()
            begin_input((80, 60))
            shot(editor, "text-input-empty")
            assert draft.read_bytes() == before
            run("xdotool", "key", "Escape", "sleep", ".3")
            save_layers(lambda values: len(values) == 1, "blank input creates no layer")

            begin_input((80, 60))
            type_text("Discard this", 1)
            shot(editor, "text-input-cancel")
            x, y = document_point((80, 60))
            click(editor, x + 100, y + 110)  # Cancel in the composing panel.
            save_layers(lambda values: len(values) == 1, "Cancel restores the original document")

            before = draft.read_bytes()
            begin_input((80, 60))
            # One paste avoids flooding X11 with thousands of synthetic key events.
            oversized = b"x" * 4097
            subprocess.run(["xclip", "-selection", "clipboard", "-i"], input=oversized,
                           env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           check=True, timeout=10)
            run("xdotool", "key", "ctrl+v", "sleep", ".5")
            subprocess.run(["xclip", "-selection", "clipboard", "-i"], input=b"sentinel",
                           env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           check=True, timeout=10)
            run("xdotool", "key", "ctrl+a", "ctrl+c", "sleep", ".3")
            assert run("xclip", "-selection", "clipboard", "-o") == oversized
            shot(editor, "text-input-error")
            assert draft.read_bytes() == before, "failed previews must not save"
            run("xdotool", "key", "ctrl+a")
            type_text("Recovered", 1)
            run("xdotool", "key", "Escape", "sleep", ".3")
            save_layers(lambda values: len(values) == 2 and values[-1]["text"] == "Recovered",
                        "typing remains editable after preview error")
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            save_layers(lambda values: len(values) == 1, "recovered input is one undo step")

            before = draft.read_bytes()
            begin_input((80, 60))
            type_text("Alpha", 1)
            run("xdotool", "key", "Return")
            type_text("Beta", 1)
            time.sleep(.3)
            shot(editor, "text-input-typing")
            assert draft.read_bytes() == before, "typing previews must not persist a draft"
            run("xdotool", "key", "Escape", "sleep", ".3")
            created = save_layers(lambda values: len(values) == 2, "multiline input finished")[-1]
            assert created["text"] == "Alpha\nBeta"
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            save_layers(lambda values: len(values) == 1, "one undo removes complete text input")
            run("xdotool", "key", "ctrl+shift+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "text input redo")[-1] == created

            begin_input((85, 70))
            run("xdotool", "key", "ctrl+a")
            type_text("Revised", 1)
            run("xdotool", "key", "Return")
            type_text("line two", 1)
            time.sleep(.3)
            shot(editor, "text-input-existing")
            run("xdotool", "key", "Escape", "sleep", ".3")
            revised = save_layers(lambda values: len(values) == 2 and values[-1]["text"] == "Revised\nline two",
                                  "existing Text hit edits without another layer")[-1]
            assert revised["id"] == created["id"]
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "existing input single undo")[-1] == created

            # Finishing/undo returns to Select. Double-click edits the same layer
            # without switching to the Text tool or committing a move/resize.
            x, y = document_point((110, 78))
            run("xdotool", "mousemove", "--window", editor, str(x), str(y), "sleep", ".2",
                "click", "--repeat", "2", "--delay", "120", "1", "sleep", ".3")
            run("xdotool", "key", "ctrl+a")
            type_text("Double-clicked", 1)
            shot(editor, "text-input-double-click")
            run("xdotool", "key", "Escape", "sleep", ".3")
            double_clicked = save_layers(
                lambda values: len(values) == 2 and values[-1]["text"] == "Double-clicked",
                "Select double-click edits existing text")[-1]
            assert double_clicked["id"] == created["id"]
            assert double_clicked["align"] == created["align"] == "center"
            assert double_clicked["y"] == created["y"]
            assert math.isclose(double_clicked["x"] + double_clicked["width"] / 2,
                                created["x"] + created["width"] / 2, abs_tol=1e-6)
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "double-click edit single undo")[-1] == created

            begin_input((400, 250))
            resize_editor(760, 540, "sleep", ".3")
            type_text("Minimum", 1)
            time.sleep(.3)
            shot(editor, "text-input-minimum")
            run("xdotool", "key", "Escape", "sleep", ".3")
            minimum = save_layers(lambda values: len(values) == 3, "minimum input finished")[-1]
            assert minimum["text"] == "Minimum"
            resize_editor(1000, 700, "sleep", ".3")
            begin_input((405, 260))
            shot(editor, "text-input-before-delete")
            # Keep this fast: Escape must not drop the preceding queued deletion.
            run("xdotool", "key", "ctrl+a", "BackSpace", "Escape", "sleep", ".3")
            save_layers(lambda values: len(values) == 2, "blank existing text removes layer")
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 3, "blank deletion undo")[-1] == minimum
            run("xdotool", "key", "ctrl+shift+z", "sleep", ".3")
            save_layers(lambda values: len(values) == 2, "blank deletion redo")
            begin_input((300, 300))
            type_text("Quit retained", 1)
            close(root)
            wait(lambda: app.poll() is not None, "composition drains on quit")
            assert app.returncode == 0 and layers()[-1]["text"] == "Quit retained"
            assert (artifact / "capture.png").read_bytes() == original
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["blank-new-no-layer", "cancel-restores-document", "error-no-draft",
                           "error-recovery-one-undo", "preview-no-draft", "multiline-exact", "one-create-undo",
                           "redo-exact", "existing-hit-same-id", "existing-one-undo", "minimum-input",
                           "select-double-click-same-id-position", "double-click-one-undo",
                           "blank-existing-delete", "delete-undo-redo", "quit-latest-buffer", "original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native Text input: transient typing, multiline, existing hit, undo, minimum and quit")
            return

        if args.text_defaults_only:
            resize_editor(1000, 1001)
            save_layers(lambda values: len(values) == 1, "baseline draft before Text defaults")
            click(editor, 736, 62)
            inspector_click(34, 128)
            shot(editor, "text-defaults-initial")
            fixture_click((488, 289))  # Default Rounded box centered at document (480,200).
            type_text("Rounded")
            run("xdotool", "key", "Escape", "sleep", ".3")
            rounded = save_layers(lambda values: len(values) == 2, "default rounded Text placed")[-1]
            assert (rounded["fontFamily"], rounded["fontSize"], rounded["color"]) == ("rounded", 24, "#ff3b5c")
            assert rounded["background"] == "#111318" and rounded["roundedBackground"]
            assert rounded["align"] == "center" and rounded["y"] == 200
            assert math.isclose(rounded["x"] + rounded["width"] / 2, 480, abs_tol=1e-6)
            assert json.loads(draft.read_text())["fonts"]["families"]["rounded"] == "Nunito"
            fixture_click((28, 109))
            shot(editor, "text-defaults-rounded")
            document_pixel("text-defaults-rounded", 480, 196, (17, 19, 24))
            click(editor, 736, 62)
            resize_editor(760, 540)
            shot(editor, "text-defaults-rounded-minimum")
            inspector_move(120, 430, "click", "--repeat", "3", "5", "sleep", ".3")
            shot(editor, "text-defaults-rounded-minimum-controls")
            inspector_move(120, 430, "click", "--repeat", "10", "4", "sleep", ".3")
            resize_editor(1000, 1001)
            click(editor, 35, 62)
            save_layers(lambda values: len(values) == 1, "default rounded creation single undo")
            before = draft.read_bytes()
            click(editor, 736, 62)
            inspector_click(34, 128)
            inspector_click(95, 337)
            shot(editor, "text-defaults-menu")
            inspector_move(60, 506,
                "click", "--repeat", "5", "5", "sleep", ".2")
            shot(editor, "text-defaults-menu-scrolled")
            inspector_click(60, 499)  # Mono box, before Rounded box.
            field(381, 37.5, x=59)
            field(452, "#2367ab", x=105)
            shot(editor, "text-defaults-staged")
            assert draft.read_bytes() == before, "defaults must not write a draft"
            fixture_click((208, 169))  # Document (200,80), at actual-size scale.
            type_text("Native")
            shot(editor, "text-defaults-composing")
            run("xdotool", "key", "Escape", "sleep", ".3")
            text = save_layers(lambda values: len(values) == 2, "styled Text placed")[-1]
            assert text["kind"] == "text" and text["fontFamily"] == "mono"
            assert text["fontSize"] == 37.5 and text["color"] == "#2367ab"
            assert text["text"] == "Native" and text["align"] == "center" and text["y"] == 80
            assert math.isclose(text["x"] + text["width"] / 2, 200, abs_tol=1e-6)
            assert text["background"] == "#111318" and text["autoWidth"]
            fixture_click((28, 109))  # Deselect: the rotation stem crosses the plate sample.
            shot(editor, "text-defaults-created")
            document_pixel("text-defaults-created", 200, 76, (17, 19, 24))
            click(editor, 35, 62)
            save_layers(lambda values: len(values) == 1, "styled creation single undo")
            click(editor, 98, 62)
            save_layers(lambda values: len(values) == 2 and values[-1]["id"] == text["id"],
                        "styled creation redo")
            click(editor, 736, 62)
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, "text-defaults-minimum")
            close(editor)
            wait(lambda: not windows("Screenshot editor"), "styled Text editor closes")
            editor = reopen()
            resize_editor(1000, 1001)
            click(editor, 736, 62)
            inspector_click(34, 128)
            shot(editor, "text-defaults-reopened")
            assert layers()[-1] == text
            fixture_click((408, 269))  # Fresh editor defaults, not saved Mono box/37.5.
            type_text("Fresh")
            run("xdotool", "key", "Escape", "sleep", ".3")
            reset = save_layers(lambda values: len(values) == 3, "fresh editor Text defaults")[-1]
            assert (reset["fontFamily"], reset["fontSize"], reset["color"]) == ("rounded", 24, "#ff3b5c")
            assert reset["align"] == "center" and reset["background"] == "#111318"
            assert reset["roundedBackground"]
            assert reset["id"] != text["id"]
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "Text defaults suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["default-rounded-font-plate-placement", "default-rounded-single-undo",
                           "defaults-no-draft-write", "chosen-preset-size-color", "centered-typed-placement",
                           "plate-pixels", "single-undo-redo", "minimum-controls", "draft-style-reopen",
                           "new-editor-default-reset", "original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native Text defaults: preset, size, color, centered plate, undo, draft, reset")
            return

        if args.text_only:
            resize_editor(1000, 1501)
            click(editor, 736, 62)  # Draw.
            inspector_click(34, 128)  # Text is the first tool.
            inspector_click(95, 337)
            inspector_click(60, 419)  # Standard: test plain glyphs before adding a plate.
            fixture_click((200, 250))
            type_text("Text")
            shot(editor, "text-composing")
            run("xdotool", "key", "Escape", "sleep", ".3")
            save_layers(lambda values: len(values) == 2 and values[-1]["kind"] == "text",
                        "text composed once")
            created = layers()[-1]
            assert created["text"] == "Text" and created["fontFamily"] == "sans"
            assert created["align"] == "left" and created.get("autoWidth") is True
            shot(editor, f"text-created-{args.appearance}")
            inspector_click(90, 357)
            shot(editor, f"text-style-menu-{args.appearance}")
            run("xdotool", "key", "Escape")
            # Text properties precede generic layer geometry in the sidebar.
            inspector_click(100, 431)
            shot(editor, f"text-font-menu-{args.appearance}")
            inspector_click(100, 605)  # Liberation Serif, after Nunito.
            inspector_click(105, 505)
            run("xdotool", "key", "ctrl+a", "type", "--clearmodifiers", "--delay", "35",
                "--", "Readable native text")
            time.sleep(.2)
            inspector_click(95, 573)   # Bold.
            inspector_click(146, 573)  # Italic.
            inspector_click(74, 864)   # Apply without plate or shadow first.
            save_layers(lambda values: values[-1]["text"] == "Readable native text"
                        and values[-1]["fontFamily"] == "serif", "plain text applied")
            shot(editor, "text-without-shadow")
            inspector_click(92, 820)   # Stage Drop shadow, leaving the plate off.
            before_shadow = draft.read_bytes()
            for x, y, value in [(95, 891, "#3b82f6"), (128, 935, "65"),
                                (60, 979, "3"), (80, 1023, "17.5"), (80, 1067, "-8")]:
                inspector_click(x, y)
                run("xdotool", "key", "ctrl+a", "type", "--clearmodifiers", "--", value)
                run("xdotool", "key", "Return")
            shot(editor, "text-shadow-staged")
            assert draft.read_bytes() == before_shadow
            inspector_click(74, 1110)
            save_layers(lambda values: values[-1].get("dropShadow") is True
                        and values[-1]["background"] is None, "glyph shadow applied")
            custom_shadow = {"color": "#3b82f6", "opacity": 65, "blur": 3,
                             "offsetX": 17.5, "offsetY": -8}
            assert layers()[-1]["dropShadowStyle"] == custom_shadow
            shot(editor, "text-glyph-shadow")
            def text_pixels(name, crop=None):
                if crop is None:
                    window, size = shot_layouts[name]
                    left, top, scale = fit_geometry(size, window)
                    crop = f"{round(size[0] * scale)}x{round(size[1] * scale)}+{round(left)}+{round(top)}"
                return run("convert", str(output / f"{name}.png"), "-crop", crop,
                           "-depth", "8", "rgba:-")
            assert text_pixels("text-shadow-staged") == text_pixels("text-without-shadow")
            assert text_pixels("text-glyph-shadow") != text_pixels("text-without-shadow")
            inspector_click(92, 820)   # Cancellation must preserve the accepted shadow.
            inspector_click(74, 909)
            shot(editor, "text-shadow-cancelled")
            assert text_pixels("text-shadow-cancelled") == text_pixels("text-glyph-shadow")
            inspector_click(170, 820)  # Outline shares the shadow row.
            shot(editor, "text-outline-staged")
            assert text_pixels("text-outline-staged") == text_pixels("text-glyph-shadow")
            inspector_click(74, 1110)
            save_layers(lambda values: values[-1]["outlined"], "text outline applied")
            shot(editor, "text-outline")
            assert text_pixels("text-outline") != text_pixels("text-glyph-shadow")
            inspector_click(170, 820)
            inspector_click(74, 1155)
            shot(editor, "text-outline-cancelled")
            assert text_pixels("text-outline-cancelled") == text_pixels("text-outline")
            inspector_click(92, 776)   # Background plate.
            shot(editor, f"text-staged-{args.appearance}")
            inspector_click(74, 1231)   # Apply text; plate owns the shadow now.
            edited = save_layers(
                lambda values: values[-1]["text"] == "Readable native text"
                and values[-1]["bold"] and values[-1]["italic"]
                and values[-1]["background"] is not None and values[-1]["fontFamily"] == "serif"
                and values[-1].get("dropShadow") is True,
                "readable styled text applied")[-1]
            assert edited["id"] == created["id"]
            shot(editor, f"text-edited-{args.appearance}")
            inspector_click(100, 431)
            inspector_click(100, 471)  # Stage Mono without applying.
            save_layers(lambda values: values[-1]["fontFamily"] == "serif",
                        "saving accepted pixels preserves staged family")
            shot(editor, f"text-family-pending-{args.appearance}")
            inspector_click(74, 1276)  # Cancel changes; later close must not be blocked.
            click(editor, 35, 62)
            save_layers(lambda values: values[-1]["background"] is None
                        and values[-1].get("dropShadow") is True, "undo shadowed plate")
            click(editor, 35, 62)
            save_layers(lambda values: not values[-1]["outlined"]
                        and values[-1].get("dropShadow") is True, "undo text outline")
            click(editor, 35, 62)
            save_layers(lambda values: not values[-1].get("dropShadow", False), "undo glyph shadow")
            click(editor, 35, 62)
            save_layers(lambda values: values[-1]["text"] == "Text" and values[-1]["fontFamily"] == "sans",
                        "text edit undo")
            click(editor, 98, 62)
            save_layers(lambda values: values[-1]["text"] == "Readable native text",
                        "text edit redo")
            click(editor, 98, 62)
            save_layers(lambda values: values[-1].get("dropShadow") is True, "redo glyph shadow")
            click(editor, 98, 62)
            save_layers(lambda values: values[-1]["outlined"], "redo text outline")
            click(editor, 98, 62)
            save_layers(lambda values: values[-1]["background"] is not None, "redo shadowed plate")
            before_preset = draft.read_bytes()
            inspector_click(90, 357)
            inspector_click(90, 622)  # Mono box, preserving the accepted plate color.
            shot(editor, "text-preset-staged")
            assert draft.read_bytes() == before_preset
            assert text_pixels("text-preset-staged") == text_pixels(f"text-edited-{args.appearance}")
            font_field = f"205x64+{inspector_x(8)}+390"
            assert text_pixels("text-preset-staged", font_field) != text_pixels(
                f"text-edited-{args.appearance}", font_field), "Preset stages a different font field"
            inspector_click(74, 1276)
            shot(editor, "text-preset-cancelled")
            assert text_pixels("text-preset-cancelled", font_field) == text_pixels(
                f"text-edited-{args.appearance}", font_field), "Cancel restores the font field"
            # Cancel restores this label, not the independently chosen future preset.
            click(editor, 736, 62)
            inspector_click(34, 128)
            shot(editor, "text-preset-carried-default")
            fixture_click((488, 389))  # Document (480,300), away from the existing label.
            type_text("Later")
            run("xdotool", "key", "Escape", "sleep", ".3")
            future = save_layers(lambda values: len(values) == 3, "carried preset creates later label")[-1]
            assert future["fontFamily"] == "mono" and future["background"] == "#111318"
            assert future["fontSize"] == 24 and future["color"] == "#ff3b5c"
            assert not future["bold"] and not future["italic"] and not future["outlined"]
            assert future["align"] == "center" and not future["roundedBackground"]
            assert future["id"] != created["id"]
            shot(editor, "text-preset-carried-created")
            click(editor, 35, 62)
            save_layers(lambda values: len(values) == 2, "future label is one undo step")
            click(editor, 464, 62)
            inspector_click(100, 153)  # Restore the original label's selected-text inspector.
            inspector_click(90, 357)
            inspector_click(90, 622)
            inspector_click(74, 1231)
            preset = save_layers(lambda values: values[-1]["fontFamily"] == "mono"
                                 and not values[-1]["outlined"], "named style applied")[-1]
            for key in ["text", "fontSize", "bold", "italic", "align", "color", "background", "dropShadowStyle"]:
                assert preset[key] == edited[key], f"preset must preserve {key}"
            shot(editor, f"text-preset-applied-{args.appearance}")
            click(editor, 35, 62)
            save_layers(lambda values: values[-1]["fontFamily"] == "serif" and values[-1]["outlined"],
                        "named style undo")
            click(editor, 98, 62)
            save_layers(lambda values: values[-1]["fontFamily"] == "mono" and not values[-1]["outlined"],
                        "named style redo")
            click(editor, 35, 62)
            save_layers(lambda values: values[-1]["fontFamily"] == "serif" and values[-1]["outlined"],
                        "restore accepted style for reopen")
            close(editor)
            wait(lambda: not windows("Screenshot editor"), "text editor closes")
            editor = reopen()
            click(editor, 464, 62)  # Layers, with the restored text selected explicitly.
            inspector_click(100, 153)
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, f"text-minimum-reopened-{args.appearance}")
            before_scroll = draft.read_bytes()
            inspector_move(120, 440,
                "click", "--repeat", "7", "--delay", "80", "5", "sleep", ".3")
            shot(editor, f"text-minimum-controls-{args.appearance}")
            inspector_move(120, 440,
                "click", "--repeat", "4", "--delay", "80", "5", "sleep", ".3")
            shot(editor, f"text-minimum-shadow-controls-{args.appearance}")
            assert draft.read_bytes() == before_scroll, "scrolling text controls must not edit"
            reopened = layers()[-1]
            assert reopened["id"] == created["id"] and reopened["text"] == "Readable native text"
            assert reopened["fontFamily"] == "serif"
            assert json.loads(draft.read_text())["fonts"]["families"] == {
                "sans": "Liberation Sans", "serif": "Liberation Serif", "mono": "Liberation Mono",
                "rounded": "Nunito"}
            assert reopened["bold"] and reopened["italic"] and reopened["background"] is not None
            assert reopened.get("dropShadow") is True
            assert reopened["dropShadowStyle"] == custom_shadow
            assert reopened["outlined"]
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "text suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["text-click-once-fresh-selection", "text-readable-explicit-apply",
                           "text-font-family", "text-family-cancel", "text-bold-italic-plate",
                           "text-glyph-shadow-pixels", "text-shadow-stage-cancel",
                           "text-custom-shadow-settings-reopen",
                           "text-named-style-staging-cancel", "text-named-style-preserve-undo-redo",
                           "text-preset-future-after-cancel", "text-preset-future-independent-traits",
                           "text-plate-shadow", "text-shadow-undo-redo-reopen",
                           "text-outline-pixels", "text-outline-stage-cancel", "text-outline-undo-redo-reopen",
                           "text-undo-redo", "text-draft-reopen",
                           "text-minimum-appearance", "original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native Text UI: create, style, undo/redo, save/reopen, minimum")
            return

        if args.text_draft_only:
            shot(editor, "text-draft-restored")
            save_until(lambda: json.loads(draft.read_text())["updated_at_ms"] > 1,
                       "font-backed draft save")
            assert (draft.parent / "fonts/regular.font").read_bytes() == font_bytes
            assert json.loads(draft.read_text())["fonts"] == {
                "families": {"sans": "Captures Shaping Test"}, "assets": ["regular"]}
            resize_editor(942, 701)
            click(editor, 470, 62)  # Layers.
            fixture_click((370, 230))  # Select the text plate, including non-ink pixels.
            drag((600, 230), (625, 247))
            save_layers(lambda values: (values[1]["x"], values[1]["y"]) == (275, 57), "text moved")
            shot(editor, "text-moved")
            click(editor, 35, 62)
            save_layers(lambda values: (values[1]["x"], values[1]["y"]) == (250, 40), "text move undone")
            drag((697, 229), (737, 229))
            save_layers(lambda values: math.isclose(values[1]["width"], 220.2, abs_tol=1e-4)
                        and values[1]["fontSize"] == 80, "text fixed-width side resize")
            shot(editor, "text-resized")
            click(editor, 35, 62)
            save_layers(lambda values: values[1]["width"] == 180, "text resize undone")
            # The top rotation handle would be outside the canvas; use the lower one.
            drag((578, 375), (428, 229), shift=True)
            save_layers(lambda values: math.isclose(values[1].get("rotation", 0), math.pi / 2, abs_tol=1e-6),
                        "text quarter-turn")
            run("xdotool", "mousemove", "0", "0")
            shot(editor, "text-rotated")
            click(editor, 35, 62)
            save_layers(lambda values: values[1].get("rotation", 0) == 0, "text rotation undone")
            close(editor)
            wait(lambda: not windows("Screenshot editor"), "font-backed editor closes")
            editor = reopen()
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            run("xdotool", "mousemove", "0", "0")
            shot(editor, "text-draft-minimum-reopened")
            run("xdotool", "windowsize", "--sync", editor, "1000", "800")
            click(editor, 535, 62)
            inspector_click(65, 463)
            export_click("copy")
            shot(editor, "text-draft-output-copy")
            png = output / "clipboard-text.png"
            # Encoding and clipboard publication complete asynchronously. Wait
            # for bytes, without resubmitting the accepted Copy action.
            png.write_bytes(wait(lambda: subprocess.run(
                ["xclip", "-selection", "clipboard", "-t", "image/png", "-o"],
                env=env, capture_output=True, timeout=5).stdout or None, "text clipboard PNG"))
            assert run("identify", "-format", "%wx%h", str(png)) == b"640x360"
            # Known font metrics: right-aligned L at x374/y58, fi at x394/y162.
            for x, y, rgba in [(375, 65, (255, 0, 0, 255)), (396, 170, (255, 0, 0, 255)),
                               (400, 80, (247, 247, 245, 255)), (2, 1, (40, 110, 166, 255))]:
                actual = run("convert", str(png), "-crop", f"1x1+{x}+{y}", "-depth", "8", "rgba:-")
                assert actual == bytes(rgba), (x, y, actual, rgba)
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "text draft suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["text-draft-restored", "font-bytes-preserved", "text-canvas-move-undo",
                           "text-canvas-resize-undo", "text-canvas-rotation-undo", "text-minimum-reopen",
                           "text-clipboard-dimensions-and-ink", "text-plate-and-original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native text draft: fonts, move/rotate/undo, save/reopen, minimum, clipboard pixels")
            return

        if args.brush_only:
            resize_editor(942, 701)
            save(640, 360, 0, 0)
            source = layers()[0]["src"]
            click(editor, 736, 62)
            inspector_click(170, 221)  # Restore before the first edit reports a recoverable error.
            fixture_click((108, 189))
            shot(editor, "brush-restore-error")
            save_layers(lambda values: values[0]["src"] == source, "restore without original is atomic")
            inspector_click(101, 221)  # Erase; keep shipping diameter/softness defaults.
            shot(editor, "brush-controls")
            before = draft.read_bytes()
            brush_start = fixture_point((108, 189))
            brush_end = fixture_point((208, 229))
            run("xdotool", "mousemove", "--window", editor, *map(str, brush_start), "mousedown", "1",
                "sleep", ".2", "mousemove", "--sync", "--window", editor, *map(str, brush_end), "sleep", ".3")
            shot(editor, "brush-active")
            assert draft.read_bytes() == before, "brush preview must not persist pixels"
            run("xdotool", "key", "Escape", "mouseup", "1", "sleep", ".3")
            save_layers(lambda values: values[0]["src"] == source, "escape cancels brush")
            drag((338, 189), (438, 229))
            erased = save_layers(lambda values: values[0]["src"] != source, "erase completed stroke")[0]
            assert erased["originalSrc"] == source and erased["locked"]
            asset_pixel(erased, 100, 100, (0, 0, 0, 0))
            asset_pixel(erased, 150, 120, (0, 0, 0, 0))
            asset_pixel(erased, 200, 140, (0, 0, 0, 0))
            edge = asset_pixel(erased, 86, 100)
            assert edge[:3] == bytes((229, 179, 68)) and 0 < edge[3] < 255, edge
            asset_pixel(erased, 2, 1, (40, 110, 166, 255))
            shot(editor, "brush-erased")
            click(editor, 35, 62)
            save_layers(lambda values: values[0]["src"] == source, "one-step brush undo")
            click(editor, 98, 62)
            save_layers(lambda values: values[0]["src"] == erased["src"], "brush redo")
            inspector_click(170, 221)
            drag((338, 189), (438, 229))
            restored = save_layers(lambda values: values[0]["src"] != erased["src"], "restore stroke")[0]
            assert restored["originalSrc"] == source
            asset_pixel(restored, 150, 120, (229, 179, 68, 255))
            shot(editor, "brush-restored")
            click(editor, 35, 62)
            save_layers(lambda values: values[0]["src"] == erased["src"], "undo restore")
            close(editor)
            wait(lambda: not windows("Screenshot editor"), "brush draft closes")
            editor = reopen()
            asset_pixel(layers()[0], 150, 120, (0, 0, 0, 0))
            resize_editor(942, 701)
            click(editor, 736, 62)
            inspector_click(101, 221)
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, "brush-minimum-reopened")
            run("xdotool", "windowsize", "--sync", editor, "1000", "800")
            click(editor, 535, 62)
            inspector_click(65, 463)
            export_click("copy")
            shot(editor, "brush-output-copied")
            png = output / "clipboard-brush.png"
            png.write_bytes(run("xclip", "-selection", "clipboard", "-t", "image/png", "-o"))
            assert run("convert", str(png), "-crop", "1x1+150+120", "-depth", "8", "rgba:-") == bytes((0, 0, 0, 0))
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "brush suite quits")
            assert app.returncode == 0
            checks = ["restore-missing-original-retry", "brush-preview-no-write-and-cancel",
                      "erase-locked-image-interpolated-pixels", "brush-feathered-alpha",
                      "brush-single-undo-redo", "restore-retained-original", "brush-minimum-draft-reopen",
                      "brush-clipboard-alpha-original-unchanged"]
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance, "checks": checks,
            }, indent=2) + "\n")
            print("PASS native brushes: cancel, soft erase, restore, undo/redo, draft, clipboard alpha")
            return

        if args.wand_only:
            resize_editor(942, 701)
            save(640, 360, 0, 0)
            source = layers()[0]["src"]
            click(editor, 736, 62)
            inspector_click(36, 221)
            shot(editor, "wand-controls")
            # Pick authored document point (100,100). Original capture stays locked.
            fixture_click((108, 189))
            edited = save_layers(lambda values: values[0]["src"] != source, "wand edit")[0]
            assert edited["locked"] and edited["originalSrc"] == source

            asset_pixel(edited, 100, 100, (0, 0, 0, 0))
            asset_pixel(edited, 310, 60, (229, 179, 68, 255))
            asset_pixel(edited, 2, 1, (40, 110, 166, 255))
            shot(editor, "wand-contiguous")
            fixture_click((108, 189))  # Transparent seed fails; must not create a new asset.
            shot(editor, "wand-no-match")
            save_layers(lambda values: values[0]["src"] == edited["src"], "no-match preserves pixels")
            click(editor, 35, 62)
            save_layers(lambda values: values[0]["src"] == source, "wand undo")
            click(editor, 98, 62)
            save_layers(lambda values: values[0]["src"] == edited["src"], "wand redo")
            click(editor, 35, 62)
            save_layers(lambda values: values[0]["src"] == source, "undo before global removal")
            inspector_click(128, 308)  # Disable Contiguous below the four tool rows.
            fixture_click((108, 189))
            global_edit = save_layers(lambda values: values[0]["src"] != source, "global wand")[0]
            asset_pixel(global_edit, 100, 100, (0, 0, 0, 0))
            asset_pixel(global_edit, 310, 60, (0, 0, 0, 0))
            assert global_edit["originalSrc"] == source
            shot(editor, "wand-global")
            close(editor)
            wait(lambda: not windows("Screenshot editor"), "wand draft closes")
            editor = reopen()
            asset_pixel(layers()[0], 310, 60, (0, 0, 0, 0))
            resize_editor(942, 701)
            click(editor, 736, 62)
            inspector_click(36, 221)
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, "wand-minimum-reopened")
            run("xdotool", "windowsize", "--sync", editor, "1000", "800")
            click(editor, 535, 62)
            inspector_click(65, 463)  # Preview PNG before copying the edited frame.
            export_click("copy")
            wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
                 "wand clipboard copy")
            shot(editor, "wand-output-copied")
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

        if args.trim_only:
            run("xdotool", "windowsize", "--sync", editor, "1000", "1000")
            field(428, 720)
            field(472, 420)
            inspector_click(58, 516)
            save(720, 420, 0, 0)
            shot(editor, "trim-before")
            inspector_click(52, 807)
            save(640, 360, 0, 0)
            shot(editor, "trim-applied")
            click(editor, 35, 62)
            save(720, 420, 0, 0)
            click(editor, 98, 62)
            save(640, 360, 0, 0)
            close(editor)
            wait(lambda: not windows("Screenshot editor"), "trim draft closes")
            editor = reopen()
            save(640, 360, 0, 0)
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            inspector_move(180, 400, "click", "--repeat", "16", "5")
            shot(editor, "trim-minimum-reopened")
            run("xdotool", "windowsize", "--sync", editor, "1000", "800")
            click(editor, 535, 62)
            inspector_click(65, 463)
            export_click("copy")
            png = output / "clipboard-trim.png"
            png.write_bytes(run("xclip", "-selection", "clipboard", "-t", "image/png", "-o"))
            assert run("identify", "-format", "%wx%h", str(png)) == b"640x360"
            assert run("convert", str(png), "-crop", "1x1+2+1", "-depth", "8", "rgba:-") == bytes((40, 110, 166, 255))
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "trim suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["trim-locked-capture-bounds", "trim-undo-redo", "trim-draft-reopen",
                           "trim-minimum-scroll", "trim-clipboard-dimensions-pixels-original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native trim: canvas bounds, undo/redo, draft, minimum, clipboard pixels")
            return

        if args.background_only:
            resize_editor(1000, 801)
            field(428, 720)
            field(472, 420)
            inspector_click(58, 516)
            save(720, 420, 0, 0)
            before = draft.read_bytes()
            shot(editor, "background-controls")
            field(633, "#214365", x=95)
            assert draft.read_bytes() == before, "unapplied fields must not edit the draft"
            inspector_click(75, 670)

            def background_is(color):
                return json.loads(draft.read_text())["document"]["background"] == color

            save_until(lambda: background_is("#214365"), "solid canvas background")
            shot(editor, "background-solid")
            fixture_pixel("background-solid", 700, 500, (33, 67, 101))
            fixture_pixel("background-solid", 40, 120, (40, 110, 166))
            field(633, "invalid", x=95)
            inspector_click(75, 670)
            shot(editor, "background-error")
            assert background_is("#214365")
            fixture_pixel("background-error", 700, 500, (33, 67, 101))
            # The error row adds 27px below the toolbar until the next command.
            inspector_click(20, 595 + 27)
            inspector_click(75, 670 + 27)
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
            inspector_move(180, 400, "click", "--repeat", "12", "5")
            shot(editor, "background-transparent-minimum-reopened")
            run("xdotool", "windowsize", "--sync", editor, "1000", "800")
            click(editor, 535, 62)
            inspector_click(65, 463)  # Preview PNG, then copy the edited frame.
            export_click("copy")
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
        document_pixel(f"viewport-before-{args.appearance}", 72, 111, (40, 110, 166))
        zoom_anchor = document_point((450, 250))
        # Probe just outside the green rectangle at x=400. Two wheel steps
        # about x=450 must move its left edge across this initially blue point.
        zoom_probe = document_point((397, 250))
        pixel(f"viewport-before-{args.appearance}", *zoom_probe, (40, 110, 166))
        run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, zoom_anchor),
            "keydown", "ctrl", "click", "4", "click", "4", "keyup", "ctrl", "sleep", ".3")
        shot(editor, f"viewport-zoom-{args.appearance}")
        pixel(f"viewport-zoom-{args.appearance}", *zoom_anchor, (46, 158, 113))
        pixel(f"viewport-zoom-{args.appearance}", *zoom_probe, (46, 158, 113))
        assert not draft.exists(), "zoom must not create a draft"
        pan_target = (zoom_anchor[0] + 65, zoom_anchor[1] + 40)
        run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, zoom_anchor),
            "mousedown", "2", "mousemove", "--sync", "--window", editor, *map(str, pan_target),
            "sleep", ".3")
        shot(editor, f"viewport-pan-active-{args.appearance}")
        assert not draft.exists(), "active pan must not enqueue an edit"
        run("xdotool", "mouseup", "2", "sleep", ".3")
        shot(editor, f"viewport-pan-settled-{args.appearance}")
        pixel(f"viewport-pan-settled-{args.appearance}", *pan_target, (46, 158, 113))
        pixel(f"viewport-pan-settled-{args.appearance}", *zoom_probe, (40, 110, 166))
        pixel(f"viewport-pan-settled-{args.appearance}", zoom_probe[0] + 65,
              zoom_probe[1] + 40, (46, 158, 113))
        assert not draft.exists(), "settled pan must not enqueue an edit"
        # Recenter keeps zoom; Fit restores the historical fixture geometry.
        click(editor, 840, 25)
        shot(editor, f"viewport-recenter-{args.appearance}")
        click(editor, 590, 18)
        shot(editor, f"viewport-fit-{args.appearance}")
        document_pixel(f"viewport-fit-{args.appearance}", 102, 111, (229, 179, 68))
        document_pixel(f"viewport-fit-{args.appearance}", 72, 111, (40, 110, 166))
        click(editor, 668, 18)
        shot(editor, "viewport-presets-menu")
        click(editor, 660, 145)  # 100% preset, without a custom row.
        shot(editor, "viewport-actual-button")
        click(editor, 778, 18)  # + (1.25x)
        shot(editor, "viewport-125-button")
        run("xdotool", "key", "ctrl+0", "sleep", ".3")
        shot(editor, "viewport-actual-key")
        run("xdotool", "key", "ctrl+equal", "sleep", ".3")
        shot(editor, "viewport-125-key")

        def viewport_pixels(name):
            return run("convert", str(output / f"{name}.png"), "-crop", "584x500+64+89", "-depth", "8", "rgba:-")

        assert viewport_pixels("viewport-actual-key") == viewport_pixels("viewport-actual-button")
        assert viewport_pixels("viewport-125-key") == viewport_pixels("viewport-125-button")
        assert viewport_pixels("viewport-actual-key") != viewport_pixels("viewport-125-key")
        inspector_click(75, 428)  # Focus the canvas width field; shortcuts still zoom.
        run("xdotool", "key", "ctrl+minus", "sleep", ".3")
        shot(editor, "viewport-field-key")
        assert viewport_pixels("viewport-field-key") == viewport_pixels("viewport-actual-button")
        run("xdotool", "key", "Escape")
        click(editor, 668, 18)
        click(editor, 660, 101)  # 50% preset.
        shot(editor, "viewport-preset-50")
        # 640×360 at 50% is 320×180, centered in the 584×603 viewport.
        # The 56px rail moves the viewport center right by 28px.
        pixel("viewport-preset-50", 198, 310, (40, 110, 166))
        pixel("viewport-preset-50", 253, 340, (229, 179, 68))
        pixel("viewport-preset-50", 191, 310,
              (245, 245, 247) if args.appearance == "light" else (16, 16, 20))
        pixel("viewport-preset-50", 516, 310,
              (245, 245, 247) if args.appearance == "light" else (16, 16, 20))
        click(editor, 668, 18)
        click(editor, 660, 189)  # 200% preset.
        shot(editor, "viewport-preset-200")
        run("xdotool", "key", "ctrl+0", "ctrl+equal", "ctrl+equal", "sleep", ".3")
        shot(editor, "viewport-preset-custom")
        click(editor, 668, 18)
        shot(editor, "viewport-presets-custom-menu")
        click(editor, 660, 233)  # 200% now follows the custom percentage row.
        shot(editor, "viewport-preset-200-from-custom")
        assert viewport_pixels("viewport-preset-200") == viewport_pixels("viewport-preset-200-from-custom")
        assert viewport_pixels("viewport-preset-50") != viewport_pixels("viewport-preset-200")
        click(editor, 668, 18)
        click(editor, 660, 57)  # Fit removes the custom row and resets pan.
        shot(editor, "viewport-preset-fit")
        assert viewport_pixels("viewport-preset-fit") == viewport_pixels(f"viewport-fit-{args.appearance}")
        run("xdotool", "mousemove", "--sync", "--window", editor, "290", "250",
            "mousedown", "2", "mousemove", "--sync", "--window", editor, "355", "290",
            "mouseup", "2", "sleep", ".3")
        shot(editor, "viewport-fit-panned")
        assert viewport_pixels("viewport-fit-panned") != viewport_pixels("viewport-preset-fit")
        click(editor, 668, 18)
        click(editor, 660, 57)  # Reselecting Fit must reset pan even when already selected.
        shot(editor, "viewport-preset-fit-reselected")
        assert viewport_pixels("viewport-preset-fit-reselected") == viewport_pixels("viewport-preset-fit")
        assert not draft.exists(), "toolbar zoom must remain outside draft state"
        click(editor, 590, 18)  # Fit also cancels any viewport gesture and restores coordinates.
        if args.zoom_only:
            # With spare width AND height, Fit keeps the 640×360 source at 1×.
            # An uncapped fit would paint beyond both independently checked edges.
            run("xdotool", "windowsize", "--sync", editor, "1180", "900", "sleep", ".3")
            shot(editor, "viewport-fit-no-upscale")
            surface = (245, 245, 247) if args.appearance == "light" else (16, 16, 20)
            # Client 1180×900 minus the rail, inspector and central-panel margins
            # leaves x=64..942, y=89..892. Its center is (503,490.5), so the
            # 640×360 source spans x=183..823, y=310.5..670.5. Raster sample
            # centers at the bottom edge are excluded by the top-left fill rule.
            pixel("viewport-fit-no-upscale", 822, 400, (40, 110, 166))
            pixel("viewport-fit-no-upscale", 823, 400, surface)
            pixel("viewport-fit-no-upscale", 300, 669, (40, 110, 166))
            pixel("viewport-fit-no-upscale", 300, 670, surface)
            assert not draft.exists(), "Fit resizing must not create a draft"
            run("xdotool", "windowsize", "--sync", editor, "760", "540",
                "key", "ctrl+0", "ctrl+equal", "ctrl+equal", "sleep", ".3")
            shot(editor, "viewport-preset-custom-minimum")
            click(editor, 542, 18)
            shot(editor, "viewport-presets-custom-menu-minimum")
            run("xdotool", "key", "Escape")
            click(editor, 465, 18)  # Fit resets the viewport-center anchor.
            click(editor, 311, 18)  # Left end of the logarithmic slider: 5%.
            shot(editor, "viewport-slider-minimum")
            # Wrapped toolbar leaves x=64..522, y=133..532, center (293,332.5).
            # The 5% source is 32×18, starting at (277,323.5).
            pixel("viewport-slider-minimum", 278, 325, (40, 110, 166))
            pixel("viewport-slider-minimum", 276, 325, surface)
            pixel("viewport-slider-minimum", 309, 325, surface)
            pixel("viewport-slider-minimum", 278, 341, surface)
            click(editor, 437, 18)  # Right end: 800%, preserving the same anchor.
            shot(editor, "viewport-slider-maximum")
            pixel("viewport-slider-maximum", 66, 150, (40, 110, 166))
            pixel("viewport-slider-maximum", 520, 530, (40, 110, 166))
            assert not draft.exists(), "slider changes must not create a draft"
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "zoom suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["viewport-wheel-anchor", "viewport-toolbar-fit-100-step-recenter",
                           "viewport-pan-active-settled-no-draft", "viewport-fit-pixels",
                           "viewport-keyboard-actual-and-step-pixels", "viewport-field-key-no-draft",
                           "viewport-presets-50-200-pixels", "viewport-custom-preset-menu",
                           "viewport-preset-fit-no-draft", "viewport-reselect-fit-clears-pan",
                           "viewport-fit-no-upscale", "viewport-custom-menu-minimum",
                           "viewport-slider-5-percent-pixels", "viewport-slider-800-percent-pixels"],
            }, indent=2) + "\n")
            print("PASS native zoom: wheel, pan, toolbar, keyboard, presets, custom zoom, focused field, no draft")
            return
        # Preserve a pixel-aligned 640px viewport beside the new 56px rail.
        resize_editor(942, 701)
        click(editor, 736, 62)
        inspector_click(154, 177)  # Pen follows Arrow on the second tool row.
        fixture_move((88, 329), "mousedown", "1", "sleep", ".2")
        for point in [(378, 209), (438, 329), (518, 249)]:
            x, y = canvas_point(point)
            run("xdotool", "mousemove", "--sync", "--window", editor, str(x), str(y), "sleep", ".2")
        shot(editor, "freehand-transient")
        assert not draft.exists()
        # First quadratic at t=1/2: (132.5,165) document pixels. The control
        # point (140,120) must remain unpainted, unlike an unsmoothed polyline.
        fixture_pixel("freehand-transient", 141, 254, (255, 59, 92))
        fixture_pixel("freehand-transient", 148, 209, (229, 179, 68))
        fixture_pixel("freehand-transient", 86, 331, (255, 59, 92))  # round start cap
        fixture_pixel("freehand-transient", 290, 247, (255, 59, 92))  # round end cap
        run("xdotool", "mouseup", "1", "sleep", ".3")
        curve = save_layers(lambda values: len(values) == 2, "freehand curve")[-1]
        assert curve["kind"] == "path" and curve["style"]["fill"] is None
        assert len(curve["points"]) == 4
        for point, expected in zip(curve["points"], [(80, 240), (140, 120), (200, 240), (280, 160)]):
            assert abs(point["x"] - expected[0]) < 1e-12 and abs(point["y"] - expected[1]) < 1e-12
        shot(editor, "freehand-curve")
        fixture_pixel("freehand-curve", 141, 254, (255, 59, 92))
        fixture_pixel("freehand-curve", 148, 209, (229, 179, 68))
        fixture_pixel("freehand-curve", 86, 331, (255, 59, 92))
        fixture_pixel("freehand-curve", 290, 247, (255, 59, 92))
        drag((700, 420), (701, 420))  # Under 1.5 screen pixels: keep only the press sample.
        dot = save_layers(lambda values: len(values) == 3, "freehand one-point dot")[-1]
        assert dot["kind"] == "path" and len(dot["points"]) == 1
        shot(editor, "freehand-dot")
        fixture_pixel("freehand-dot", 470, 420, (255, 59, 92))
        click(editor, 35, 62)
        save_layers(lambda values: len(values) == 2, "freehand dot undo")
        before_cancel = draft.read_bytes()
        cancel_start = fixture_point((170, 400))
        cancel_end = fixture_point((230, 410))
        run("xdotool", "mousemove", "--window", editor, *map(str, cancel_start), "mousedown", "1",
            "sleep", ".2", "mousemove", "--sync", "--window", editor, *map(str, cancel_end),
            "sleep", ".2", "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
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
        resize_editor(942, 701)
        shot(editor, "freehand-reopened")
        fixture_pixel("freehand-reopened", 141, 254, (255, 59, 92))
        assert layers()[1]["id"] == curve["id"] and layers()[1]["points"] == curve["points"]
        assert (artifact / "capture.png").read_bytes() == original
        click(editor, 275, 62)
        click(editor, 55, 128)
        wait(lambda: not draft.exists(), "discard freehand edits")

        resize_editor(942, 701)
        click(editor, 736, 62)
        shot(editor, "open-shape-tools")
        inspector_click(32, 177)  # Line starts the second tool row.
        drag((320, 310), (500, 310))
        horizontal = save_layers(lambda values: len(values) == 2, "horizontal line")[-1]
        assert horizontal["shape"] == "line" and horizontal["style"]["fill"] is None
        assert (horizontal["x"], horizontal["y"], horizontal["endX"], horizontal["endY"]) == (82, 221, 262, 221)
        shot(editor, "open-shape-horizontal")
        fixture_pixel("open-shape-horizontal", 170, 310, (255, 59, 92))
        fixture_pixel("open-shape-horizontal", 170, 300, (40, 110, 166))
        drag((540, 360), (540, 200))
        vertical = save_layers(lambda values: len(values) == 3, "reverse vertical line")[-1]
        assert (vertical["x"], vertical["y"], vertical["endX"], vertical["endY"]) == (302, 271, 302, 111)
        fixture_click((470, 420))
        point_line = save_layers(lambda values: len(values) == 4, "zero-length line click")[-1]
        assert (point_line["x"], point_line["y"]) == (point_line["endX"], point_line["endY"])
        inspector_click(94, 176)  # Arrow follows Line.
        before_arrow = draft.read_bytes()
        arrow_start = fixture_point((520, 320))
        arrow_end = fixture_point((350, 190))
        run("xdotool", "mousemove", "--window", editor, *map(str, arrow_start), "mousedown", "1",
            "sleep", ".2", "mousemove", "--sync", "--window", editor, *map(str, arrow_end), "sleep", ".3")
        shot(editor, "open-shape-arrow-transient")
        fixture_pixel("open-shape-arrow-transient", 435, 255, (255, 59, 92))
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
        fixture_pixel("open-shape-arrow", 435, 255, (255, 59, 92))
        fixture_pixel("open-shape-arrow", 310, 260, (255, 59, 92))
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
        resize_editor(942, 701)
        shot(editor, "open-shape-reopened")
        fixture_pixel("open-shape-reopened", 435, 255, (255, 59, 92))
        assert layers()[-1]["id"] == arrow["id"]
        assert (artifact / "capture.png").read_bytes() == original
        click(editor, 275, 62)
        click(editor, 55, 128)
        wait(lambda: not draft.exists(), "discard open-shape edits")

        # Leave room below the annotation form for rotation-snap controls and
        # the 80px pinned export row. This preserves the bottom-scrolled form's
        # coordinates; the narrow/minimum-size scroll path is exercised below.
        run("xdotool", "windowsize", "--sync", editor, "942", "923")
        click(editor, 736, 62)
        inspector_click(105, 133)  # Rectangle follows Text.
        drag((320, 250), (480, 370))
        annotation = save_layers(lambda values: len(values) == 2, "annotation fixture")[-1]
        click(editor, 463, 62)
        inspector_click(79, 300)
        save_layers(lambda values: values[-1]["locked"], "locked annotation remains style editable")
        inspector_move(180, 400, "click", "--repeat", "20", "5")
        shot(editor, "annotation-fields")
        unchanged = draft.read_bytes()
        inspector_click(50, 503)  # Unchanged Apply is disabled.
        field(415, "#23b5a9")
        assert draft.read_bytes() == unchanged
        shot(editor, "annotation-unapplied")
        fixture_pixel("annotation-unapplied", 170, 310, (255, 59, 92))
        inspector_click(150, 503)  # Reset does not mutate the document.
        assert draft.read_bytes() == unchanged
        inspector_click(50, 503)  # A broken Reset would apply the staged cyan here.
        save_layers(lambda values: values[-1]["style"]["fill"] == "#ff3b5c", "reset cleared staged fill")
        field(415, "#23b5a9")
        inspector_click(50, 503)
        save_layers(lambda values: values[-1]["style"]["fill"] == "#23b5a9", "annotation fill")
        shot(editor, "annotation-fill")
        fixture_pixel("annotation-fill", 170, 310, (35, 181, 169))
        click(editor, 35, 62)
        save_layers(lambda values: values[-1]["style"]["fill"] == "#ff3b5c", "one-step style undo")
        click(editor, 98, 62)
        save_layers(lambda values: values[-1]["style"]["fill"] == "#23b5a9", "style redo")
        inspector_click(15, 300)  # Enable stroke, then keep the final controls in view.
        inspector_move(180, 400, "click", "--repeat", "20", "5")
        field(256, "#3269d6")
        field(300, 12, 130)
        inspector_click(15, 344)  # Clear fill.
        inspector_move(180, 400, "click", "--repeat", "20", "5")
        inspector_click(50, 503)
        save_layers(lambda values: values[-1]["style"]["fill"] is None and values[-1]["style"]["strokeWidth"] == 12, "annotation outline")
        shot(editor, "annotation-outline")
        fixture_pixel("annotation-outline", 170, 310, (40, 110, 166))
        fixture_pixel("annotation-outline", 93, 310, (50, 105, 214))
        inspector_click(15, 415)  # Restore fill; enter a different color from the stroke.
        inspector_move(180, 400, "click", "--repeat", "20", "5")
        field(415, "#23b5a9")
        inspector_click(15, 459)  # Enable custom shadow controls.
        inspector_move(180, 400, "click", "--repeat", "25", "5")
        shot(editor, "annotation-shadow-fields")
        field(283, "#ff8800")
        field(327, 80, 125)
        field(371, 0)
        field(415, 25, 90)
        field(459, -12, 90)
        inspector_click(50, 503)
        styled = save_layers(lambda values: values[-1]["style"].get("dropShadowStyle", {}).get("offsetX") == 25, "custom annotation shadow")[-1]
        assert styled["id"] == annotation["id"] and styled["locked"]
        assert styled["style"]["dropShadowStyle"] == {"color": "#ff8800", "opacity": 80, "blur": 0, "offsetX": 25, "offsetY": -12}
        shot(editor, "annotation-shadow")
        fixture_pixel("annotation-shadow", 279, 310, (212, 131, 33), tolerance=1)
        inspector_click(28, 283)
        shot(editor, "annotation-color-picker")
        run("xdotool", "key", "Escape", "sleep", ".2")
        inspector_click(15, 212)  # Disable shadow without losing custom knobs.
        inspector_move(180, 400, "click", "--repeat", "20", "5")
        inspector_click(50, 503)
        disabled = save_layers(lambda values: values[-1]["style"]["dropShadow"] is False, "shadow off")[-1]
        assert disabled["style"]["dropShadowStyle"] == styled["style"]["dropShadowStyle"]
        shot(editor, "annotation-shadow-off")
        fixture_pixel("annotation-shadow-off", 279, 310, (40, 110, 166))
        inspector_click(15, 459)
        inspector_move(180, 400, "click", "--repeat", "25", "5")
        inspector_click(50, 503)
        save_layers(lambda values: values[-1]["style"] == styled["style"], "shadow settings restored")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        inspector_move(180, 400, "click", "--repeat", "25", "5")
        shot(editor, "annotation-minimum")
        close(editor)
        wait(lambda: not windows("Screenshot editor"), "styled editor closes")
        editor = reopen()
        resize_editor(942, 701)
        shot(editor, "annotation-reopened")
        fixture_pixel("annotation-reopened", 279, 310, (212, 131, 33), tolerance=1)
        assert layers()[-1]["style"] == styled["style"]
        assert (artifact / "capture.png").read_bytes() == original
        click(editor, 275, 62)
        click(editor, 55, 128)
        wait(lambda: not draft.exists(), "discard annotation edits")
        resize_editor(1000, 701)

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
        fixture_pixel("imported-canvas", 293, 468, (60, 179, 113))
        fixture_pixel("imported-canvas", 370, 514, (45, 100, 189))
        click(editor, 463, 62)
        inspector_move(180, 400, "click", "--repeat", "25", "4")
        shot(editor, "imported-selected-layer")
        inspector_click(78, 371)
        run("xdotool", "key", "ctrl+a", "ctrl+c", "sleep", ".2")
        assert run("xclip", "-selection", "clipboard", "-o").decode() == imported_path.name
        run("xdotool", "key", "Escape")  # The single-line name field scrolls; its value is intact.
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "imported-minimum")
        inspector_move(180, 400, "click", "--repeat", "8", "5")
        shot(editor, "imported-minimum-scrolled")
        resize_editor(1000, 701)
        inspector_move(180, 400, "click", "--repeat", "12", "4")
        assert imported_path.read_bytes() == imported_bytes
        imported_path.unlink()  # A saved import must no longer depend on its source file.
        click(editor, 35, 62)
        save(640, 360, 0, 0)
        assert len(layers()) == 1
        click(editor, 98, 62)
        save(640, 440, 0, 0)
        assert layers()[-1]["id"] == imported_id

        inspector_click(100, 158)  # Redo retained the original's selection; pick the imported row.
        resize_before = draft.read_bytes()
        resize_start = fixture_point((387, 489))
        resize_end = fixture_point((423, 489))
        run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, resize_start),
            "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
            *map(str, resize_end), "sleep", ".3")
        shot(editor, "layer-resize-active-guides")
        assert draft.read_bytes() == resize_before, "resize preview must remain transient"
        run("xdotool", "mouseup", "1", "sleep", ".3")
        # Fit stays at 1×. Resize follows the absolute pointer, not the press
        # one point inside the grip (adding the raw delta would give 156).
        resized_width = 423 - 8 - 260
        resized = save_layers(
            lambda values: math.isclose(values[-1]["width"], resized_width, abs_tol=1e-5)
            and values[-1]["height"] == 80 and values[-1]["x"] == 260,
            "imported image resized from east grip")[-1]
        assert resized["id"] == imported_id
        shot(editor, "layer-resize-committed")
        # Independently scale the fixture's green center (25,19) from its left edge.
        resized_green = (260 + 25 * resized_width / 120, 360 + 19)
        document_pixel("layer-resize-committed", *resized_green, (60, 179, 113))
        click(editor, 35, 62)
        save_layers(lambda values: values[-1]["width"] == 120 and values[-1]["height"] == 80,
                    "undo imported image resize")
        shot(editor, "layer-resize-undone")
        fixture_pixel("layer-resize-undone", 293, 468, (60, 179, 113))
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
        document_pixel("layer-resize-reopened", *resized_green, (60, 179, 113))
        # Undo history is session-local. Restore through a fresh east-grip resize
        # at 1:1 scale, where the desired edge lands on an exact pointer pixel.
        click(editor, 463, 62)  # Reopened editors start in Geometry, not Layers.
        inspector_click(100, 158)
        resize_editor(942, 701, "sleep", ".3")
        drag((round(238 + 260 + resized_width), 489), (618, 489))
        save_layers(lambda values: values[-1]["width"] == 120 and values[-1]["height"] == 80,
                    "restore imported size after draft reopen")
        resize_editor(1000, 701, "sleep", ".3")

        # At 1× the 120x80 image's grip is (558,425), around pivot (558,489).
        # Start five points above the grip, within its hit radius.
        # Exercise a free-angle transient first; Escape must leave draft/pixels intact.
        rotation_before = draft.read_bytes()
        free_rotation_start = fixture_point((328, 420))
        free_rotation_end = fixture_point((363, 400))
        run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, free_rotation_start),
            "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
            *map(str, free_rotation_end), "sleep", ".3")
        shot(editor, "layer-rotation-free-transient")
        run("xdotool", "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
        assert draft.read_bytes() == rotation_before
        # Vector (0,-69) to (39,-79) is 26.27 degrees, snapping to 30 degrees.
        drag((558, 420), (597, 410), shift=True)
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
        expected_document = (expected_x, expected_y)
        assert tuple(map(round, expected_document)) == (300, 364)
        document_pixel("layer-rotation-shift-result", *expected_document, (60, 179, 113))
        click(editor, 35, 62)
        save_layers(lambda values: "rotation" not in values[-1], "undo imported image rotation")
        shot(editor, "layer-rotation-undone")
        fixture_pixel("layer-rotation-undone", 293, 468, (60, 179, 113))
        click(editor, 98, 62)
        save_layers(lambda values: math.isclose(values[-1].get("rotation", 0), math.pi / 6,
                                                abs_tol=1e-12),
                    "redo imported image rotation")
        close(editor)
        wait(lambda: not windows("Screenshot editor"), "imported draft closes")
        editor = reopen()
        shot(editor, "layer-rotation-reopened")
        document_pixel("layer-rotation-reopened", *expected_document, (60, 179, 113))
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

        # Keep Image transform above the pinned footer while exercising its menu.
        resize_editor(1000, 781)
        click(editor, 463, 62)  # Layers, preserving the Geometry panel's scroll position.
        shot(editor, "layers-original-locked")
        inspector_click(88, 632)
        shot(editor, "layers-transform-menu")
        run("xdotool", "key", "Escape")

        def transform(index, orientation, width, height):
            inspector_click(88, 632)
            inspector_click(58, 464 + 44 * index)
            save_layers(lambda values: values[0].get("orientation") == orientation,
                        f"transform {orientation}")
            assert saved(width, height, 0, 0), "fresh photo rotates its canvas"
            assert layers()[0]["locked"], "transform must not unlock the original"

        def assert_transformed_pixels(name, gold_left, gold_above):
            shot(editor, name)
            window, size = shot_layouts[name]
            left, top, scale = fit_geometry(size, window)
            crop_width, crop_height = round(size[0] * scale), round(size[1] * scale)
            pixels = run("convert", str(output / f"{name}.png"), "-crop",
                         f"{crop_width}x{crop_height}+{round(left)}+{round(top)}",
                         "-depth", "8", "rgb:-")

            def center(color):
                points = [(i // 3 % crop_width, i // 3 // crop_width)
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
        resize_editor(1000, 781)
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
        inspector_click(17, 299)  # A hidden, locked image remains transformable.
        save_layers(lambda values: not values[0]["visible"], "hide original before transform")
        transform(2, "flip-horizontal", 640, 360)
        assert not layers()[0]["visible"]
        click(editor, 36, 62)
        save_layers(lambda values: values[0].get("orientation") is None, "undo hidden transform")
        click(editor, 36, 62)
        save_layers(lambda values: values[0]["visible"], "restore original visibility")
        assert_transformed_pixels("layers-transform-restored", True, True)
        inspector_click(47, 591)  # Duplicate the locked original, not delete or move it.
        first = save_layers(lambda values: len(values) == 2, "duplicate original")
        copy_id = first[1]["id"]
        assert first[0]["locked"] and first[1]["visible"] and not first[1]["locked"]
        assert (first[1]["x"], first[1]["y"]) == (24, 24)
        assert first[0]["src"] == first[1]["src"]
        long_name = "Layer with a deliberately long name to retain"
        field(371, long_name)
        inspector_click(182, 371)
        save_layers(lambda values: values[-1]["name"] == long_name, "renamed image")
        field(459, 190)
        field(503, 70)
        inspector_click(34, 547)
        save_layers(lambda values: (values[-1]["x"], values[-1]["y"]) == (190, 70), "moved duplicate")
        shot(editor, "layers-moved")
        fixture_pixel("layers-moved", 361, 277, (229, 179, 68))
        # Canvas picking is based on the rendered document, not the layer-list selection.
        # Escape cancels the translated outline without touching the saved draft.
        canvas_before = draft.read_bytes()
        move_start = fixture_point((361, 277))
        move_preview = fixture_point((401, 307))
        run("xdotool", "mousemove", "--window", editor, *map(str, move_start), "mousedown", "1",
            "sleep", ".2", "mousemove", "--sync", "--window", editor, *map(str, move_preview), "sleep", ".2")
        shot(editor, "layers-canvas-active-outline")
        run("xdotool", "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
        assert draft.read_bytes() == canvas_before
        fixture_click((30, 200))  # Empty point before the unlocked copy clears selection.
        assert draft.read_bytes() == canvas_before
        # The raw horizontal delta lands just inside the canvas edge's magnetic
        # range. The shared move geometry snaps the duplicate's left edge to the
        # canvas/background layer edge while retaining the asymmetric raw Y move.
        snap_target = fixture_point((177, 307))
        run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, move_start),
            "sleep", ".2", "mousedown", "1", "sleep", ".2", "mousemove", "--sync",
            "--window", editor, *map(str, snap_target), "sleep", ".3")
        shot(editor, "layers-canvas-snapped-guides")
        assert draft.read_bytes() == canvas_before
        run("xdotool", "mouseup", "1", "sleep", ".2")
        # Fit leaves the 640px image at 1×. Raw x=6 snaps to zero; Y stays exact.
        expected_position = (0, 100)
        moved_canvas = save_layers(
            lambda values: all(math.isclose(values[-1][axis], expected, abs_tol=1e-5)
                               for axis, expected in zip(("x", "y"), expected_position)),
            "canvas drag moved duplicate")[-1]
        assert moved_canvas["id"] == copy_id
        shot(editor, "layers-canvas-moved-selection")
        fixture_pixel("layers-canvas-moved-selection", 361, 277, (40, 110, 166))
        fixture_pixel("layers-canvas-moved-selection", 137, 307, (229, 179, 68))
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
        fixture_pixel("layers-snapped-move-reopened", 137, 307, (229, 179, 68))
        # Reopen starts in Geometry and undo history is intentionally not persisted.
        # Switch to Layers and restore explicitly so downstream fixtures stay stable.
        click(editor, 463, 62)
        inspector_click(100, 158)
        field(459, 190)
        field(503, 70)
        inspector_click(34, 547)
        save_layers(lambda values: (values[-1]["x"], values[-1]["y"]) == (190, 70),
                    "restore snapped move after reopen")
        field(415, 50)
        inspector_click(154, 415)
        save_layers(lambda values: values[-1]["opacity"] == 50, "half opacity")
        shot(editor, "layers-half-opacity")
        fixture_pixel("layers-half-opacity", 361, 277, (134, 144, 117), tolerance=1)
        inspector_click(15, 300)  # Hide the copy; the original blue pixel is restored.
        save_layers(lambda values: not values[-1]["visible"], "hidden duplicate")
        shot(editor, "layers-hidden")
        fixture_pixel("layers-hidden", 361, 277, (40, 110, 166))
        click(editor, 35, 62)  # Undo must restore the rendered half-opacity layer.
        save_layers(lambda values: values[-1]["visible"], "undo visibility")
        shot(editor, "layers-undo-visible")
        fixture_pixel("layers-undo-visible", 361, 277, (134, 144, 117), tolerance=1)
        inspector_click(79, 300)
        save_layers(lambda values: values[-1]["locked"], "lock duplicate")
        inspector_click(124, 591)  # Delete is disabled while locked.
        save_layers(lambda values: len(values) == 2 and values[-1]["locked"], "locked layer retained")
        shot(editor, "layers-locked")
        inspector_click(79, 300)
        save_layers(lambda values: not values[-1]["locked"], "unlock duplicate")
        inspector_click(47, 591)
        third = save_layers(lambda values: len(values) == 3, "second duplicate")[-1]["id"]
        assert (layers()[-1]["x"], layers()[-1]["y"]) == (214, 94)
        inspector_click(149, 547)  # Down: the third layer moves behind the first copy.
        save_layers(lambda values: [value["id"] for value in values] == ["capture-background", third, copy_id], "reordered down")
        shot(editor, "layers-reordered")
        inspector_click(92, 547)
        save_layers(lambda values: [value["id"] for value in values] == ["capture-background", copy_id, third], "reordered up")
        inspector_click(124, 591)
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
        fixture_pixel("layers-reopened", 361, 277, (134, 144, 117), tolerance=1)
        assert layers()[-1]["name"] == long_name
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        inspector_move(180, 400, "click", "--repeat", "8", "5")
        shot(editor, "layers-small-scrolled")
        resize_editor(1000, 701)
        inspector_move(180, 400, "click", "--repeat", "12", "4")
        inspector_click(100, 202)  # Select and explicitly unlock the original.
        inspector_click(79, 300)
        save_layers(lambda values: not values[0]["locked"], "unlock original")
        inspector_click(124, 591)
        save_layers(lambda values: [value["id"] for value in values] == [copy_id], "delete original layer")
        inspector_click(124, 591)
        save_layers(lambda values: len(values) == 0, "empty saved document")
        shot(editor, "layers-empty")
        click(editor, 35, 62)
        save_layers(lambda values: [value["id"] for value in values] == [copy_id], "undo empty document")
        click(editor, 275, 62)
        click(editor, 55, 128)
        wait(lambda: not draft.exists(), "discard layer edits")
        click(editor, 398, 62)  # Geometry has an independent scroll position.

        # Odd height keeps the centered 1:1 document origin pixel-aligned.
        resize_editor(942, 701)

        click(editor, 736, 62)  # Draw keeps the chosen shape active after each release.
        drag((658, 289), (538, 169))
        rectangle = save_layers(lambda values: len(values) == 2, "reverse rectangle")[-1]
        assert rectangle["kind"] == "shape" and rectangle["shape"] == "rectangle"
        assert (rectangle["x"], rectangle["y"], rectangle["endX"], rectangle["endY"]) == (420, 200, 300, 80)
        assert rectangle["style"]["fill"] == "#ff3b5c" and not rectangle["style"]["strokeEnabled"]
        shot(editor, "shape-rectangle")
        fixture_pixel("shape-rectangle", 370, 230, (255, 59, 92))
        click(editor, 35, 62)
        save_layers(lambda values: len(values) == 1, "single-step shape undo")
        shot(editor, "shape-undone")
        fixture_pixel("shape-undone", 370, 230, (40, 110, 166))
        click(editor, 98, 62)
        save_layers(lambda values: len(values) == 2 and values[-1]["id"] == rectangle["id"], "shape redo keeps id")
        inspector_click(185, 133)  # Ellipse.
        drag((608, 349), (778, 399))
        ellipse = save_layers(lambda values: len(values) == 3, "ellipse layer")[-1]
        assert ellipse["shape"] == "ellipse" and ellipse["id"] != rectangle["id"]
        assert (ellipse["x"], ellipse["y"], ellipse["endX"], ellipse["endY"]) == (370, 260, 540, 310)
        shot(editor, "shape-ellipse")
        fixture_pixel("shape-ellipse", 463, 374, (255, 59, 92))
        fixture_pixel("shape-ellipse", 380, 350, (40, 110, 166))
        before_draw = draft.read_bytes()
        shape_start = fixture_point((90, 300))
        shape_end = fixture_point((190, 380))
        run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, shape_start), "mousedown", "1",
            "sleep", ".2", "mousemove", "--sync", "--window", editor, *map(str, shape_end), "sleep", ".3")
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
        resize_editor(942, 701)
        shot(editor, "shape-reopened")
        fixture_pixel("shape-reopened", 370, 230, (255, 59, 92))
        fixture_pixel("shape-reopened", 463, 374, (255, 59, 92))
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

        inspector_click(159, 335)
        drag((638, 359), (278, 119))  # Reverse drag: 40,30 with size 360x240.
        shot(editor, "crop-selection")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "crop-selection-minimum")
        resize_editor(942, 701)
        assert not draft.exists(), "selection must not write a draft"
        assert (artifact / "capture.png").read_bytes() == original
        run("xdotool", "key", "Escape", "sleep", ".2")
        shot(editor, "crop-cancelled")
        save(640, 360, 0, 0)  # Escape must not crop or mutate the document.
        before_selection = draft.read_bytes()
        inspector_click(159, 335)
        drag((278, 119), (438, 219), shift=True)
        shot(editor, "crop-shift-square")
        assert draft.read_bytes() == before_selection
        inspector_click(50, 335)
        save(160, 160, -40, -30)
        click(editor, 35, 62)
        save(640, 360, 0, 0)
        inspector_click(159, 335)
        drag((638, 500), (278, 119))  # Starts below the image; clamps to y=360.
        shot(editor, "crop-outside-start")
        inspector_click(50, 335)
        save(360, 330, -40, -30)
        click(editor, 35, 62)
        save(640, 360, 0, 0)
        inspector_click(159, 335)
        inspector_click(125, 379)
        shot(editor, "crop-aspect-menu")
        run("xdotool", "key", "Escape", "sleep", ".2")
        inspector_click(125, 379)
        inspector_click(106, 507)  # 4:3 preset takes precedence over Shift's square.
        drag((278, 119), (438, 219), shift=True)
        shot(editor, "crop-preset-four-three")
        inspector_click(50, 335)
        save(160, 120, -40, -30)
        click(editor, 35, 62)
        save(640, 360, 0, 0)
        inspector_click(159, 335)
        inspector_click(125, 379)
        inspector_click(106, 419)  # Free for the following asymmetric crop.
        drag((638, 359), (278, 119))
        inspector_click(159, 335)  # Cancel restores numeric fields as well as pixels.
        inspector_click(50, 335)
        save(640, 360, 0, 0)
        inspector_click(159, 335)
        drag((638, 359), (278, 119))
        inspector_click(50, 335)
        resize_editor(1000, 701)
        save(360, 240, -40, -30)
        shot(editor, "editor-cropped")
        fixture_pixel("editor-cropped", 308, 299, (40, 110, 166))
        fixture_pixel("editor-cropped", 120, 200, (229, 179, 68))
        click(editor, 35, 62)  # Undo
        save(640, 360, 0, 0)
        click(editor, 98, 62)  # Redo
        save(360, 240, -40, -30)
        field(428, 480)
        field(472, 300)
        inspector_click(58, 516)
        save(480, 300, -40, -30)
        shot(editor, "editor-resized")
        fixture_pixel("editor-resized", 428, 289, (46, 158, 113))

        saved_draft = draft.read_bytes()
        resize_editor(1000, 901)
        click(editor, 535, 62)  # Output: encode the edited frame, not History PNG.
        inspector_click(65, 463)
        shot(editor, "output-png")
        fixture_pixel("output-png", 120, 200, (229, 179, 68))
        fixture_pixel("output-png", 428, 289, (46, 158, 113))
        inspector_click(20, 371)  # PNG Compress with an explicit palette.
        inspector_click(20, 503)
        field(547, 4)
        inspector_click(65, 595)
        shot(editor, "output-png-palette")
        fixture_pixel("output-png-palette", 428, 289, (46, 158, 113))
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        inspector_move(180, 400, "click", "--repeat", "8", "5")
        shot(editor, "output-palette-minimum-scrolled")
        resize_editor(1000, 901)
        inspector_move(180, 400, "click", "--repeat", "12", "4")
        inspector_click(85, 256)  # JPEG invalidates the PNG comparison.
        inspector_click(20, 371)  # Compress.
        inspector_click(65, 507)
        shot(editor, "output-jpeg")
        fixture_pixel("output-jpeg", 428, 289, (46, 158, 113), tolerance=4)
        inspector_click(65, 579)  # Edited canvas comparison.
        shot(editor, "output-edited-canvas")
        fixture_pixel("output-edited-canvas", 428, 289, (46, 158, 113))
        inspector_click(65, 622)  # Encoded output comparison.
        inspector_click(150, 256)  # WebP, still Compress.
        inspector_click(65, 507)
        shot(editor, "output-webp")
        fixture_pixel("output-webp", 428, 289, (46, 158, 113), tolerance=4)
        inspector_click(20, 415)  # Maximum file size enables the hard cap.
        field(459, 0)
        inspector_click(65, 507)
        shot(editor, "output-budget-error")
        assert app.poll() is None and windows("Screenshot editor")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "output-budget-error-minimum")
        resize_editor(1000, 901)
        inspector_click(20, 354)  # Preserve clears the failed budget (error adds 27px).
        inspector_click(65, 490)  # Retry clears error without persisting a draft.
        shot(editor, "output-retry")
        fixture_pixel("output-retry", 428, 289, (46, 158, 113))
        assert draft.read_bytes() == saved_draft, "preview must not write a draft"
        assert not (output / "exports").exists(), "preview must not publish files"

        exported = output / "exports" / "edited.webp"
        initial_directory = output / "unchosen folder"
        initial_directory.mkdir()
        exported.parent.mkdir()
        chooser.selected = exported.parent
        chooser.calls.clear()
        field(715, initial_directory / exported.name)
        inspector_click(149, 681)
        wait(lambda: len(chooser.calls) == 1, "folder dialog cancellation")
        shot(editor, "export-folder-pending")
        export_click("save")  # Save is disabled until the folder choice completes.
        assert not list(initial_directory.iterdir()) and not list(exported.parent.iterdir())
        GLib.idle_add(chooser.respond, True)
        time.sleep(.5)
        inspector_click(149, 681)
        wait(lambda: len(chooser.calls) == 2, "folder dialog selection")
        GLib.idle_add(chooser.respond, False)
        time.sleep(.5)
        for title, options in chooser.calls:
            assert title == "Choose save location" and options["directory"] and not options.get("multiple", False)
            assert bytes(options["current_folder"]).rstrip(b"\0") == os.fsencode(initial_directory)
        assert not list(initial_directory.iterdir()) and not list(exported.parent.iterdir())
        assert draft.read_bytes() == saved_draft and len(list(history.glob("*/metadata.json"))) == 1
        shot(editor, "export-folder-selected")
        export_click("save")
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
        inspector_move(180, 400, "click", "--repeat", "12", "5")
        shot(editor, "export-saved-scrolled")
        inspector_move(180, 400, "click", "--repeat", "20", "4")
        export_click("save")  # Same filename must fail rather than replace.
        shot(editor, "export-collision")
        assert exported.read_bytes() == exported_bytes
        assert len(list(history.glob("*/metadata.json"))) == 2

        # Retry after a real History failure must report the successfully saved file.
        history.rename(output / "previous-history")
        history.write_text("blocks History creation")
        recovered = output / "exports" / "recovered.webp"
        field(742, recovered)  # The collision error adds 27px above the panel.
        export_click("save")  # Editing the destination clears the collision error.
        wait(recovered.exists, "file saved despite unavailable History")
        assert recovered.read_bytes() == exported_bytes
        inspector_move(180, 400, "click", "--repeat", "20", "5")
        shot(editor, "export-history-warning-scrolled")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        inspector_move(180, 400, "click", "--repeat", "20", "5")
        shot(editor, "export-history-warning-minimum")
        inspector_move(100, window_size()[1] - 27, "sleep", "1")
        shot(editor, "export-history-warning-detail-minimum")
        resize_editor(1000, 901)
        history.unlink()
        (output / "previous-history").rename(history)
        assert draft.read_bytes() == saved_draft
        assert (artifact / "capture.png").read_bytes() == original

        inspector_move(180, 400, "click", "--repeat", "20", "4")
        export_click("copy")  # Copy the edited canvas, not the History source.
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
        inspector_move(180, 400, "click", "--repeat", "20", "5")
        shot(editor, "clipboard-copied")
        click(editor, 398, 62)  # Geometry restores its own scroll position.

        close(editor)
        wait(lambda: not windows("Screenshot editor"), "saved editor closes")
        assert run("xclip", "-selection", "clipboard", "-t", "image/png", "-o") == copied, "workspace retains clipboard after editor closes"
        editor = reopen()
        shot(editor, "editor-reopened")
        fixture_pixel("editor-reopened", 428, 289, (46, 158, 113))
        field(428, 510)
        inspector_click(58, 516)
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
        inspector_click(58, 516)
        click(editor, 170, 62)
        shot(editor, "editor-save-error")
        assert app.poll() is None and windows("Screenshot editor")
        close(root)  # With no tray host, normal root close must flush or cancel.
        assert app.poll() is None and windows("Screenshot editor"), "failed flush must cancel quit"
        shot(editor, "editor-quit-error")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "editor-small-error")
        inspector_move(180, 400, "click", "--repeat", "8", "5")
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
                       "viewport-keyboard-actual-and-step-pixels", "viewport-field-key-no-draft",
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
