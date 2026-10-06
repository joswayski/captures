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
from urllib.parse import unquote, urlparse

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
        selected = self.selected if isinstance(self.selected, list) else [self.selected]
        self.pending.Response(1 if cancel else 0, {} if cancel else {
            "uris": dbus.Array([path.as_uri() for path in selected], signature="s"),
        })
        self.pending = None
        return False


class FileManager(dbus.service.Object):
    """Record Save's folder reveal without launching a real file manager."""

    def __init__(self, bus):
        self.name = dbus.service.BusName("org.freedesktop.FileManager1", bus=bus, do_not_queue=True)
        super().__init__(self.name, "/org/freedesktop/FileManager1")
        self.revealed = []

    @dbus.service.method("org.freedesktop.FileManager1", in_signature="ass", out_signature="")
    def ShowItems(self, uris, startup_id):
        self.revealed.extend(Path(unquote(urlparse(str(uri)).path)) for uri in uris)


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
    parser.add_argument("--combine-only", action="store_true",
                        help="Exercise real-input layer merging, flattening, history and output pixels")
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
    parser.add_argument("--drawing-defaults-only", action="store_true",
                        help="Exercise pre-placement shape style, opacity, retained defaults and pixels")
    parser.add_argument("--rotation-snap-only", action="store_true",
                        help="Exercise custom Shift rotation stops without saving the UI setting")
    parser.add_argument("--output-presets-only", action="store_true",
                        help="Exercise compression presets and real saved PNG pixels")
    parser.add_argument("--output-size-only", action="store_true",
                        help="Exercise percentage/custom export dimensions without changing the document")
    parser.add_argument("--overwrite-only", action="store_true",
                        help="Exercise confirmed original replacement, History identity and retained drafts")
    parser.add_argument("--canvas-interactions-only", action="store_true",
                        help="Exercise image file drop guides, Expand canvas and line curve editing")
    parser.add_argument("--shape-transforms-only", action="store_true",
                        help="Transform freshly drawn shapes without switching tools; undo and reopen")
    parser.add_argument("--properties-heading-only", action="store_true",
                        help="Keep Properties titles visible while fields scroll at minimum size")
    parser.add_argument("--external-image-only", action="store_true",
                        help="Open external images, preserve per-file errors and safely reopen drafts/sources")
    parser.add_argument("--import-formats-only", action="store_true",
                        help="Import GIF first frames, BMP and SVG layers, undo/redo and reopen without sources")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    # Live hosts must never unbind the developer's real OS screenshot keys.
    env["CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER"] = "1"
    env.pop("WAYLAND_DISPLAY", None)
    env.update(WGPU_BACKEND="gl", WINIT_X11_SCALE_FACTOR="1", XDG_SESSION_TYPE="x11")
    for variable, name in (("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data"),
                           ("XDG_CACHE_HOME", "cache"), ("XDG_RUNTIME_DIR", "runtime")):
        path = output / name
        path.mkdir(mode=0o700)
        env[variable] = str(path)
    # Save reveals its file through FileManager1 (below); a stub xdg-open
    # records the folder fallback instead of opening anything.
    opened_folders = output / "opened-folders.jsonl"
    tools = output / "tools"
    tools.mkdir()
    (tools / "xdg-open").write_text("#!/usr/bin/python3\nimport json, sys\n"
        f"with open({str(opened_folders)!r}, 'a') as stream:\n"
        "    stream.write(json.dumps(sys.argv[1:]) + '\\n')\n")
    (tools / "xdg-open").chmod(0o755)
    env["PATH"] = f"{tools}:{env['PATH']}"
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
            # Cold CI X servers can exceed ten seconds before announcing readiness.
            timeout = 60 if name == "xvfb" else 10
            assert select.select([process.stdout], [], [], timeout)[0], (
                f"{name} did not announce readiness within {timeout}s; inspect its stderr")
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

    def active_window():
        result = subprocess.run(["xdotool", "getactivewindow"], env=env,
                                capture_output=True, text=True, timeout=5)
        # Openbox can temporarily unset _NET_ACTIVE_WINDOW while changing focus.
        # Keep polling until the expected window actually owns focus.
        assert result.returncode in (0, 1), result.stderr
        return result.stdout.strip() if result.returncode == 0 else None

    shot_layouts = {}

    def shot(window, name):
        time.sleep(.5)
        run("import", "-window", window, str(output / f"{name}.png"))
        if editor is not None and str(window) == str(editor):
            shot_layouts[name] = (window_size(), document_size(), export_bar_height())

    def pixel(name, x, y, expected, tolerance=0):
        actual = run("convert", str(output / f"{name}.png"), "-crop", f"1x1+{x}+{y}",
                     "-depth", "8", "rgb:-")
        assert len(actual) == 3 and all(abs(a - b) <= tolerance for a, b in zip(actual, expected)), (name, x, y, actual, expected)

    def document_pixel(name, x, y, expected, tolerance=0):
        window, size, bar = shot_layouts[name]
        left, top, scale = fit_geometry(size, window, bar)
        pixel(name, round(left + x * scale), round(top + y * scale), expected, tolerance)

    def settled_document_pixel(name, x, y, expected, tolerance=0):
        """Re-shoot until the document pixel matches, for repaint-driven paint."""
        def check():
            shot(editor, name)
            try:
                document_pixel(name, x, y, expected, tolerance)
                return True
            except AssertionError:
                return False
        if not wait_quiet(check):
            document_pixel(name, x, y, expected, tolerance)

    def wait_quiet(predicate, seconds=10):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if predicate():
                return True
            time.sleep(.1)
        return False

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
    # inspector coordinates are local to the fixed right panel (shipping's 320px
    # sidebar column, INSPECTOR_WIDTH), document points
    # are authored in image pixels, and exported-image pixels never pass through these
    # helpers. A few visual fixtures use the historical screenshot coordinates whose
    # document origin was (8, 89); fixture_to_document names that conversion directly.
    # Shipping's `.screenshot-sidebar` column. Rows were authored for the former
    # 230px panel; its 8px inner margins leave INSPECTOR_CONTENT for controls.
    INSPECTOR_WIDTH = 320
    INSPECTOR_GROWTH = INSPECTOR_WIDTH - 230
    INSPECTOR_CONTENT = INSPECTOR_WIDTH - 16

    def window_size():
        geometry = run("xdotool", "getwindowgeometry", "--shell", editor).decode()
        return tuple(int(re.search(rf"^{axis}=(\d+)$", geometry, re.MULTILINE).group(1))
                     for axis in ("WIDTH", "HEIGHT"))

    def editor_width():
        return window_size()[0]

    def editor_width_of(window):
        geometry = run("xdotool", "getwindowgeometry", "--shell", window).decode()
        return int(re.search(r"^WIDTH=(\d+)$", geometry, re.MULTILINE).group(1))

    def document_size():
        if draft is not None and draft.exists():
            document = json.loads(draft.read_text())["document"]
            return document["width"], document["height"]
        return 640, 360

    def fit_geometry(size=None, window=None, bar=None):
        width, height = window or window_size()
        image_width, image_height = size or document_size()
        available = (64., 60., width - INSPECTOR_WIDTH - 8.,
                     height - 8. - (export_bar_height() if bar is None else bar))
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
        # document's centered Fit origin lands on an integer device pixel. They
        # predate the full-width export bar: add its collapsed 80px so the canvas
        # keeps its historical geometry. The minimum size stays the minimum.
        # The shipping header moved the canvas top from y=89 to y=60; one more
        # pixel keeps those authored heights' Fit origin on an integer pixel.
        if height > 540:
            height += 80 + 1
        # They also predate shipping's 320px sidebar: widen the window by the
        # inspector's growth so the canvas keeps its historical width.
        if width > 760:
            width += INSPECTOR_GROWTH
        run("xdotool", "windowsize", "--sync", editor, str(width), str(height), *map(str, tail))

    def resize_inspector_fixture(width, height, *tail):
        # Inspector-authored fixtures keep their historical inspector height:
        # the export bar replaces the old 80px footer below the inspector.
        # Like resize_editor, the window grows with the 320px sidebar.
        if width > 760:
            width += INSPECTOR_GROWTH
        run("xdotool", "windowsize", "--sync", editor, str(width), str(height), *map(str, tail))

    def fixture_to_document(point):
        return point[0] - 8, point[1] - 89

    def inspector_x(x):
        return editor_width() - INSPECTOR_WIDTH + x

    # Inspector rows were authored below the former two-row workbench toolbar.
    # The shipping 52px header ends 29px higher; bottom-clamped rows (bottom())
    # keep their window position.
    # Its 30px section heading then sits 9px lower than the former headings.
    INSPECTOR_SHIFT = -20

    def inspector_click(x, y):
        click(editor, inspector_x(x), y + INSPECTOR_SHIFT)

    # Shipping sidebar: Layers takes max(188 px, 40%) of the panel above
    # Properties, always. Rows are 54 px on a 58 px pitch, front to back, with
    # eye, lock and ⋯ quick actions; Properties opens with a 48 px heading.
    def sidebar_geometry():
        inner = window_size()[1] - export_bar_height() - 52 - 4
        return 54, max(188, .4 * inner)

    def layer_row(index):
        return inspector_x(100), round(sidebar_geometry()[0] + 95 + 58 * index)

    def layer_quick(index, action):
        # Quick actions are right-aligned in the row.
        offset = {"eye": 81, "lock": 54, "more": 27}[action]
        return inspector_x(INSPECTOR_WIDTH - offset), layer_row(index)[1]

    def layer_click(index, action=None):
        click(editor, *(layer_quick(index, action) if action else layer_row(index)))

    def properties_top():
        top, height = sidebar_geometry()
        return round(top + height)

    # The ⋯ layer settings popover: 280 px beside the sidebar, top-aligned with
    # the ⋯ button. Offsets are from its top-left corner; annotation layers have
    # no 132 px Transform section. The popover shifts up to stay on screen.
    LAYER_MENU = {
        "blend": (140, 70), "opacity": (143, 142),
        "rotate-left": (80, 222), "rotate-right": (208, 222),
        "flip-horizontal": (80, 266), "flip-vertical": (208, 266),
        "bring-front": (100, 354), "send-back": (100, 390),
        "merge-down": (100, 476), "merge-visible": (100, 512), "flatten": (100, 548),
        "duplicate": (100, 610), "delete": (100, 648),
    }

    LAYER_MENU_HEIGHT = 674

    def layer_menu_point(index, item, image=True):
        height = window_size()[1]
        top = layer_quick(index, "more")[1] - 14
        menu_height = LAYER_MENU_HEIGHT - (0 if image else 132)
        top = max(8, min(top, height - 8 - min(height - 16, menu_height)))
        dx, dy = LAYER_MENU[item]
        if not image and dy > 300:
            dy -= 132
        return editor_width() - INSPECTOR_WIDTH - 288 + dx, top + dy

    def layer_menu_click(index, item, image=True):
        click(editor, *layer_menu_point(index, item, image))

    def layer_menu(index, item, image=True):
        # Escape closes a menu left open by an earlier action; the ⋯ toggles it.
        run("xdotool", "key", "Escape", "sleep", ".2")
        layer_click(index, "more")
        layer_menu_click(index, item, image)

    # Shipping Crop properties: the Aspect ratio select, then (with a
    # selection) read-only Width/Height and the Clear / Apply crop pair.
    CROP = {"aspect": 104, "clear": (60, 212), "apply": (170, 212)}
    CROP_ASPECT_ROWS = {name: 145 + 28 * index for index, name in
                        enumerate(["free", "1:1", "4:3", "3:2", "16:9"])}

    def prop_click(x, offset):
        click(editor, inspector_x(x), properties_top() + offset)

    def prop_field(offset, value, x=78):
        prop_click(x, offset)
        run("xdotool", "key", "ctrl+a")
        type_text(value, 60)
        run("xdotool", "key", "Return", "sleep", ".2")

    # Shipping Properties sections (`.screenshot-property-section`): offsets
    # from properties_top() of each control's centre. The 48 px heading, its
    # 8 px margin and the section's 12 px padding put the first label's top
    # at 68; a label and its control are 20 px apart and items 12 px apart.
    TEXT_DEFAULTS = {"style": 106, "standard": 191, "mono-box": 391, "size": 174, "shadow": 216}
    # Grouped shapes open with the 88 px shape picker and the 88 px preview.
    # In the 320px column a ColorField's two swatch rows add 106 px, a
    # labelled RangeSlider 69 px.
    CLOSED_DRAW_ROWS = {"stroke": 278, "color": 348, "size": 455, "opacity": 524,
                        "fill": 632, "shadow": 706}
    OPEN_DRAW_ROWS = {"color": 308, "size": 416, "opacity": 484, "shadow": 522,
                      "shadow-color": 592, "shadow-opacity": 699, "shadow-blur": 768,
                      "shadow-offset": 828}
    # DropShadowFields align with the "Drop shadow" label: 15 px box + 8 px gap.
    SHADOW_INDENT = 23

    def swatch_grid(width):
        # Shipping ColorField: `repeat(auto-fill, minmax(44px, 1fr))` for eight
        # swatches and the custom tile; auto-fill keeps empty tracks.
        fit = max(1, int(width // 44))
        return min(fit, len(SWATCHES) + 1), width / fit

    def prop_swatch(first_row, color, indent=0):
        # ColorField swatches in Properties across the column, 24 px tiles on a
        # 32 px row pitch, then the custom tile. first_row is the offset of the
        # first tile row's centres from properties_top().
        index = SWATCHES.index(color) if color in SWATCHES else len(SWATCHES)
        columns, pitch = swatch_grid(INSPECTOR_CONTENT - indent)
        prop_click(round(8 + indent + (index % columns + .5) * pitch), first_row + index // columns * 32)

    def properties_end():
        # Wheel Properties until it clamps at the end of its content.
        properties_move("click", "--repeat", "14", "--delay", "60", "5", "sleep", ".6")

    def properties_start():
        # Wheel Properties back to its top.
        properties_move("click", "--repeat", "14", "--delay", "60", "4", "sleep", ".6")

    def end_click(x, above):
        # A control `above` px over the bottom of Properties scrolled to its end.
        click(editor, inspector_x(x), window_size()[1] - export_bar_height() - above)

    def end_slider(above, *keys, x=100):
        end_click(x, above)
        for key in keys:
            run("xdotool", "key", key, "sleep", ".12")
        run("xdotool", "sleep", ".3")

    def end_field(above, value, x):
        end_click(x, above)
        run("xdotool", "key", "ctrl+a")
        type_text(value, 60)
        run("xdotool", "key", "Return", "sleep", ".2")

    def prop_slider(offset, *keys, x=100):
        # A shipping RangeSlider takes focus when pressed; Home/End, Page and
        # arrow keys then set its value exactly.
        # Paced keys keep every press under software-GL frame times.
        prop_click(x, offset)
        for key in keys:
            run("xdotool", "key", key, "sleep", ".12")
        run("xdotool", "sleep", ".3")

    # A selected layer opens with the Shift rotation snap section; image
    # Width/Height/X/Y follow in the next section as two number pairs.
    ROTATION_SNAP = 104
    # The 320px column fits the snap hint on two lines and puts the pair's
    # second column 164px in.
    IMAGE_FIELDS = {"width": (50, 220), "height": (240, 220), "x": (50, 280), "y": (240, 280)}

    def image_field(name, value):
        x, offset = IMAGE_FIELDS[name]
        prop_field(offset, value, x)

    def double_click_layer(index):
        # Select first and let the frame settle, so the double-click's two
        # presses reach egui within its 0.3 s window even on a slow debug build.
        layer_click(index)
        run("xdotool", "sleep", ".5", "click", "--repeat", "2", "--delay", "40", "1", "sleep", ".3")

    def rename_layer(index, name):
        double_click_layer(index)
        run("xdotool", "key", "ctrl+a")
        type_text(name, 30)
        run("xdotool", "key", "Return", "sleep", ".2")

    def drag_layer(index, target, below):
        start = layer_row(index)
        end = layer_row(target)
        end = end[0], end[1] + (18 if below else -18)
        run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, start),
            "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
            str(start[0]), str(start[1] + 8), "sleep", ".1", "mousemove", "--sync",
            "--window", editor, *map(str, end), "sleep", ".3", "mouseup", "1", "sleep", ".3")

    # Independent geometry expectations for the shipped token fonts at 1x.
    # Only visible groups reserve space; captures retain their own geometry.
    export_bar = {"open": False, "quality": 0, "custom": False, "dismissed": False}

    def settings_rows():
        widths = [200] + ([250] if export_bar["custom"] else []) + [150]
        widths += {0: [], 1: [160], 2: [190]}[export_bar["quality"]] + [150]
        if export_bar["quality"] and export_bar["dismissed"]:
            widths.append(160)
        available, used, rows = window_size()[0] - 50, 0, 1
        for width in widths:
            if used and used + 12 + width > available:
                rows += 1
                used = width
            else:
                used += (12 if used else 0) + width
        return rows

    def export_bar_height():
        if not export_bar["open"]:
            return 80
        rows = settings_rows()
        return 80 + 22 + 50 * rows + 4 * (rows - 1)

    def export_widths(width):
        # Mirrors export_widths in the editor: fixed actions, then disclosure, then filename.
        flexible = max(0, width - 24 - (68 + 92 + 128 + 100 + 5 * 8))
        disclosure = min(max(flexible - 128, 160), 210)
        return disclosure, min(max(flexible - disclosure, 128), 320)

    def export_point(action):
        width, height = window_size()
        disclosure, stem = export_widths(width)
        row, heading = height - 34, height - 66
        return {
            "settings": (12 + disclosure // 2, row),
            "filename": (12 + disclosure + stem // 2, row),
            "format": (12 + disclosure + stem + 8 + 34, row),
            "copy": (12 + disclosure + stem + 8 + 68 + 8 + 46, row),
            "new-file": (width - 164, row),
            "save": (width - 62, row),
            "change": (12 + disclosure + stem + 76 - 40, heading),
            "show": (width - 57, heading),
        }[action]

    def export_click(action):
        # Independent of inspector section and scroll position.
        click(editor, *export_point(action))

    def export_format(label):
        # The suffix menu opens upward from the bottom row.
        x, y = export_point("format")
        click(editor, x, y)
        click(editor, x, y - 126 + 44 * ["PNG", "JPEG", "WebP"].index(label))

    def export_settings(state):
        if export_bar["open"] != state:
            export_click("settings")
            export_bar["open"] = state
            time.sleep(.4)

    def setting_point(x, row=0):
        # The caption and control centre are 51px below the bar's top. Each
        # wrapped row adds its whole measured height and the 4px token gap.
        assert export_bar["open"]
        return 25 + x, window_size()[1] - export_bar_height() + 51 + 54 * row

    def setting_click(x, row=0):
        click(editor, *setting_point(x, row))
        if x == 770:  # Show before / after, in the wide one-row fixtures.
            export_bar["dismissed"] = False

    def preview_encoded():
        # Show the automatic before/after comparison (Compress) without
        # saving anything; Copy must still copy the edited frame.
        export_settings(True)
        quality_mode(1)
        compare_settled("comparison-before-copy")

    def setting_menu(x, index, count, row=0, item_height=44):
        # The compact card leaves less room below its controls. A described
        # listbox flips above; the smaller, label-only units menu may fit below.
        px, py = setting_point(x, row)
        click(editor, px, py)
        above = window_size()[1] - py < item_height * count + 42
        first = (py - item_height * count - 31 + item_height / 2
                 if above else py + 27 + item_height / 2)
        click(editor, px, round(first + item_height * index))

    def quality_mode(index):
        # Home/End reach the open listbox without egui's arrow-focus navigation.
        if index == 1:
            setting_menu(282, 1, 3)
        else:
            setting_click(282)
            for key in ("Home" if index == 0 else "End", "Return"):
                run("xdotool", "key", key, "sleep", ".2")
        export_bar.update(quality=index, dismissed=False)

    def setting_field(x, value, row=0):
        setting_click(x, row)
        run("xdotool", "key", "ctrl+a")
        type_text(value, 60)
        run("xdotool", "key", "Return", "sleep", ".3")

    # `--glass-text` on the comparison divider (the fixed media palette).
    GLASS_TEXT = (246, 246, 248)

    def divider_shown(name, split=.5):
        """The divider paints a glass-text column over the canvas at `split`."""
        window, size, bar = shot_layouts[name]
        left, top, scale = fit_geometry(size, window, bar)
        x = round(left + size[0] * split * scale)
        for y in (30, 50, 70, 90):  # Clear of the centred handle.
            actual = run("convert", str(output / f"{name}.png"), "-crop",
                         f"1x1+{x}+{round(top + y * scale)}", "-depth", "8", "rgb:-")
            if any(abs(a - b) > 24 for a, b in zip(actual, GLASS_TEXT)):
                return False
        return True

    def compare_settled(name):
        """Wait out the comparison's refresh delay and encode, then capture."""
        time.sleep(.6)
        previous = None
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            shot(editor, name)
            current = run("convert", str(output / f"{name}.png"), "-depth", "8", "rgb:-")
            if current == previous:
                return
            previous = current
        raise AssertionError(f"comparison never settled: {name}")

    def drag_split(split, start_split=.5):
        """Drag the comparison's round handle from `start_split` to `split`."""
        size = document_size()
        left, top, scale = fit_geometry(size)
        y = round(top + size[1] / 2 * scale)
        start = round(left + size[0] * start_split * scale)
        end = round(left + size[0] * split * scale)
        run("xdotool", "mousemove", "--sync", "--window", editor, str(start), str(y), "sleep", ".2",
            "mousedown", "1", "sleep", ".2")
        for x in (start + (end - start) // 2, end):
            run("xdotool", "mousemove", "--sync", "--window", editor, str(x), str(y), "sleep", ".2")
        run("xdotool", "mouseup", "1", "sleep", ".3")

    def export_filename(stem):
        export_click("filename")
        run("xdotool", "key", "ctrl+a")
        type_text(stem, 30)
        run("xdotool", "key", "Return", "sleep", ".3")

    # Shipping header (52px): the Canvas toolbar on the left; Undo/Redo (only
    # above 1040px), the zoom group and Add images on the right. Drafts autosave.
    # Widths below are measured under the token fonts.
    ADD_IMAGES_WIDTH = 116
    HEADER_Y = 26
    # The restored-draft notice's Discard and Dismiss, from the window's right edge.
    DRAFT_DISCARD_RIGHT = 110
    DRAFT_DISMISS_RIGHT = 42
    # Top-left of the background card's swatch grid relative to its trigger-derived
    # left edge, and the first tile row's centre.
    BACKGROUND_GRID_X = 13
    BACKGROUND_GRID_Y = 115
    # Bottom-clamped Layers → Annotation style rows (see `bottom`): first swatch
    # row centres and toggles, for fill-only, stroke+fill and shadow layouts.
    # Styles apply live (no Apply row at the end); shipping's Opacity slider
    # sits between the stroke width and the fill toggle.
    # Shipping annotation Properties below the Shift rotation snap section
    # (offsets from properties_top()): with Stroke off, Fill on and no
    # shadow. Stroke adds its color swatches and width slider; Drop shadow
    # its indented fields; `end` is where each state's content ends.
    # In shipping's 320px column the rotation snap hint takes two lines and
    # each ColorField two rows of six swatches.
    ANNOTATION = {"stroke": 198, "stroke-row": 268, "stroke-width": 375, "shadow": 306,
                  "filled": 346, "fill-row": 416, "end": 478, "stroke-rows": 175,
                  "shadow-rows": 308, "check-end": 27}

    def header_controls(width):
        compact = width <= 1040
        slider, preset = (72, 72) if compact else (92, 76)
        add_right = width - 12
        zoom_left = add_right - ADD_IMAGES_WIDTH - 8 - (3 * 30 + slider + preset + 6)
        x = zoom_left + 1
        points = {"import": add_right - ADD_IMAGES_WIDTH // 2, "fit": x + 15, "minus": x + 46}
        pad = 6 if compact else 8
        points["slider-min"] = x + 62 + pad
        points["slider-max"] = x + 62 + slider - pad - 1
        points["plus"] = x + 62 + slider + 1 + 15
        points["zoom"] = x + 62 + slider + 32 + preset // 2
        left = zoom_left
        if not compact:
            points["redo"] = zoom_left - 8 - 17
            points["undo"] = points["redo"] - 38
            left = points["undo"] - 17
        return points, left

    def canvas_toolbar_point(name):
        # The toolbar drops its "Canvas" label, then its Trim/Background text,
        # as the right-hand controls leave less room.
        # Thresholds are window widths measured under the token fonts.
        width = editor_width()
        if width <= 1040:
            assert not 956 <= width <= 980, width
            mode = "label" if width > 980 else "compact"
        else:
            assert not 1053 <= width <= 1069, width
            mode = "label" if width > 1069 else "full"
        return {
            "label": {"width": 124, "height": 224, "trim": 323, "background": 440},
            "full": {"width": 69, "height": 169, "trim": 268, "background": 385},
            "compact": {"width": 68, "height": 167, "trim": 230, "background": 260},
        }[mode][name], HEADER_Y

    def canvas_click(name):
        click(editor, *canvas_toolbar_point(name))

    def canvas_field(axis, value):
        canvas_click(axis)
        run("xdotool", "key", "ctrl+a")
        type_text(value, 60)
        run("xdotool", "key", "Return", "sleep", ".2")

    # The canvas background card opens below its trigger's left edge: a Solid
    # background toggle, then the shipping compact swatch grid (224 px wide,
    # six 37.33 px columns, 24 px tiles with 6 px row gaps).
    SWATCHES = ["#ff3b5c", "#ff8a22", "#ffd22e", "#36c96b", "#2d9cff", "#8b5cf6",
                "#111318", "#ffffff"]

    def background_point(name):
        x, _ = canvas_toolbar_point("background")
        left = {440: 375, 385: 320, 260: 246}[x]
        if name == "solid":
            return left + 30, 75
        index = SWATCHES.index(name)
        return (round(left + BACKGROUND_GRID_X + (index % 6 + .5) * 224 / 6),
                BACKGROUND_GRID_Y + index // 6 * 30)

    def background_click(name, dy=0):
        # dy: a banner above the header moves the header and its card down.
        x, y = background_point(name)
        click(editor, x, y + dy)

    def background_open(dy=0):
        x, y = canvas_toolbar_point("background")
        click(editor, x, y + dy)

    def background_color(value, dy=0):
        # Swatches apply live, one undo step each; Escape closes the card.
        background_open(dy)
        background_click(value, dy)
        run("xdotool", "key", "Escape", "sleep", ".2")

    # Rail button centres: 8px top padding, 38px buttons and 2px gaps.
    rail_keys = ["select", "crop", "text", "shapes", "arrow", "pen", "eraser"]

    def rail_point(name):
        return 28, 52 + 27 + 40 * rail_keys.index(name)

    def rail_click(name):
        click(editor, *rail_point(name))

    shape_keys = ["rectangle", "ellipse", "line", "triangle", "diamond", "star"]

    def shape_flyout_point(name):
        # 10px right of Shapes, centred on it: 3x2 grid of 44px buttons,
        # 4px gaps and 6px padding.
        index = shape_keys.index(name)
        _, y = rail_point("shapes")
        return 47 + 10 + 7 + 48 * (index % 3) + 22, y - 53 + 7 + 48 * (index // 3) + 22

    tool_state = {"draw": "rectangle"}

    def select_draw_tool(name):
        if name in shape_keys:
            rail_click("shapes")
            click(editor, *shape_flyout_point(name))
        elif name in ("wand", "erase", "restore"):
            rail_click("eraser")
            # Shipping's Eraser mode group below the Properties heading.
            prop_click({"wand": 58, "erase": 160, "restore": 263}[name], ERASER_MODE_ROW)
        else:
            rail_click({"text": "text", "arrow": "arrow", "pen": "pen"}[name])

    def toolbar_click(name):
        if name in ("undo", "redo"):
            points, _ = header_controls(editor_width())
            if name in points:
                click(editor, points[name], HEADER_Y)
            else:
                # Shipping hides Undo/Redo at 1040px and below; use the key.
                click(editor, 28, 52 + 8 + 7 * 40 + 30)  # Empty rail: drop field focus.
                run("xdotool", "key", "ctrl+z" if name == "undo" else "ctrl+shift+z",
                    "sleep", ".2")
        elif name == "import":
            click(editor, header_controls(editor_width())[0]["import"], HEADER_Y)
        elif name == "layers":
            rail_click("select")
        elif name == "geometry":
            rail_click("crop")
        elif name == "draw":
            select_draw_tool(tool_state["draw"])
        else:
            raise AssertionError(name)

    # Eraser mode buttons (Wand, Erase, Restore) span the 320px column below
    # a one-line intro; the Eraser rows that follow move down by ERASER_ROWS.
    ERASER_MODE_ROW = 109
    ERASER_ROWS = 78

    def draw_tool(name):
        tool_state["draw"] = name
        select_draw_tool(name)

    def draw_row(y):
        # Draw rows were authored below the historical tool grid, whose first
        # following row centered at y=332. The rail now picks the tool, so rows
        # start below the Layers section and the Properties heading.
        return y + 44 + properties_top() + 75 - 332

    # Shape tools show shipping's 88px DrawToolPreview card (plus one 12px
    # item gap) above their rows; the Wand and Text rows have no preview.
    DRAW_PREVIEW_ROWS = 100

    def shape_row(y):
        return draw_row(y + DRAW_PREVIEW_ROWS)

    def zoom_menu_x():
        # Preset menu rows, 44px apart from y=64, right-aligned to the preset.
        return header_controls(editor_width())[0]["zoom"] + 10

    def discard_restored(window):
        # Shipping's restored-draft notice above the header: Discard, then Dismiss.
        click(window, editor_width_of(window) - DRAFT_DISCARD_RIGHT, 20)

    def blur_click():
        # Empty rail space below the tools: takes focus from fields, edits nothing.
        click(editor, 28, 52 + 8 + 7 * 40 + 30)

    def topbar_click(name):
        points, _ = header_controls(editor_width())
        if name == "recenter":
            # Shipping's floating pill at the top of the canvas viewport.
            click(editor, 64 + (editor_width() - INSPECTOR_WIDTH - 8 - 64) // 2, 60 + 12 + 14)
        else:
            click(editor, points[name], HEADER_Y)

    def inspector_move(x, y, *tail):
        run("xdotool", "mousemove", "--window", editor, str(inspector_x(x)),
            str(y + INSPECTOR_SHIFT), *tail)

    def properties_move(*tail):
        # Wheel over Properties (below the Layers section) to scroll it.
        run("xdotool", "mousemove", "--window", editor, str(inspector_x(180)),
            str(properties_top() + 120), *tail)

    def window_move(point, *tail):
        run("xdotool", "mousemove", "--window", editor, *map(str, point), *tail)

    def bottom(y):
        # Rows authored against an inspector scrolled until it clamps at its end.
        # Its content used to end with a 125px development footer; without it,
        # a bottom-clamped scroll leaves every row 125px lower in the window.
        # The export bar below the inspector adds 80px more to a taller window.
        # They were authored for a 1004 px window; the live rotation-snap tail
        # ends 7 px lower. Bottom-clamped rows follow the window's bottom edge.
        return y + 125 + 80 + 7 - INSPECTOR_SHIFT + window_size()[1] - 1004

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
            # Wide enough for the 1200px fixtures plus shipping's wider sidebar.
            "1400x1600x24" if args.text_only or args.drawing_defaults_only else "1400x1200x24",
            "-dpi", "96", "-nolisten", "tcp"], True)
        address = spawn("dbus", ["dbus-daemon", "--session", "--nofork", "--print-address=1"], True)
        env["DBUS_SESSION_BUS_ADDRESS"] = env["DBUS_SYSTEM_BUS_ADDRESS"] = address
        DBusGMainLoop(set_as_default=True)
        bus = dbus.bus.BusConnection(address)
        name = dbus.service.BusName("org.freedesktop.ScreenSaver", bus=bus, do_not_queue=True)
        saver = ScreenSaver(name, "/org/freedesktop/ScreenSaver")
        imported_path = output / "Imported sample é.png"
        chooser = FileChooser(bus, imported_path)
        file_manager = FileManager(bus)
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
            "onboarding_completed": True,
            "settings_schema_version": 5, "appearance": args.appearance, "theme": "mustard",
            "output_directory": str(output / "exports"), "launch_at_login": False,
            "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
            "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10",
            "auto_copy_to_clipboard": False, "show_mini_previews": False,
        }))
        app_command = [str(binary), "--live", "--open-history", "--history-root", str(history),
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
        root = wait(lambda: windows("Capture History"), "History workspace")[0]
        run("xdotool", "windowmove", "--sync", root, "0", "0")
        time.sleep(1)
        if args.external_image_only:
            entries = wait(lambda: values if len(values := opened_entries()) == 3 else None,
                           "three imported History rows, not an alias duplicate")
            opened = next(value for value in entries if value["saved_path"] == str(source_png))
            artifact_id = opened["id"]
            artifact = history / artifact_id
            wait(lambda: len(windows("Captures Screenshot Editor")) == 3, "three native image editors")
            editor = wait(lambda: active if (active := active_window())
                          in windows("Captures Screenshot Editor") else None, "last opened image focused")
            shot(root, "history")
        else:
            shot(root, "history")
            click(root, 107, 432)  # First History card: Edit, below the header and filters.
            editor = wait(lambda: windows("Captures Screenshot Editor"), "screenshot editor")[0]
        run("xdotool", "windowmove", "--sync", editor, "100", "80")
        time.sleep(1)
        shot(editor, "editor-original")
        pixel("editor-original", 520, 600,
              (245, 245, 247) if args.appearance == "light" else (16, 16, 20))

        def type_text(value, delay=10):
            # xdotool type consumes all remaining arguments; do not chain keys after it.
            run("xdotool", "type", "--clearmodifiers", "--delay", str(delay), "--", str(value))

        def field(y, value, x=78):
            inspector_click(x, y)
            run("xdotool", "key", "ctrl+a")
            type_text(value, 60)
            run("xdotool", "key", "Return", "sleep", ".2")

        def swatch_at(first_row, color):
            # Shipping ColorField swatch row across the inspector, 24 px tiles on
            # a 32 px row pitch, then the custom tile. first_row is the
            # inspector y of the first tile row's centres.
            index = SWATCHES.index(color) if color in SWATCHES else len(SWATCHES)
            columns, pitch = swatch_grid(INSPECTOR_CONTENT)
            inspector_click(round(8 + (index % columns + .5) * pitch), first_row + index // columns * 32)

        draft = output / "editor-drafts" / artifact_id / "manifest.json"

        def saved(width, height, x, y):
            if not draft.exists():
                # No draft means the original capture: autosave drops a draft
                # whose document is back at the capture, as shipping does.
                return (width, height, x, y) == (640, 360, 0, 0)
            document = json.loads(draft.read_text())["document"]
            layer = document["elements"][0]
            return (document["width"], document["height"], layer["x"], layer["y"]) == (width, height, x, y)

        def settled_draft():
            # Shipping autosaves 700 ms after the latest edit. Once the editor is
            # idle, wait until the draft (or its absence) has held for longer.
            wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
                 "editor idle before reading the draft")
            last, since = draft_bytes(), time.monotonic()
            deadline = since + 20
            while time.monotonic() < deadline:
                time.sleep(.1)
                current = draft_bytes()
                if current != last:
                    last, since = current, time.monotonic()
                elif time.monotonic() - since >= 1:
                    return
            raise AssertionError("the autosaved draft never settled")

        def save_until(predicate, description):
            def attempt():
                settled_draft()
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

        def history_edit_point(edit_y=432):
            # History cards are newest first in a three-column grid (1020 px window,
            # 24 px padding, 16 px gaps). Edit is the left action of the card body.
            entries = sorted((json.loads(path.read_text()) for path in history.glob("*/metadata.json")
                              if not path.parent.name.startswith(".")),
                             key=lambda entry: entry["created_at"], reverse=True)
            index = [entry["id"] for entry in entries].index(artifact_id)
            assert index < 3, "the edited capture must be in the first History row"
            card_width = (972 - 2 * 16) / 3
            return round(24 + index * (card_width + 16) + 12 + (card_width - 24 - 6) / 4), edit_y

        def reopen(edit_y=432, keep_banner=False):
            export_bar.update(open=False, quality=0, custom=False, dismissed=False)
            tool_state["draw"] = "rectangle"  # Each editor starts with Rectangle.
            click(root, *history_edit_point(edit_y))  # The original capture's History card: Edit.
            restored = draft.exists()
            window = wait(lambda: windows("Captures Screenshot Editor"), "reopened editor")[0]
            run("xdotool", "windowmove", "--sync", window, "100", "80")
            time.sleep(.6)
            if restored:
                # A restored draft shows shipping's banner above the header; keep
                # the authored layout by dismissing it unless Discard follows.
                shot(window, "restored-draft-banner")
                if not keep_banner:
                    click(window, editor_width_of(window) - DRAFT_DISMISS_RIGHT, 20)
            return window

        def discard_draft(description):
            # Shipping discards only from the restored-draft notice: close (the
            # draft flushes), reopen at the same size, then Discard.
            size = window_size()
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), f"{description}: editor closes")
            assert draft.exists(), f"{description}: closing keeps the autosaved draft"
            window = reopen(keep_banner=True)
            discard_restored(window)
            wait(lambda: not draft.exists(), description)
            run("xdotool", "windowsize", "--sync", window, *map(str, size), "sleep", ".3")
            return window

        # The original capture's single layer. No draft means the document is
        # back at the capture: shipping autosave drops such a draft.
        # Its session-local asset source is not on disk, so stand in a stable one.
        ORIGINAL_LAYERS = [{"kind": "image", "id": "capture-background", "source": "background",
                            "src": "draft-asset:original-capture", "locked": True, "visible": True}]

        def draft_bytes():
            # None while no draft exists (the document is the original capture).
            return draft.read_bytes() if draft.exists() else None

        def layers():
            return json.loads(draft.read_text())["document"]["elements"] if draft.exists() else ORIGINAL_LAYERS

        def save_layers(predicate, description):
            save_until(lambda: predicate(layers()), description)
            return layers()

        def asset_pixel(layer, x, y, expected=None):
            asset = draft.parent / "assets" / (layer["src"].split(":", 1)[1] + ".png")
            actual = run("convert", str(asset), "-crop", f"1x1+{x}+{y}", "-depth", "8", "rgba:-")
            if expected is not None:
                assert actual == bytes(expected), (x, y, actual, expected)
            return actual

        if args.import_formats_only:
            resize_editor(1200, 701)
            gif = output / "First frame.GIF"
            bmp = output / "Asymmetric.BMP"
            svg = output / "Vector.SVG"
            notes = output / "notes.txt"
            notes.write_text("not an image")
            # Only frame zero should become a layer; frame one is solid blue.
            run("convert", "-size", "120x80", "xc:none", "-fill", "#d53e55",
                "-draw", "rectangle 0,0 45,79", "-fill", "#3cb371",
                "-draw", "rectangle 46,18 70,49", "-delay", "5",
                "(", "-size", "120x80", "xc:#2d64bd", ")", "-loop", "0", "-strip", str(gif))
            run("convert", "-size", "80x50", "xc:#ebbf48", "-fill", "#8e44ad",
                "-draw", "rectangle 31,19 79,49", "-strip", "BMP3:" + str(bmp))
            # Infer height from width/viewBox; browser and native pixels must stay 90×50.
            svg.write_text('<svg xmlns="http://www.w3.org/2000/svg" width="90" viewBox="0 0 180 100">'
                '<defs><g id="shapes">'
                '<path d="M0 0H70V100H0Z" fill="#2d64bd"/>'
                '<rect x="100" y="40" width="50" height="50" fill="white" opacity="0.5"/>'
                '<text x="78" y="28" font-family="sans-serif" font-size="24" fill="#2d64bd">A7</text>'
                '</g></defs><use href="#shapes"/></svg>')
            source_bytes = {path: path.read_bytes() for path in [gif, bmp, svg]}
            chooser.selected = [gif, notes, bmp, svg]
            toolbar_click("import")
            wait(lambda: chooser.pending, "GIF/BMP/SVG multi-select picker")
            title, options = chooser.calls[-1]
            assert title == "Import images" and options.get("multiple", False)
            filters = str(options["filters"])
            assert all("*." + extension in filters for extension in ["gif", "bmp", "svg"]), filters
            GLib.idle_add(chooser.respond, False)
            imported = save_layers(lambda values: len(values) == 4, "GIF, BMP and SVG imported in order")
            assert [layer["name"] for layer in imported[1:]] == [gif.name, bmp.name, svg.name]
            assert [(layer["naturalWidth"], layer["naturalHeight"]) for layer in imported[1:]] == [(120, 80), (80, 50), (90, 50)]
            for previous, current in zip(imported[1:], imported[2:]):
                assert current["y"] >= previous["y"] + previous["height"]
            asset_pixel(imported[1], 10, 10, (213, 62, 85, 255))
            asset_pixel(imported[1], 50, 20, (60, 179, 113, 255))
            asset_pixel(imported[1], 100, 10, (0, 0, 0, 0))
            asset_pixel(imported[2], 10, 10, (235, 191, 72, 255))
            asset_pixel(imported[2], 50, 30, (142, 68, 173, 255))
            asset_pixel(imported[3], 10, 10, (45, 100, 189, 255))
            asset_pixel(imported[3], 80, 10, (0, 0, 0, 0))
            asset_pixel(imported[3], 60, 30, (255, 255, 255, 128))
            svg_asset = draft.parent / "assets" / (imported[3]["src"].split(":", 1)[1] + ".png")
            text_pixels = run("convert", str(svg_asset), "-crop", "35x16+38+0", "-depth", "8", "rgba:-")
            assert any(text_pixels[3::4]), "bundled SVG text must not silently disappear"
            shot(editor, "gif-bmp-svg-imported")
            document_pixel("gif-bmp-svg-imported", imported[1]["x"] + 10, imported[1]["y"] + 10, (213, 62, 85))
            document_pixel("gif-bmp-svg-imported", imported[2]["x"] + 50, imported[2]["y"] + 30, (142, 68, 173))
            document_pixel("gif-bmp-svg-imported", imported[3]["x"] + 10, imported[3]["y"] + 10, (45, 100, 189))
            toolbar_click("undo")
            save_layers(lambda values: len(values) == 3, "SVG undone independently")
            toolbar_click("undo")
            save_layers(lambda values: len(values) == 2, "one file per Undo")
            toolbar_click("undo")
            save_layers(lambda values: len(values) == 1, "all imports undone")
            toolbar_click("redo")
            save_layers(lambda values: len(values) == 2, "GIF redone")
            toolbar_click("redo")
            save_layers(lambda values: len(values) == 3, "BMP redone")
            toolbar_click("redo")
            assert save_layers(lambda values: len(values) == 4, "SVG redone") == imported
            unsupported_svg = output / "Resource.SVG"
            unsupported_svg.write_text('<svg xmlns="http://www.w3.org/2000/svg" width="90" height="50">'
                '<use href="private.svg#shapes"/></svg>')
            chooser.selected = [unsupported_svg]
            toolbar_click("import")
            wait(lambda: chooser.pending, "unsupported SVG picker")
            GLib.idle_add(chooser.respond, False)
            assert save_layers(lambda values: values == imported,
                               "rejected SVG leaves the document untouched") == imported
            shot(editor, "svg-conversion-error")
            toolbar_click("undo")
            save_layers(lambda values: len(values) == 3,
                        "rejected SVG adds no undo step; last valid SVG undone")
            toolbar_click("redo")
            assert save_layers(lambda values: len(values) == 4, "valid SVG restored after rejection") == imported
            assert all(path.read_bytes() == before for path, before in source_bytes.items())
            for path in source_bytes:
                path.unlink()
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "imported editor closes")
            editor = reopen()
            assert layers() == imported, "reopened draft retains detached image assets"
            asset_pixel(imported[1], 50, 20, (60, 179, 113, 255))
            asset_pixel(imported[2], 50, 30, (142, 68, 173, 255))
            asset_pixel(imported[3], 60, 30, (255, 255, 255, 128))
            shot(editor, "gif-bmp-svg-reopened")
            resize_editor(760, 540)
            shot(editor, "gif-bmp-svg-minimum")
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "import format suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["gif-bmp-svg-picker-filters", "mixed-case-extensions", "unsupported-file-skipped",
                           "first-gif-frame-only", "transparent-gif-pixels", "asymmetric-bmp-pixels",
                           "svg-inferred-height", "svg-local-reference", "svg-viewbox-rasterization",
                           "straight-svg-alpha", "svg-bundled-text", "external-svg-reference-rejected",
                           "unsupported-svg-preserves-document-and-undo",
                           "ordered-placement", "one-undo-step-per-file", "stable-redo",
                           "immutable-sources", "draft-reopens-after-source-removal", "original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native GIF/BMP/SVG imports: picker, pixels, alpha, undo/redo and detached draft reopen")
            return

        if args.external_image_only:
            assert {item["saved_path"]: (item["width"], item["height"]) for item in entries} == {
                str(source_png): (640, 360), str(source_jpeg): (73, 41), str(source_webp): (81, 53),
            }
            for item in entries:
                saved = history / item["id"] / "capture.png"
                assert run("convert", str(saved), "-depth", "8", "rgba:-") == run(
                    "convert", item["saved_path"], "-depth", "8", "rgba:-"), "imported pixels match source decoding"
            assert len(windows("Captures Screenshot Editor")) == 3
            shot(editor, "external-image-opened")
            document_pixel("external-image-opened", 130, 100, (229, 179, 68))
            document_pixel("external-image-opened", 500, 250, (46, 158, 113))
            document_pixel("external-image-opened", 20, 20, (40, 110, 166))
            assert not draft.exists(), "opening/focusing must not create edit drafts"
            toolbar_click("draw")
            draw_tool("rectangle")
            drag((320, 250), (480, 370))
            save_layers(lambda values: len(values) == 2, "external image edit is a real draft")
            preserved_draft = draft_bytes()
            assert all(path.read_bytes() == before for path, before in source_bytes.items())

            # A second executable must forward, not initialize another renderer,
            # settings writer or set of capture shortcuts. Sender CWD differs.
            run("xdotool", "windowactivate", "--sync", root)
            assert active_window() == root
            forwarded = subprocess.run(
                [str(binary), "--live", "--history-root", str(history),
                 "--settings-file", str(output / "unused-secondary-settings.json"),
                 "--open-media", alias.name], cwd=output, env=env,
                capture_output=True, text=True, timeout=10)
            assert forwarded.returncode == 0, forwarded.stderr
            assert '"event":"forwarded"' in forwarded.stdout
            assert '"event":"ready"' not in forwarded.stdout
            assert not (output / "unused-secondary-settings.json").exists()
            wait(lambda: active_window() == editor,
                 "forwarded canonical alias focuses existing edited window")
            assert len(windows("Captures Screenshot Editor")) == 3
            assert len(opened_entries()) == 3
            assert draft_bytes() == preserved_draft
            assert len(layers()) == 2

            # Shipping reopen priority: an open editor before History.
            run("xdotool", "windowminimize", root)
            run("xdotool", "windowminimize", editor)
            relaunched = subprocess.run(app_command, cwd=output, env=env,
                                        capture_output=True, text=True, timeout=10)
            assert relaunched.returncode == 0, relaunched.stderr
            assert '"event":"forwarded"' in relaunched.stdout
            wait(lambda: active_window() in windows("Captures Screenshot Editor"),
                 "empty relaunch restores and focuses an open editor window")
            assert root not in windows("Capture History"), "empty relaunch restored History over an editor"
            assert app.poll() is None
            close(root)
            wait(lambda: app.poll() is not None, "external image batch quits")
            assert app.returncode == 0

            # History still restores the autosaved edit of a closed source.
            app = spawn("app-draft", app_command)
            root = wait(lambda: windows("Capture History"), "draft History")[0]
            assert draft_bytes() == preserved_draft
            editor = reopen(keep_banner=True)
            shot(editor, "external-draft-restored")
            assert len(layers()) == 2
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "restored draft editor closes")
            assert draft_bytes() == preserved_draft, "closing an unchanged restored draft keeps it"
            close(root)
            wait(lambda: app.poll() is not None, "draft History process quits")
            assert app.returncode == 0

            # Opening the closed source again reloads it and drops its draft, as
            # shipping does, once the source decodes.
            app = spawn("app-draft-reload", app_command + ["--open-image", str(source_png)])
            root = wait(lambda: windows("Capture History"), "source reload History")[0]
            editor = wait(lambda: windows("Captures Screenshot Editor"), "reloaded source editor")[0]
            wait(lambda: not draft.exists(), "reloading a closed source drops its draft")
            run("xdotool", "windowmove", "--sync", editor, "100", "80")
            time.sleep(1)
            shot(editor, "external-source-reopened")
            close(root)
            wait(lambda: app.poll() is not None, "source reload process quits")
            assert app.returncode == 0

            run("convert", str(source_png), "-fill", "#1234ab", "-draw", "rectangle 8,8 50,50",
                "-strip", "PNG32:" + str(source_png))
            changed_source = source_png.read_bytes()
            app = spawn("app-reload", app_command + ["--open-image", str(source_png)])
            root = wait(lambda: windows("Capture History"), "reloaded source History")[0]
            editor = wait(lambda: windows("Captures Screenshot Editor"), "reloaded external editor")[0]
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
                           "edits-do-not-overwrite-source", "history-restores-autosaved-draft",
                           "closed-source-reload-drops-draft",
                           "reload-preserves-history-identity", "reloaded-source-pixels",
                           "secondary-exits-before-renderer-and-settings",
                           "forwarded-relative-alias-preserves-edits",
                           "empty-relaunch-focuses-open-editor"],
            }, indent=2) + "\n")
            print("PASS native external images: batch, aliases, errors, pixels, autosaved drafts and source reload")
            return

        if args.combine_only:
            resize_editor(1000, 801, "sleep", ".3")

            # Build an asymmetric document through the same tool shortcuts and
            # canvas input a person uses. Keep the first annotation hidden so
            # Merge visible has a slot it must retain and Flatten must remove.
            run("xdotool", "key", "r", "sleep", ".2")
            drag((300, 220), (380, 300))
            first = save_layers(lambda values: len(values) == 2, "first combine layer")[-1]
            toolbar_click("layers")
            layer_click(0, "eye")
            save_layers(lambda values: len(values) == 2 and not values[-1]["visible"],
                        "hidden combine fixture")
            run("xdotool", "key", "r", "sleep", ".2")
            drag((420, 250), (500, 330))
            second = save_layers(lambda values: len(values) == 3, "second combine layer")[-1]
            run("xdotool", "key", "r", "sleep", ".2")
            drag((540, 280), (620, 360))
            third = save_layers(lambda values: len(values) == 4, "third combine layer")[-1]
            fixture_ids = [value["id"] for value in layers()]
            shot(editor, "combine-asymmetric-fixture")

            # A context action belongs to its clicked row, not to the previous
            # selection. Select the second row, then merge the top row downward.
            toolbar_click("layers")
            layer_click(1)
            window_move(layer_row(0), "sleep", ".2", "mousedown", "3",
                        "sleep", ".15", "mouseup", "3", "sleep", ".3")
            shot(editor, "combine-row-context-menu")
            row_x, row_y = layer_row(0)
            click(editor, row_x + 30, row_y + 200)  # Fifth context item: Merge down.
            merged_down = save_layers(lambda values: len(values) == 3, "clicked-row merge down")
            assert [value["id"] for value in merged_down[:2]] == fixture_ids[:2]
            assert merged_down[-1]["id"] not in fixture_ids
            assert not merged_down[1]["visible"] and merged_down[1]["id"] == first["id"]
            assert second["id"] not in {value["id"] for value in merged_down}
            assert third["id"] not in {value["id"] for value in merged_down}
            merged_down_snapshot = merged_down
            shot(editor, "combine-merge-down")
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            assert [value["id"] for value in save_layers(
                lambda values: len(values) == 4, "undo merge down")] == fixture_ids
            run("xdotool", "key", "ctrl+shift+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 3, "redo merge down") == merged_down_snapshot

            # Exercise the row's ⋯ layer menu. Merge visible replaces only
            # visible layers and leaves the hidden annotation in its old slot.
            layer_click(0, "more")
            shot(editor, "combine-heading-menu")
            layer_menu_click(0, "merge-visible")
            merged_visible = save_layers(
                lambda values: len(values) == 2, "heading menu merge visible")
            hidden = [value for value in merged_visible if not value["visible"]]
            assert len(hidden) == 1 and hidden[0]["id"] == first["id"]
            assert any(value["visible"] and value["id"] not in fixture_ids
                       for value in merged_visible)
            shot(editor, "combine-merge-visible-hidden-retained")
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 3,
                               "undo merge visible") == merged_down_snapshot
            run("xdotool", "key", "ctrl+shift+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2,
                               "redo merge visible") == merged_visible

            # Set a real canvas background, then flatten from the same Combine
            # menu. Flatten bakes it, removes hidden slots and locks one image.
            background_color("#2d9cff")
            save_until(lambda: json.loads(draft.read_text())["document"]["background"] == "#2d9cff",
                       "combine fixture background")
            toolbar_click("layers")
            layer_click(1, "more")  # The merged image; the hidden annotation stays above it.
            layer_menu_click(1, "flatten")
            flattened = save_layers(lambda values: len(values) == 1, "flatten image")
            flattened_document = json.loads(draft.read_text())["document"]
            assert flattened_document["background"] is None
            assert flattened[0]["locked"] and flattened[0]["name"] == "Flattened"
            assert flattened[0]["id"] not in fixture_ids
            shot(editor, "combine-flattened")
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "undo flatten") == merged_visible
            run("xdotool", "key", "ctrl+shift+z", "sleep", ".3")
            flattened = save_layers(lambda values: len(values) == 1, "redo flatten")

            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "combined draft closes")
            editor = reopen()
            reopened = layers()
            assert len(reopened) == 1 and reopened[0] == flattened[0]
            assert json.loads(draft.read_text())["document"]["background"] is None
            toolbar_click("layers")
            shot(editor, "combine-flattened-reopened")

            resize_editor(760, 540, "sleep", ".3")
            layer_click(0, "more")
            shot(editor, "combine-minimum-disabled-menu")
            run("xdotool", "key", "Escape", "sleep", ".2")
            resize_editor(1000, 801, "sleep", ".3")
            preview_encoded()
            export_click("copy")
            wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
                 "flattened clipboard copy completes")
            copied = output / "clipboard-combined.png"
            copied.write_bytes(run("xclip", "-selection", "clipboard", "-t", "image/png", "-o"))
            assert run("identify", "-format", "%wx%h", str(copied)) == b"640x360"
            assert run("convert", str(copied), "-crop", "1x1+350+230", "-depth", "8",
                       "rgb:-") == bytes((255, 59, 92))
            assert run("convert", str(copied), "-crop", "1x1+20+20", "-depth", "8",
                       "rgb:-") == bytes((40, 110, 166))
            shot(editor, "combine-output-copied")
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "combine suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["asymmetric-real-input-fixture", "clicked-row-merge-down",
                           "merge-down-undo-redo", "heading-combine-menu",
                           "merge-visible-hidden-retained", "merge-visible-undo-redo",
                           "flatten-removes-hidden-and-background", "flatten-undo-redo",
                           "saved-draft-reopen", "clipboard-output-pixels",
                           "original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native combine: menus, clicked row, undo/redo, hidden layers, flatten, reopen and pixels")
            return

        if args.history_shortcuts_only:
            resize_editor(1000, 901, "sleep", ".3")  # Integer-pixel Fit origin for exact movement.
            run("xdotool", "windowactivate", "--sync", editor, "key", "ctrl+d", "sleep", "1")
            assert not draft.exists(), "opening with no selection must not duplicate an implicit layer"
            shot(editor, "initial-no-selection")
            layer_click(0)
            shot(editor, "explicit-layer-selection")
            assert not draft.exists(), "selecting a layer does not edit the document"
            rail_click("shapes")  # Persistent Shapes rail button.
            shot(editor, "tool-rail-shapes-menu")
            run("xdotool", "key", "Escape", "sleep", ".3")
            assert not draft.exists(), "opening/closing tool menus must not save edits"
            rail_click("arrow")  # Arrow, independent of the inspector section.
            drag((320, 250), (480, 370))
            arrow = save_layers(lambda values: len(values) == 2, "rail Arrow creates one layer")[-1]
            assert arrow["shape"] == "arrow", arrow
            shot(editor, "created-shape-properties")
            # Open shapes omit the 28px Stroke checkbox and its 12px gap.
            prop_swatch(ANNOTATION["stroke-row"] - 40, "#2d9cff")
            recolored = save_layers(lambda values: values[-1]["style"]["color"] == "#2d9cff",
                                    "Properties edit the created arrow without selecting another tool")[-1]
            assert recolored == dict(arrow, style=dict(arrow["style"], color="#2d9cff"))
            shot(editor, "created-shape-properties-edited")
            resize_editor(760, 540, "sleep", ".3")
            shot(editor, "created-shape-properties-minimum")
            properties_end()
            shot(editor, "created-shape-properties-minimum-end")
            resize_editor(1000, 901, "sleep", ".3")
            properties_start()
            blur_click()
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            assert save_layers(lambda values: values[-1] == arrow, "selected style undo")[-1] == arrow
            before_deselection = draft_bytes()
            run("xdotool", "key", "a", "Delete", "ctrl+d", "Right", "sleep", "1")
            assert draft_bytes() == before_deselection, "reactivating Arrow clears layer shortcut targets"
            shot(editor, "tool-cleared-shape-selection")
            resize_editor(760, 540, "sleep", ".3")
            rail_click("shapes")  # Shapes at the minimum size.
            shot(editor, "tool-rail-minimum-menu")
            run("xdotool", "key", "Escape", "sleep", ".3")
            rail_click("arrow")
            shot(editor, "tool-rail-minimum-arrow")
            resize_editor(1000, 901, "sleep", ".3")
            # Undo back to the capture: shipping autosave then drops the draft.
            blur_click()
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            wait(lambda: not draft.exists(), "undoing the rail fixture drops the draft")
            blur_click()
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
            before_crop = draft_bytes()
            run("xdotool", "key", "c", "sleep", ".3")
            run("xdotool", "key", "Delete", "ctrl+d", "Right", "sleep", "1")
            assert draft_bytes() == before_crop, "switching to Crop clears layer shortcut targets"
            drag((330, 270), (460, 350))
            run("xdotool", "key", "c", "sleep", ".3")
            shot(editor, "shortcut-crop-candidate")
            run("xdotool", "key", "v", "sleep", ".3")
            assert draft_bytes() == before_crop, "tool selection/cancellation must not save edits"
            assert save_layers(lambda values: len(values) == 2, "crop cancellation")[-1] == moved
            blur_click()
            run("xdotool", "key", "ctrl+z", "sleep", ".3", "key", "ctrl+z", "sleep", ".3")
            wait(lambda: not draft.exists(), "undoing the tool-key fixture drops the draft")
            blur_click()  # Leave control focus before selecting a canvas tool.
            run("xdotool", "key", "r", "sleep", ".3")
            drag((320, 250), (480, 370))
            shape = save_layers(lambda values: len(values) == 2, "shortcut shape fixture")[-1]
            assert shape["shape"] == "rectangle", shape
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            save_layers(lambda values: len(values) == 1, "keyboard undo")
            canvas_click("width")
            run("xdotool", "key", "ctrl+shift+z", "sleep", ".3")
            save_layers(lambda values: len(values) == 1, "field focus does not redo document")
            toolbar_click("draw")
            run("xdotool", "key", "ctrl+shift+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "keyboard redo")[-1] == shape

            canvas_click("width")  # Focus canvas width, not a document action.
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "field focus keeps document")[-1] == shape
            blur_click()  # Leaving the field returns shortcuts to the document.
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            save_layers(lambda values: len(values) == 1, "document shortcut after leaving the field")
            run("xdotool", "key", "ctrl+shift+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "redo after leaving the field")[-1] == shape
            toolbar_click("layers")  # Layers; select the restored shape, not the original.
            layer_click(0)
            run("xdotool", "key", "ctrl+d", "sleep", ".3")
            copied = save_layers(lambda values: len(values) == 3, "keyboard duplicate")[-1]
            assert copied["id"] != shape["id"]
            expected = dict(shape, id=copied["id"], x=shape["x"] + 24, y=shape["y"] + 24,
                            endX=shape["endX"] + 24, endY=shape["endY"] + 24,
                            visible=True, locked=False)
            assert copied == expected, (copied, expected)
            nudged = copied
            positions = [copied]
            for key, dx, dy in [("Left", -1, 0), ("shift+Up", 0, -10),
                                ("shift+Right", 10, 0), ("Down", 0, 1)]:
                expected = dict(nudged, x=nudged["x"] + dx, y=nudged["y"] + dy,
                                endX=nudged["endX"] + dx, endY=nudged["endY"] + dy)
                run("xdotool", "key", key, "sleep", ".3")
                nudged = save_layers(lambda values: len(values) == 3 and values[-1] == expected,
                                     f"keyboard nudge {key}")[-1]
                positions.append(expected)
            # Busy commands intentionally do not queue; verify each accepted
            # undo before sending the next, even on slow software rendering.
            for expected in reversed(positions[:-1]):
                run("xdotool", "key", "ctrl+z", "sleep", ".3")
                save_layers(lambda values: len(values) == 3 and values[-1] == expected,
                            "undo each nudge exactly")
            layer_click(0)  # Restore the copy selection after Undo.
            canvas_click("width")
            run("xdotool", "key", "ctrl+d", "Delete", "Left", "shift+Up", "sleep", ".3")
            run("xdotool", "key", "p", "c", "r", "sleep", ".3")
            shot(editor, "shortcut-field-keeps-tool-letters")
            assert save_layers(lambda values: len(values) == 3, "field protects layer shortcuts")[-1] == copied
            toolbar_click("layers")
            run("xdotool", "key", "Delete", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "Delete removes selected copy")[-1] == shape
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 3, "undo keyboard deletion")[-1] == copied
            layer_click(0)  # Select restored copy explicitly after undo.
            run("xdotool", "key", "BackSpace", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "Backspace removes selected copy")[-1] == shape
            layer_click(1)  # Original image is locked.
            run("xdotool", "key", "Delete", "Right", "shift+Down", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 2, "locked keyboard deletion")[-1] == shape
            # Layer snapshots must work even with an empty OS clipboard. Copy
            # then move/delete the source, so paste cannot just duplicate it.
            subprocess.run(["xclip", "-selection", "clipboard", "-i"], input=b"",
                           env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           check=True, timeout=10)
            layer_click(0)
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
            wait(lambda: not windows("Captures Screenshot Editor"), "copied editor closes")
            editor = reopen()
            run("xdotool", "key", "ctrl+v", "sleep", ".3")
            assert save_layers(lambda values: len(values) == 3, "reopen has no layer clipboard")[-1] == pasted_twice
            toolbar_click("layers")
            before_menu = draft_bytes()
            window_move(layer_row(1), "sleep", ".2", "mousedown", "3",
                        "sleep", ".15", "mouseup", "3", "sleep", ".3")
            shot(editor, "layer-context-menu")
            run("xdotool", "key", "Escape", "sleep", ".3")
            assert draft_bytes() == before_menu, "opening/cancelling a row menu must not edit"
            resize_editor(760, 540, "sleep", ".3")
            window_move(layer_row(0), "sleep", ".2", "mousedown", "3",
                        "sleep", ".15", "mouseup", "3", "sleep", ".3")
            shot(editor, "layer-context-menu-minimum")
            # Copy the first row through the actual popup. A missed opening must
            # fail this flow, not silently produce a menu-free review capture.
            row_x, row_y = layer_row(0)
            click(editor, row_x + 30, row_y + 15)
            assert draft_bytes() == before_menu
            run("xdotool", "key", "ctrl+v", "sleep", ".3")
            menu_paste = save_layers(lambda values: len(values) == 4, "context-menu copy then paste")[-1]
            assert menu_paste == dict(pasted_twice, id=menu_paste["id"],
                                      x=pasted_twice["x"] + 24, y=pasted_twice["y"] + 24,
                                      endX=pasted_twice["endX"] + 24, endY=pasted_twice["endY"] + 24), (menu_paste, pasted_twice)
            assert menu_paste["id"] not in {shape["id"], pasted["id"], pasted_twice["id"]}
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "shortcut suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["open-no-selection", "explicit-layer-selection",
                           "keyboard-undo", "keyboard-redo-exact-layer", "field-undo-focus", "field-redo-focus",
                           "confirmation-focus", "shortcut-restored-after-dialog", "original-unchanged",
                           "duplicate-offset-fresh-id", "field-layer-shortcuts", "delete-selected-copy",
                           "backspace-selected-copy", "locked-delete-guard", "arrow-1px", "shift-arrow-10px",
                           "nudge-undo-exact", "field-nudge-focus", "locked-nudge-guard",
                           "S-star", "V-select-move", "C-crop-cancel", "R-rectangle", "field-tool-letters",
                           "rail-arrow-create", "rail-menu-escape", "rail-minimum",
                           "same-tool-clears-selection", "crop-clears-selection",
                           "created-layer-properties-edit", "created-layer-properties-undo",
                           "created-layer-properties-minimum",
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
            toolbar_click("draw")
            draw_tool("rectangle")
            drag((320, 250), (480, 370))
            save_layers(lambda values: len(values) == 2, "edited original fixture")
            saved_draft = draft_bytes()
            # A saved source starts beside itself: same folder and name, Save overwrites.
            export_click("filename")
            run("xdotool", "key", "ctrl+a", "ctrl+c", "sleep", ".2")
            assert run("xclip", "-selection", "clipboard", "-o").decode() == "original"
            blur_click()  # Leave the field without editing it.
            shot(editor, "overwrite-controls")
            assert source_export.read_bytes() == original
            assert json.loads(metadata_path.read_text()) == original_metadata
            # Like the shipping editor, Save replaces the source without a second step;
            # the shared publisher revalidates this History entry and path first.
            export_click("save")
            wait(lambda: source_export.read_bytes() != original, "original file replaced")
            wait(lambda: (artifact / "capture.png").read_bytes() != original, "same History image replaced")
            # As in shipping, every Save then shows the saved file in its folder.
            wait(lambda: file_manager.revealed == [source_export], "overwrite reveals the saved file")
            shot(editor, "overwrite-saved")
            for path in [source_export, artifact / "capture.png"]:
                assert run("identify", "-format", "%wx%h", str(path)) == b"640x360"
                assert run("convert", str(path), "-crop", "1x1+162+221", "-depth", "8", "rgb:-") == bytes((255, 59, 92))
            updated = json.loads(metadata_path.read_text())
            assert (updated["id"], updated["created_at"], updated["saved_path"]) == (
                artifact_id, original_metadata["created_at"], str(source_export))
            assert updated["size_bytes"] == len(source_export.read_bytes())
            assert len(list(history.glob("*/metadata.json"))) == 1
            assert draft_bytes() == saved_draft
            replaced = source_export.read_bytes()
            toolbar_click("undo")
            save_layers(lambda values: len(values) == 1, "undo preserved after output")
            assert source_export.read_bytes() == replaced
            toolbar_click("redo")
            save_layers(lambda values: len(values) == 2, "redo preserved after output")

            # "Save as new file" suggests an -edited name beside the source and never touches it.
            export_click("new-file")
            shot(editor, "overwrite-new-file-switch")
            export_click("save")
            copy_path = output / "original-edited.png"
            wait(copy_path.exists, "new file beside the original")
            wait(lambda: file_manager.revealed == [source_export, copy_path], "new file revealed")
            assert source_export.read_bytes() == replaced
            entries = [json.loads(path.read_text()) for path in history.glob("*/metadata.json")]
            assert len(entries) == 2
            copy_entry = next(entry for entry in entries if entry["id"] != artifact_id)
            assert copy_entry["saved_path"] == str(copy_path)
            shot(editor, "overwrite-new-file-saved")
            # The saved copy becomes the file Save overwrites, as in the shipping app.
            toolbar_click("undo")
            save_layers(lambda values: len(values) == 1, "undo before saving the adopted copy")
            copied_before = copy_path.read_bytes()
            export_click("save")
            wait(lambda: copy_path.read_bytes() != copied_before, "adopted copy overwritten")
            wait(lambda: file_manager.revealed == [source_export, copy_path, copy_path],
                 "adopted overwrite revealed")
            assert not opened_folders.exists(), "ShowItems selected the file; no folder fallback"
            assert run("convert", str(copy_path), "-crop", "1x1+162+221", "-depth", "8", "rgb:-") == bytes((40, 110, 166))
            assert source_export.read_bytes() == replaced
            entries = {json.loads(path.read_text())["id"] for path in history.glob("*/metadata.json")}
            assert entries == {artifact_id, copy_entry["id"]}
            toolbar_click("redo")
            save_layers(lambda values: len(values) == 2, "redo after saving the adopted copy")
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            properties_move("click", "--repeat", "25", "5")
            shot(editor, "overwrite-minimum")
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "overwritten editor closes")
            editor = reopen()
            assert len(layers()) == 2
            shot(editor, "overwrite-draft-reopened")
            assert source_export.read_bytes() == replaced
            close(root)
            wait(lambda: app.poll() is not None, "overwrite suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["source-name-and-folder-default", "save-overwrites-source",
                           "exact-file-pixels", "same-history-id-date", "draft-preserved",
                           "undo-redo-preserved", "save-as-new-file-keeps-source",
                           "saved-copy-becomes-overwrite-target", "every-save-reveals-file",
                           "minimum-controls",
                           "editable-draft-reopen"],
            }, indent=2) + "\n")
            print("PASS native overwrite: default overwrite, same History, new-file switch, adopted copy, draft and undo")
            return

        if args.rotation_snap_only:
            resize_editor(1000, 1001)
            toolbar_click("layers")
            layer_click(0, "lock")  # Unlock the original image for canvas rotation.
            save_layers(lambda values: not values[0]["locked"], "unlocked original")
            shot(editor, "rotation-snap-controls")
            before = draft_bytes()
            prop_field(ROTATION_SNAP, 37, x=50)
            shot(editor, "rotation-snap-custom")
            assert draft_bytes() == before, "snap preference must not edit or save a draft"
            # Full-canvas image uses the inset top grip at (558,117), pivot (558,269).
            # Vector (0,-152) to (100,-110) is 42.27°, giving 37°, not default 45°.
            rotation_start = fixture_point((328, 117))
            rotation_end = fixture_point((428, 159))
            run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, rotation_start),
                "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
                *map(str, rotation_end), "keydown", "Shift_L", "sleep", ".3")
            shot(editor, "rotation-snap-transient")
            assert draft_bytes() == before
            run("xdotool", "key", "Escape", "sleep", ".2", "mouseup", "1", "keyup", "Shift_L")
            assert draft_bytes() == before
            drag((558, 117), (658, 159), shift=True)
            angle = 37 * math.pi / 180
            save_layers(lambda values: math.isclose(values[0].get("rotation", 0), angle, abs_tol=1e-12),
                        "custom 37-degree rotation")
            shot(editor, "rotation-snap-committed")
            # Independently rotate the original yellow rectangle's interior point (150,130).
            yellow_document = (320 - 170 * math.cos(angle) + 50 * math.sin(angle),
                               180 - 170 * math.sin(angle) - 50 * math.cos(angle))
            document_pixel("rotation-snap-committed", *yellow_document, (229, 179, 68))
            toolbar_click("undo")
            save_layers(lambda values: values[0].get("rotation", 0) == 0, "custom rotation undo")
            toolbar_click("redo")
            save_layers(lambda values: math.isclose(values[0].get("rotation", 0), angle, abs_tol=1e-12),
                        "custom rotation redo")
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            properties_move("click", "--repeat", "14", "5")
            shot(editor, "rotation-snap-minimum")
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "custom rotation closes")
            editor = reopen()
            resize_editor(1000, 1001)
            toolbar_click("layers")
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

        if args.drawing_defaults_only:
            # Tall enough that every shadow row stays visible below the shape
            # picker, the DrawToolPreview card and the Color swatch rows.
            resize_editor(942, 1580)
            save(640, 360, 0, 0)
            before = draft_bytes()
            toolbar_click("draw")
            prop_click(16, CLOSED_DRAW_ROWS["stroke"])  # Enable the initially disabled closed-shape stroke.
            shot(editor, "drawing-default-controls")
            # Stroke color and Fill color are shipping swatch rows; Size and
            # Opacity are shipping RangeSliders set from the keyboard.
            prop_swatch(CLOSED_DRAW_ROWS["color"], "#111318")
            prop_slider(CLOSED_DRAW_ROWS["size"], "Home", "Prior", "Right")  # 2 + 10 + 1 = 13.
            prop_slider(CLOSED_DRAW_ROWS["opacity"], "Home", *["Prior"] * 3, *["Right"] * 7)  # 37.
            prop_swatch(CLOSED_DRAW_ROWS["fill"], "#36c96b")
            shot(editor, "drawing-custom-controls")
            assert draft_bytes() == before, "default controls alone wrote a draft"

            start, end = document_point((250, 45)), document_point((350, 145))
            run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, start),
                "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
                *map(str, end), "sleep", ".3")
            shot(editor, "drawing-styled-transient")
            assert draft_bytes() == before, "transient drawing wrote a draft"
            run("xdotool", "mouseup", "1", "sleep", ".3")
            created = save_layers(lambda values: len(values) == 2, "styled rectangle created")[-1]
            assert created["opacity"] == 37 and created["style"]["strokeWidth"] == 13, created
            assert created["style"]["color"] == "#111318" and created["style"]["fill"] == "#36c96b", created
            assert created["style"]["strokeEnabled"] is True
            shot(editor, "drawing-styled-committed")
            # Independently blend 37% #36c96b / #111318 over the #286ea6 source.
            document_pixel("drawing-styled-committed", 300, 95, (45, 144, 144), 1)
            # The active shape's north resize grip covers the stroke midpoint.
            # Keep the same border-color check ten pixels away from that chrome.
            document_pixel("drawing-styled-committed", 290, 40, (31, 76, 113), 1)
            toolbar_click("undo")
            save_layers(lambda values: len(values) == 1, "one undo removes the new shape")

            # A line must stroke despite the retained closed-shape toggle being off.
            toolbar_click("draw")
            prop_click(16, CLOSED_DRAW_ROWS["stroke"])
            draw_tool("line")
            shot(editor, "drawing-line-controls")
            start, end = document_point((250, 70)), document_point((370, 70))
            run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, start),
                "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
                *map(str, end), "sleep", ".3")
            shot(editor, "drawing-line-transient")
            document_pixel("drawing-line-transient", 300, 70, (31, 76, 113), 2)
            run("xdotool", "mouseup", "1", "sleep", ".3")
            line = save_layers(lambda values: len(values) == 2, "styled line created")[-1]
            assert line["shape"] == "line" and line["opacity"] == 37, line
            assert line["style"]["color"] == "#111318" and line["style"]["strokeWidth"] == 13, line
            # Like Tauri, preserve the closed-only flag in metadata; open rendering ignores it.
            assert line["style"]["fill"] is None and line["style"]["strokeEnabled"] is False
            shot(editor, "drawing-line-committed")
            document_pixel("drawing-line-committed", 300, 70, (31, 76, 113), 1)
            toolbar_click("undo")
            save_layers(lambda values: len(values) == 1, "undo line")
            toolbar_click("draw")
            prop_slider(OPEN_DRAW_ROWS["opacity"], "Home")  # Open tools have no closed Stroke toggle.
            start, end = document_point((250, 70)), document_point((370, 70))
            run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, start),
                "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
                *map(str, end), "sleep", ".3", "mouseup", "1", "sleep", ".3")
            invisible = save_layers(lambda values: len(values) == 2, "zero-opacity line remains an editable layer")[-1]
            assert invisible["opacity"] == 0, invisible
            shot(editor, "drawing-zero-opacity")
            document_pixel("drawing-zero-opacity", 300, 70, (40, 110, 166))

            toolbar_click("undo")
            save_layers(lambda values: len(values) == 1, "undo invisible line")
            toolbar_click("draw")
            before = draft_bytes()
            prop_slider(OPEN_DRAW_ROWS["opacity"], "End")
            prop_click(16, OPEN_DRAW_ROWS["shadow"])  # Line's pre-placement Drop shadow.
            shot(editor, "drawing-shadow-controls")
            # Shipping DropShadowFields, indented under the check row.
            prop_swatch(OPEN_DRAW_ROWS["shadow-color"], "#ffd22e", indent=SHADOW_INDENT)
            prop_slider(OPEN_DRAW_ROWS["shadow-opacity"], "End")
            prop_slider(OPEN_DRAW_ROWS["shadow-blur"], "Home")
            prop_field(OPEN_DRAW_ROWS["shadow-offset"], "-23", x=100)
            prop_field(OPEN_DRAW_ROWS["shadow-offset"], "31", x=240)
            shot(editor, "drawing-shadow-custom-controls")
            assert draft_bytes() == before, "shadow defaults alone wrote a draft"
            start, end = document_point((250, 70)), document_point((370, 70))
            run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, start),
                "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
                *map(str, end), "sleep", ".5")
            shot(editor, "drawing-shadow-transient")
            document_pixel("drawing-shadow-transient", 300, 70, (17, 19, 24), 1)
            document_pixel("drawing-shadow-transient", 277, 101, (255, 210, 46), 1)
            assert draft_bytes() == before, "pixel preview wrote a draft"
            run("xdotool", "key", "Escape", "mouseup", "1", "sleep", ".3")
            shot(editor, "drawing-shadow-cancelled")
            document_pixel("drawing-shadow-cancelled", 300, 70, (40, 110, 166), 1)
            document_pixel("drawing-shadow-cancelled", 277, 101, (40, 110, 166), 1)
            assert draft_bytes() == before, "cancelled preview wrote a draft"
            run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, start),
                "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
                *map(str, end), "sleep", ".5", "mouseup", "1", "sleep", ".3")
            shadowed = save_layers(lambda values: len(values) == 2, "shadowed line created")[-1]
            assert shadowed["style"]["dropShadow"] is True, shadowed
            custom = {"color": "#ffd22e", "opacity": 100, "blur": 0, "offsetX": -23, "offsetY": 31}
            assert shadowed["style"]["dropShadowStyle"] == custom, shadowed
            shot(editor, "drawing-shadow-committed")
            document_pixel("drawing-shadow-committed", 300, 70, (17, 19, 24), 1)
            document_pixel("drawing-shadow-committed", 277, 101, (255, 210, 46), 1)
            toolbar_click("undo")
            save_layers(lambda values: len(values) == 1, "one undo removes drawing and shadow")
            toolbar_click("draw")
            prop_click(16, OPEN_DRAW_ROWS["shadow"])
            run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, start),
                "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
                *map(str, end), "sleep", ".3", "mouseup", "1", "sleep", ".3")
            unshadowed = save_layers(lambda values: len(values) == 2, "disabled shadow line")[-1]
            assert unshadowed["style"]["dropShadow"] is False, unshadowed
            assert unshadowed["style"]["dropShadowStyle"] == custom, unshadowed
            shot(editor, "drawing-shadow-disabled")
            document_pixel("drawing-shadow-disabled", 277, 101, (40, 110, 166), 1)
            toolbar_click("draw")
            prop_click(16, OPEN_DRAW_ROWS["shadow"])
            shot(editor, "drawing-shadow-retained")
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "drawing defaults suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["default-fields-no-write", "asymmetric-stroke-fill", "custom-width-opacity",
                           "independent-composited-pixels", "single-undo", "open-stroke-ignores-closed-toggle",
                           "tool-and-response-retention", "zero-opacity-layer", "original-unchanged",
                           "shadow-defaults-no-write", "asymmetric-shadow-offset-pixels",
                           "transient-shadow-pixels", "cancel-restores-pixels-without-write",
                           "disabled-shadow-pixels", "retained-shadow-style"],
            }, indent=2) + "\n")
            print("PASS native drawing defaults: style, opacity, shadow pixels, undo and retained local choices")
            return

        if args.polygon_only:
            resize_editor(942, 701)
            save(640, 360, 0, 0)
            toolbar_click("draw")
            shot(editor, "polygon-tools")
            ids = []
            for name, start, end, center in [
                ("triangle", (418, 239), (278, 119), (348, 179)),
                ("diamond", (598, 289), (498, 129), (548, 209)),
                ("star", (818, 419), (658, 299), (738, 359)),
            ]:
                draw_tool(name)
                before = draft_bytes()
                transient_start, transient_end = canvas_point(start), canvas_point(end)
                run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, transient_start),
                    "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
                    *map(str, transient_end), "sleep", ".3")
                shot(editor, f"polygon-{name}-transient")
                pixel(f"polygon-{name}-transient", *canvas_point(center), (255, 59, 92))
                assert draft_bytes() == before, "transient polygon cannot write the draft"
                run("xdotool", "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
                assert draft_bytes() == before
                drag(start, (start[0], end[1]))
                save_layers(lambda values: len(values) == len(ids) + 1, "zero-width polygon rejected")
                drag(start, end)
                created = save_layers(lambda values: len(values) == len(ids) + 2, f"{name} created")[-1]
                assert created["shape"] == name and created["id"] not in ids
                assert (created["x"], created["y"], created["endX"], created["endY"]) == (
                    start[0] - 238, start[1] - 89, end[0] - 238, end[1] - 89)
                shot(editor, f"polygon-{name}-committed")
                pixel(f"polygon-{name}-committed", *canvas_point(center), (255, 59, 92))
                toolbar_click("undo")
                save_layers(lambda values: len(values) == len(ids) + 1, f"{name} undo")
                toolbar_click("redo")
                save_layers(lambda values: values[-1]["id"] == created["id"], f"{name} redo stable ID")
                ids.append(created["id"])
            # A convex hull or fan triangulation would incorrectly fill this star notch.
            fixture_pixel("polygon-star-transient", 508, 409, (46, 158, 113))
            fixture_pixel("polygon-star-committed", 508, 409, (46, 158, 113))
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, "polygon-minimum")
            properties_move("click", "--repeat", "5", "5")
            shot(editor, "polygon-minimum-scrolled")
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "polygon draft closes")
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

        if args.properties_heading_only:
            resize_editor(760, 540)
            before = draft_bytes()
            for name in ("image", "rectangle", "text", "crop"):
                if name == "image":
                    toolbar_click("layers")
                    layer_click(0)
                elif name == "crop":
                    toolbar_click("geometry")
                    start, end = document_point((50, 70)), document_point((500, 280))
                    run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, start),
                        "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
                        *map(str, end), "sleep", ".2", "mouseup", "1", "sleep", ".3")
                else:
                    draw_tool(name)
                top = properties_top()
                # Wheel inside the small fields viewport, not on its pinned title.
                run("xdotool", "mousemove", "--window", editor, str(inspector_x(180)),
                    str(top + 65), "click", "--repeat", "20", "--delay", "40", "4", "sleep", ".6")
                shot(editor, f"properties-{name}-top")
                run("xdotool", "click", "--repeat", "20", "--delay", "40", "5", "sleep", ".6")
                shot(editor, f"properties-{name}-scrolled")

                def band(state, offset, height):
                    return run("convert", str(output / f"properties-{name}-{state}.png"),
                               "-crop", f"280x{height}+{inspector_x(8)}+{top + offset}", "rgba:-")

                assert band("top", 1, 46) == band("scrolled", 1, 46), f"{name}: title scrolled away"
                assert band("top", 58, 20) != band("scrolled", 58, 20), f"{name}: fields did not scroll"
                assert draft_bytes() == before, f"{name}: scrolling changed the draft"
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "Properties heading suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["image-title", "shape-title", "text-title", "crop-title",
                           "real-fields-scroll", "minimum-layout", "unchanged-draft", "original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native Properties: pinned image/shape/text/crop titles, real scroll, unchanged draft")
            return

        if args.shape_transforms_only:
            resize_editor(1200, 701)
            save(640, 360, 0, 0)

            def shape_gesture(start, end, capture=None, cancel=False):
                before = draft_bytes()
                start, end = document_point(start), document_point(end)
                run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, start),
                    "sleep", ".2", "mousedown", "1", "sleep", ".2", "mousemove", "--sync",
                    "--window", editor, *map(str, end), "sleep", ".3")
                if capture:
                    shot(editor, capture)
                    assert draft_bytes() == before, "transient transform wrote a draft"
                if cancel:
                    run("xdotool", "key", "Escape", "sleep", ".2")
                run("xdotool", "mouseup", "1", "sleep", ".3")

            def rectangle_is(x, y, end_x, end_y):
                values = layers()
                return len(values) == 2 and all(math.isclose(values[-1][key], value, abs_tol=1e-6)
                    for key, value in (("x", x), ("y", y), ("endX", end_x), ("endY", end_y)))

            draw_tool("rectangle")
            shape_gesture((260, 110), (380, 210))
            rectangle = save_layers(lambda values: len(values) == 2, "fresh rectangle")[-1]
            rectangle_id = rectangle["id"]
            assert rectangle["style"]["strokeWidth"] == 8
            shot(editor, "shape-active-created")
            pixel("shape-active-created", 14, rail_point("shapes")[1], (255, 202, 40))
            shape_gesture((320, 160), (357, 191), capture="shape-active-moving")
            save_until(lambda: rectangle_is(297, 141, 417, 241), "active Rectangle moves its body")
            assert layers()[-1]["id"] == rectangle_id
            shot(editor, "shape-active-moved")
            document_pixel("shape-active-moved", 350, 190, (255, 59, 92))
            document_pixel("shape-active-moved", 300, 120, (40, 110, 166))
            toolbar_click("undo")
            save_until(lambda: rectangle_is(260, 110, 380, 210), "body move is one undo step")
            toolbar_click("redo")
            save_until(lambda: rectangle_is(297, 141, 417, 241), "body move redo")

            # Shipping scales the padded selection box affinely: 130x110
            # becomes 150x130. Endpoints lie 5px inside that original box.
            resized = (272 + 5 * 150 / 130, 116 + 5 * 130 / 110,
                       272 + 125 * 150 / 130, 116 + 105 * 130 / 110)
            shape_gesture((292, 136), (272, 116), capture="shape-active-resizing")
            save_until(lambda: rectangle_is(*resized), "active Rectangle resizes")
            shot(editor, "shape-active-resized")
            pixel("shape-active-resized", 14, rail_point("shapes")[1], (255, 202, 40))
            toolbar_click("undo")
            save_until(lambda: rectangle_is(297, 141, 417, 241), "resize is one undo step")
            toolbar_click("redo")
            save_until(lambda: rectangle_is(*resized), "resize redo")

            # Center (347,181), padded top near 117, grip 28px above it.
            # The release vector is (31,-73), independently deriving the angle.
            before = draft_bytes()
            shape_gesture((347, 88), (378, 108), capture="shape-active-rotation-cancel", cancel=True)
            assert draft_bytes() == before
            shape_gesture((347, 88), (378, 108))
            angle = math.atan2(31, 73)
            save_layers(lambda values: len(values) == 2 and math.isclose(
                values[-1].get("rotation", 0), angle, abs_tol=1e-12), "active Rectangle rotates")
            shot(editor, "shape-active-rotated")
            pixel("shape-active-rotated", 14, rail_point("shapes")[1], (255, 202, 40))
            document_pixel("shape-active-rotated", 347, 181, (255, 59, 92))
            toolbar_click("undo")
            save_layers(lambda values: values[-1].get("rotation", 0) == 0, "rotation single undo")
            toolbar_click("redo")
            save_layers(lambda values: math.isclose(values[-1].get("rotation", 0), angle,
                abs_tol=1e-12), "rotation redo")

            # Empty space must draw another rectangle, not move/select the original.
            shape_gesture((470, 270), (560, 320))
            created = save_layers(lambda values: len(values) == 3, "empty space starts another shape")[-1]
            assert created["id"] != rectangle_id and created["shape"] == "rectangle"
            toolbar_click("undo")
            save_layers(lambda values: len(values) == 2 and values[-1]["id"] == rectangle_id,
                        "new shape is one undo step")

            # Newly drawn Lines keep curve dots live; no Select click in between.
            draw_tool("line")
            shape_gesture((180, 290), (580, 290))
            line = save_layers(lambda values: len(values) == 3, "fresh line")[-1]
            shape_gesture((380, 290), (380, 250))
            curved = save_layers(lambda values: len(values) == 3 and len(values[-1]["controls"]) == 3,
                                 "active Line bends its starter dot")[-1]
            assert curved["id"] == line["id"] and curved["shape"] == "line"
            assert [(round(p["x"]), round(p["y"])) for p in curved["controls"]] == [
                (280, 290), (380, 250), (480, 290)]
            shot(editor, "shape-active-curve")
            pixel("shape-active-curve", 14, rail_point("shapes")[1], (255, 202, 40))
            document_pixel("shape-active-curve", 330, 270, (255, 59, 92), 8)
            saved = layers()
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "active-shape editor closes")
            editor = reopen()
            assert layers() == saved
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, "shape-active-reopened-minimum")
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "active-shape suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["active-rectangle-body", "body-move-pixels", "active-corner-resize",
                           "active-rotation", "rotation-cancel", "single-undo-redo",
                           "empty-space-new-shape", "active-line-starter", "curve-pixels",
                           "retained-active-tool", "exact-draft-reopen", "minimum-layout", "original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native active shapes: move, resize, rotate, cancel, curve, undo, pixels, draft")
            return

        if args.canvas_interactions_only:
            accent = (255, 202, 40)
            stroke = (255, 59, 92)

            def settled_pixel(name, point, expected, tolerance=3):
                # Pixel checks poll fresh screenshots until the frame settles.
                def matches():
                    shot(editor, name)
                    try:
                        document_pixel(name, *point, expected, tolerance)
                        return True
                    except AssertionError:
                        return False
                wait(matches, f"{name} pixel {point} settles to {expected}")

            def document_drag(start, end, double=False):
                start, end = document_point(start), document_point(end)
                run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, start),
                    "sleep", ".2", "mousedown", "1", "sleep", ".2", "mousemove", "--sync",
                    "--window", editor, *map(str, (start[0] + (end[0] - start[0]) // 2,
                                                   start[1] + (end[1] - start[1]) // 2)),
                    "sleep", ".1", "mousemove", "--sync", "--window", editor, *map(str, end),
                    "sleep", ".3", "mouseup", "1", "sleep", ".3")

            def document_double_click(point):
                x, y = document_point(point)
                run("xdotool", "mousemove", "--sync", "--window", editor, str(x), str(y),
                    "sleep", ".3", "click", "--repeat", "2", "--delay", "90", "1", "sleep", ".3")

            def document_json():
                return json.loads(draft.read_text())["document"]

            resize_editor(1200, 701)
            save(640, 360, 0, 0)

            # Curve: draw a line, drag its middle starter dot, then remove and
            # re-add points by double-clicking.
            draw_tool("line")
            document_drag((180, 150), (580, 150))
            line = save_layers(lambda values: len(values) == 2, "line created")[-1]
            assert line["shape"] == "line" and line["controls"] == []
            toolbar_click("layers")
            shot(editor, "curve-starters")
            document_drag((380, 150), (380, 230))
            curved = save_layers(lambda values: len(values[-1]["controls"]) == 3,
                                 "starter drag curves the line")[-1]
            assert curved["id"] == line["id"]
            assert [(round(c["x"]), round(c["y"])) for c in curved["controls"]] == [
                (280, 150), (380, 230), (480, 150)], curved["controls"]
            # The smooth path passes through the midpoints between controls.
            settled_pixel("curve-dragged", (330, 190), stroke, 8)
            document_double_click((380, 230))
            save_layers(lambda values: len(values[-1]["controls"]) == 2,
                        "double-click removes a curve point")
            toolbar_click("undo")
            save_layers(lambda values: len(values[-1]["controls"]) == 3, "curve point undo")
            toolbar_click("redo")
            save_layers(lambda values: len(values[-1]["controls"]) == 2, "curve point redo")

            # Lock blocks canvas dots, not the shipping Properties commands.
            layer_click(0, "lock")
            locked = save_layers(lambda values: values[-1]["locked"], "locked line")[-1]
            assert len(locked["controls"]) == 2
            document_drag((180, 150), (140, 110))
            document_double_click((280, 150))
            assert save_layers(lambda values: values[-1] == locked, "locked canvas gestures ignored")[-1] == locked
            layer_click(0)  # The blocked canvas press cleared UI selection.
            properties_end()
            shot(editor, "locked-curve-straighten")
            end_click(80, 82)
            straight = save_layers(lambda values: values[-1]["controls"] == [], "locked Properties Straighten")[-1]
            assert straight == dict(locked, controls=[])
            toolbar_click("undo")
            assert save_layers(lambda values: values[-1] == locked, "locked Straighten is one undo step")[-1] == locked
            properties_end()
            end_click(80, 82)
            save_layers(lambda values: values[-1]["controls"] == [], "straighten before bending")
            properties_end()
            shot(editor, "locked-curve-slider")
            # Keep focus through worker edits: Home is -100%, then three Right
            # keys reach -97%, each with its own undo step.
            end_slider(96, "Home", "Right", "Right", "Right", x=INSPECTOR_WIDTH // 2)
            bent = save_layers(lambda values: len(values[-1]["controls"]) == 1 and
                               values[-1]["controls"][0] == {"x": 380, "y": -238}, "locked Curve -97 percent")[-1]
            assert bent == dict(locked, controls=[{"x": 380, "y": -238}])
            shot(editor, "locked-curve-keyboard-result")
            # Restore the earlier curve before the remainder of the canvas suite.
            for controls in ([{"x": 380, "y": -242}], [{"x": 380, "y": -246}],
                             [{"x": 380, "y": -250}], [], locked["controls"]):
                toolbar_click("undo")
                restored = dict(locked, controls=controls)
                assert save_layers(lambda values: values[-1] == restored,
                                   "undo each locked Properties edit")[-1] == restored
            layer_click(0, "lock")
            save_layers(lambda values: not values[-1]["locked"], "unlock line for canvas suite")
            properties_start()

            # Expand canvas: a second line hangs past the right edge.
            draw_tool("line")
            document_drag((520, 300), (720, 300))
            hanging = save_layers(lambda values: len(values) == 3, "hanging line created")[-1]
            assert hanging["endX"] > 640
            toolbar_click("layers")
            shot(editor, "expand-idle")
            right, center_y = document_point((640, 300))
            click(editor, right + 22, center_y)
            save_until(lambda: document_json()["width"] > 640, "Expand canvas grows the canvas")
            expanded = document_json()
            assert expanded["height"] == 360 and expanded["width"] >= 721, expanded["width"]
            assert expanded["elements"][0]["x"] == 0, "right growth never shifts layers"
            shot(editor, "expand-applied")
            toolbar_click("undo")
            save_until(lambda: document_json()["width"] == 640, "Expand canvas is one undo step")
            # Hovering the action arms the ghost: its crossed right side carries
            # shipping's pulsing accent bar, with the bloom and particles beyond.
            run("xdotool", "mousemove", "--sync", "--window", editor, str(right + 22), str(center_y),
                "sleep", ".3")
            settled_pixel("expand-armed-edge", (expanded["width"], 150), accent, 3)
            run("xdotool", "mousemove", "--sync", "--window", editor, str(right - 200), str(center_y),
                "sleep", ".3")
            toolbar_click("redo")
            save_until(lambda: document_json()["width"] == expanded["width"], "Expand canvas redo")

            # Drop: a real XDND source offers a PNG while the pointer sits near the
            # capture's top edge, then drops it above the capture.
            dropped = output / "Dropped image.png"
            # Untagged sRGB, like the external-image fixtures.
            run("convert", "-size", "100x50", "xc:#8a2be2", "-strip", "PNG32:" + str(dropped))
            x, y = document_point((320, 8))
            run("xdotool", "mousemove", "--sync", "--window", editor, str(x), str(y), "sleep", ".3")
            source = subprocess.Popen(
                ["/usr/bin/python3", str(Path(__file__).with_name("x11_drag_source.py")),
                 "--target", str(editor), "--file", str(dropped),
                 "--log", str(output / "drag-source.jsonl")],
                env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
            children.append(source)
            assert source.stdout.readline().strip() == "entered"
            run("xdotool", "mousemove", "--sync", "--window", editor, str(x), str(y + 1),
                "sleep", ".3")
            source.stdin.write("position\n")
            source.stdin.flush()
            # The shipping top-edge bar paints the accent over the capture's edge.
            settled_pixel("drop-hover-top", (320, 1), accent, 3)
            source.stdin.write("drop\n")
            source.stdin.flush()
            assert source.stdout.readline().strip() == "done"
            source.wait(timeout=10)
            events = [json.loads(line)["event"] for line in
                      (output / "drag-source.jsonl").read_text().splitlines()]
            assert "converted" in events and events[-1] == "finished", events
            save_until(lambda: len(document_json()["elements"]) == 4, "dropped image imported")
            placed = document_json()
            image = placed["elements"][-1]
            assert image["kind"] == "image" and image["name"] == "Dropped image.png"
            # Placed above: fully outside, so the canvas grows once and shifts down.
            assert placed["height"] == 410 and (image["x"], image["y"]) == (270, 0), (
                placed["height"], image["x"], image["y"])
            settled_pixel("drop-placed", (320, 25), (138, 43, 226), 2)
            toolbar_click("undo")
            save_until(lambda: len(document_json()["elements"]) == 3, "drop is one undo step")
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "canvas draft closes")
            editor = reopen()
            reopened = document_json()
            assert reopened["width"] == expanded["width"]
            assert len(reopened["elements"][1]["controls"]) == 2, "curve survives the draft"
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "canvas suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["curve-starter-drag", "curve-point-double-click-remove", "curve-undo-redo",
                           "locked-canvas-curve-guard", "locked-properties-straighten", "locked-properties-bend",
                           "curve-keyboard-focus-through-worker", "curve-discrete-key-undo", "locked-properties-undo",
                           "expand-canvas-action", "expand-canvas-single-undo", "xdnd-drop-guide-top",
                           "xdnd-drop-placed-above", "drop-single-undo", "curve-draft-reopen",
                           "original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native canvas interactions: curve dots, Expand canvas, XDND drop guide and import")
            return

        if args.output_size_only:
            run("xdotool", "windowsize", "--sync", editor, "1000", "1000")
            export_settings(True)
            shot(editor, "output-size-original")
            exports = output / "exports"
            exports.mkdir()

            def size_export(name, expected, section=None):
                wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
                     "settings applied")
                shot(editor, f"output-size-{name}-settings")
                path = exports / f"{name}.png"
                export_filename(name)
                if section is not None:
                    toolbar_click(section)  # Save does not depend on the inspector section.
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

            def output_size(index):
                setting_menu(54, index, 4, item_height=40)
                export_bar["custom"] = index == 3

            setting_click(54)
            shot(editor, "output-size-menu")
            run("xdotool", "key", "Escape", "sleep", ".2")
            output_size(1)  # 75%.
            size_export("75-percent", "480x270", section="geometry")
            output_size(2)  # 50%.
            size_export("50-percent", "320x180", section="layers")
            output_size(3)  # Custom starts from 640x360, locked.
            setting_field(237, 96)
            size_export("locked", "96x54", section="draw")
            setting_click(372)  # Unlock aspect.
            setting_field(307, 31)
            size_export("unlocked", "96x31")
            export_click("copy")  # Copy ignores output dimensions.
            wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
                 "full-size copy")
            copied = output / "clipboard-original-size.png"
            copied.write_bytes(run("xclip", "-selection", "clipboard", "-t", "image/png", "-o"))
            assert run("identify", "-format", "%wx%h", str(copied)) == b"640x360"
            setting_field(237, 0)
            shot(editor, "output-size-invalid")
            export_click("save")  # Invalid dimensions keep Save disabled.
            time.sleep(.5)
            assert len(list(exports.iterdir())) == 4
            setting_field(237, 96)
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, "output-size-minimum")
            export_settings(False)
            toolbar_click("draw")  # Minimum width: no Output tab or scrolling.
            # xclip forks a selection owner; do not capture its inherited stdout pipe.
            subprocess.run(["xclip", "-selection", "clipboard", "/dev/null"], env=env,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True, timeout=10)
            export_click("copy")
            wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
                 "minimum copy")
            copied.write_bytes(run("xclip", "-selection", "clipboard", "-t", "image/png", "-o"))
            assert run("identify", "-format", "%wx%h", str(copied)) == b"640x360"
            shot(editor, "output-size-minimum-draw-copy")
            close(root)
            wait(lambda: app.poll() is not None, "output size suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["75-percent-save-history", "50-percent-save-history",
                           "custom-aspect-lock", "custom-independent-height", "copy-full-resolution",
                           "invalid-dimensions-disable-save", "minimum-controls",
                           "no-draft-or-original-write", "save-from-all-sections",
                           "copy-from-minimum-draw"],
            }, indent=2) + "\n")
            print("PASS native output sizing: percentages, custom lock/unlock, History, copy, no edits")
            return

        if args.output_presets_only:
            run("xdotool", "windowsize", "--sync", editor, "1000", "1000")
            export_settings(True)
            setting_click(282)
            shot(editor, "output-quality-menu")  # Per-format Save quality descriptions.
            run("xdotool", "key", "Escape", "sleep", ".2")
            quality_mode(1)  # Save quality: Compress.
            # Compress rows: size, quality, preset (426), estimate. The
            # before/after comparison covers the canvas on its own.
            compare_settled("output-compare-default")
            assert divider_shown("output-compare-default")
            # Dragging the round handle moves the split without editing anything.
            drag_split(.25)
            compare_settled("output-compare-dragged")
            assert divider_shown("output-compare-dragged", .25)
            assert not divider_shown("output-compare-dragged")
            # The focused split steps like shipping's range: Page Up is a tenth
            # of its 6-94 % span and Home is its minimum.
            run("xdotool", "key", "Prior", "sleep", ".3")
            compare_settled("output-compare-page-up")
            assert divider_shown("output-compare-page-up", .338)
            run("xdotool", "key", "Home", "sleep", ".3")
            compare_settled("output-compare-home")
            assert divider_shown("output-compare-home", .06)
            drag_split(.5, .06)
            compare_settled("output-compare-recentred")
            assert divider_shown("output-compare-recentred")
            assert not draft.exists(), "moving the split never saves a draft"
            # Release the comparison range's keyboard focus before capturing
            # the preset menu (the first select click otherwise transfers focus).
            px, py = setting_point(426)
            click(editor, px, py - 24)
            setting_click(426)
            shot(editor, "output-preset-menu")  # Per-format preset descriptions.
            run("xdotool", "key", "Escape", "sleep", ".2")
            setting_menu(426, 0, 5)  # Tiny.
            compare_settled("output-preset-tiny-preview")
            assert divider_shown("output-preset-tiny-preview")
            exports = output / "exports"
            exports.mkdir()
            tiny = exports / "tiny.png"
            export_filename("tiny")
            export_click("filename")
            run("xdotool", "key", "ctrl+a", "ctrl+c", "sleep", ".2")
            assert run("xclip", "-selection", "clipboard", "-o").decode() == "tiny"
            export_click("save")
            wait(tiny.exists, "Tiny PNG saved")
            wait(lambda: file_manager.revealed == [tiny], "Save reveals the new file")
            shot(editor, "output-preset-tiny")
            assert int(run("identify", "-format", "%k", str(artifact / "capture.png"))) > 256
            assert int(run("identify", "-format", "%k", str(tiny))) <= 32
            # Hide dismisses the comparison; Show before / after returns it.
            click(editor, *document_point((604, 24)))
            export_bar["dismissed"] = True
            compare_settled("output-compare-hidden")
            assert not divider_shown("output-compare-hidden")
            setting_click(770)
            compare_settled("output-compare-shown")
            assert divider_shown("output-compare-shown")
            setting_menu(426, 4, 5)  # Highest, not an arbitrary high numeric value.
            compare_settled("output-preset-highest-preview")
            highest = exports / "highest.png"
            export_filename("highest")
            export_click("save")
            wait(highest.exists, "Highest PNG saved")
            wait(lambda: file_manager.revealed == [tiny, highest], "Save reveals each new file")
            shot(editor, "output-preset-highest")
            assert tiny.exists(), "a new filename never replaces the previous save"
            assert run("convert", str(highest), "-depth", "8", "rgba:-") == run(
                "convert", str(artifact / "capture.png"), "-depth", "8", "rgba:-")
            quality_mode(2)  # Maximum file size: 10 MB by default.
            compare_settled("output-maximum-default")
            setting_field(418, "9")  # 9 MB still encodes.
            setting_menu(506, 0, 3, item_height=28)  # KB converts the typed value.
            setting_click(418)
            run("xdotool", "key", "ctrl+a", "ctrl+c", "sleep", ".2")
            assert run("xclip", "-selection", "clipboard", "-o") == b"9000", "MB → KB conversion"
            compare_settled("output-maximum-kb")
            setting_field(418, "9")  # 9 KB is below the 10 KB floor.
            shot(editor, "output-maximum-error")
            before_invalid_save = highest.read_bytes()
            export_click("save")  # The status explains the limit; Save stays disabled.
            time.sleep(.5)
            assert len(list(exports.iterdir())) == 2
            assert highest.read_bytes() == before_invalid_save, "invalid Save cannot overwrite the last output"
            assert file_manager.revealed == [tiny, highest], "invalid Save cannot publish or reveal a file"
            assert not draft.exists(), "output controls and exports never save a draft"
            assert (artifact / "capture.png").read_bytes() == original
            quality_mode(1)  # Inspect the affected preset control at minimum size too.
            compare_settled("output-preset-return-to-compress")
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, "output-preset-minimum")
            setting_click(426)
            shot(editor, "output-preset-minimum-menu")
            run("xdotool", "key", "Escape")
            # Extra groups resize the card and canvas; the comparison action
            # stays reachable in its new second row at the minimum size.
            click(editor, *document_point((604, 24)))
            export_bar["dismissed"] = True
            compare_settled("output-compare-minimum-hidden")
            assert not divider_shown("output-compare-minimum-hidden")
            setting_click(80, row=1)
            export_bar["dismissed"] = False
            compare_settled("output-compare-minimum-shown")
            assert divider_shown("output-compare-minimum-shown")
            quality_mode(2)
            setting_field(418, "1000")  # Valid KB budget; estimate wraps below it.
            compare_settled("output-maximum-minimum")
            assert divider_shown("output-maximum-minimum")
            setting_click(506)
            shot(editor, "output-maximum-minimum-menu")
            run("xdotool", "key", "Escape")
            assert not draft.exists() and (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "output preset suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["quality-and-preset-descriptions", "automatic-compare",
                           "split-handle-drag", "split-keyboard-page-home",
                           "tiny-saved-png-32-colors", "save-reveals-file", "compare-hide-and-show",
                           "highest-saved-png-exact-pixels", "maximum-size-units",
                           "maximum-size-floor-disables-save", "no-draft-or-original-write",
                           "minimum-controls-and-menu", "minimum-wrapped-comparison-action",
                           "minimum-wrapped-maximum-and-menu"],
            }, indent=2) + "\n")
            print("PASS native output presets: descriptions, automatic comparison, Tiny palette, Highest exact pixels, size units")
            return


        if args.text_input_only:
            save_layers(lambda values: len(values) == 1, "composition baseline")

            def begin_input(point):
                toolbar_click("draw")
                draw_tool("text")
                click(editor, *document_point(point))

            before = draft_bytes()
            begin_input((80, 60))
            shot(editor, "text-input-empty")
            assert draft_bytes() == before
            run("xdotool", "key", "Escape", "sleep", ".3")
            save_layers(lambda values: len(values) == 1, "blank input creates no layer")

            # Shipping has no Done/Cancel: the box sits on the canvas in the
            # layer's own style, clicking away commits and blank text discards.
            begin_input((80, 60))
            type_text("Discard this", 1)
            shot(editor, "text-input-inline")
            run("xdotool", "key", "ctrl+a", "BackSpace", "sleep", ".3")
            blur_click()
            save_layers(lambda values: len(values) == 1, "clicking away from blank text adds no layer")

            before = draft_bytes()
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
            # Clipboard publication follows a UI repaint. Wait for the one
            # accepted Copy command, without reissuing it or relying on a
            # fixed sleep under software rendering / concurrent build load.
            copied = wait(lambda: value if (value := run("xclip", "-selection", "clipboard", "-o"))
                          == oversized else None, "selected oversized input copied after preview error")
            shot(editor, "text-input-error")
            assert copied == oversized, (len(copied), copied[:80])
            assert draft_bytes() == before, "failed previews must not save"
            run("xdotool", "key", "ctrl+a")
            type_text("Recovered", 1)
            run("xdotool", "key", "Escape", "sleep", ".3")
            save_layers(lambda values: len(values) == 2 and values[-1]["text"] == "Recovered",
                        "typing remains editable after preview error")
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            save_layers(lambda values: len(values) == 1, "recovered input is one undo step")

            before = draft_bytes()
            begin_input((80, 60))
            type_text("Alpha", 1)
            run("xdotool", "key", "Return")
            type_text("Beta", 1)
            time.sleep(.3)
            shot(editor, "text-input-typing")
            # The session preview omits the typed layer; the inline editor
            # paints its Rounded Box plate at the layer's own position.
            settled_document_pixel("text-input-typing", 80, 57, (17, 19, 24), 2)
            assert draft_bytes() == before, "typing previews must not persist a draft"
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
            blur_click()  # Clicking away commits, like shipping's textarea blur.
            revised = save_layers(lambda values: len(values) == 2 and values[-1]["text"] == "Revised\nline two",
                                  "existing Text hit edits and commits on click-away")[-1]
            assert revised["id"] == created["id"]
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            # An edit and its undo have the same layer count. Wait for the exact
            # old layer, not a stable but not-yet-updated autosave manifest.
            assert save_layers(lambda values: len(values) == 2 and values[-1] == created,
                               "existing input single undo")[-1] == created
            shot(editor, "text-input-existing-undo")

            # Select explicitly; finishing/undo keeps Text active. Double-click edits the same layer
            # without switching to the Text tool or committing a move/resize.
            toolbar_click("layers")
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
            assert save_layers(lambda values: len(values) == 2 and values[-1] == created,
                               "double-click edit single undo")[-1] == created

            # Outlined typing must remain hollow even when TextEdit recolors
            # selected glyphs. The document renderer already has this style;
            # exercise the separate live input, not only the accepted preview.
            resize_editor(1000, 1001, "sleep", ".3")
            toolbar_click("layers")
            click(editor, *document_point((600, 320)))
            toolbar_click("draw")
            draw_tool("text")
            prop_click(95, TEXT_DEFAULTS["style"])
            prop_click(60, 271)  # Outlined: Plain, Standard, Rounded, Outlined.
            prop_field(TEXT_DEFAULTS["size"], 128, x=59)
            before = draft_bytes()
            begin_input((100, 140))
            type_text("Oo", 1)
            shot(editor, "text-input-outline-wide-stroke")
            assert draft_bytes() == before
            run("xdotool", "key", "Escape", "sleep", ".3")
            large = save_layers(lambda values: len(values) == 3, "large outlined input finishes")[-1]
            assert large["outlined"] and large["fontSize"] == 128 and large["text"] == "Oo"
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            save_layers(lambda values: len(values) == 2 and values[-1] == created,
                        "large outlined creation undo")
            # Undo can restore the prior selected layer. Deselect before
            # changing defaults, rather than typing into its live Properties.
            toolbar_click("layers")
            click(editor, *document_point((600, 320)))
            toolbar_click("draw")
            draw_tool("text")
            prop_field(TEXT_DEFAULTS["size"], 72, x=59)
            before = draft_bytes()
            begin_input((300, 180))
            type_text("BOLD", 1)
            shot(editor, "text-input-outline-new")
            assert draft_bytes() == before
            run("xdotool", "key", "Escape", "sleep", ".3")
            outlined = save_layers(lambda values: len(values) == 3, "outlined input finishes")[-1]
            assert outlined["text"] == "BOLD" and outlined["outlined"]
            assert outlined["fontSize"] == 72 and outlined["background"] is None
            toolbar_click("layers")
            prop_click(37, 430)  # Existing Text Properties: Bold.
            bold = save_layers(lambda values: values[-1]["bold"], "outlined bold applied")[-1]
            before = draft_bytes()
            begin_input((300, 200))
            run("xdotool", "key", "ctrl+a", "ctrl+c", "sleep", ".2")
            wait(lambda: run("xclip", "-selection", "clipboard", "-o") == b"BOLD",
                 "outlined selection copied from the live input")
            shot(editor, "text-input-outline-selected")
            run("xdotool", "key", "ctrl+End", "Left", "sleep", ".2")
            shot(editor, "text-input-outline-caret")
            run("xdotool", "key", "Escape", "sleep", ".3")
            assert save_layers(lambda values: values[-1] == bold, "unchanged outlined input")[-1] == bold
            assert draft_bytes() == before
            toolbar_click("layers")
            layer_click(0)  # Unchanged input restores its previous selection.
            # A one-line 72pt label has a 90px frame. Turn its top grip to
            # the right of the centre with the shipping Shift snap (90°).
            center_x = bold["x"] + bold["width"] / 2
            center_y = bold["y"] + 45
            _, _, scale = fit_geometry()
            radius = 45 + 28 / scale
            # drag() uses the historical left-inspector fixture coordinates.
            drag((center_x + 238, center_y - radius + 89),
                 (center_x + radius + 238, center_y + 89), shift=True)
            rotated = save_layers(lambda values: math.isclose(values[-1].get("rotation", 0),
                                  math.pi / 2, abs_tol=1e-6), "outlined label rotated")[-1]
            before = draft_bytes()
            begin_input((center_x, center_y))
            run("xdotool", "key", "ctrl+End", "Left", "sleep", ".2")
            shot(editor, "text-input-outline-rotated")
            run("xdotool", "key", "Escape", "sleep", ".3")
            save_layers(lambda values: values[-1] == rotated, "unchanged rotated outlined input")
            assert draft_bytes() == before
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            save_layers(lambda values: values[-1] == bold, "outlined rotation undo")
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            save_layers(lambda values: values[-1] == outlined, "outlined bold undo")
            run("xdotool", "key", "ctrl+z", "sleep", ".3")
            save_layers(lambda values: len(values) == 2 and values[-1] == created, "outlined creation undo")
            toolbar_click("layers")
            click(editor, *document_point((600, 320)))
            toolbar_click("draw")
            draw_tool("text")
            prop_click(95, TEXT_DEFAULTS["style"])
            prop_click(60, 431)  # Rounded Box, restoring the suite's defaults.
            prop_field(TEXT_DEFAULTS["size"], 24, x=59)
            resize_editor(1000, 700, "sleep", ".3")

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
                "checks": ["blank-new-no-layer", "click-away-blank-discards", "error-no-draft",
                           "error-recovery-one-undo", "preview-no-draft", "multiline-exact", "one-create-undo",
                           "redo-exact", "existing-hit-same-id", "click-away-commits", "existing-one-undo", "minimum-input",
                           "select-double-click-same-id-position", "double-click-one-undo",
                           "outlined-wide-stroke-preview", "outlined-wide-stroke-undo",
                           "outlined-preview-no-draft", "outlined-input-style", "outlined-selection-clipboard",
                           "outlined-unchanged-no-draft", "outlined-bold-and-create-undo",
                           "outlined-rotated-input", "outlined-rotation-undo",
                           "blank-existing-delete", "delete-undo-redo", "quit-latest-buffer", "original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native Text input: transient typing, multiline, existing hit, undo, minimum and quit")
            return

        if args.text_defaults_only:
            resize_editor(1000, 1001)
            save_layers(lambda values: len(values) == 1, "baseline draft before Text defaults")
            toolbar_click("draw")
            draw_tool("text")
            shot(editor, "text-defaults-initial")
            fixture_click((488, 289))  # Default Rounded Box centered at document (480,200).
            type_text("Rounded")
            run("xdotool", "key", "Escape", "sleep", ".3")
            rounded = save_layers(lambda values: len(values) == 2, "default rounded Text placed")[-1]
            assert (rounded["fontFamily"], rounded["fontSize"], rounded["color"]) == ("rounded", 24, "#ff3b5c")
            assert rounded["background"] == "#111318" and rounded["roundedBackground"]
            assert rounded["align"] == "center" and rounded["y"] == 200
            assert math.isclose(rounded["x"] + rounded["width"] / 2, 480, abs_tol=1e-6)
            assert json.loads(draft.read_text())["fonts"]["families"]["rounded"] == "Nunito"
            toolbar_click("layers")  # Deselect with Select; Text stays active after Finish.
            fixture_click((28, 109))
            shot(editor, "text-defaults-rounded")
            document_pixel("text-defaults-rounded", 480, 196, (17, 19, 24))
            toolbar_click("draw")
            resize_editor(760, 540)
            shot(editor, "text-defaults-rounded-minimum")
            properties_move("click", "--repeat", "3", "5", "sleep", ".3")
            shot(editor, "text-defaults-rounded-minimum-controls")
            properties_move("click", "--repeat", "10", "4", "sleep", ".3")
            resize_editor(1000, 1001)
            toolbar_click("undo")
            save_layers(lambda values: len(values) == 1, "default rounded creation single undo")
            before = draft_bytes()
            toolbar_click("draw")
            draw_tool("text")
            prop_click(95, TEXT_DEFAULTS["style"])
            # The shipping TextStylePicker menu: 38 px chip rows, Plain first.
            shot(editor, "text-defaults-menu")
            prop_click(120, TEXT_DEFAULTS["mono-box"])  # Mono Box, before Rounded Box.
            prop_field(TEXT_DEFAULTS["size"], 37.5, x=59)
            # Shipping's Text section has no Color: new text takes the one
            # drawing Color, chosen here with the Line.
            draw_tool("line")
            prop_swatch(OPEN_DRAW_ROWS["color"], "#2d9cff")
            draw_tool("text")
            shot(editor, "text-defaults-staged")
            assert draft_bytes() == before, "defaults must not write a draft"
            fixture_click((208, 169))  # Document (200,80), at actual-size scale.
            type_text("Native")
            shot(editor, "text-defaults-composing")
            # The inline editor draws the Mono Box plate where the label lands.
            settled_document_pixel("text-defaults-composing", 200, 76, (17, 19, 24), 2)
            run("xdotool", "key", "Escape", "sleep", ".3")
            text = save_layers(lambda values: len(values) == 2, "styled Text placed")[-1]
            assert text["kind"] == "text" and text["fontFamily"] == "mono"
            assert text["fontSize"] == 37.5 and text["color"] == "#2d9cff", text
            assert text["text"] == "Native" and text["align"] == "center" and text["y"] == 80
            assert math.isclose(text["x"] + text["width"] / 2, 200, abs_tol=1e-6)
            assert text["background"] == "#111318" and text["autoWidth"]
            toolbar_click("layers")
            fixture_click((28, 109))  # Deselect: the rotation stem crosses the plate sample.
            shot(editor, "text-defaults-created")
            document_pixel("text-defaults-created", 200, 76, (17, 19, 24))
            toolbar_click("undo")
            save_layers(lambda values: len(values) == 1, "styled creation single undo")
            toolbar_click("redo")
            save_layers(lambda values: len(values) == 2 and values[-1]["id"] == text["id"],
                        "styled creation redo")
            toolbar_click("draw")
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, "text-defaults-minimum")
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "styled Text editor closes")
            editor = reopen()
            resize_editor(1000, 1001)
            toolbar_click("draw")
            draw_tool("text")
            shot(editor, "text-defaults-reopened")
            assert layers()[-1] == text
            fixture_click((408, 269))  # Fresh editor defaults, not saved Mono Box/37.5.
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
            # Shipping selected-text Properties, below the Shift rotation snap
            # section: offsets of control centres from properties_top().
            # The 320px column fits the snap hint on two lines and each
            # ColorField on two swatch rows.
            TEXT = {"style": 222, "text": 306, "font": 386, "size": 386, "format": 430,
                    "color": 502, "background": 578, "shadow": 618}
            # The style menu lists the seven shipping styles (no Plain) on a
            # 40 px pitch below the trigger.
            TEXT_STYLE_ROWS = {name: TEXT["style"] + 45 + 40 * index for index, name in enumerate(
                ["standard", "rounded", "outlined", "mono", "box", "mono-box", "rounded-box"])}
            # A plate adds its Background color swatches above Drop shadow.
            PLATE_ROWS = 106
            # With the shadow open and Properties scrolled to its end, rows
            # measured up from the bottom of the Properties area.
            TEXT_SHADOW_END = {"opacity": 154, "blur": 85, "offset": 27}

            def shadow_rows(plated):
                check = TEXT["shadow"] + (PLATE_ROWS if plated else 0)
                return {"check": check, "color": check + 70, "opacity": check + 177,
                        "blur": check + 246, "offset": check + 306}

            resize_editor(1000, 1501)
            toolbar_click("draw")  # Draw.
            draw_tool("text")
            prop_click(95, TEXT_DEFAULTS["style"])
            prop_click(60, TEXT_DEFAULTS["standard"])  # Standard: plain glyphs before a plate.
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
            pixel(f"text-created-{args.appearance}", 14, rail_point("text")[1], (255, 202, 40))
            # Text keeps its live style fields without transform chrome. Select
            # restores the rotation section used by these inspector coordinates.
            toolbar_click("layers")
            shot(editor, f"text-selected-{args.appearance}")
            pixel(f"text-selected-{args.appearance}", 14, rail_point("select")[1], (255, 202, 40))
            prop_click(95, TEXT["style"])
            shot(editor, f"text-style-menu-{args.appearance}")
            run("xdotool", "key", "Escape")
            # Shipping applies every text property as it changes: a typing burst
            # in one field is one undo step, each toggle or menu choice another.
            prop_click(60, TEXT["font"])
            shot(editor, f"text-font-menu-{args.appearance}")
            # The token Font listbox opens below: Sans serif, Serif, Monospace,
            # Rounded on a 28 px pitch.
            prop_click(40, TEXT["font"] + 69)  # Serif.
            save_layers(lambda values: values[-1]["fontFamily"] == "serif", "font family applied live")
            prop_click(105, TEXT["text"])
            run("xdotool", "key", "ctrl+a", "type", "--clearmodifiers", "--delay", "35",
                "--", "Readable native text")
            time.sleep(.2)
            save_layers(lambda values: values[-1]["text"] == "Readable native text"
                        and values[-1]["fontFamily"] == "serif", "plain text applied")
            prop_click(37, TEXT["format"])   # Bold: the first of five format buttons.
            prop_click(98, TEXT["format"])   # Italic.
            save_layers(lambda values: values[-1]["bold"] and values[-1]["italic"], "traits applied")
            shot(editor, "text-without-shadow")
            rows = shadow_rows(False)
            prop_click(92, rows["check"])   # Drop shadow, leaving the plate off.
            save_layers(lambda values: values[-1].get("dropShadow") is True
                        and values[-1]["background"] is None, "glyph shadow applied")
            prop_swatch(rows["color"], "#2d9cff", indent=SHADOW_INDENT)
            # Expanded numeric rows sit below the viewport; reach them before
            # pressing keys rather than clicking clipped inspector content.
            properties_end()
            shot(editor, "text-shadow-scrolled")
            end_slider(TEXT_SHADOW_END["opacity"], "Home", *["Prior"] * 6, *["Right"] * 5)  # 65%.
            save_layers(lambda values: values[-1]["dropShadowStyle"]["opacity"] == 65,
                        "shadow opacity applied")
            end_slider(TEXT_SHADOW_END["blur"], "Home", *["Right"] * 3)  # 3 px.
            end_field(TEXT_SHADOW_END["offset"], "17", x=100)
            end_field(TEXT_SHADOW_END["offset"], "-8", x=240)
            custom_shadow = {"color": "#2d9cff", "opacity": 65, "blur": 3,
                             "offsetX": 17, "offsetY": -8}
            save_layers(lambda values: values[-1].get("dropShadowStyle") == custom_shadow,
                        "custom shadow applied")
            shot(editor, "text-glyph-shadow")
            def text_pixels(name, crop=None):
                if crop is None:
                    window, size, bar = shot_layouts[name]
                    left, top, scale = fit_geometry(size, window, bar)
                    crop = f"{round(size[0] * scale)}x{round(size[1] * scale)}+{round(left)}+{round(top)}"
                return run("convert", str(output / f"{name}.png"), "-crop", crop,
                           "-depth", "8", "rgba:-")
            assert text_pixels("text-glyph-shadow") != text_pixels("text-without-shadow")
            properties_start()
            # Shipping "Text background": a #111318 plate that owns the shadow now.
            prop_click(92, TEXT["background"])
            edited = save_layers(
                lambda values: values[-1]["text"] == "Readable native text"
                and values[-1]["bold"] and values[-1]["italic"]
                and values[-1]["background"] == "#111318" and values[-1]["fontFamily"] == "serif"
                and values[-1].get("dropShadow") is True and not values[-1]["outlined"],
                "readable styled text applied")[-1]
            assert edited["id"] == created["id"]
            shot(editor, f"text-edited-{args.appearance}")
            toolbar_click("undo")
            save_layers(lambda values: values[-1]["background"] is None
                        and values[-1].get("dropShadow") is True, "undo shadowed plate")
            toolbar_click("undo")
            save_layers(lambda values: values[-1]["dropShadowStyle"]["offsetY"] != -8,
                        "one typed shadow field is one undo step")
            toolbar_click("redo")
            save_layers(lambda values: values[-1]["dropShadowStyle"] == custom_shadow, "redo shadow field")
            toolbar_click("redo")
            save_layers(lambda values: values[-1]["background"] is not None, "redo shadowed plate")
            # Like shipping, outline is a style (Outlined), not its own control.
            prop_click(95, TEXT["style"])
            prop_click(60, TEXT_STYLE_ROWS["outlined"])
            save_layers(lambda values: values[-1]["outlined"] and values[-1]["background"] is None
                        and values[-1]["fontFamily"] == "sans", "outlined style applied live")
            shot(editor, "text-outline")
            assert text_pixels("text-outline") != text_pixels("text-glyph-shadow")
            toolbar_click("undo")
            save_layers(lambda values: not values[-1]["outlined"]
                        and values[-1]["background"] is not None, "undo outlined style")
            prop_click(95, TEXT["style"])
            shot(editor, "text-style-menu-edited")
            prop_click(60, TEXT_STYLE_ROWS["mono-box"])  # Mono Box, preserving the accepted plate color.
            save_layers(lambda values: values[-1]["fontFamily"] == "mono"
                        and values[-1]["background"] == edited["background"], "named style applied live")
            shot(editor, "text-preset-applied-live")
            toolbar_click("undo")  # The preset is one undo step; its future default stays.
            save_layers(lambda values: values[-1]["fontFamily"] == "serif", "named style undo")
            toolbar_click("draw")
            draw_tool("text")
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
            toolbar_click("undo")
            save_layers(lambda values: len(values) == 2, "future label is one undo step")
            toolbar_click("layers")
            layer_click(0)  # Restore the original label's selected-text inspector.
            prop_click(95, TEXT["style"])
            prop_click(60, TEXT_STYLE_ROWS["mono-box"])  # Mono Box again.
            preset = save_layers(lambda values: values[-1]["fontFamily"] == "mono",
                                 "named style applied")[-1]
            for key in ["text", "fontSize", "bold", "italic", "align", "color", "background", "dropShadowStyle"]:
                assert preset[key] == edited[key], f"preset must preserve {key}"
            shot(editor, f"text-preset-applied-{args.appearance}")
            toolbar_click("undo")
            save_layers(lambda values: values[-1]["fontFamily"] == "serif", "named style undo")
            toolbar_click("redo")
            save_layers(lambda values: values[-1]["fontFamily"] == "mono", "named style redo")
            toolbar_click("undo")
            save_layers(lambda values: values[-1]["fontFamily"] == "serif",
                        "restore accepted style for reopen")
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "text editor closes")
            editor = reopen()
            toolbar_click("layers")  # Layers, with the restored text selected explicitly.
            layer_click(0)
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, f"text-minimum-reopened-{args.appearance}")
            before_scroll = draft_bytes()
            properties_move(
                "click", "--repeat", "7", "--delay", "80", "5", "sleep", ".3")
            shot(editor, f"text-minimum-controls-{args.appearance}")
            properties_move(
                "click", "--repeat", "4", "--delay", "80", "5", "sleep", ".3")
            shot(editor, f"text-minimum-shadow-controls-{args.appearance}")
            assert draft_bytes() == before_scroll, "scrolling text controls must not edit"
            reopened = layers()[-1]
            assert reopened["id"] == created["id"] and reopened["text"] == "Readable native text"
            assert reopened["fontFamily"] == "serif"
            assert json.loads(draft.read_text())["fonts"]["families"] == {
                "sans": "Liberation Sans", "serif": "Liberation Serif", "mono": "Liberation Mono",
                "rounded": "Nunito"}
            assert reopened["bold"] and reopened["italic"] and reopened["background"] is not None
            assert reopened.get("dropShadow") is True
            assert reopened["dropShadowStyle"] == custom_shadow
            assert not reopened["outlined"]
            toolbar_click("draw")
            draw_tool("pen")
            start, end = document_point((100, 60)), document_point((180, 100))
            run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, start),
                "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
                *map(str, end), "sleep", ".3", "mouseup", "1", "sleep", ".3")
            placed = save_layers(lambda values: len(values) == 3, "Pen Properties fixture")
            assert placed[-1]["kind"] == "path" and placed[-1]["points"]
            properties_start()
            shot(editor, f"pen-created-properties-{args.appearance}")
            pixel(f"pen-created-properties-{args.appearance}", 14, rail_point("pen")[1], (255, 202, 40))
            toolbar_click("layers")
            properties_start()
            shot(editor, f"pen-selected-properties-{args.appearance}")
            pixel(f"pen-selected-properties-{args.appearance}", 14, rail_point("select")[1], (255, 202, 40))
            assert layers() == placed, "switching tool Properties must not edit layers"
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "text suite quits")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance,
                "checks": ["text-click-once-fresh-selection", "text-readable-live-apply",
                           "text-font-family", "text-typing-one-undo-step", "text-bold-italic-plate",
                           "text-glyph-shadow-pixels", "text-shadow-field-undo",
                           "text-custom-shadow-settings-reopen",
                           "text-named-style-live", "text-named-style-preserve-undo-redo",
                           "text-preset-future-after-undo", "text-preset-future-independent-traits",
                           "text-plate-shadow", "text-shadow-undo-redo-reopen",
                           "text-outlined-style-pixels", "text-outlined-style-undo",
                           "text-undo-redo", "text-draft-reopen",
                           "text-minimum-appearance", "pen-properties-tool-switch-no-edit",
                           "original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native Text UI: create, style, undo/redo, save/reopen, minimum")
            return

        if args.text_draft_only:
            shot(editor, "text-draft-restored")
            # Dismiss the restored-draft notice so the authored layout applies.
            click(editor, editor_width() - DRAFT_DISMISS_RIGHT, 20)
            resize_editor(942, 701)
            toolbar_click("layers")  # Layers.
            fixture_click((370, 230))  # Select the text plate, including non-ink pixels.
            drag((600, 230), (625, 247))
            save_layers(lambda values: (values[1]["x"], values[1]["y"]) == (275, 57), "text moved")
            # The edit autosaved the restored draft with its pinned font bytes.
            assert json.loads(draft.read_text())["updated_at_ms"] > 1, "font-backed draft autosave"
            assert (draft.parent / "fonts/regular.font").read_bytes() == font_bytes
            assert json.loads(draft.read_text())["fonts"] == {
                "families": {"sans": "Captures Shaping Test"}, "assets": ["regular"]}
            shot(editor, "text-moved")
            toolbar_click("undo")
            save_layers(lambda values: (values[1]["x"], values[1]["y"]) == (250, 40), "text move undone")
            drag((697, 229), (737, 229))
            save_layers(lambda values: math.isclose(values[1]["width"], 220.2, abs_tol=1e-4)
                        and values[1]["fontSize"] == 80, "text fixed-width side resize")
            shot(editor, "text-resized")
            toolbar_click("undo")
            save_layers(lambda values: values[1]["width"] == 180, "text resize undone")
            # The top rotation handle would be outside the canvas; use the lower one.
            drag((578, 375), (428, 229), shift=True)
            save_layers(lambda values: math.isclose(values[1].get("rotation", 0), math.pi / 2, abs_tol=1e-6),
                        "text quarter-turn")
            run("xdotool", "mousemove", "0", "0")
            shot(editor, "text-rotated")
            # Double-clicking edits the turned label in place: the inline box
            # rotates with the layer in the draft's own face, and Escape with
            # no change adds no undo step.
            before = draft_bytes()
            x, y = fixture_point((370, 230))
            run("xdotool", "mousemove", "--window", editor, str(x), str(y), "sleep", ".2",
                "click", "--repeat", "2", "--delay", "120", "1", "sleep", ".5")
            shot(editor, "text-rotated-inline")
            run("xdotool", "key", "Escape", "sleep", ".3")
            save_layers(lambda values: math.isclose(values[1].get("rotation", 0), math.pi / 2, abs_tol=1e-6)
                        and values[1]["text"] == "L\nfi", "rotated inline edit round trip")
            assert draft_bytes() == before, "an unchanged inline edit writes nothing"
            toolbar_click("undo")
            save_layers(lambda values: values[1].get("rotation", 0) == 0, "text rotation undone")
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "font-backed editor closes")
            editor = reopen()
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            run("xdotool", "mousemove", "0", "0")
            shot(editor, "text-draft-minimum-reopened")
            run("xdotool", "windowsize", "--sync", editor, "1000", "800")
            preview_encoded()
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
                           "text-canvas-resize-undo", "text-canvas-rotation-undo",
                           "rotated-inline-round-trip", "text-minimum-reopen",
                           "text-clipboard-dimensions-and-ink", "text-plate-and-original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native text draft: fonts, move/rotate/undo, save/reopen, minimum, clipboard pixels")
            return

        if args.brush_only:
            resize_editor(942, 701)
            save(640, 360, 0, 0)
            source = layers()[0]["src"]
            toolbar_click("draw")
            draw_tool("restore")  # Restore before the first edit reports a recoverable error.
            fixture_click((108, 189))
            shot(editor, "brush-restore-error")
            save_layers(lambda values: values[0]["src"] == source, "restore without original is atomic")
            draw_tool("erase")  # Erase; keep shipping diameter/softness defaults.
            shot(editor, "brush-controls")
            # Shipping's DrawToolPreview brush dab: opaque `--solid` at its centre,
            # in the 88px card below the Eraser mode row.
            dab = run("convert", str(output / "brush-controls.png"), "-crop",
                      f"1x1+{inspector_x(160)}+{properties_top() + ERASER_MODE_ROW + 16 + 12 + 44}",
                      "-depth", "8", "rgb:-")
            assert (min(dab) >= 200) if args.appearance == "dark" else (max(dab) <= 60), dab
            # Shipping `.screenshot-brush-cursor`: over the image the system
            # cursor hides behind a white ring of the brush's displayed size,
            # with a dark halo just outside it.
            ring_x, ring_y = fixture_point((108, 189))
            run("xdotool", "mousemove", "--window", editor, str(ring_x), str(ring_y), "sleep", ".3")
            shot(editor, "brush-ring")
            radius = 28 * fit_geometry(document_size(), window_size())[2] / 2
            def ring_rgb(offset):
                return run("convert", str(output / "brush-ring.png"), "-crop",
                           f"1x1+{round(ring_x + offset)}+{ring_y}", "-depth", "8", "rgb:-")
            border = max((ring_rgb(radius - inset) for inset in (0.5, 1, 1.5)), key=min)
            # The fixture is (229, 179, 68): only the white border lifts blue this far.
            assert min(border) >= 190, ("brush ring border", border)
            halo = min((ring_rgb(radius + outset) for outset in (0.5, 1)), key=max)
            assert halo[0] <= 190, ("brush ring halo darkens the fixture", halo)
            before = draft_bytes()
            brush_start = fixture_point((108, 189))
            brush_end = fixture_point((208, 229))
            run("xdotool", "mousemove", "--window", editor, *map(str, brush_start), "mousedown", "1",
                "sleep", ".2", "mousemove", "--sync", "--window", editor, *map(str, brush_end), "sleep", ".3")
            shot(editor, "brush-active")
            left, top, scale = fit_geometry(document_size(), window_size())
            def brush_pixel(name):
                x, y = round(left + 100 * scale), round(top + 100 * scale)
                return run("convert", str(output / f"{name}.png"), "-crop", f"1x1+{x}+{y}",
                           "-depth", "8", "rgb:-")
            assert brush_pixel("brush-active") != brush_pixel("brush-controls"), "erase must show live pixels before release"
            assert draft_bytes() == before, "brush preview must not persist pixels"
            run("xdotool", "key", "Escape", "mouseup", "1", "sleep", ".3")
            shot(editor, "brush-cancelled")
            assert brush_pixel("brush-cancelled") == brush_pixel("brush-controls"), "Escape restores published pixels"
            save_layers(lambda values: values[0]["src"] == source, "escape cancels brush")
            drag((338, 189), (438, 229))
            erased = save_layers(lambda values: values[0]["src"] != source, "erase completed stroke")[0]
            # The unedited capture has no draft, so its session asset first
            # appears as the edited layer's original.
            original_src = erased["originalSrc"]
            assert original_src.startswith("draft-asset:") and original_src != erased["src"]
            assert erased["locked"]
            asset_pixel(erased, 100, 100, (0, 0, 0, 0))
            asset_pixel(erased, 150, 120, (0, 0, 0, 0))
            asset_pixel(erased, 200, 140, (0, 0, 0, 0))
            edge = asset_pixel(erased, 86, 100)
            assert edge[:3] == bytes((229, 179, 68)) and 0 < edge[3] < 255, edge
            asset_pixel(erased, 2, 1, (40, 110, 166, 255))
            shot(editor, "brush-erased")
            assert brush_pixel("brush-active") == brush_pixel("brush-erased"), "live erase matches committed transparency"
            toolbar_click("undo")
            save_layers(lambda values: values[0]["src"] == source, "one-step brush undo")
            toolbar_click("redo")
            save_layers(lambda values: values[0]["src"] == erased["src"], "brush redo")
            draw_tool("restore")
            before = draft_bytes()
            start, end = fixture_point((108, 189)), fixture_point((208, 229))
            run("xdotool", "mousemove", "--window", editor, *map(str, start), "mousedown", "1",
                "sleep", ".2", "mousemove", "--sync", "--window", editor, *map(str, end), "sleep", ".3")
            shot(editor, "brush-restore-active")
            document_pixel("brush-restore-active", 100, 100, (229, 179, 68))
            assert draft_bytes() == before, "restore preview must not persist pixels"
            run("xdotool", "key", "Escape", "mouseup", "1", "sleep", ".3")
            shot(editor, "brush-restore-cancelled")
            assert brush_pixel("brush-restore-cancelled") == brush_pixel("brush-erased"), "cancelled restore retains erased pixels"
            drag((338, 189), (438, 229))
            restored = save_layers(lambda values: values[0]["src"] != erased["src"], "restore stroke")[0]
            assert restored["originalSrc"] == original_src
            asset_pixel(restored, 150, 120, (229, 179, 68, 255))
            shot(editor, "brush-restored")
            toolbar_click("undo")
            save_layers(lambda values: values[0]["src"] == erased["src"], "undo restore")
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "brush draft closes")
            editor = reopen()
            asset_pixel(layers()[0], 150, 120, (0, 0, 0, 0))
            resize_editor(942, 701)
            toolbar_click("draw")
            draw_tool("erase")
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, "brush-minimum-reopened")
            run("xdotool", "windowsize", "--sync", editor, "1000", "800")
            preview_encoded()
            export_click("copy")
            shot(editor, "brush-output-copied")
            png = output / "clipboard-brush.png"
            png.write_bytes(run("xclip", "-selection", "clipboard", "-t", "image/png", "-o"))
            assert run("convert", str(png), "-crop", "1x1+150+120", "-depth", "8", "rgba:-") == bytes((0, 0, 0, 0))
            assert (artifact / "capture.png").read_bytes() == original
            close(root)
            wait(lambda: app.poll() is not None, "brush suite quits")
            assert app.returncode == 0
            checks = ["brush-draw-tool-preview", "brush-hover-ring", "restore-missing-original-retry",
                      "brush-preview-no-write-and-cancel",
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
            toolbar_click("draw")
            draw_tool("wand")
            shot(editor, "wand-controls")
            # Shipping's colour loupe: hovering the image magnifies its natural
            # pixels beside the crosshair; the centre tile is the keyed sample.
            hover = fixture_point((108, 189))
            run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, hover), "sleep", ".3")
            loupe_center = (hover[0] + 18 + 42, hover[1] + 18 + 42)

            def loupe_shows(expected):
                def check():
                    shot(editor, "wand-loupe")
                    try:
                        pixel("wand-loupe", *loupe_center, expected, 3)
                        return True
                    except AssertionError:
                        return False
                return check

            wait(loupe_shows((229, 179, 68)), "Wand loupe magnifies the sampled colour")
            blue = fixture_point((58, 139))  # Document (50, 50): the #286ea6 capture.
            run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, blue), "sleep", ".3")
            loupe_center = (blue[0] + 18 + 42, blue[1] + 18 + 42)
            wait(loupe_shows((40, 110, 166)), "Wand loupe follows the pointer")
            # Pick authored document point (100,100). Original capture stays locked.
            fixture_click((108, 189))
            edited = save_layers(lambda values: values[0]["src"] != source, "wand edit")[0]
            original_src = edited["originalSrc"]
            assert original_src.startswith("draft-asset:") and original_src != edited["src"]
            assert edited["locked"]

            asset_pixel(edited, 100, 100, (0, 0, 0, 0))
            asset_pixel(edited, 310, 60, (229, 179, 68, 255))
            asset_pixel(edited, 2, 1, (40, 110, 166, 255))
            shot(editor, "wand-contiguous")
            fixture_click((108, 189))  # Transparent seed fails; must not create a new asset.
            shot(editor, "wand-no-match")
            save_layers(lambda values: values[0]["src"] == edited["src"], "no-match preserves pixels")
            toolbar_click("undo")
            save_layers(lambda values: values[0]["src"] == source, "wand undo")
            toolbar_click("redo")
            save_layers(lambda values: values[0]["src"] == edited["src"], "wand redo")
            toolbar_click("undo")
            save_layers(lambda values: values[0]["src"] == source, "undo before global removal")
            # Disable Contiguous below the mode row and the Tolerance slider.
            inspector_click(60, draw_row(398 + ERASER_ROWS))
            fixture_click((108, 189))
            global_edit = save_layers(lambda values: values[0]["src"] != source, "global wand")[0]
            asset_pixel(global_edit, 100, 100, (0, 0, 0, 0))
            asset_pixel(global_edit, 310, 60, (0, 0, 0, 0))
            assert global_edit["originalSrc"] == original_src
            shot(editor, "wand-global")
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "wand draft closes")
            editor = reopen()
            asset_pixel(layers()[0], 310, 60, (0, 0, 0, 0))
            resize_editor(942, 701)
            toolbar_click("draw")
            draw_tool("wand")
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            shot(editor, "wand-minimum-reopened")
            run("xdotool", "windowsize", "--sync", editor, "1000", "800")
            preview_encoded()  # Preview PNG before copying the edited frame.
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
            checks = ["wand-loupe-magnified-sample", "wand-locked-contiguous-exact-pixels",
                      "wand-no-match-preserves-state",
                      "wand-undo-redo", "wand-global-disconnected-pixels", "wand-retains-original",
                      "wand-minimum-draft-reopen", "wand-clipboard-alpha-original-unchanged"]
            (output / "result.json").write_text(json.dumps({
                "passed": True, "appearance": args.appearance, "checks": checks,
            }, indent=2) + "\n")
            print("PASS native Wand: locked image, contiguous/global, rollback, undo/redo, draft, clipboard alpha")
            return

        if args.trim_only:
            run("xdotool", "windowsize", "--sync", editor, "1000", "1000")

            def document_rgb(name, point):
                window, size, bar = shot_layouts[name]
                left, top, scale = fit_geometry(size, window, bar)
                x, y = round(left + point[0] * scale), round(top + point[1] * scale)
                return run("convert", str(output / f"{name}.png"), "-crop", f"1x1+{x}+{y}",
                           "-depth", "8", "rgb:-")

            def hover_trim():
                run("xdotool", "mousemove", "--sync", "--window", editor,
                    *map(str, canvas_toolbar_point("trim")), "sleep", ".3")

            def leave_trim():
                run("xdotool", "mousemove", "--sync", "--window", editor,
                    *map(str, document_point((320, 180))), "sleep", ".3")

            # A tight capture has nothing to trim: shipping disables the button,
            # so hovering previews nothing and clicking edits nothing.
            save(640, 360, 0, 0)
            leave_trim()
            shot(editor, "trim-tight")
            hover_trim()
            shot(editor, "trim-disabled-hover")
            for probe in ((630, 180), (320, 5), (5, 180)):
                assert document_rgb("trim-disabled-hover", probe) == document_rgb("trim-tight", probe), probe
            canvas_click("trim")
            save(640, 360, 0, 0)
            canvas_field("width", 720)
            save(720, 360, 0, 0)
            canvas_field("height", 420)
            save(720, 420, 0, 0)
            leave_trim()
            shot(editor, "trim-before")
            margin = (680, 390)
            before = document_rgb("trim-before", margin)
            kept = document_rgb("trim-before", (320, 180))

            def tinted():
                # The hint breathes, so poll until the red margin tint shows.
                shot(editor, "trim-hover")
                actual = document_rgb("trim-hover", margin)
                return (before[1] - actual[1] >= 12 and before[2] - actual[2] >= 10
                        and actual[0] + 8 >= before[0]
                        and document_rgb("trim-hover", (320, 180)) == kept)

            hover_trim()
            wait(tinted, "Trim edges hover tints the discarded margins red")
            leave_trim()
            wait(lambda: (shot(editor, "trim-left"), document_rgb("trim-left", margin) == before)[1],
                 "leaving Trim edges clears the preview")
            canvas_click("trim")
            save(640, 360, 0, 0)
            leave_trim()
            shot(editor, "trim-applied-hover-clear")
            hover_trim()
            shot(editor, "trim-applied-disabled")
            assert document_rgb("trim-applied-disabled", (630, 350)) == document_rgb("trim-applied-hover-clear", (630, 350))
            shot(editor, "trim-applied")
            toolbar_click("undo")
            save(720, 420, 0, 0)
            toolbar_click("redo")
            save(640, 360, 0, 0)
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "trim draft closes")
            editor = reopen()
            save(640, 360, 0, 0)
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            properties_move("click", "--repeat", "16", "5")
            shot(editor, "trim-minimum-reopened")
            run("xdotool", "windowsize", "--sync", editor, "1000", "800")
            preview_encoded()
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
                "checks": ["trim-disabled-when-tight", "trim-hover-margin-preview",
                           "trim-locked-capture-bounds", "trim-undo-redo", "trim-draft-reopen",
                           "trim-minimum-scroll", "trim-clipboard-dimensions-pixels-original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native trim: canvas bounds, undo/redo, draft, minimum, clipboard pixels")
            return

        if args.background_only:
            resize_editor(1000, 801)
            canvas_field("width", 720)
            save(720, 360, 0, 0)
            canvas_field("height", 420)
            save(720, 420, 0, 0)
            before = draft_bytes()
            background_open()
            shot(editor, "background-controls")
            assert draft_bytes() == before, "opening the card must not edit the draft"

            def background_is(color):
                return json.loads(draft.read_text())["document"]["background"] == color

            # A swatch applies live, as shipping does; no Apply button.
            background_click("#2d9cff")
            wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
                 "live background applied")
            shot(editor, "background-live")
            fixture_pixel("background-live", 700, 500, (45, 156, 255))
            run("xdotool", "key", "Escape", "sleep", ".2")
            save_until(lambda: background_is("#2d9cff"), "solid canvas background")
            shot(editor, "background-solid")
            fixture_pixel("background-solid", 700, 500, (45, 156, 255))
            fixture_pixel("background-solid", 40, 120, (40, 110, 166))
            # Re-choosing the same swatch adds no undo step.
            background_color("#2d9cff")
            background_open()
            background_click("solid")
            save_until(lambda: background_is(None), "transparent canvas background")
            shot(editor, "background-transparent")
            toolbar_click("undo")
            save_until(lambda: background_is("#2d9cff"), "undo canvas background")
            toolbar_click("undo")
            save_until(lambda: background_is("#f7f7f5"), "undo skips the unchanged swatch")
            toolbar_click("redo")
            save_until(lambda: background_is("#2d9cff"), "redo canvas background")
            # Solid on restores the last solid color; undo returns to transparent.
            toolbar_click("redo")
            save_until(lambda: background_is(None), "redo transparent background")
            background_open()
            background_click("solid")
            save_until(lambda: background_is("#2d9cff"), "solid restores the last color")
            toolbar_click("undo")
            save_until(lambda: background_is(None), "undo solid toggle")
            close(editor)
            wait(lambda: not windows("Captures Screenshot Editor"), "background draft closes")
            editor = reopen()
            assert background_is(None)
            run("xdotool", "windowsize", "--sync", editor, "760", "540")
            properties_move("click", "--repeat", "12", "5")
            shot(editor, "background-transparent-minimum-reopened")
            run("xdotool", "windowsize", "--sync", editor, "1000", "800")
            preview_encoded()  # Preview PNG, then copy the edited frame.
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
                "checks": ["background-open-no-write", "background-live-swatch-pixels",
                           "background-same-swatch-no-undo-step", "background-transparent-undo-redo",
                           "background-solid-restores-last-color",
                           "background-minimum-draft-reopen", "background-clipboard-alpha-original-unchanged"],
            }, indent=2) + "\n")
            print("PASS native canvas backgrounds: live swatches, transparency, undo/redo, draft, clipboard alpha")
            return

        # 896 (986 with the wider sidebar) keeps the header clear of its
        # font-measured label threshold.
        resize_editor(896, 700)
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
        # Shipping shows Recenter only once pan leaves the canvas mostly off
        # screen. Recenter keeps zoom; Fit restores the historical fixture geometry.
        for _ in range(2):
            run("xdotool", "mousemove", "--sync", "--window", editor, "90", "90",
                "mousedown", "2", "mousemove", "--sync", "--window", editor, "340", "380",
                "mousemove", "--sync", "--window", editor, "620", "660",
                "mouseup", "2", "sleep", ".3")
        shot(editor, f"viewport-offscreen-{args.appearance}")
        viewport_center = (64 + (editor_width() - INSPECTOR_WIDTH - 8 - 64) // 2, 376)
        pixel(f"viewport-offscreen-{args.appearance}", *viewport_center,
              (245, 245, 247) if args.appearance == "light" else (16, 16, 20))
        topbar_click("recenter")
        shot(editor, f"viewport-recenter-{args.appearance}")
        pixel(f"viewport-recenter-{args.appearance}", *viewport_center, (40, 110, 166))
        topbar_click("fit")
        shot(editor, f"viewport-fit-{args.appearance}")
        document_pixel(f"viewport-fit-{args.appearance}", 102, 111, (229, 179, 68))
        document_pixel(f"viewport-fit-{args.appearance}", 72, 111, (40, 110, 166))
        topbar_click("zoom")
        shot(editor, "viewport-presets-menu")
        click(editor, zoom_menu_x(), 152)  # 100% preset, without a custom row.
        shot(editor, "viewport-actual-button")
        topbar_click("plus")  # 1.25x
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
        canvas_click("width")  # Focus the canvas width field; shortcuts still zoom.
        run("xdotool", "key", "ctrl+minus", "sleep", ".3")
        shot(editor, "viewport-field-key")
        assert viewport_pixels("viewport-field-key") == viewport_pixels("viewport-actual-button")
        run("xdotool", "key", "Escape")
        topbar_click("zoom")
        click(editor, zoom_menu_x(), 108)  # 50% preset.
        shot(editor, "viewport-preset-50")
        # 640×360 at 50% is 320×180, centered in the 594×603 viewport.
        # The 56px rail moves the viewport center right by 28px.
        pixel("viewport-preset-50", 203, 310, (40, 110, 166))
        pixel("viewport-preset-50", 258, 340, (229, 179, 68))
        pixel("viewport-preset-50", 196, 310,
              (245, 245, 247) if args.appearance == "light" else (16, 16, 20))
        pixel("viewport-preset-50", 521, 310,
              (245, 245, 247) if args.appearance == "light" else (16, 16, 20))
        topbar_click("zoom")
        click(editor, zoom_menu_x(), 196)  # 200% preset.
        shot(editor, "viewport-preset-200")
        run("xdotool", "key", "ctrl+0", "ctrl+equal", "ctrl+equal", "sleep", ".3")
        shot(editor, "viewport-preset-custom")
        topbar_click("zoom")
        shot(editor, "viewport-presets-custom-menu")
        click(editor, zoom_menu_x(), 240)  # 200% now follows the custom percentage row.
        shot(editor, "viewport-preset-200-from-custom")
        assert viewport_pixels("viewport-preset-200") == viewport_pixels("viewport-preset-200-from-custom")
        assert viewport_pixels("viewport-preset-50") != viewport_pixels("viewport-preset-200")
        topbar_click("zoom")
        click(editor, zoom_menu_x(), 64)  # Fit removes the custom row and resets pan.
        shot(editor, "viewport-preset-fit")
        assert viewport_pixels("viewport-preset-fit") == viewport_pixels(f"viewport-fit-{args.appearance}")
        run("xdotool", "mousemove", "--sync", "--window", editor, "290", "250",
            "mousedown", "2", "mousemove", "--sync", "--window", editor, "355", "290",
            "mouseup", "2", "sleep", ".3")
        shot(editor, "viewport-fit-panned")
        assert viewport_pixels("viewport-fit-panned") != viewport_pixels("viewport-preset-fit")
        topbar_click("zoom")
        click(editor, zoom_menu_x(), 64)  # Reselecting Fit must reset pan even when already selected.
        shot(editor, "viewport-preset-fit-reselected")
        assert viewport_pixels("viewport-preset-fit-reselected") == viewport_pixels("viewport-preset-fit")
        assert not draft.exists(), "toolbar zoom must remain outside draft state"
        topbar_click("fit")  # Fit also cancels any viewport gesture and restores coordinates.
        if args.zoom_only:
            # With spare width AND height, Fit keeps the 640×360 source at 1×.
            # An uncapped fit would paint beyond both independently checked edges.
            resize_editor(1180, 900, "sleep", ".3")
            shot(editor, "viewport-fit-no-upscale")
            surface = (245, 245, 247) if args.appearance == "light" else (16, 16, 20)
            # Client 1180×900 minus the rail, inspector and central-panel margins
            # leaves x=64..942, y=60..892. Its center is (503,476), so the
            # 640×360 source spans x=183..823, y=296..656. Raster sample
            # centers at the bottom edge are excluded by the top-left fill rule.
            pixel("viewport-fit-no-upscale", 822, 400, (40, 110, 166))
            pixel("viewport-fit-no-upscale", 823, 400, surface)
            pixel("viewport-fit-no-upscale", 300, 655, (40, 110, 166))
            pixel("viewport-fit-no-upscale", 300, 656, surface)
            assert not draft.exists(), "Fit resizing must not create a draft"
            run("xdotool", "windowsize", "--sync", editor, "760", "540",
                "key", "ctrl+0", "ctrl+equal", "ctrl+equal", "sleep", ".3")
            shot(editor, "viewport-preset-custom-minimum")
            topbar_click("zoom")
            shot(editor, "viewport-presets-custom-menu-minimum")
            run("xdotool", "key", "Escape")
            topbar_click("fit")  # Fit resets the viewport-center anchor.
            topbar_click("slider-min")  # Left end of the logarithmic slider: 5%.
            shot(editor, "viewport-slider-minimum")
            # The header, export bar and 320px sidebar leave x=64..432,
            # y=60..452, center (248,256). The 5% source is 32×18, starting
            # at (232,247).
            pixel("viewport-slider-minimum", 233, 248, (40, 110, 166))
            pixel("viewport-slider-minimum", 231, 248, surface)
            pixel("viewport-slider-minimum", 264, 248, surface)
            pixel("viewport-slider-minimum", 233, 265, surface)
            topbar_click("slider-max")  # Right end: 800%, preserving the same anchor.
            shot(editor, "viewport-slider-maximum")
            pixel("viewport-slider-maximum", 66, 150, (40, 110, 166))
            pixel("viewport-slider-maximum", 430, 450, (40, 110, 166))
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
        toolbar_click("draw")
        draw_tool("pen")
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
        toolbar_click("undo")
        save_layers(lambda values: len(values) == 2, "freehand dot undo")
        before_cancel = draft_bytes()
        cancel_start = fixture_point((170, 400))
        cancel_end = fixture_point((230, 410))
        run("xdotool", "mousemove", "--window", editor, *map(str, cancel_start), "mousedown", "1",
            "sleep", ".2", "mousemove", "--sync", "--window", editor, *map(str, cancel_end),
            "sleep", ".2", "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
        assert draft_bytes() == before_cancel
        toolbar_click("redo")
        assert save_layers(lambda values: len(values) == 3, "cancel preserves freehand redo")[-1]["id"] == dot["id"]
        drag((320, 500), (420, 560))
        outside = save_layers(lambda values: len(values) == 4, "outside freehand stroke")[-1]
        outside_y = max(point["y"] for point in outside["points"])
        # The canvas can grow while X11 delivers this drag, changing the Fit transform
        # before the final pointer event. Check the authored geometry rather than the
        # nominal pre-growth coordinate: shipping adds 8px beyond its furthest sample.
        assert outside_y > 360 and saved(640, math.ceil(outside_y) + 8, 0, 0)
        shot(editor, "freehand-outside")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "freehand-minimum")
        close(editor)
        wait(lambda: not windows("Captures Screenshot Editor"), "freehand editor closes")
        editor = reopen()
        resize_editor(942, 701)
        shot(editor, "freehand-reopened")
        fixture_pixel("freehand-reopened", 141, 254, (255, 59, 92))
        assert layers()[1]["id"] == curve["id"] and layers()[1]["points"] == curve["points"]
        assert (artifact / "capture.png").read_bytes() == original
        editor = discard_draft("discard freehand edits")

        resize_editor(942, 701)
        toolbar_click("draw")
        shot(editor, "open-shape-tools")
        draw_tool("line")
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
        draw_tool("arrow")
        before_arrow = draft_bytes()
        arrow_start = fixture_point((520, 320))
        arrow_end = fixture_point((350, 190))
        run("xdotool", "mousemove", "--window", editor, *map(str, arrow_start), "mousedown", "1",
            "sleep", ".2", "mousemove", "--sync", "--window", editor, *map(str, arrow_end), "sleep", ".3")
        shot(editor, "open-shape-arrow-transient")
        fixture_pixel("open-shape-arrow-transient", 435, 255, (255, 59, 92))
        assert draft_bytes() == before_arrow
        run("xdotool", "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
        save_layers(lambda values: len(values) == 4, "arrow Escape cancellation")
        drag((750, 320), (580, 190))
        shot(editor, "open-shape-arrow-result")
        arrow = save_layers(lambda values: len(values) == 5, "reverse diagonal arrow")[-1]
        assert arrow["shape"] == "arrow" and arrow["controls"] == []
        assert all(abs(actual - expected) < 1e-12 for actual, expected in zip(
            (arrow["x"], arrow["y"], arrow["endX"], arrow["endY"]), (512, 231, 342, 101)))
        shot(editor, "open-shape-arrow")
        # Sample 40% along the shaft, document (444,179), not its 50% starter dot.
        fixture_pixel("open-shape-arrow", 452, 268, (255, 59, 92))
        fixture_pixel("open-shape-arrow", 310, 260, (255, 59, 92))
        drag((500, 400), (502, 400))  # Two screen/document pixels is below the 3px gesture threshold.
        save_layers(lambda values: len(values) == 5, "short arrow cancellation")
        toolbar_click("undo")
        save_layers(lambda values: len(values) == 4, "arrow single undo")
        toolbar_click("redo")
        assert save_layers(lambda values: len(values) == 5, "arrow redo")[-1]["id"] == arrow["id"]
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "open-shape-minimum")
        close(editor)
        wait(lambda: not windows("Captures Screenshot Editor"), "open-shape editor closes")
        editor = reopen()
        resize_editor(942, 701)
        shot(editor, "open-shape-reopened")
        fixture_pixel("open-shape-reopened", 435, 255, (255, 59, 92))
        assert layers()[-1]["id"] == arrow["id"]
        assert (artifact / "capture.png").read_bytes() == original
        editor = discard_draft("discard open-shape edits")

        # Leave room below the annotation form for rotation-snap controls, and
        # above it for the Layers section, so the bottom-scrolled form fits in
        # Properties; the narrow/minimum-size scroll path is exercised below.
        resize_editor(942, 1029)  # A 1110 px window.
        toolbar_click("draw")
        draw_tool("rectangle")
        drag((320, 250), (480, 370))
        annotation = save_layers(lambda values: len(values) == 2, "annotation fixture")[-1]
        toolbar_click("layers")
        layer_click(0, "lock")
        save_layers(lambda values: values[-1]["locked"], "locked annotation remains style editable")
        properties_move("click", "--repeat", "20", "5")
        shot(editor, "annotation-fields")

        # Shipping annotation Properties: Stroke, Stroke color, Stroke width,
        # Opacity, Drop shadow (its fields indented), Filled shape, Fill color.
        # Offsets are from properties_top(); `end` is where the content ends in
        # that state, so a Properties area scrolled to its end shifts every row
        # up by the overflow.
        def annotation_point(x, offset, end):
            visible = window_size()[1] - export_bar_height() - properties_top()
            return inspector_x(x), properties_top() + offset - max(0, end - visible)

        def annotation_click(x, offset, end):
            click(editor, *annotation_point(x, offset, end))

        def annotation_swatch(first_row, color, end, indent=0):
            index = SWATCHES.index(color) if color in SWATCHES else len(SWATCHES)
            columns, pitch = swatch_grid(INSPECTOR_CONTENT - indent)
            annotation_click(round(8 + indent + (index % columns + .5) * pitch),
                             first_row + index // columns * 32, end)

        def annotation_slider(offset, end, *keys):
            annotation_click(100, offset, end)
            for key in keys:
                run("xdotool", "key", key, "sleep", ".12")
            run("xdotool", "sleep", ".3")

        def annotation_field(offset, end, value, x):
            annotation_click(x, offset, end)
            run("xdotool", "key", "ctrl+a")
            type_text(value, 60)
            run("xdotool", "key", "Return", "sleep", ".2")

        def scroll_inspector_end():
            properties_move("click", "--repeat", "25", "5", "sleep", ".6")

        A = ANNOTATION
        # Shipping applies each style change as it is made: no Apply or Reset.
        unchanged = draft_bytes()
        annotation_swatch(A["fill-row"], "#36c96b", A["end"])
        save_layers(lambda values: values[-1]["style"]["fill"] == "#36c96b", "annotation fill applies live")
        assert draft_bytes() != unchanged
        shot(editor, "annotation-fill")
        fixture_pixel("annotation-fill", 170, 310, (54, 201, 107))
        toolbar_click("undo")
        save_layers(lambda values: values[-1]["style"]["fill"] == "#ff3b5c", "one-step style undo")
        toolbar_click("redo")
        save_layers(lambda values: values[-1]["style"]["fill"] == "#36c96b", "style redo")
        annotation_click(15, A["stroke"], A["end"])  # Enable stroke.
        scroll_inspector_end()
        shot(editor, "annotation-stroke-fields")
        stroked = A["end"] + A["stroke-rows"]
        annotation_swatch(A["stroke-row"], "#8b5cf6", stroked)
        annotation_slider(A["stroke-width"], stroked, "Home", "Prior")  # 2 + 10 = 12 px.
        annotation_click(15, A["filled"] + A["stroke-rows"], stroked)  # Clear fill.
        save_layers(lambda values: values[-1]["style"]["fill"] is None and values[-1]["style"]["strokeWidth"] == 12, "annotation outline")
        scroll_inspector_end()
        shot(editor, "annotation-outline")
        fixture_pixel("annotation-outline", 170, 310, (40, 110, 166))
        fixture_pixel("annotation-outline", 93, 310, (139, 92, 246))
        outline = A["filled"] + A["stroke-rows"] + A["check-end"]
        # Restore fill; choose a color other than the stroke.
        annotation_click(15, A["filled"] + A["stroke-rows"], outline)
        scroll_inspector_end()
        annotation_swatch(A["fill-row"] + A["stroke-rows"], "#36c96b", stroked)
        annotation_click(15, A["shadow"] + A["stroke-rows"], stroked)  # Enable custom shadow controls.
        scroll_inspector_end()
        shot(editor, "annotation-shadow-fields")
        shadowed = stroked + A["shadow-rows"]
        shadow = A["shadow"] + A["stroke-rows"]
        annotation_swatch(shadow + 70, "#ff8a22", shadowed, indent=SHADOW_INDENT)
        # Two swatch rows put Opacity, Blur and the offsets 32px higher.
        annotation_slider(shadow + 177, shadowed, "End", "Next", "Next")  # 80%.
        annotation_slider(shadow + 246, shadowed, "Home")  # 0 px blur.
        # The pinned-heading layout puts the text centres 12px above the old
        # click points, which hit the lower padding instead of focusing input.
        annotation_field(shadow + 294, shadowed, "25", 100)
        annotation_field(shadow + 294, shadowed, "-12", 240)
        styled = save_layers(lambda values: values[-1]["style"].get("dropShadowStyle", {}) == {"color": "#ff8a22", "opacity": 80, "blur": 0, "offsetX": 25, "offsetY": -12}, "custom annotation shadow")[-1]
        assert styled["id"] == annotation["id"] and styled["locked"]
        assert styled["style"]["fill"] == "#36c96b" and styled["style"]["strokeWidth"] == 12
        shot(editor, "annotation-shadow")
        fixture_pixel("annotation-shadow", 279, 310, (212, 132, 60), tolerance=1)
        # The custom tile opens the picker inline, and closes it without an edit.
        annotation_swatch(shadow + 70, "custom", shadowed, indent=SHADOW_INDENT)
        shot(editor, "annotation-color-picker")
        annotation_swatch(shadow + 70, "custom", shadowed, indent=SHADOW_INDENT)
        annotation_click(15, shadow, shadowed)  # Disable shadow without losing custom knobs.
        disabled = save_layers(lambda values: values[-1]["style"]["dropShadow"] is False, "shadow off")[-1]
        assert disabled["style"]["dropShadowStyle"] == styled["style"]["dropShadowStyle"]
        scroll_inspector_end()
        shot(editor, "annotation-shadow-off")
        fixture_pixel("annotation-shadow-off", 279, 310, (40, 110, 166))
        annotation_click(15, shadow, stroked)
        save_layers(lambda values: values[-1]["style"] == styled["style"], "shadow settings restored")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        properties_move("click", "--repeat", "25", "5")
        shot(editor, "annotation-minimum")
        close(editor)
        wait(lambda: not windows("Captures Screenshot Editor"), "styled editor closes")
        editor = reopen()
        resize_editor(942, 701)
        shot(editor, "annotation-reopened")
        fixture_pixel("annotation-reopened", 279, 310, (212, 132, 60), tolerance=1)
        assert layers()[-1]["style"] == styled["style"]
        assert (artifact / "capture.png").read_bytes() == original
        editor = discard_draft("discard annotation edits")
        resize_editor(1000, 701)

        # These synthetic hex colors are sRGB. Keep the fixture untagged rather
        # than ImageMagick's gamma/chromaticity-only PNG; profiles have unit coverage.
        run("convert", "-size", "120x80", "xc:#d53e55", "-fill", "#3cb371",
            "-draw", "rectangle 10,9 39,29", "-fill", "#2d64bd",
            "-draw", "rectangle 88,51 119,79", "-strip", "PNG32:" + str(imported_path))
        imported_bytes = imported_path.read_bytes()
        # Shipping's `<input type="file" multiple>`: every chosen image becomes a
        # layer through the canvas-drop queue; unsupported files are skipped.
        batch = [output / "batch-a.png", output / "batch-notes.txt", output / "batch-b.png"]
        run("convert", "-size", "40x30", "xc:#c0392b", "-strip", "PNG32:" + str(batch[0]))
        batch[1].write_text("not an image")
        run("convert", "-size", "50x20", "xc:#8e44ad", "-strip", "PNG32:" + str(batch[2]))
        chooser.selected = batch
        toolbar_click("import")
        wait(lambda: chooser.pending, "multi-select image picker opened")
        title, options = chooser.calls[-1]
        assert title == "Import images" and not options.get("directory", False)
        assert options.get("multiple", False), "Add images allows several files"
        GLib.idle_add(chooser.respond, False)
        batch_layers = save_layers(lambda values: len(values) == 3, "multi-select imported two layers")
        assert [layer["name"] for layer in batch_layers[1:]] == ["batch-a.png", "batch-b.png"]
        assert all(layer["source"] == "imported" for layer in batch_layers[1:])
        # Each later file stacks below the previous import.
        assert batch_layers[2]["y"] >= batch_layers[1]["y"] + batch_layers[1]["height"]
        shot(editor, "import-multi-select")
        # Drafts autosave: discard through the restored-draft notice (#852).
        editor = discard_draft("discard multi-select import")
        chooser.selected = imported_path
        toolbar_click("import")
        wait(lambda: chooser.pending, "image file picker opened")
        title, options = chooser.calls[-1]
        assert title == "Import images" and options.get("multiple", False)
        shot(editor, "import-picker-pending")
        save(640, 360, 0, 0)  # A waiting picker must not occupy the session worker.
        before_import = draft_bytes()
        GLib.idle_add(chooser.respond, True)
        time.sleep(.4)
        assert draft_bytes() == before_import

        invalid = output / "broken.png"
        invalid.write_text("not an image")
        chooser.selected = invalid
        toolbar_click("import")
        wait(lambda: chooser.pending, "picker after cancellation")
        GLib.idle_add(chooser.respond, False)
        time.sleep(.8)
        shot(editor, "import-decode-error")
        assert draft_bytes() == before_import
        chooser.selected = imported_path
        toolbar_click("import")
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
        toolbar_click("layers")
        properties_move("click", "--repeat", "25", "4")
        shot(editor, "imported-selected-layer")
        # Double-click renames inline; Escape keeps the full, intact name.
        double_click_layer(0)
        shot(editor, "imported-rename-field")
        run("xdotool", "key", "ctrl+a", "ctrl+c", "sleep", ".2")
        assert run("xclip", "-selection", "clipboard", "-o").decode() == imported_path.name
        run("xdotool", "key", "Escape")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "imported-minimum")
        properties_move("click", "--repeat", "8", "5")
        shot(editor, "imported-minimum-scrolled")
        resize_editor(1000, 701)
        properties_move("click", "--repeat", "12", "4")
        assert imported_path.read_bytes() == imported_bytes
        imported_path.unlink()  # A saved import must no longer depend on its source file.
        toolbar_click("undo")
        save(640, 360, 0, 0)
        assert len(layers()) == 1
        toolbar_click("redo")
        save(640, 440, 0, 0)
        assert layers()[-1]["id"] == imported_id

        layer_click(0)  # Redo retained the original's selection; pick the imported row.
        resize_before = draft_bytes()
        resize_start = fixture_point((387, 489))
        resize_end = fixture_point((423, 489))
        run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, resize_start),
            "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
            *map(str, resize_end), "sleep", ".3")
        shot(editor, "layer-resize-active-guides")
        assert draft_bytes() == resize_before, "resize preview must remain transient"
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
        toolbar_click("undo")
        save_layers(lambda values: values[-1]["width"] == 120 and values[-1]["height"] == 80,
                    "undo imported image resize")
        shot(editor, "layer-resize-undone")
        fixture_pixel("layer-resize-undone", 293, 468, (60, 179, 113))
        toolbar_click("redo")
        save_layers(lambda values: math.isclose(values[-1]["width"], resized_width, abs_tol=1e-5),
                    "redo imported image resize")
        close(editor)
        wait(lambda: not windows("Captures Screenshot Editor"), "resized imported draft closes")
        editor = reopen()
        shot(editor, "layer-resize-reopened")
        reopened_resize = layers()[-1]
        assert reopened_resize["id"] == imported_id
        assert math.isclose(reopened_resize["width"], resized_width, abs_tol=1e-5)
        document_pixel("layer-resize-reopened", *resized_green, (60, 179, 113))
        # Undo history is session-local. Restore through a fresh east-grip resize
        # at 1:1 scale, where the desired edge lands on an exact pointer pixel.
        toolbar_click("layers")  # Reopened editors start in Geometry, not Layers.
        layer_click(0)
        resize_editor(942, 701, "sleep", ".3")
        drag((round(238 + 260 + resized_width), 489), (618, 489))
        save_layers(lambda values: values[-1]["width"] == 120 and values[-1]["height"] == 80,
                    "restore imported size after draft reopen")
        resize_editor(1000, 701, "sleep", ".3")

        # At 1× the 120x80 image's grip is (558,425), around pivot (558,489).
        # Start five points above the grip, within its hit radius.
        # Exercise a free-angle transient first; Escape must leave draft/pixels intact.
        rotation_before = draft_bytes()
        free_rotation_start = fixture_point((328, 420))
        free_rotation_end = fixture_point((363, 400))
        run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, free_rotation_start),
            "mousedown", "1", "sleep", ".2", "mousemove", "--sync", "--window", editor,
            *map(str, free_rotation_end), "sleep", ".3")
        shot(editor, "layer-rotation-free-transient")
        run("xdotool", "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
        assert draft_bytes() == rotation_before
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
        toolbar_click("undo")
        save_layers(lambda values: "rotation" not in values[-1], "undo imported image rotation")
        shot(editor, "layer-rotation-undone")
        fixture_pixel("layer-rotation-undone", 293, 468, (60, 179, 113))
        toolbar_click("redo")
        save_layers(lambda values: math.isclose(values[-1].get("rotation", 0), math.pi / 6,
                                                abs_tol=1e-12),
                    "redo imported image rotation")
        close(editor)
        wait(lambda: not windows("Captures Screenshot Editor"), "imported draft closes")
        editor = reopen()
        shot(editor, "layer-rotation-reopened")
        document_pixel("layer-rotation-reopened", *expected_document, (60, 179, 113))
        assert layers()[-1]["id"] == imported_id
        assert math.isclose(layers()[-1]["rotation"], math.pi / 6, abs_tol=1e-12)
        assert saved(640, 440, 0, 0)
        # Discard returns to the original capture, without deleting exports or source data.
        editor = discard_draft("discard imported draft")
        chooser.selected = artifact / "capture.png"
        toolbar_click("import")
        wait(lambda: chooser.pending, "picker before close")
        close(editor)
        wait(lambda: not windows("Captures Screenshot Editor"), "picker does not prevent closing")
        editor = reopen()
        GLib.idle_add(chooser.respond, False)  # Late result belongs to the old editor only.
        time.sleep(.5)
        save(640, 360, 0, 0)
        assert len(layers()) == 1
        assert (artifact / "capture.png").read_bytes() == original

        # Keep Image transform above the pinned footer while exercising its menu.
        resize_inspector_fixture(1000, 781)
        toolbar_click("layers")  # Layers, preserving the Geometry panel's scroll position.
        shot(editor, "layers-original-locked")
        layer_click(0, "more")
        shot(editor, "layers-transform-menu")
        run("xdotool", "key", "Escape")

        def transform(index, orientation, width, height):
            layer_menu(0, ["rotate-left", "rotate-right", "flip-horizontal", "flip-vertical"][index])
            save_layers(lambda values: values[0].get("orientation") == orientation,
                        f"transform {orientation}")
            assert saved(width, height, 0, 0), "fresh photo rotates its canvas"
            assert layers()[0]["locked"], "transform must not unlock the original"

        def assert_transformed_pixels(name, gold_left, gold_above):
            shot(editor, name)
            window, size, bar = shot_layouts[name]
            left, top, scale = fit_geometry(size, window, bar)
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
        resize_inspector_fixture(1000, 781)
        toolbar_click("layers")
        assert_transformed_pixels("layers-transform-reopened", False, True)
        transform(2, "transpose", 360, 640)
        assert_transformed_pixels("layers-flip-horizontal", True, True)
        toolbar_click("undo")
        save_layers(lambda values: values[0].get("orientation") == "rotate-90", "undo flip")
        # Reopening starts a new undo history, so rotate left explicitly restores the photo.
        transform(0, None, 640, 360)
        transform(0, "rotate-270", 360, 640)
        assert_transformed_pixels("layers-rotate-left", True, False)
        transform(3, "transpose", 360, 640)
        assert_transformed_pixels("layers-flip-vertical", True, True)
        toolbar_click("undo")
        save_layers(lambda values: values[0].get("orientation") == "rotate-270", "undo vertical flip")
        toolbar_click("undo")
        save_layers(lambda values: values[0].get("orientation") is None, "undo left rotation")
        assert saved(640, 360, 0, 0)
        run("xdotool", "key", "Escape", "sleep", ".2")
        layer_click(0, "eye")  # A hidden, locked image remains transformable.
        save_layers(lambda values: not values[0]["visible"], "hide original before transform")
        transform(2, "flip-horizontal", 640, 360)
        assert not layers()[0]["visible"]
        toolbar_click("undo")
        save_layers(lambda values: values[0].get("orientation") is None, "undo hidden transform")
        toolbar_click("undo")
        save_layers(lambda values: values[0]["visible"], "restore original visibility")
        assert_transformed_pixels("layers-transform-restored", True, True)
        layer_menu(0, "duplicate")  # Duplicate the locked original, not delete or move it.
        first = save_layers(lambda values: len(values) == 2, "duplicate original")
        copy_id = first[1]["id"]
        assert first[0]["locked"] and first[1]["visible"] and not first[1]["locked"]
        assert (first[1]["x"], first[1]["y"]) == (24, 24)
        assert first[0]["src"] == first[1]["src"]
        long_name = "Layer with a deliberately long name to retain"
        rename_layer(0, long_name)
        save_layers(lambda values: values[-1]["name"] == long_name, "renamed image")
        image_field("x", 190)
        image_field("y", 70)
        save_layers(lambda values: (values[-1]["x"], values[-1]["y"]) == (190, 70), "moved duplicate")
        shot(editor, "layers-moved")
        fixture_pixel("layers-moved", 361, 277, (229, 179, 68))
        # Canvas picking is based on the rendered document, not the layer-list selection.
        # Escape cancels the translated outline without touching the saved draft.
        canvas_before = draft_bytes()
        move_start = fixture_point((361, 277))
        move_preview = fixture_point((401, 307))
        run("xdotool", "mousemove", "--window", editor, *map(str, move_start), "mousedown", "1",
            "sleep", ".2", "mousemove", "--sync", "--window", editor, *map(str, move_preview), "sleep", ".2")
        shot(editor, "layers-canvas-active-outline")
        run("xdotool", "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
        assert draft_bytes() == canvas_before
        fixture_click((30, 200))  # Empty point before the unlocked copy clears selection.
        assert draft_bytes() == canvas_before
        # The raw horizontal delta lands just inside the canvas edge's magnetic
        # range. The shared move geometry snaps the duplicate's left edge to the
        # canvas/background layer edge while retaining the asymmetric raw Y move.
        snap_target = fixture_point((177, 307))
        run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, move_start),
            "sleep", ".2", "mousedown", "1", "sleep", ".2", "mousemove", "--sync",
            "--window", editor, *map(str, snap_target), "sleep", ".3")
        shot(editor, "layers-canvas-snapped-guides")
        assert draft_bytes() == canvas_before
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
        toolbar_click("undo")
        save_layers(lambda values: (values[-1]["x"], values[-1]["y"]) == (190, 70),
                    "undo snapped canvas move")
        toolbar_click("redo")
        save_layers(
            lambda values: all(math.isclose(values[-1][axis], expected, abs_tol=1e-5)
                               for axis, expected in zip(("x", "y"), expected_position)),
            "redo snapped canvas move")
        close(editor)
        wait(lambda: not windows("Captures Screenshot Editor"), "snapped layer draft closes")
        editor = reopen()
        reopened_move = layers()[-1]
        assert reopened_move["id"] == copy_id
        assert all(math.isclose(reopened_move[axis], expected, abs_tol=1e-5)
                   for axis, expected in zip(("x", "y"), expected_position))
        shot(editor, "layers-snapped-move-reopened")
        fixture_pixel("layers-snapped-move-reopened", 137, 307, (229, 179, 68))
        # Reopen starts in Geometry and undo history is intentionally not persisted.
        # Switch to Layers and restore explicitly so downstream fixtures stay stable.
        toolbar_click("layers")
        layer_click(0)
        image_field("x", 190)
        image_field("y", 70)
        save_layers(lambda values: (values[-1]["x"], values[-1]["y"]) == (190, 70),
                    "restore snapped move after reopen")
        layer_menu(0, "opacity")
        shot(editor, "layers-opacity-menu")
        run("xdotool", "key", "Escape", "sleep", ".2")
        save_layers(lambda values: values[-1]["opacity"] == 50, "half opacity")
        shot(editor, "layers-half-opacity")
        fixture_pixel("layers-half-opacity", 361, 277, (134, 144, 117), tolerance=1)
        layer_click(0, "eye")  # Hide the copy; the original blue pixel is restored.
        save_layers(lambda values: not values[-1]["visible"], "hidden duplicate")
        shot(editor, "layers-hidden")
        fixture_pixel("layers-hidden", 361, 277, (40, 110, 166))
        toolbar_click("undo")  # Undo must restore the rendered half-opacity layer.
        save_layers(lambda values: values[-1]["visible"], "undo visibility")
        shot(editor, "layers-undo-visible")
        fixture_pixel("layers-undo-visible", 361, 277, (134, 144, 117), tolerance=1)
        layer_click(0, "lock")
        save_layers(lambda values: values[-1]["locked"], "lock duplicate")
        layer_menu(0, "delete")  # Delete is disabled while locked.
        shot(editor, "layers-locked-menu")
        run("xdotool", "key", "Escape", "sleep", ".2")
        save_layers(lambda values: len(values) == 2 and values[-1]["locked"], "locked layer retained")
        shot(editor, "layers-locked")
        layer_click(0, "lock")
        save_layers(lambda values: not values[-1]["locked"], "unlock duplicate")
        layer_menu(0, "duplicate")
        third = save_layers(lambda values: len(values) == 3, "second duplicate")[-1]["id"]
        assert (layers()[-1]["x"], layers()[-1]["y"]) == (214, 94)
        drag_layer(0, 1, below=True)  # The third layer moves behind the first copy.
        save_layers(lambda values: [value["id"] for value in values] == ["capture-background", third, copy_id], "reordered down")
        shot(editor, "layers-reordered")
        layer_menu(1, "bring-front")
        save_layers(lambda values: [value["id"] for value in values] == ["capture-background", copy_id, third], "reordered up")
        layer_menu(0, "delete")
        save_layers(lambda values: len(values) == 2 and values[-1]["id"] == copy_id, "deleted selected copy")
        toolbar_click("undo")
        save_layers(lambda values: len(values) == 3, "undo deletion")
        toolbar_click("redo")
        save_layers(lambda values: len(values) == 2, "redo deletion")
        close(editor)
        wait(lambda: not windows("Captures Screenshot Editor"), "saved layers close")
        editor = reopen()
        toolbar_click("layers")
        shot(editor, "layers-reopened")
        fixture_pixel("layers-reopened", 361, 277, (134, 144, 117), tolerance=1)
        assert layers()[-1]["name"] == long_name
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        properties_move("click", "--repeat", "8", "5")
        shot(editor, "layers-small-scrolled")
        resize_editor(1000, 701)
        properties_move("click", "--repeat", "12", "4")
        layer_click(1)  # Select and explicitly unlock the original.
        layer_click(1, "lock")
        save_layers(lambda values: not values[0]["locked"], "unlock original")
        layer_menu(1, "delete")
        save_layers(lambda values: [value["id"] for value in values] == [copy_id], "delete original layer")
        layer_menu(0, "delete")
        save_layers(lambda values: len(values) == 0, "empty saved document")
        shot(editor, "layers-empty")
        toolbar_click("undo")
        save_layers(lambda values: [value["id"] for value in values] == [copy_id], "undo empty document")
        editor = discard_draft("discard layer edits")
        toolbar_click("geometry")  # Geometry has an independent scroll position.

        # Odd height keeps the centered 1:1 document origin pixel-aligned.
        resize_editor(942, 701)

        toolbar_click("draw")  # Draw keeps the chosen shape active after each release.
        drag((658, 289), (538, 169))
        rectangle = save_layers(lambda values: len(values) == 2, "reverse rectangle")[-1]
        assert rectangle["kind"] == "shape" and rectangle["shape"] == "rectangle"
        assert (rectangle["x"], rectangle["y"], rectangle["endX"], rectangle["endY"]) == (420, 200, 300, 80)
        assert rectangle["style"]["fill"] == "#ff3b5c" and not rectangle["style"]["strokeEnabled"]
        shot(editor, "shape-rectangle")
        fixture_pixel("shape-rectangle", 370, 230, (255, 59, 92))
        toolbar_click("undo")
        save_layers(lambda values: len(values) == 1, "single-step shape undo")
        shot(editor, "shape-undone")
        fixture_pixel("shape-undone", 370, 230, (40, 110, 166))
        toolbar_click("redo")
        save_layers(lambda values: len(values) == 2 and values[-1]["id"] == rectangle["id"], "shape redo keeps id")
        draw_tool("ellipse")
        drag((608, 349), (778, 399))
        ellipse = save_layers(lambda values: len(values) == 3, "ellipse layer")[-1]
        assert ellipse["shape"] == "ellipse" and ellipse["id"] != rectangle["id"]
        assert (ellipse["x"], ellipse["y"], ellipse["endX"], ellipse["endY"]) == (370, 260, 540, 310)
        shot(editor, "shape-ellipse")
        fixture_pixel("shape-ellipse", 463, 374, (255, 59, 92))
        fixture_pixel("shape-ellipse", 380, 350, (40, 110, 166))
        before_draw = draft_bytes()
        shape_start = fixture_point((90, 300))
        shape_end = fixture_point((190, 380))
        run("xdotool", "mousemove", "--sync", "--window", editor, *map(str, shape_start), "mousedown", "1",
            "sleep", ".2", "mousemove", "--sync", "--window", editor, *map(str, shape_end), "sleep", ".3")
        shot(editor, "shape-transient")
        assert draft_bytes() == before_draw
        run("xdotool", "key", "Escape", "sleep", ".2", "mouseup", "1", "sleep", ".2")
        save_layers(lambda values: len(values) == 3, "escape cancels shape")
        drag((320, 300), (320, 380))  # Degenerate zero-width gesture.
        save_layers(lambda values: len(values) == 3, "degenerate shape has no layer")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "shape-minimum")
        close(editor)
        wait(lambda: not windows("Captures Screenshot Editor"), "saved shapes close")
        editor = reopen()
        resize_editor(942, 701)
        shot(editor, "shape-reopened")
        fixture_pixel("shape-reopened", 370, 230, (255, 59, 92))
        fixture_pixel("shape-reopened", 463, 374, (255, 59, 92))
        assert layers()[-1]["id"] == ellipse["id"]
        toolbar_click("draw")
        drag((298, 500), (358, 570))  # Fully outside the image grows the canvas.
        outside = save_layers(lambda values: len(values) == 4, "outside shape retained")[-1]
        assert outside["shape"] == "rectangle"
        assert (outside["x"], outside["y"], outside["endX"]) == (60, 411, 120)
        # The release point, not one remapped through the grown live preview.
        assert abs(outside["endY"] - 481) < 1e-6
        stroke_extent = math.ceil(outside["style"]["strokeWidth"] / 2) + 1
        expected_height = math.ceil(max(outside["y"], outside["endY"]) + stroke_extent)
        assert saved(640, expected_height, 0, 0)
        shot(editor, "shape-outside-expanded")
        assert (artifact / "capture.png").read_bytes() == original
        editor = discard_draft("discard shape edits")
        toolbar_click("geometry")  # The Crop tool starts a crop selection.

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
        before_selection = draft_bytes()
        # Shipping's `cta-pulse` on Apply crop: an accent halo swells just
        # outside the button only while a crop is staged.
        halo = (inspector_x(CROP["apply"][0]), properties_top() + CROP["apply"][1] - 16 - 3)

        def halo_rgb(name):
            return run("convert", str(output / f"{name}.png"), "-crop",
                       f"1x1+{halo[0]}+{halo[1]}", "-depth", "8", "rgb:-")

        resting = halo_rgb("crop-cancelled")
        # Like shipping, the Crop tool stays ready for a new selection after
        # Escape, Clear or Apply crop; there is no Draw crop button.
        drag((278, 119), (438, 219), shift=True)
        shot(editor, "crop-shift-square")
        assert draft_bytes() == before_selection

        def pulsing():
            shot(editor, "crop-apply-pulse")
            return max(abs(a - b) for a, b in zip(halo_rgb("crop-apply-pulse"), resting)) >= 12

        wait(pulsing, "Apply crop pulses while a crop is staged")
        prop_click(*CROP["apply"])
        save(160, 160, -40, -30)
        toolbar_click("undo")
        save(640, 360, 0, 0)
        drag((638, 500), (278, 119))  # Starts below the image; clamps to y=360.
        shot(editor, "crop-outside-start")
        prop_click(*CROP["apply"])
        save(360, 330, -40, -30)
        toolbar_click("undo")
        save(640, 360, 0, 0)
        prop_click(95, CROP["aspect"])
        shot(editor, "crop-aspect-menu")
        run("xdotool", "key", "Escape", "sleep", ".2")
        prop_click(95, CROP["aspect"])
        # The token Aspect ratio listbox; 4:3 takes precedence over Shift's square.
        prop_click(60, CROP_ASPECT_ROWS["4:3"])
        drag((278, 119), (438, 219), shift=True)
        shot(editor, "crop-preset-four-three")
        prop_click(*CROP["apply"])
        save(160, 120, -40, -30)
        toolbar_click("undo")
        save(640, 360, 0, 0)
        prop_click(95, CROP["aspect"])
        prop_click(60, CROP_ASPECT_ROWS["free"])  # Free for the following asymmetric crop.
        drag((638, 359), (278, 119))
        prop_click(*CROP["clear"])  # Clear restores the full canvas as well as pixels.
        prop_click(*CROP["apply"])  # Apply is gone with the selection: nothing crops.
        save(640, 360, 0, 0)
        drag((638, 359), (278, 119))
        prop_click(*CROP["apply"])
        resize_editor(1000, 701)
        save(360, 240, -40, -30)
        shot(editor, "editor-cropped")
        fixture_pixel("editor-cropped", 308, 299, (40, 110, 166))
        fixture_pixel("editor-cropped", 120, 200, (229, 179, 68))
        toolbar_click("undo")  # Undo
        save(640, 360, 0, 0)
        toolbar_click("redo")  # Redo
        save(360, 240, -40, -30)
        canvas_field("width", 480)  # Each committed dimension is one canvas resize.
        save(480, 240, -40, -30)
        canvas_field("height", 300)
        save(480, 300, -40, -30)
        shot(editor, "editor-resized")
        fixture_pixel("editor-resized", 428, 289, (46, 158, 113))

        saved_draft = draft_bytes()
        resize_editor(1000, 901)
        export_settings(True)
        # Compress shows the before/after comparison on its own: the edited
        # frame on the left, its encoded file on the right of the divider.
        quality_mode(1)  # PNG Compress.
        compare_settled("output-png")
        assert divider_shown("output-png")
        fixture_pixel("output-png", 120, 200, (229, 179, 68))
        fixture_pixel("output-png", 428, 289, (46, 158, 113))
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        compare_settled("output-compare-minimum")
        resize_editor(1000, 901)
        export_format("JPEG")  # JPEG re-encodes the After side; Compress stays.
        compare_settled("output-jpeg")
        fixture_pixel("output-jpeg", 428, 289, (46, 158, 113), tolerance=4)
        click(editor, *document_point((448, 23)))  # Hide: the edited canvas alone.
        export_bar["dismissed"] = True
        compare_settled("output-edited-canvas")
        assert not divider_shown("output-edited-canvas")
        fixture_pixel("output-edited-canvas", 428, 289, (46, 158, 113))
        setting_click(770)  # Show before / after.
        export_format("WebP")  # WebP, still Compress.
        compare_settled("output-webp")
        assert divider_shown("output-webp")
        fixture_pixel("output-webp", 428, 289, (46, 158, 113), tolerance=4)
        quality_mode(2)  # Maximum file size: 10 MB until edited.
        setting_field(418, 0)
        shot(editor, "output-budget-error")
        export_click("save")  # The status explains the limit; Save stays disabled.
        time.sleep(.5)
        assert app.poll() is None and windows("Captures Screenshot Editor")
        assert not (output / "exports").exists(), "an invalid limit never publishes"
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "output-budget-error-minimum")
        resize_editor(1000, 901)
        quality_mode(0)  # Preserve clears the failed budget and the comparison.
        compare_settled("output-retry")
        assert not divider_shown("output-retry")
        fixture_pixel("output-retry", 428, 289, (46, 158, 113))
        assert draft_bytes() == saved_draft, "preview must not write a draft"
        assert not (output / "exports").exists(), "preview must not publish files"

        exported = output / "exports" / "edited.webp"
        initial_directory = output / "unchosen folder"
        initial_directory.mkdir()
        exported.parent.mkdir()
        export_filename("edited")
        chooser.calls.clear()
        chooser.selected = initial_directory
        export_click("change")
        wait(lambda: len(chooser.calls) == 1, "folder dialog selection")
        GLib.idle_add(chooser.respond, False)
        time.sleep(.5)
        chooser.selected = exported.parent
        export_click("change")
        wait(lambda: len(chooser.calls) == 2, "folder dialog cancellation")
        shot(editor, "export-folder-pending")
        export_click("save")  # Save is disabled until the folder choice completes.
        assert not list(initial_directory.iterdir()) and not list(exported.parent.iterdir())
        GLib.idle_add(chooser.respond, True)
        time.sleep(.5)
        export_click("change")
        wait(lambda: len(chooser.calls) == 3, "folder dialog selection")
        GLib.idle_add(chooser.respond, False)
        time.sleep(.5)
        for title, options in chooser.calls:
            assert title == "Choose save location" and options["directory"] and not options.get("multiple", False)
        for _, options in chooser.calls[1:]:
            assert bytes(options["current_folder"]).rstrip(b"\0") == os.fsencode(initial_directory)
        assert not list(initial_directory.iterdir()) and not list(exported.parent.iterdir())
        assert draft_bytes() == saved_draft and len(list(history.glob("*/metadata.json"))) == 1
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
        assert draft_bytes() == saved_draft, "export must not save the draft"
        shot(root, "export-history-refreshed")
        shot(editor, "export-saved")
        # The saved file becomes the file Save overwrites, in place and in History.
        exported_inode = exported.stat().st_ino
        export_click("save")
        wait(lambda: exported.stat().st_ino != exported_inode, "adopted file atomically replaced")
        assert json.loads(metadata[0].read_text())["created_at"] == entry["created_at"]
        assert exported.read_bytes() == exported_bytes
        assert len(list(history.glob("*/metadata.json"))) == 2
        export_click("new-file")
        export_filename("edited")
        export_click("save")  # Save as new file with the same name must fail rather than replace.
        shot(editor, "export-collision")
        assert exported.read_bytes() == exported_bytes
        assert len(list(history.glob("*/metadata.json"))) == 2

        # Retry after a real History failure must report the successfully saved file.
        history.rename(output / "previous-history")
        history.write_text("blocks History creation")
        recovered = output / "exports" / "recovered.webp"
        export_filename("recovered")  # Editing the filename clears the collision error.
        export_click("save")
        wait(recovered.exists, "file saved despite unavailable History")
        assert recovered.read_bytes() == exported_bytes
        shot(editor, "export-history-warning")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "export-history-warning-minimum")
        run("xdotool", "mousemove", "--window", editor, "560", str(window_size()[1] - 54), "sleep", "1.5")
        shot(editor, "export-history-warning-detail-minimum")
        resize_editor(1000, 901)
        history.unlink()
        (output / "previous-history").rename(history)
        assert draft_bytes() == saved_draft
        assert (artifact / "capture.png").read_bytes() == original

        export_click("copy")  # Copy the edited canvas, not the History source.
        wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(),
             "edited clipboard copy completes")
        copied = run("xclip", "-selection", "clipboard", "-t", "image/png", "-o")
        clipboard_png = output / "clipboard-edited.png"
        clipboard_png.write_bytes(copied)
        assert run("identify", "-format", "%wx%h", str(clipboard_png)) == b"480x300"
        for x, y, expected in ((70, 80, (229, 179, 68)), (450, 250, (46, 158, 113))):
            assert run("convert", str(clipboard_png), "-crop", f"1x1+{x}+{y}", "-depth", "8", "rgb:-") == bytes(expected)
        assert draft_bytes() == saved_draft and len(list(history.glob("*/metadata.json"))) == 2
        assert len(list((output / "exports").iterdir())) == 2
        shot(editor, "clipboard-copied")
        export_settings(False)
        toolbar_click("geometry")  # Geometry restores its own scroll position.

        close(editor)
        wait(lambda: not windows("Captures Screenshot Editor"), "saved editor closes")
        assert run("xclip", "-selection", "clipboard", "-t", "image/png", "-o") == copied, "workspace retains clipboard after editor closes"
        editor = reopen()
        shot(editor, "editor-reopened")
        fixture_pixel("editor-reopened", 428, 289, (46, 158, 113))
        canvas_field("width", 510)
        close(editor)  # Shipping closes at once and flushes the new edit.
        wait(lambda: not windows("Captures Screenshot Editor"), "edited editor closes without a prompt")
        wait(lambda: saved(510, 300, -40, -30), "closing flushes the latest edit")
        editor = reopen(keep_banner=True)
        shot(editor, "editor-restored-draft-notice")
        discard_restored(editor)
        wait(lambda: not draft.exists(), "restored-draft Discard removes the saved draft")
        shot(editor, "editor-discarded")

        # Force a genuine filesystem failure without replacing a user's data.
        drafts = output / "editor-drafts"
        drafts.rename(output / "previous-drafts")
        drafts.write_text("blocks directory creation")
        canvas_field("width", 500)
        time.sleep(1.5)  # The autosave fails quietly, as in shipping.
        shot(editor, "editor-autosave-failed")
        assert app.poll() is None and windows("Captures Screenshot Editor")
        close(root)  # With no tray host, normal root close must flush or cancel.
        assert app.poll() is None and windows("Captures Screenshot Editor"), "failed flush must cancel quit"
        shot(editor, "editor-quit-error")
        run("xdotool", "windowsize", "--sync", editor, "760", "540")
        shot(editor, "editor-small-error")
        properties_move("click", "--repeat", "8", "5")
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
                       "annotation-live-swatch", "annotation-locked-fill-stroke-pixels",
                       "annotation-style-undo-redo", "annotation-shadow-toggle-retains-custom",
                       "annotation-color-picker-minimum-reopen-pixels",
                       "canvas", "undo-redo", "draft-reopen", "close-flushes-latest-edit",
                       "image-picker-pending-cancel-retry", "image-import-exact-pixels",
                       "image-import-owned-draft-reopen", "image-picker-stale-close-result",
                       "layer-resize-guides-commit-undo-draft-reopen-pixels",
                       "layer-rotation-free-cancel-shift-snap", "layer-rotation-undo-draft-reopen-pixels",
                       "close-flushes-draft", "restored-draft-discard", "autosave-failure-quiet",
                       "quit-error-retry", "original-unchanged",
                       "layer-duplicate-rename-move", "layer-canvas-click-drag-escape",
                       "layer-canvas-snap-edge-pixels-undo-reopen",
                       "layer-opacity-visibility-pixels",
                       "layer-lock-order-delete", "layer-draft-reopen", "layer-undo-redo",
                       "layer-empty-undo", "image-transform-four-actions-pixels",
                       "image-transform-locked-hidden", "image-transform-canvas-draft-undo",
                       "output-png-jpeg-webp", "output-comparison",
                       "output-budget-error-retry", "output-no-draft-or-file-write",
                       "output-png-palette-minimum", "export-new-copy-history",
                       "export-saved-file-becomes-overwrite-target",
                       "export-collision-original-protection", "export-history-warning-recovery",
                       "folder-portal-cancel-select-filename-no-persistence",
                       "clipboard-edited-pixels-no-persistence", "clipboard-survives-editor-close"],
            "originalSha256": hashlib.sha256(original).hexdigest(),
        }, indent=2) + "\n")
        print("PASS native editor: layers, crop, canvas, undo/redo, autosaved draft reopen, close flush/discard, quit recovery, original unchanged")
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
