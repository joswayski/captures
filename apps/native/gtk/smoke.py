#!/usr/bin/python3
"""AT-SPI smoke test for the GTK4 host: real widgets on a private D-Bus and
accessibility bus under Xvfb, driven only through AT-SPI actions.

History: a 1000-entry on-disk fixture is reachable by name and role, keeps far
fewer card widgets than entries alive (virtualized), filters, and deletes one
entry from disk through the two-step Delete. Preferences: a switch row exposes
the switch role and checked state, and toggling it saves the setting.
Editor: History's Edit opens the screenshot editor; a real pointer drag
(xdotool) draws a rectangle layer, Undo and Redo go through AT-SPI, and Save
writes a new PNG. Previews: Restore opens floating preview cards; Copy shows
its status chip, Close dismisses a card and Delete removes its entry from disk.
Feedback: Preferences' About opens the form as a fixture whose Send stays off;
with a loopback endpoint, keyboard entry sends one draft and the client's
cooldown refuses a second.

Run under `xvfb-run -a dbus-run-session --`, with `--binary` pointing at a
built `captures-gtk`. Needs python3-gi and gir1.2-atspi-2.0. With
`--wayland --injector PATH` (under `dbus-run-session --` only) the same
steps run on a private headless Sway, with the wayland_drag_probe's virtual
pointer and keyboard instead of xdotool.
"""
import argparse
import contextlib
import ctypes
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import re
import select
import signal
import subprocess
import sys
import tempfile
import threading
import time

import gi

gi.require_version("Atspi", "2.0")
gi.require_version("GdkPixbuf", "2.0")
from gi.repository import Atspi, GdkPixbuf, GLib  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from history_fixture import fixture_png, write_history_entries, write_video_entry  # noqa: E402


def wait(predicate, what, timeout=20):
    deadline = time.time() + timeout
    while time.time() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.2)
    raise AssertionError(f"timed out waiting for {what}")


def application(pid):
    desktop = Atspi.get_desktop(0)
    for index in range(desktop.get_child_count()):
        child = desktop.get_child_at_index(index)
        try:
            if child and child.get_process_id() == pid:
                return child
        except GLib.Error:
            continue
    return None


def nodes(root):
    """Every live node; rows rebinding mid-walk disappear and are skipped."""
    stack = [root]
    while stack:
        node = stack.pop()
        try:
            if node is None:
                continue
            identity = (node.get_role_name(), node.get_name())
            children = [node.get_child_at_index(i) for i in range(node.get_child_count())]
        except GLib.Error:
            continue
        yield node, identity
        stack.extend(reversed(children))


def node_text(node, name):
    """The accessible name, else the live Text content: GTK 4.14's AT-SPI
    cache can keep a label's first (empty) name after its text changes."""
    if name:
        return name
    try:
        if "Text" in node.get_interfaces():
            return Atspi.Text.get_text(node, 0, -1)
    except GLib.Error:
        pass
    return ""


def find(root, role, name):
    return [node for node, identity in nodes(root) if identity == (role, name)]


def has_state(node, state):
    try:
        return node.get_state_set().contains(state)
    except GLib.Error:
        return False


# Roles a screen reader announces by name, and roles that mean nothing alone.
NAMED_ROLES = {"push button", "toggle button", "check box", "radio button", "switch",
               "combo box", "slider", "spin button", "menu item", "radio menu item",
               "check menu item", "link", "image", "tab", "page tab", "progress bar"}
GENERIC_ROLES = {"filler", "panel", "unknown", "redundant object", "section", "label",
                 "generic"}


def assert_accessible(root, what):
    """The accessibility gate's tree check: every control and image has a
    name (or text), editable text has a name, and nothing focusable is a
    generic container a screen reader cannot describe."""
    problems = []
    for node, (role, name) in nodes(root):
        focusable = has_state(node, Atspi.StateType.FOCUSABLE)
        if role in NAMED_ROLES and not node_text(node, name):
            problems.append(f"unnamed {role}")
        elif role in ("text", "entry") and focusable and not name:
            problems.append(f"unnamed editable {role}")
        if focusable and role in GENERIC_ROLES:
            problems.append(f"focusable {role} {name!r}")
    assert not problems, f"{what}: {sorted(set(problems))}"


def press(node):
    names = [Atspi.Action.get_action_name(node, i) for i in range(Atspi.Action.get_n_actions(node))]
    assert names, f"{node.get_name()} has no action"
    index = names.index("click") if "click" in names else 0
    assert Atspi.Action.do_action(node, index), f"action failed on {node.get_name()}"


class X11:
    """Real input and window handling under Xvfb (no window manager): xdotool,
    xclip and WM_DELETE_WINDOW. Keys use xdotool names."""

    def window(self, title, pid=None):
        args = ["xdotool", "search", "--all", "--sync"]
        args += ["--onlyvisible", "--pid", str(pid)] if pid else []
        return subprocess.run([*args, "--name", title], capture_output=True, text=True,
                              check=True).stdout.split()[0]

    def origin(self, window):
        shell = subprocess.run(["xdotool", "getwindowgeometry", "--shell", window],
                               capture_output=True, text=True, check=True).stdout
        geometry = dict(line.split("=", 1) for line in shell.split())
        return int(geometry["X"]), int(geometry["Y"])

    def focus(self, window):
        subprocess.run(["xdotool", "windowfocus", "--sync", window], check=True)

    def point_into(self, window, x, y):
        # Without a window manager, keys go to the window under the pointer.
        subprocess.run(["xdotool", "mousemove", "--window", window, str(x), str(y)], check=True)
        time.sleep(0.2)

    def slide(self, window, dx):
        """Move `window` horizontally (xvfb-run's 640 px screen hides its
        right side); returns the offset to add to window coordinates."""
        x, y = self.origin(window)
        subprocess.run(["xdotool", "windowmove", window, str(x + dx), str(y)], check=True)
        time.sleep(0.5)
        return dx

    def click(self, x, y, count=1):
        # GTK must see the pointer enter before the press.
        subprocess.run(["xdotool", "mousemove", str(x), str(y)], check=True)
        time.sleep(0.2)
        subprocess.run(["xdotool", "click", "--repeat", str(count), "--delay", "80", "1"],
                       check=True)

    def drag(self, x, y, to_x, to_y):
        subprocess.run(["xdotool", "mousemove", str(x), str(y)], check=True)
        time.sleep(0.2)
        subprocess.run(["xdotool", "mousedown", "1"], check=True)
        for step in range(1, 11):
            subprocess.run(["xdotool", "mousemove", str(x + (to_x - x) * step // 10),
                            str(y + (to_y - y) * step // 10)], check=True)
            time.sleep(0.03)
        subprocess.run(["xdotool", "mouseup", "1"], check=True)

    def key(self, *names):
        subprocess.run(["xdotool", "key", *names], check=True)

    def type(self, text):
        subprocess.run(["xdotool", "type", "--delay", "40", text], check=True)

    def copy(self, text):
        subprocess.run(["xclip", "-selection", "clipboard"], input=text, text=True, check=True)

    def visible(self, pid):
        return subprocess.run(["xdotool", "search", "--all", "--onlyvisible", "--pid", str(pid),
                               "--name", "."], capture_output=True, text=True).stdout.split()

    def paste_types(self):
        return subprocess.run(["xclip", "-selection", "clipboard", "-t", "TARGETS", "-o"],
                              capture_output=True, text=True).stdout.split()

    def close(self, window):
        """Ask `window` to close with WM_DELETE_WINDOW, as a taskbar or a
        Wayland compositor does: unlike a window manager's Alt+F4 there is no
        keyboard grab, so a focused text field does not commit first."""
        import ctypes

        class ClientMessage(ctypes.Structure):
            _fields_ = [("type", ctypes.c_int), ("serial", ctypes.c_ulong),
                        ("send_event", ctypes.c_int), ("display", ctypes.c_void_p),
                        ("window", ctypes.c_ulong), ("message_type", ctypes.c_ulong),
                        ("format", ctypes.c_int), ("data", ctypes.c_long * 5)]

        class Event(ctypes.Union):
            _fields_ = [("xclient", ClientMessage), ("pad", ctypes.c_long * 24)]

        x11 = ctypes.CDLL("libX11.so.6")
        x11.XOpenDisplay.restype = ctypes.c_void_p
        x11.XInternAtom.restype = ctypes.c_ulong
        x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
        x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int,
                                   ctypes.c_long, ctypes.c_void_p]
        x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
        display = x11.XOpenDisplay(None)
        event = Event()
        message = event.xclient
        message.type = 33  # ClientMessage
        message.display = display
        message.window = int(window)
        message.format = 32
        message.message_type = x11.XInternAtom(display, b"WM_PROTOCOLS", 0)
        message.data[0] = x11.XInternAtom(display, b"WM_DELETE_WINDOW", 0)
        x11.XSendEvent(display, int(window), 0, 0, ctypes.byref(event))
        x11.XCloseDisplay(display)


# Linux evdev codes for the Wayland virtual keyboard, which publishes a US keymap.
EVDEV = dict(zip("qwertyuiopasdfghjklzxcvbnm", [16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 30,
             31, 32, 33, 34, 35, 36, 37, 38, 44, 45, 46, 47, 48, 49, 50], strict=True))
EVDEV.update(dict(zip("1234567890", range(2, 12), strict=True)))
EVDEV.update({"escape": 1, "return": 28, "tab": 15, "space": 57, "right": 106, "left": 105,
              "f3": 61, " ": 57, "-": 12, ".": 52, "/": 53})
SHIFTED = {"@": "2", "_": "-"}


class Wayland:
    """The same operations on a private headless Sway: swaymsg for windows,
    and the wayland_drag_probe's virtual pointer and keyboard for input."""

    WIDTH, HEIGHT = 1280, 1024

    def __init__(self, env, pointer, keyboard):
        self.env, self.pointer, self.keyboard = env, pointer, keyboard

    def _nodes(self):
        tree = json.loads(subprocess.run(["swaymsg", "-t", "get_tree"], env=self.env,
                                         capture_output=True, text=True, check=True).stdout)
        stack = [tree]
        while stack:
            node = stack.pop()
            stack += node.get("nodes", []) + node.get("floating_nodes", [])
            if node.get("pid"):
                yield node

    def _node(self, window):
        return next(n for n in self._nodes() if n["id"] == window)

    def window(self, title, pid=None):
        def found():
            return next((n["id"] for n in self._nodes() if re.search(title, n.get("name") or "")
                         and (pid is None or n["pid"] == pid)), None)
        return wait(found, f"the {title} window")

    def origin(self, window):
        rect = self._node(window)["rect"]
        return rect["x"], rect["y"]

    def _sway(self, window, command):
        subprocess.run(["swaymsg", f"[con_id={window}] {command}"], env=self.env, check=True,
                       stdout=subprocess.DEVNULL)

    def focus(self, window):
        self._sway(window, "focus")
        wait(lambda: self._node(window)["focused"], "compositor focus")
        time.sleep(0.2)

    def point_into(self, window, x, y):
        self.focus(window)

    def slide(self, window, dx):
        return 0  # The 1280 px output shows the whole editor.

    def _pointer(self, line, reply):
        self.pointer.stdin.write(f"{line}\n".encode())
        self.pointer.stdin.flush()
        assert select.select([self.pointer.stdout], [], [], 10)[0], "pointer stalled"
        assert self.pointer.stdout.readline().strip() == reply

    def click(self, x, y, count=1):
        for _ in range(count):
            self._pointer(f"{x} {y} {self.WIDTH} {self.HEIGHT}", b"CLICKED")

    def drag(self, x, y, to_x, to_y):
        self._pointer(f"drag {x} {y} {self.WIDTH} {self.HEIGHT} {to_x} {to_y}", b"DRAGGED")

    def _press(self, key, modifiers):
        code = EVDEV[key]
        mods = "+".join(modifiers) or "none"
        self.keyboard.stdin.write(f"{code} {mods}\n".encode())
        self.keyboard.stdin.flush()
        assert select.select([self.keyboard.stdout], [], [], 10)[0], "keyboard stalled"
        assert self.keyboard.stdout.readline().strip() == f"KEYED {code} {mods}".encode()

    def key(self, *names):
        for name in names:
            *modifiers, key = name.lower().split("+")
            self._press(key, [{"control": "ctrl"}.get(m, m) for m in modifiers])

    def type(self, text):
        for char in text:
            if char in SHIFTED:
                self._press(SHIFTED[char], ["shift"])
            else:
                self._press(char.lower(), ["shift"] if char.isupper() else [])
            time.sleep(0.04)

    def copy(self, text):
        subprocess.run(["wl-copy"], input=text, text=True, env=self.env, check=True)

    def close(self, window):
        self._sway(window, "kill")

    def visible(self, pid):
        return [n["id"] for n in self._nodes() if n["pid"] == pid]

    def paste_types(self):
        return subprocess.run(["wl-paste", "--list-types"], env=self.env, capture_output=True,
                              text=True).stdout.split()


DESKTOP = X11()


def choose_file(app, title, accept, path):
    """Pick `path` in GTK's own file chooser: open its Location field (Ctrl+L
    once the dialog has focus), paste the whole path, then press `accept`.
    Typing key by key races the field's inline autocompletion on GTK 4.14,
    and AT-SPI text edits do not reach it."""
    dialog = DESKTOP.window(f"^{title}$")
    # GTK 4.8 names the stock button "Add Add"; GTK 4.14 names it "Add".
    button = wait(lambda: [n for n, (role, name) in nodes(app)
                           if role == "push button" and name in (accept, f"{accept} {accept}")],
                  f"the {title} dialog")[0]
    DESKTOP.copy(str(path))
    DESKTOP.focus(dialog)
    DESKTOP.key("ctrl+l")
    time.sleep(0.5)
    DESKTOP.key("ctrl+a", "ctrl+v")
    time.sleep(0.5)
    press(button)


def launch(binary, *args, **env):
    env = dict(os.environ, GSK_RENDERER="cairo", GTK_A11Y="atspi", NO_AT_BRIDGE="0", **env)
    return subprocess.Popen([binary, *args], env=env, stdout=subprocess.DEVNULL)


def history(binary, scratch):
    root = scratch / "history"
    write_history_entries(root, 1000)
    process = launch(binary, "--scene", "history", "--history-root", str(root),
                     "--settings-file", str(scratch / "settings.json"), "--size", "1020x720")
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        wait(lambda: find(app, "push button", "All 1000"), "1000 loaded entries")
        assert_accessible(app, "History")
        assert find(app, "heading", "Capture History"), "no Capture History heading"
        everything = find(app, "push button", "All 1000")[0]
        assert has_state(everything, Atspi.StateType.PRESSED), "All filter is not pressed"
        cards = find(app, "push button", "Delete from History")
        assert 0 < len(cards) < 1000, f"{len(cards)} card widgets for 1000 entries"
        images = find(app, "image", "Screenshot from capture history")
        assert images, "thumbnails are not exposed as named images"
        print(f"history: {len(cards)} card widgets alive for 1000 entries")

        press(wait(lambda: find(app, "push button", "GIF 114"), "GIF filter")[0])
        wait(lambda: any(has_state(n, Atspi.StateType.PRESSED)
                         for n in find(app, "push button", "GIF 114")), "GIF filter pressed")
        wait(lambda: find(app, "push button", "Open GIF in editor")
             and not find(app, "push button", "Open screenshot in editor"), "GIF-only cards")
        press(find(app, "push button", "All 1000")[0])
        wait(lambda: find(app, "push button", "Open screenshot in editor"), "all cards again")

        before = len(list(root.iterdir()))
        delete = find(app, "push button", "Delete from History")[0]
        press(delete)
        # The same button re-labels itself; confirm within shipping's 4 s.
        wait(lambda: delete.get_name() == "Confirm permanent deletion", "the armed Delete", 3)
        assert len(list(root.iterdir())) == before, "deleted before confirmation"
        press(delete)
        wait(lambda: len(list(root.iterdir())) == before - 1, "the entry removed from disk")
        wait(lambda: find(app, "push button", "All 999"), "the count to drop to 999")
        print("history: filter and two-step delete passed")
    finally:
        process.terminate()
        process.wait(timeout=10)


def preferences(binary, scratch):
    settings = scratch / "preferences.json"
    # A null ALSA capture device, so the microphone list has a device to show.
    home = scratch / "home"
    home.mkdir()
    (home / ".asoundrc").write_text("pcm.!default { type null }\n")
    process = launch(binary, "--settings-file", str(settings), "--size", "880x660",
                     HOME=str(home))
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        title = "Automatically copy captures to the clipboard"
        # GTK maps the switch role to AT-SPI "switch" or, before 4.10, "check box".
        switch = wait(lambda: [n for role in ("switch", "check box", "toggle button")
                               for n in find(app, role, title)], "the clipboard switch")[0]
        assert_accessible(app, "Preferences")
        before = has_state(switch, Atspi.StateType.CHECKED)
        press(switch)
        wait(lambda: has_state(switch, Atspi.StateType.CHECKED) != before, "checked to flip")
        saved = wait(lambda: settings.exists() and json.loads(settings.read_text()), "settings file")
        assert saved["auto_copy_to_clipboard"] == (not before), saved
        print("preferences: switch role, checked state and save passed")
        # A development build has no Preview identity: no update worker.
        check_now = find(app, "push button", "Check Now")[0]
        assert not has_state(check_now, Atspi.StateType.SENSITIVE), "dev build can check"

        def saved_value(*path):
            value = json.loads(settings.read_text())
            for key in path:
                value = value[key]
            return value

        # Interface theme, accent chips and the corner picker are real
        # pressed / radio widgets, and each writes its setting.
        press(wait(lambda: find(app, "push button", "Dark"), "the Dark segment")[0])
        wait(lambda: saved_value("appearance") == "dark", "Dark to save")
        cobalt = wait(lambda: find(app, "radio button", "Cobalt: True blue and coral"),
                      "the Cobalt chip")[0]
        press(cobalt)
        wait(lambda: has_state(cobalt, Atspi.StateType.CHECKED), "Cobalt to be checked")
        wait(lambda: saved_value("theme") == "cobalt", "Cobalt to save")
        press(wait(lambda: find(app, "radio button", "Custom: Build your own RGB palette"),
                   "the Custom chip")[0])
        hex_field = wait(lambda: [n for role in ("text", "entry")
                                  for n in find(app, role, "Accent hex value")], "Accent hex")[0]
        editable = hex_field.get_editable_text_iface()
        editable.delete_text(0, 7)
        editable.insert_text(0, "#12AB34", 7)
        wait(lambda: saved_value("custom_theme", "accent") == "#12ab34", "the custom accent")
        press(wait(lambda: find(app, "radio button", "Top right"), "the Top right corner")[0])
        wait(lambda: saved_value("mini_preview_placement") == "top_right", "Top right to save")
        print("preferences: Interface theme, accent chips, custom colour and corner saved")

        # Selects write numbers for numeric settings. The sidebar scrolls the
        # card into view first, so the menu has a monitor to open on.
        press(find(app, "push button", "GIF export")[0])
        press(wait(lambda: [n for n, (role, name) in nodes(app)
                            if role == "combo box" and name.startswith("GIF frames per second")],
                   "the GIF frames per second select")[0])
        press(wait(lambda: find(app, "radio menu item", "24 FPS"), "24 FPS")[0])
        wait(lambda: saved_value("recording", "gif_fps") == 24, "24 FPS to save")
        # The microphone list comes from the shared engine's enumeration.
        press(find(app, "push button", "Recording")[0])
        press(wait(lambda: [n for n, (role, name) in nodes(app)
                            if role == "combo box" and name.startswith("Default microphone")],
                   "the Default microphone select")[0])
        press(wait(lambda: find(app, "radio menu item", "Default — default"),
                   "the enumerated default microphone")[0])
        wait(lambda: saved_value("recording", "microphone_device_id") == "default",
             "the microphone to save")

        # The recorder takes the next combination, as shipping's ShortcutInput.
        region = wait(lambda: find(app, "push button", "Region"), "the Region recorder")[0]
        press(region)
        wait(lambda: has_state(region, Atspi.StateType.PRESSED), "the recorder to listen")
        DESKTOP.focus(DESKTOP.window("Preferences$", process.pid))
        DESKTOP.key("ctrl+shift+4")
        wait(lambda: saved_value("region_shortcut") == "Control+Shift+Digit4", "the shortcut")
        wait(lambda: not has_state(region, Atspi.StateType.PRESSED), "the recorder to stop")
        print("preferences: selects, the microphone list and the shortcut recorder saved")

        # Find: Ctrl+F opens a search landmark; the live count follows the
        # query and Ctrl+G, and Escape closes it.
        def find_count():
            return [node_text(n, name) for n, (role, name) in nodes(app)
                    if role in ("status bar", "status")]

        DESKTOP.key("ctrl+f")
        wait(lambda: [n for role in ("text", "entry") for n in find(app, role, "Find settings")],
             "the Find settings field")
        DESKTOP.type("shortcut")
        wait(lambda: "1 of 2" in find_count(), "Find to count two matches")
        DESKTOP.key("ctrl+g")
        wait(lambda: "2 of 2" in find_count(), "Ctrl+G to step to the second match")
        DESKTOP.key("shift+F3")
        wait(lambda: "1 of 2" in find_count(), "Shift+F3 to step back")
        DESKTOP.key("Escape")
        wait(lambda: not [n for role in ("text", "entry") for n in find(app, role, "Find settings")
                          if has_state(n, Atspi.StateType.SHOWING)], "Escape to close Find")
        print("preferences: Find counted, stepped and closed from the keyboard")

    finally:
        process.terminate()
        process.wait(timeout=10)

    # Compact (≤720 px): a narrow window opens without the section nav.
    # GTK hides it from the accessibility tree; Xvfb has no window manager
    # to resize a live window, so open a second, narrow one.
    process = launch(binary, "--settings-file", str(settings), "--size", "640x660")
    try:
        app = wait(lambda: application(process.pid), "the narrow Preferences window")
        wait(lambda: find(app, "push button", "Capture History…"), "the narrow page")
        time.sleep(0.5)
        assert not find(app, "push button", "Shortcuts"), "the nav stayed in a compact window"
        print("preferences: a compact window hides the section nav")
    finally:
        process.terminate()
        process.wait(timeout=10)


def editor(binary, scratch):
    root = scratch / "editor-history"
    write_history_entries(root, 3)
    out = scratch / "exports"
    out.mkdir()
    settings = scratch / "editor-settings.json"
    saved = json.loads((scratch / "preferences.json").read_text())
    saved["output_directory"] = str(out)
    settings.write_text(json.dumps(saved))
    process = launch(binary, "--scene", "history", "--history-root", str(root),
                     "--settings-file", str(settings), "--size", "1020x720")
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        press(wait(lambda: find(app, "push button", "Open screenshot in editor"), "Edit")[0])
        canvas = wait(lambda: find(app, "image", "Screenshot canvas"), "the editor canvas")[0]
        wait(lambda: find(app, "list item", "Original screenshot · Locked background"),
             "the background layer")
        assert_accessible(app, "the screenshot editor")
        press(wait(lambda: find(app, "push button", "Shapes · Rectangle (R)"), "Shapes")[0])
        press(wait(lambda: find(app, "push button", "Rectangle (R)"), "the Shapes flyout")[0])
        # The closing flyout keeps its input grab until it is gone.
        wait(lambda: not find(app, "push button", "Rectangle (R)"), "the flyout to close")
        # Drawing defaults: with nothing selected the Rectangle tool shows its
        # defaults; a green default fill must reach the new layer.
        green = wait(lambda: find(app, "push button", "Fill color: #36c96b"), "the shape defaults")[0]
        press(green)
        wait(lambda: has_state(green, Atspi.StateType.PRESSED), "the green default fill")
        # Drag across the middle of the canvas with the real pointer.
        window = DESKTOP.window("Captures Screenshot Editor")
        origin_x, origin_y = DESKTOP.origin(window)
        extents = canvas.get_extents(Atspi.CoordType.WINDOW)
        # Near the canvas's top-left: xvfb-run's default screen is 640×480,
        # smaller than the editor window.
        x = origin_x + extents.x + 150
        y = origin_y + extents.y + 150
        # GTK must see the pointer enter before the press, or it drops the drag.
        DESKTOP.drag(x, y, x + 160, y + 100)
        layer = lambda: [n for n, (role, name) in nodes(app)
                         if role == "list item" and name.startswith("Rectangle")]
        wait(layer, "a rectangle layer from the drag")
        # The new layer is selected, so its own Fill color shows the default.
        wait(lambda: any(has_state(n, Atspi.StateType.PRESSED)
                         for n in find(app, "push button", "Fill color: #36c96b")),
             "the rectangle to use the default fill")
        press(find(app, "push button", "Undo")[0])
        wait(lambda: not layer(), "Undo to remove the rectangle")
        press(find(app, "push button", "Redo")[0])
        wait(layer, "Redo to restore the rectangle")
        # Saving also adds a History entry, so pick the source first.
        source = next(root.glob("*/capture.png"))
        press(find(app, "push button", "Save")[0])
        written = wait(lambda: list(out.glob("*.png")), "Save to write a PNG")
        time.sleep(0.5)
        pixels = [GdkPixbuf.Pixbuf.new_from_file(str(path)).get_pixels()
                  for path in (source, written[0])]
        assert pixels[0] != pixels[1], "the saved PNG does not contain the rectangle"
        print(f"editor: drag drew a layer, undo/redo, saved {written[0].name}")

        # Properties: select the rectangle and edit it through real widgets;
        # each control reflects the document the worker publishes back.
        select = find(app, "push button", "Select & move (V)")[0]
        press(select)
        wait(lambda: has_state(select, Atspi.StateType.PRESSED), "the Select tool")
        DESKTOP.click(x + 80, y + 50)
        shadow = wait(lambda: find(app, "check box", "Drop shadow"), "the shape properties")[0]
        press(shadow)
        wait(lambda: find(app, "slider", "Shadow blur"), "the shadow settings to open")
        opacity = find(app, "slider", "Opacity")[0]
        for value in (80, 60, 40):
            Atspi.Value.set_current_value(opacity, value)
            time.sleep(0.15)
        wait(lambda: Atspi.Value.get_current_value(opacity) == 40, "opacity to settle at 40")
        press(find(app, "push button", "Fill color: #2d9cff")[0])
        blue = find(app, "push button", "Fill color: #2d9cff")[0]
        wait(lambda: has_state(blue, Atspi.StateType.PRESSED), "the blue fill to apply")
        # Undo reverts the fill, then the whole opacity drag in one step.
        press(find(app, "push button", "Undo")[0])
        wait(lambda: not has_state(blue, Atspi.StateType.PRESSED), "Undo to revert the fill")
        press(find(app, "push button", "Undo")[0])
        wait(lambda: Atspi.Value.get_current_value(opacity) == 100,
             "one Undo to revert the opacity drag")
        print("editor: shape shadow, opacity drag and fill over AT-SPI; drag was one undo step")

        # Text: click with the Text tool, type into the real text field (two
        # lines), and Escape commits a layer named after its first line.
        text_tool = find(app, "push button", "Text (T)")[0]
        press(text_tool)
        # AT-SPI activation clicks after GTK's short press animation.
        wait(lambda: has_state(text_tool, Atspi.StateType.PRESSED), "the Text tool")
        DESKTOP.click(x + 40, y + 60)
        field = wait(lambda: find(app, "text", "Edit text on canvas"), "the inline text field")[0]
        wait(lambda: has_state(field, Atspi.StateType.FOCUSED), "the field to take focus")
        DESKTOP.type("Smoke label")
        DESKTOP.key("Return")
        DESKTOP.type("second")
        typed = Atspi.Text.get_text(field, 0, -1)
        assert typed == "Smoke label\nsecond", f"the field holds {typed!r}"
        DESKTOP.key("Escape")
        wait(lambda: find(app, "list item", "Smoke label · Text"),
             "Escape to commit the text layer")
        wait(lambda: not find(app, "text", "Edit text on canvas"), "the field to close")
        print("editor: Text tool typed two lines into a real text field and committed")

        # The committed label is selected; Bold and Align right round-trip.
        bold = wait(lambda: find(app, "push button", "Bold"), "the text properties")[0]
        was_bold = has_state(bold, Atspi.StateType.PRESSED)
        press(bold)
        wait(lambda: has_state(bold, Atspi.StateType.PRESSED) != was_bold, "Bold to toggle")
        right = find(app, "push button", "Align right")[0]
        press(right)
        wait(lambda: has_state(right, Atspi.StateType.PRESSED), "Align right to apply")
        print("editor: text Bold and alignment round-tripped through the session")

        # Eraser: its properties are real slider and check box widgets, and a
        # Wand click is one undoable edit (Undo leaves the text layer).
        eraser = find(app, "push button", "Eraser (B)")[0]
        press(eraser)
        wait(lambda: has_state(eraser, Atspi.StateType.PRESSED), "the Eraser tool")
        slider = wait(lambda: find(app, "slider", "Color tolerance"), "the tolerance slider")[0]
        assert Atspi.Value.get_current_value(slider) == 36, "tolerance starts at 36"
        contiguous = find(app, "check box", "Contiguous only")[0]
        assert has_state(contiguous, Atspi.StateType.CHECKED), "Contiguous starts checked"
        press(contiguous)
        wait(lambda: not has_state(contiguous, Atspi.StateType.CHECKED), "Contiguous to clear")
        press(contiguous)
        wait(lambda: has_state(contiguous, Atspi.StateType.CHECKED), "Contiguous to check")
        DESKTOP.click(x + 300, y + 120)
        # The worker publishes the wand edit, then Undo reverts only it.
        time.sleep(1.5)
        press(find(app, "push button", "Undo")[0])
        time.sleep(1)
        assert find(app, "list item", "Smoke label · Text"), "Undo removed the text, not the wand"
        # Then Align right, Bold and finally the label itself.
        press(find(app, "push button", "Undo")[0])
        wait(lambda: not has_state(right, Atspi.StateType.PRESSED), "Undo to revert alignment")
        press(find(app, "push button", "Undo")[0])
        wait(lambda: has_state(bold, Atspi.StateType.PRESSED) == was_bold, "Undo to revert Bold")
        press(find(app, "push button", "Undo")[0])
        wait(lambda: not find(app, "list item", "Smoke label · Text"),
             "Undo to remove the text")
        print("editor: Eraser slider and check box over AT-SPI; Wand click was one undo step")

        # Add images: the real GTK chooser imports a PNG as one selected layer.
        picked = scratch / "Pasted logo.png"
        picked.write_bytes(fixture_png(accent=(45, 156, 255)))
        press(find(app, "push button", "Add images")[0])
        choose_file(app, "Add images", "Add", picked)
        wait(lambda: find(app, "list item", "Pasted logo.png · Image"), "the imported image layer")
        wait(lambda: find(app, "spin button", "Layer width"), "the image properties")
        press(find(app, "push button", "Undo")[0])
        wait(lambda: not find(app, "list item", "Pasted logo.png · Image"), "Undo to remove it")
        print("editor: Add images imported a PNG through the GTK chooser as one undo step")

        # Crop stages its rectangle (1 : 1 here) until Apply crop, as shipping.
        crop_tool = find(app, "push button", "Crop (C)")[0]
        press(crop_tool)
        wait(lambda: has_state(crop_tool, Atspi.StateType.PRESSED), "the Crop tool")
        press(wait(lambda: [n for n, (role, name) in nodes(app)
                            if role == "combo box" and name.startswith("Aspect ratio")],
                   "the Aspect ratio select")[0])
        press(wait(lambda: find(app, "radio menu item", "1 : 1"), "the 1 : 1 option")[0])
        wait(lambda: not find(app, "radio menu item", "1 : 1"), "the aspect menu to close")

        def drag_crop():
            DESKTOP.drag(x, y, x + 160, y + 60)
            return wait(lambda: find(app, "spin button", "Crop width"), "the staged crop")[0]

        crop_width = drag_crop()
        crop_height = find(app, "spin button", "Crop height")[0]
        assert_accessible(app, "the staged crop")
        staged = Atspi.Value.get_current_value(crop_width)
        assert staged > 0 and staged == Atspi.Value.get_current_value(crop_height), \
            "the 1 : 1 crop is not square"
        canvas_width = find(app, "spin button", "Canvas width")[0]
        before = Atspi.Value.get_current_value(canvas_width)
        assert before != staged, "the drag cropped before Apply crop"
        press(find(app, "push button", "Clear")[0])
        wait(lambda: not find(app, "spin button", "Crop width"), "Clear to drop the crop")
        crop_width = drag_crop()
        staged = Atspi.Value.get_current_value(crop_width)
        press(find(app, "push button", "Apply crop")[0])
        wait(lambda: Atspi.Value.get_current_value(canvas_width) == staged, "Apply crop to crop")
        wait(lambda: has_state(select, Atspi.StateType.PRESSED), "Apply crop to return to Select")
        press(find(app, "push button", "Undo")[0])
        wait(lambda: Atspi.Value.get_current_value(canvas_width) == before, "Undo to uncrop")
        print(f"editor: Crop staged a 1 : 1 {staged:.0f} px square, Clear, Apply crop and Undo")

        # Canvas toolbar: W resizes, Trim edges shrinks back to the visible
        # layers, and Solid background off makes the canvas transparent.
        width = find(app, "spin button", "Canvas width")[0]
        Atspi.Value.set_current_value(width, 800)
        trim = find(app, "push button", "Trim edges")[0]
        wait(lambda: has_state(trim, Atspi.StateType.SENSITIVE), "Trim edges to enable at 800 px")
        press(trim)
        # The rectangle's drop shadow counts as visible content, so the trimmed
        # width can exceed the 640 px capture slightly.
        wait(lambda: Atspi.Value.get_current_value(width) < 700, "Trim edges to shrink the canvas")
        background = next(n for n, (role, name) in nodes(app)
                          if role == "push button" and name.startswith("Background color: #"))
        press(background)
        solid = wait(lambda: find(app, "check box", "Solid background"), "the background card")[0]
        press(solid)
        wait(lambda: find(app, "push button", "Background color: transparent"),
             "the canvas to become transparent")
        DESKTOP.key("Escape")
        print("editor: canvas width, Trim edges and transparent background over AT-SPI")

        # Export settings: Compress, 50% output and JPEG reach the saved file,
        # and Est. size is encoded off the UI thread.
        def choose(trigger, option):
            press(wait(lambda: [n for n, (role, name) in nodes(app)
                                if role == "combo box" and name.startswith(trigger)],
                       f"the {trigger} select")[0])
            press(wait(lambda: find(app, "radio menu item", option), f"the {option} option")[0])

        disclosure = find(app, "push button", "Export settings")[0]
        press(disclosure)
        wait(lambda: has_state(disclosure, Atspi.StateType.EXPANDED), "Export settings to open")
        document_width = round(Atspi.Value.get_current_value(width))
        choose("Save quality", "Compress")
        wait(lambda: find(app, "combo box", "Compression quality: Highest"), "the Compress preset")
        choose("Output size", "50%")
        choose("Format", "JPEG")
        wait(lambda: [n for n, (role, name) in nodes(app)
                      if role == "label" and name.startswith("≈")], "the Est. size")
        before = set(out.glob("*.jpg"))
        save = find(app, "push button", "Save")[0]
        wait(lambda: has_state(save, Atspi.StateType.SENSITIVE), "Save to enable")
        press(save)
        jpeg = wait(lambda: list(set(out.glob("*.jpg")) - before), "Save to write a JPEG")[0]
        time.sleep(0.5)
        saved_width = GdkPixbuf.Pixbuf.new_from_file(str(jpeg)).get_width()
        assert abs(saved_width - document_width / 2) <= 1, \
            f"50% of {document_width} px saved {saved_width} px"
        print(f"editor: Export settings saved {jpeg.name} at {saved_width} px (Compress, 50%, JPEG)")

        # Change… picks the folder Save writes to through GTK's chooser.
        elsewhere = scratch / "Elsewhere"
        elsewhere.mkdir()
        press(wait(lambda: find(app, "push button", "Change save location"),
                   "Change save location")[0])
        choose_file(app, "Choose save location", "Select", elsewhere)
        wait(lambda: [n for n, (role, name) in nodes(app)
                      if role == "label" and name == f"Saving to {elsewhere}"],
             "the new save location")
        press(save)
        wait(lambda: list(elsewhere.glob("*.jpg")), "Save to write into the chosen folder")
        print("editor: Change… moved Save to another folder")

        # Compress with Export settings open shows the before/after split.
        wait(lambda: find(app, "push button", "Drag to compare before and after"),
             "the compression comparison")
        wait(lambda: [n for n, (role, name) in nodes(app)
                      if role == "label" and name.startswith("After · ")
                      and "Processing" not in name], "the After badge size")
        Atspi.Value.set_current_value(find(app, "slider", "Before and after comparison")[0], 25)
        press(find(app, "push button", "Hide compression comparison")[0])
        show = wait(lambda: find(app, "push button", "Show before / after"), "Show before / after")
        assert not find(app, "push button", "Drag to compare before and after"), "Hide kept the split"
        press(show[0])
        wait(lambda: find(app, "push button", "Drag to compare before and after"),
             "Show before / after to restore the comparison")
        print("editor: Compress before/after comparison: split, Hide and Show")

        # Layers: double-click renames an image layer in its row, and dragging
        # its row below the next layer reorders it. The 640 px screen cannot
        # show the layers panel, so slide the window left for the pointer.
        press(find(app, "push button", "Add images")[0])
        choose_file(app, "Add images", "Add", picked)
        wait(lambda: find(app, "list item", "Pasted logo.png · Image"), "the imported image layer")
        shift = DESKTOP.slide(window, -500)
        origin_x, origin_y = DESKTOP.origin(window)

        def row_center(row):
            extents = row.get_extents(Atspi.CoordType.WINDOW)
            return (origin_x + extents.x + extents.width // 2,
                    origin_y + extents.y + extents.height // 2)

        def rows():
            return [name for _, (role, name) in nodes(app)
                    if role == "list item" and " · " in name]

        logo = find(app, "list item", "Pasted logo.png · Image")[0]
        x, y = row_center(logo)
        DESKTOP.click(x, y, count=2)
        wait(lambda: find(app, "text", "Rename layer"), "the Rename layer field")
        time.sleep(0.5)  # The row's entry takes focus on idle.
        DESKTOP.key("ctrl+a")
        DESKTOP.type("  Logo  ")
        DESKTOP.key("Return")
        wait(lambda: find(app, "list item", "Logo · Image"), "the trimmed new layer name")
        before = rows()
        assert before[0] == "Logo · Image", f"unexpected layer order {before}"
        below = find(app, "list item", before[1])[0]
        x, y = row_center(find(app, "list item", "Logo · Image")[0])
        _, target_y = row_center(below)
        target_y += below.get_extents(Atspi.CoordType.WINDOW).height // 4
        DESKTOP.drag(x, y, x, target_y)
        wait(lambda: rows()[:2] == [before[1], "Logo · Image"],
             f"the drag to move Logo below {before[1]}")
        press(find(app, "push button", "Undo")[0])
        wait(lambda: rows()[:2] == before[:2], "Undo to restore the layer order")
        DESKTOP.slide(window, -shift)
        print(f"editor: renamed an image layer to Logo and dragged it below {before[1]}")

        # The ⋯ layer settings card: opacity, Send to back, Duplicate, Flatten.
        def layer_menu(name):
            press(wait(lambda: find(app, "push button", f"Layer settings for {name}"),
                       f"the {name} settings button")[0])
            return wait(lambda: find(app, "dialog", f"Layer settings for {name}"),
                        f"the {name} settings card")[0]

        card = layer_menu("Logo")
        Atspi.Value.set_current_value(find(card, "slider", "Layer opacity")[0], 50)
        time.sleep(0.5)
        press(find(card, "push button", "Send to back")[0])
        wait(lambda: rows()[-2:] == ["Logo · Image", "Original screenshot · Locked background"],
             "Send to back to stop above the locked background")
        count = len(rows())
        card = layer_menu("Logo")
        opacity = Atspi.Value.get_current_value(find(card, "slider", "Layer opacity")[0])
        assert round(opacity) == 50, f"the card reopened at {opacity}% opacity"
        press(find(card, "push button", "Duplicate")[0])
        wait(lambda: len(rows()) == count + 1, "Duplicate to add a layer")
        card = layer_menu("Logo")
        press(find(card, "push button", "Flatten image")[0])
        wait(lambda: len(rows()) == 1, "Flatten image to leave one layer")
        print("editor: layer card set opacity, sent to back, duplicated and flattened")
    finally:
        process.terminate()
        process.wait(timeout=10)


def editor_close_keeps_open_text(binary, scratch):
    """Closing the editor with a text label still open commits it into the
    draft: reopening the capture restores both labels."""
    root = scratch / "draft-history"
    write_history_entries(root, 1)
    process = launch(binary, "--scene", "history", "--history-root", str(root),
                     "--settings-file", str(scratch / "draft-settings.json"), "--size", "1020x720")
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")

        def open_editor():
            press(wait(lambda: find(app, "push button", "Open screenshot in editor"), "Edit")[0])
            wait(lambda: find(app, "list item", "Original screenshot · Locked background"),
                 "the background layer")
            return DESKTOP.window("Captures Screenshot Editor")

        def label(window, dx, words):
            tool = find(app, "push button", "Text (T)")[0]
            press(tool)
            wait(lambda: has_state(tool, Atspi.StateType.PRESSED), "the Text tool")
            DESKTOP.focus(window)
            # Near the canvas's top-left: xvfb-run's default screen is 640×480.
            canvas = find(app, "image", "Screenshot canvas")[0]
            extents = canvas.get_extents(Atspi.CoordType.WINDOW)
            origin_x, origin_y = DESKTOP.origin(window)
            DESKTOP.click(origin_x + extents.x + 120 + dx, origin_y + extents.y + 120)
            field = wait(lambda: find(app, "text", "Edit text on canvas"), "the inline field")[0]
            wait(lambda: has_state(field, Atspi.StateType.FOCUSED), "the field to take focus")
            DESKTOP.type(words)
            wait(lambda: find(app, "list item", f"{words} · Text"), f"the {words} layer")

        window = open_editor()
        label(window, 0, "abc")
        DESKTOP.key("Escape")
        wait(lambda: not find(app, "text", "Edit text on canvas"), "Escape to commit abc")
        label(window, 150, "OPEN")
        DESKTOP.close(window)
        wait(lambda: not find(app, "image", "Screenshot canvas"), "the editor to close")
        open_editor()
        wait(lambda: find(app, "list item", "OPEN · Text") and find(app, "list item", "abc · Text"),
             "the reopened draft to restore both labels")
        print("editor: closing with a label open kept it and the committed one in the draft")
    finally:
        process.terminate()
        process.wait(timeout=10)


def recording_output(binary, scratch):
    """Output options: crop with the aspect lock, GIF format (its settings
    card, Save as new file forced on) and a real Save at the cropped size."""
    root = scratch / "output-history"
    write_video_entry(root, seconds=2)
    exports = root.with_name(root.name + "-exports")
    process = launch(binary, "--scene", "history", "--history-root", str(root),
                     "--settings-file", str(scratch / "preferences.json"), "--size", "1020x720")
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        press(wait(lambda: find(app, "push button", "Open video in editor"), "video Edit")[0])
        wait(lambda: find(app, "heading", "Crop & size"), "the Crop & size card")

        def choose(trigger, option):
            press(wait(lambda: [n for n, (role, name) in nodes(app)
                                if role == "combo box" and name.startswith(trigger)], trigger)[0])
            press(wait(lambda: find(app, "radio menu item", option), option)[0])
            wait(lambda: not find(app, "radio menu item", option), f"the {trigger} menu to close")

        # Compress shows the before/after comparison over the preview.
        choose("Save quality", "Compress")
        split = wait(lambda: find(app, "slider", "Before and after comparison"),
                     "the encoded comparison", timeout=40)[0]
        Atspi.Value.set_current_value(split, 25)
        wait(lambda: round(Atspi.Value.get_current_value(split)) == 25, "the split at 25%")
        press(find(app, "push button", "Hide compression comparison")[0])
        show = wait(lambda: find(app, "push button", "Show before / after"), "Show before / after")
        press(show[0])
        wait(lambda: find(app, "slider", "Before and after comparison"), "the comparison back")
        print("recording: Compress before/after comparison: split, Hide and Show")

        press(find(app, "check box", "Crop recording")[0])
        width = find(app, "spin button", "Width")[0]
        Atspi.Value.set_current_value(width, 320)
        height = find(app, "spin button", "Height")[0]
        wait(lambda: Atspi.Value.get_current_value(height) == 180,
             "the aspect lock to set Height 180")
        wait(lambda: find(app, "combo box", "Output resolution: Original — 320 × 180"),
             "Output resolution to follow the crop")
        # The drawn crop handles are AT-SPI sliders over source-pixel edges.
        # They show once the uncropped source frame arrives.
        wait(lambda: len([name for _, (role, name) in nodes(app)
                          if role == "slider" and name.startswith("Recording crop")]) == 8,
             "the eight crop handle sliders")
        assert_accessible(app, "the recording crop")
        east = find(app, "slider", "Recording crop east handle")[0]
        left = Atspi.Value.get_current_value(find(app, "spin button", "X")[0])
        assert Atspi.Value.get_current_value(east) == left + 320, "east handle off the crop edge"
        Atspi.Value.set_current_value(east, left + 288)
        wait(lambda: Atspi.Value.get_current_value(width) == 288, "the east handle to set Width")
        wait(lambda: Atspi.Value.get_current_value(height) == 162, "the lock to follow the handle")
        Atspi.Value.set_current_value(east, left + 320)
        wait(lambda: Atspi.Value.get_current_value(height) == 180, "the handle back to 320 × 180")
        print("recording: crop handle sliders move the box over AT-SPI")
        choose("Format", "GIF")
        wait(lambda: find(app, "heading", "GIF settings"), "the GIF settings card")
        assert find(app, "combo box", "Save quality: Compress"), "GIF did not stay on Compress"
        switch = [n for role in ("switch", "check box", "toggle button")
                  for n in find(app, role, "Save as new file")][0]
        wait(lambda: has_state(switch, Atspi.StateType.CHECKED), "GIF to save a new file")
        # Change… picks the folder Save writes to through GTK's chooser.
        elsewhere = scratch / "gif-elsewhere"
        elsewhere.mkdir()
        press(find(app, "push button", "Change save location")[0])
        choose_file(app, "Choose save location", "Select", elsewhere)
        wait(lambda: [n for n, (role, name) in nodes(app)
                      if role == "label" and str(elsewhere) in node_text(n, name)],
             "the footer to show the new folder")
        save = find(app, "push button", "Save")[0]
        wait(lambda: has_state(save, Atspi.StateType.SENSITIVE), "Save to enable")
        press(save)
        saved = wait(lambda: [p for p in elsewhere.iterdir() if p.name.endswith("-edited.gif")],
                     "the saved GIF", timeout=60)[0]
        assert not [p for p in exports.iterdir() if p.suffix == ".gif"], "GIF saved beside source"
        # The saved toast, Show in Folder, and Save off until something changes.
        wait(lambda: [n for n, (role, name) in nodes(app) if role == "status bar"
                      and node_text(n, name).startswith("GIF saved")], "the saved toast")
        assert has_state(find(app, "push button", "Show in Folder")[0],
                         Atspi.StateType.SENSITIVE), "Show in Folder is off after Save"
        wait(lambda: not has_state(save, Atspi.StateType.SENSITIVE),
             "Save to stay off for the saved settings")
        gif = GdkPixbuf.PixbufAnimation.new_from_file(str(saved))
        assert (gif.get_width(), gif.get_height()) == (320, 180), \
            f"saved {gif.get_width()}×{gif.get_height()}"
        print(f"recording: cropped to 320×180 and saved {saved.name} as GIF via Change…")
    finally:
        process.terminate()
        process.wait(timeout=10)


def previews(binary, scratch):
    root = scratch / "preview-history"
    write_history_entries(root, 3)
    process = launch(binary, "--scene", "history", "--history-root", str(root),
                     "--settings-file", str(scratch / "preview-settings.json"),
                     "--size", "1020x720")
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        cards = lambda: find(app, "image", "Screenshot preview")
        restores = wait(lambda: find(app, "push button", "Restore"), "Restore buttons")
        press(restores[0])
        wait(lambda: len(cards()) == 1, "one preview card")
        assert_accessible(app, "the previews")
        press(wait(lambda: len(find(app, "push button", "Restore")) > 1
                   and find(app, "push button", "Restore"), "a second Restore")[1])
        wait(lambda: len(cards()) == 2, "two preview cards")
        # The chip shows for 2.5 s; walk only the previews window so a slow
        # runner's full-tree walk cannot miss it. The status role differs
        # across GTK releases, so match the chip by its text.
        stack = wait(lambda: find(app, "frame", "Captures Previews"), "the previews window")[0]
        press(find(stack, "push button", "Copy")[0])
        try:
            chip = wait(lambda: [text for text in (node_text(n, name)
                                                   for n, (_, name) in nodes(stack))
                                 if text == "Copied to clipboard"
                                 or text.startswith("Couldn’t copy")],
                        "the Copied chip", timeout=10)
        except AssertionError:
            print("previews tree:", [identity for _, identity in nodes(stack)], flush=True)
            raise
        assert chip == ["Copied to clipboard"], f"copy reported {chip}"
        # Minimize folds the stack into the pile: one expand button, the
        # rear card out of the tree; the button brings the list back.
        press(find(stack, "push button", "Minimize previews")[0])
        expand = wait(lambda: find(stack, "push button", "Expand 2 previews"), "the pile")[0]
        wait(lambda: len(find(stack, "image", "Screenshot preview")) == 1,
             "the rear card to leave the tree")
        press(expand)
        wait(lambda: find(stack, "push button", "Minimize previews")
             and len(find(stack, "image", "Screenshot preview")) == 2, "the pile to expand")
        print("previews: minimize to the pile and expand over AT-SPI")
        press(find(app, "push button", "Close")[0])
        wait(lambda: len(cards()) == 1, "Close to dismiss a card")
        before = len(list(root.iterdir()))
        press(find(app, "push button", "Delete")[0])
        wait(lambda: len(list(root.iterdir())) == before - 1, "Delete to remove the entry")
        wait(lambda: not cards(), "the last card to leave")
        wait(lambda: find(app, "push button", "All 2"), "History to reload")
        print("previews: restore, copy, close and delete passed")
    finally:
        process.terminate()
        process.wait(timeout=10)


def preview_overflow(binary, scratch):
    """Five cards (976 px) overflow a 1024 px screen's 936 px of room: the stack scrolls, showing
    the newest card, and the overflow cues scroll it a card at a time."""
    root = scratch / "overflow-history"
    write_history_entries(root, 6)
    process = launch(binary, "--scene", "history", "--history-root", str(root),
                     "--settings-file", str(scratch / "overflow-settings.json"),
                     "--size", "1260x1000")
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        for count in range(1, 6):
            restores = wait(lambda: len(find(app, "push button", "Restore")) >= count
                            and find(app, "push button", "Restore"), f"{count} Restore buttons")
            press(restores[count - 1])
            wait(lambda: len(find(app, "image", "Screenshot preview")) == count,
                 f"{count} preview cards")
        stack = wait(lambda: find(app, "frame", "Captures Previews"), "the previews window")[0]
        older = wait(lambda: find(stack, "push button", "Show older captures"), "the older cue")
        assert not find(stack, "push button", "Show newer captures"), "the stack began scrolled up"
        for _ in range(6):
            press(older[0])
            if find(stack, "push button", "Show newer captures"):
                break
            time.sleep(0.5)
        wait(lambda: find(stack, "push button", "Show newer captures"), "the newer cue")
        wait(lambda: not find(stack, "push button", "Show older captures"),
             "the older cue to go once the oldest card shows", timeout=10)
        print("previews: five cards scroll with the overflow cues")
    finally:
        process.terminate()
        process.wait(timeout=10)


def feedback(binary, scratch):
    # Without an endpoint the form is a fixture: Send stays off.
    process = launch(binary, "--settings-file", str(scratch / "feedback-settings.json"),
                     "--size", "880x660")
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        press(wait(lambda: find(app, "push button", "Open: Send feedback"), "About's Open")[0])
        form = wait(lambda: [n for n, (_, name) in nodes(app) if name == "Send Feedback"],
                    "the Send Feedback window")[0]
        wait(lambda: [n for n, (_, name) in nodes(form)
                      if node_text(n, name) == "Fixture mode — sending feedback is disabled."],
             "the fixture note")
        assert_accessible(form, "Send Feedback")
        send = find(form, "push button", "Send feedback")[0]
        assert not has_state(send, Atspi.StateType.SENSITIVE), "a fixture can send"
        print("feedback: Preferences opens the fixture form with Send off")
    finally:
        process.terminate()
        process.wait(timeout=10)

    received = []

    class Service(BaseHTTPRequestHandler):
        def do_POST(self):
            received.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            body = b'{"ok":true}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    server = HTTPServer(("127.0.0.1", 0), Service)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    endpoint = f"http://127.0.0.1:{server.server_port}/api/feedback"
    process = launch(binary, "--scene", "feedback", "--feedback-endpoint", endpoint,
                     "--settings-file", str(scratch / "feedback-settings.json"))
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        bug = wait(lambda: find(app, "radio button", "Bug"), "the Bug category")[0]
        idea = find(app, "radio button", "Idea")[0]
        wait(lambda: has_state(bug, Atspi.StateType.CHECKED), "Bug to start checked")
        press(idea)
        wait(lambda: has_state(idea, Atspi.StateType.CHECKED)
             and not has_state(bug, Atspi.StateType.CHECKED), "Idea to take the check")
        send = find(app, "push button", "Send feedback")[0]
        # GTK 4.14's cache keeps the first name, "…": also read live text.
        def shows_system(node, name):
            texts = [name]
            if "Text" in node.get_interfaces():
                texts.append(Atspi.Text.get_text(node, 0, -1))
            return any(text.startswith("linux · ") for text in texts)

        wait(lambda: [n for n, (_, name) in nodes(app) if shows_system(n, name)],
             "the system details")
        assert not has_state(send, Atspi.StateType.SENSITIVE), "Send is on without a message"
        # Keyboard only: nothing starts focused, so Tab walks the three
        # categories into the message, then on to the contact field. Without
        # a window manager, keys go to the window under the pointer.
        DESKTOP.point_into(DESKTOP.window("^Send Feedback$"), 20, 20)
        DESKTOP.key("Tab", "Tab", "Tab", "Tab")
        DESKTOP.type("Toolbar overlaps")
        DESKTOP.key("Tab")
        DESKTOP.type("@captures")
        wait(lambda: has_state(send, Atspi.StateType.SENSITIVE), "Send to enable")
        press(send)
        wait(lambda: received, "the loopback service to receive the draft")
        sent = received[0]
        assert (sent["message"], sent["category"], sent["contact"], sent["source"],
                sent["os"]) == ("Toolbar overlaps", "idea", "@captures", "desktop",
                                "linux"), sent
        wait(lambda: [n for n, (_, name) in nodes(app)
                      if node_text(n, name) == "Thanks — feedback sent."], "the sent note")
        message = [n for n, (role, name) in nodes(app) if name == "Message"
                   and role != "label" and "Text" in n.get_interfaces()][0]
        wait(lambda: Atspi.Text.get_text(message, 0, -1) == "", "the message to clear")
        # A second draft inside a minute stops at the client's cooldown.
        # Sending disabled the fields, which dropped focus as shipping's
        # browser does: Shift+Tab reaches Contact, then the message.
        DESKTOP.key("shift+Tab", "shift+Tab")
        DESKTOP.type("Again")
        wait(lambda: has_state(send, Atspi.StateType.SENSITIVE), "Send to enable again")
        press(send)
        wait(lambda: [n for n, (_, name) in nodes(app) if node_text(n, name)
                      == "Please wait a minute before sending more feedback."],
             "the cooldown alert")
        assert len(received) == 1, received
        print("feedback: keyboard draft sent to a loopback service; cooldown refused a second")
    finally:
        process.terminate()
        process.wait(timeout=10)
        server.shutdown()


def open_with(binary, scratch):
    """Open With: files after `--` import into History and open their
    editors without Preferences; a file Captures cannot open reports it in
    shipping's "Captures" error dialog."""
    root = scratch / "open-history"
    files = scratch / "open-files"
    files.mkdir()
    image = files / "Shared screenshot.png"
    image.write_bytes(fixture_png(accent=(45, 156, 255)))
    movie = files / "Shared clip.mp4"
    subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i",
                    "testsrc2=size=320x180:rate=10:duration=2", "-c:v", "mpeg4", "-an",
                    str(movie)], check=True)
    note = files / "notes.txt"
    note.write_text("not media")
    process = launch(binary, "--history-root", str(root), "--settings-file",
                     str(scratch / "preferences.json"), "--", str(image), str(movie), str(note))
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        wait(lambda: find(app, "image", "Screenshot canvas"), "the screenshot editor")
        wait(lambda: [n for n, (_, name) in nodes(app) if name == "Captures Editor"],
             "the recording editor")
        imported = sorted(json.loads(path.read_text())["kind"]
                          for path in root.glob("*/metadata.json"))
        assert imported == ["screenshot", "video"], imported
        dialog = wait(lambda: [n for n, (role, name) in nodes(app)
                               if name == "Captures" and role in ("alert", "dialog")],
                      "the open error dialog")[0]
        assert not [n for n, (_, name) in nodes(app) if name == "Preferences"], \
            "Open With also opened Preferences"
        assert [n for n, (_, name) in nodes(dialog) if name.startswith("Captures can open")]
        # GTK 4.8 names the stock button "_OK OK"; later releases "OK".
        press([n for n, (role, name) in nodes(dialog)
               if role == "push button" and name in ("OK", "_OK OK")][0])
        wait(lambda: not [n for n, (role, name) in nodes(app)
                          if name == "Captures" and role in ("alert", "dialog")],
             "OK to close the error")
        print("open with: a PNG and an MP4 opened their editors; a text file was refused")
    finally:
        process.terminate()
        process.wait(timeout=10)


def live(binary, scratch):
    """Live app: a quiet launch stays resident with no window; a second
    launch for the profile forwards a relaunch or files and exits before GTK
    starts; closing every window keeps the app running; SIGTERM quits and
    releases the profile for the next launch."""
    root = scratch / "live-history"
    settings = scratch / "live-settings.json"
    settings.write_text(json.dumps({
        "settings_schema_version": 5, "onboarding_completed": True, "appearance": "dark",
        "theme": "mustard", "output_directory": str(scratch / "live-exports"),
        "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
        "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10",
        "launch_at_login": False, "show_mini_previews": False, "auto_copy_to_clipboard": False}))
    image = scratch / "Live share.png"
    image.write_bytes(fixture_png(accent=(240, 90, 60)))
    log = scratch / "live.log"
    profile = ["--live", "--history-root", str(root)]

    def events():
        return [json.loads(line) for line in log.read_text().splitlines() if line.startswith("{")]

    def forward(*files):
        unused = scratch / "live-unused.json"
        result = subprocess.run([binary, *profile, "--settings-file", str(unused), "--", *files],
                                capture_output=True, text=True, timeout=10)
        assert result.returncode == 0, result.stderr
        assert [json.loads(line)["event"] for line in result.stdout.splitlines()] == ["forwarded"]
        assert not unused.exists(), "the forwarded launch wrote settings"

    env = dict(os.environ, GSK_RENDERER="cairo", GTK_A11Y="atspi", NO_AT_BRIDGE="0")
    with log.open("w") as stream:
        process = subprocess.Popen([binary, *profile, "--settings-file", str(settings),
                                    "--captures-autostart"], env=env, stdout=stream)
    try:
        ready = wait(lambda: [e for e in events() if e["event"] == "ready"], "live readiness")[0]
        # A quiet launch into the tray shows only shipping's 5 s launch
        # notice: caretless (no tray geometry), with the New Capture keys on
        # X11 and the tray route on Wayland, where the portal owns the keys.
        assert ready["windows"] == 1, events()
        assert {"event": "launch", "action": "StartupNotice", "crashReview": False} in events(), \
            events()
        shown = [e for e in events() if e["event"] == "startup-notice"]
        assert shown and (shown[0]["visibleMs"], shown[0]["caret"]) == (5000, "none"), events()
        wayland = isinstance(DESKTOP, Wayland)
        assert shown[0]["keys"] == ([] if wayland else ["Ctrl", "Shift", "F10"]), shown
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        # The status card named for its title, or unnamed on GTK 4.14, which
        # drops a status node's label and labelled-by; either way it holds
        # the title.
        title = "Captures is ready to use"
        card = wait(lambda: [n for n, (role, name) in nodes(app)
                             if role in ("status", "status bar") and name in (title, "")
                             and [c for c, (_, text) in nodes(n) if text == title]],
                    "the launch notice")[0]
        hint = "Open History from the tray menu" if wayland else "Open New Capture with"
        assert [n for n, (_, name) in nodes(card) if node_text(n, name) == hint], hint
        press(find(card, "push button", "Close")[0])
        wait(lambda: {"event": "startup-notice-closed", "by": "close"} in events(),
             "Close to dismiss the launch notice")
        forward()
        preferences = DESKTOP.window("Preferences$", process.pid)
        wait(lambda: any(e["event"] == "instance-relaunch" for e in events()), "the relaunch")
        DESKTOP.close(preferences)
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        wait(lambda: not [n for n, (_, name) in nodes(app) if name.endswith("Preferences")],
             "Preferences to close")
        time.sleep(1)
        assert process.poll() is None, "closing the last window quit the resident app"
        forward(str(image))
        wait(lambda: find(app, "image", "Screenshot canvas"), "the forwarded file's editor")
        assert len(list(root.glob("*/metadata.json"))) == 1
        process.send_signal(signal.SIGTERM)
        assert process.wait(timeout=10) == 0
        # The next launch becomes the primary instead of forwarding.
        restart = subprocess.run([binary, *profile, "--settings-file", str(settings),
                                  "--captures-autostart", "--quit-after", "2"], env=env,
                                 capture_output=True, text=True, timeout=20)
        assert restart.returncode == 0, restart.stderr
        assert '"event":"ready"' in restart.stdout, restart.stdout
        print("live: quiet launch showed only the launch notice; relaunch, close and a "
              "forwarded PNG handled; SIGTERM released the profile")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()


def desktop_fixture(log, still):
    """`--desktop-fixture`: an unlocked org.freedesktop.ScreenSaver (the
    shared capture-session check), a StatusNotifierWatcher with a registered
    host that logs each tray item, and, with `still`, a Screenshot portal that
    answers each request with that PNG one second later (on a private session
    bus, so it never races a real portal)."""
    from gi.repository import Gio

    xml = """<node><interface name="org.freedesktop.ScreenSaver">
      <method name="GetActive"><arg type="b" direction="out"/></method></interface>
      <interface name="org.freedesktop.portal.Screenshot">
      <method name="Screenshot"><arg type="s" direction="in"/><arg type="a{sv}" direction="in"/>
      <arg type="o" direction="out"/></method>
      <property name="version" type="u" access="read"/></interface>
      <interface name="org.freedesktop.portal.ScreenCast">
      <method name="CreateSession"><arg type="a{sv}" direction="in"/>
      <arg type="o" direction="out"/></method>
      <property name="version" type="u" access="read"/></interface>
      <interface name="org.kde.StatusNotifierWatcher">
      <method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method>
      <method name="RegisterStatusNotifierHost"><arg type="s" direction="in"/></method>
      <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
      <property name="ProtocolVersion" type="i" access="read"/>
      <property name="RegisteredStatusNotifierItems" type="as" access="read"/></interface></node>"""
    saver, portal, screencast, watcher = Gio.DBusNodeInfo.new_for_xml(xml).interfaces
    items = []

    def call(connection, sender, path, interface, method, params, invocation):
        if method == "GetActive":
            invocation.return_value(GLib.Variant("(b)", (Path(log).with_suffix(".locked").exists(),)))
            return
        if method == "RegisterStatusNotifierHost":
            invocation.return_value(None)
            return
        if method == "RegisterStatusNotifierItem":
            service = params.unpack()[0]
            items.append(service)
            with open(log, "a") as stream:
                stream.write(json.dumps({"event": "item", "service": service, "sender": sender})
                             + "\n")
            invocation.return_value(None)
            return
        token = params.unpack()[-1]["handle_token"]
        request = f"/org/freedesktop/portal/desktop/request/{sender[1:].replace('.', '_')}/{token}"
        with open(log, "a") as stream:
            stream.write(json.dumps({"event": "request", "method": method}) + "\n")
        invocation.return_value(GLib.Variant("(o)", (request,)))

        def respond():
            # A screenshot answers with the still; a ScreenCast session is
            # refused as a person dismissing the window picker would.
            if method == "CreateSession":
                response, results = 1, {}
            else:
                response, results = 0, {"uri": GLib.Variant("s", Path(still).as_uri())}
            connection.emit_signal(sender, request, "org.freedesktop.portal.Request", "Response",
                                   GLib.Variant("(ua{sv})", (response, results)))
            return False
        GLib.timeout_add(1000, respond)

    def version(*_):
        return GLib.Variant("u", 3)

    def watched(connection, sender, path, interface, name):
        return {"IsStatusNotifierHostRegistered": GLib.Variant("b", True),
                "ProtocolVersion": GLib.Variant("i", 0),
                "RegisteredStatusNotifierItems": GLib.Variant("as", items)}[name]

    def owned(connection, name):
        if name == "org.freedesktop.ScreenSaver":
            connection.register_object("/org/freedesktop/ScreenSaver", saver, call, None, None)
        elif name == "org.kde.StatusNotifierWatcher":
            connection.register_object("/StatusNotifierWatcher", watcher, call, watched, None)
        else:
            for interface in (portal, screencast):
                connection.register_object("/org/freedesktop/portal/desktop", interface, call,
                                           version, None)
        names.remove(name)
        if not names:
            print("READY", flush=True)

    names = ["org.freedesktop.ScreenSaver", "org.kde.StatusNotifierWatcher"]
    names += ["org.freedesktop.portal.Desktop"] if still else []
    for name in list(names):
        Gio.bus_own_name(Gio.BusType.SESSION, name, 0, owned, None, None)
    GLib.MainLoop().run()


@contextlib.contextmanager
def private_desktop(scratch, name, still=None):
    """A private session bus with the desktop fixture (unlocked screen
    saver, tray watcher and, with `still`, the Screenshot portal answering
    with that PNG), so portal requests never reach a real portal. Yields the
    launch environment and the fixture's log."""
    fixture_log = scratch / f"{name}-fixture.log"
    fixture_log.touch()
    # Services the private bus activates (the GTK portal backend) would
    # otherwise start a second accessibility bus for this display, whose
    # exit unlinks the shared socket the later AT-SPI steps use.
    a11y = subprocess.run(["gdbus", "call", "--session", "--dest", "org.a11y.Bus",
                           "--object-path", "/org/a11y/bus", "--method", "org.a11y.Bus.GetAddress"],
                          capture_output=True, text=True, check=True).stdout.strip()[2:-3]
    data = scratch / f"{name}-bus-data"
    if still:
        # Some D-Bus versions require an activation entry even when the
        # fixture already owns the name. Never launch a real desktop portal;
        # a lost fixture owner must fail rather than start a second service.
        services = data / "dbus-1/services"
        services.mkdir(parents=True)
        (services / "org.freedesktop.portal.Desktop.service").write_text(
            "[D-BUS Service]\nName=org.freedesktop.portal.Desktop\nExec=/bin/false\n")
    bus = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1"],
                           stdout=subprocess.PIPE, text=True,
                           env=dict(os.environ, AT_SPI_BUS_ADDRESS=a11y, XDG_DATA_HOME=str(data)))
    address = bus.stdout.readline().strip()
    env = dict(os.environ, DBUS_SESSION_BUS_ADDRESS=address, DBUS_SYSTEM_BUS_ADDRESS=address,
               AT_SPI_BUS_ADDRESS=a11y, GSK_RENDERER="cairo", GTK_A11Y="none")
    command = [sys.executable, __file__, "--binary", "-", "--desktop-fixture", str(fixture_log)]
    service = subprocess.Popen(command + (["--still", str(still)] if still else []), env=env,
                               stdout=subprocess.PIPE)
    try:
        assert select.select([service.stdout], [], [], 20)[0], "the desktop fixture stalled"
        assert service.stdout.readline().strip() == b"READY"
        yield env, fixture_log
    finally:
        for child in (service, bus):
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=10)


def capture(binary, scratch):
    """Live Screenshot Display through the shared capture flow: X11 reads the
    display under the pointer; Wayland asks the Screenshot portal (a fixture
    here) with every Captures window unmapped. The capture joins History and
    the preview stack, and auto-copy puts it on the clipboard."""
    portal = isinstance(DESKTOP, Wayland)
    root = scratch / "capture-history"
    settings = scratch / "capture-settings.json"
    settings.write_text(json.dumps({
        "settings_schema_version": 5, "onboarding_completed": True,
        "output_directory": str(scratch / "capture-exports"),
        "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
        "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10",
        "launch_at_login": False, "show_mini_previews": True, "auto_copy_to_clipboard": True}))
    still = scratch / "portal still.png"
    still.write_bytes(fixture_png(accent=(30, 200, 120)))
    pixbuf = GdkPixbuf.Pixbuf.new_from_file(str(still))
    still_size = (pixbuf.get_width(), pixbuf.get_height())
    log = scratch / "capture.log"
    process = None
    with private_desktop(scratch, "capture", still if portal else None) as (env, fixture_log):
        env = dict(env, GTK_A11Y="atspi", NO_AT_BRIDGE="0")
        try:
            with log.open("w") as stream:
                process = subprocess.Popen([binary, "--live", "--history-root", str(root),
                                            "--settings-file", str(settings), "--capture", "display"],
                                           env=env, stdout=stream)

            def events():
                return [json.loads(line) for line in log.read_text().splitlines()
                        if line.startswith("{")]
            if portal:
                wait(lambda: "request" in fixture_log.read_text(), "the portal request")
                assert not DESKTOP.visible(process.pid), "a Captures window was mapped for the portal"
            app = wait(lambda: application(process.pid), "the capture app")
            # Sway's tree excludes layer surfaces; AT-SPI covers both hosts.
            wait(lambda: find(app, "frame", "Captures New Capture"), "New Capture controls")
            assert not list(root.glob("*/metadata.json")), "Display captured before confirmation"
            if not portal:
                selector = DESKTOP.window("^Captures New Capture$", process.pid)
                DESKTOP.point_into(selector, 40, 40)
            DESKTOP.key("Return")
            captured = wait(lambda: [e for e in events() if e["event"].startswith("capture")],
                            "the capture", timeout=30)[0]
            assert captured["event"] == "captured", events()
            entry = json.loads(next(root.glob("*/metadata.json")).read_text())
            assert (entry["kind"], entry["mode"]) == ("screenshot", "display"), entry
            if portal:
                assert (entry["width"], entry["height"]) == still_size, entry
            else:
                assert entry["width"] > 0 and entry["height"] > 0, entry
            wait(lambda: find(app, "frame", "Captures Previews"), "the preview stack")
            wait(lambda: "image/png" in DESKTOP.paste_types(), "auto-copy to the clipboard")
            if portal:
                DESKTOP.window("Preferences$", process.pid)
            print(f"capture: Screenshot Display saved {entry['width']}×{entry['height']} "
                  f"{'through the portal' if portal else 'from X11'}, into the preview stack "
                  "and the clipboard")
        finally:
            if process and process.poll() is None:
                process.terminate()
                process.wait(timeout=10)


class X11Window:
    """A plain top-level X window that the shared window list sees: shipping
    reads `_NET_CLIENT_LIST_STACKING`, which this window-manager-less Xvfb
    leaves to the smoke (libX11 through ctypes, no extra tools)."""

    def __init__(self, title, x, y, width, height):
        c = ctypes
        self.x11 = x11 = c.cdll.LoadLibrary("libX11.so.6")
        x11.XOpenDisplay.restype = c.c_void_p
        x11.XOpenDisplay.argtypes = [c.c_char_p]
        x11.XDefaultRootWindow.restype = c.c_ulong
        x11.XDefaultRootWindow.argtypes = [c.c_void_p]
        x11.XCreateSimpleWindow.restype = c.c_ulong
        x11.XCreateSimpleWindow.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_int, c.c_uint,
                                            c.c_uint, c.c_uint, c.c_ulong, c.c_ulong]
        x11.XInternAtom.restype = c.c_ulong
        x11.XInternAtom.argtypes = [c.c_void_p, c.c_char_p, c.c_int]
        x11.XChangeProperty.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong, c.c_ulong, c.c_int,
                                        c.c_int, c.c_void_p, c.c_int]
        x11.XDeleteProperty.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong]
        for name in ("XMapWindow", "XDestroyWindow"):
            getattr(x11, name).argtypes = [c.c_void_p, c.c_ulong]
        x11.XSync.argtypes = [c.c_void_p, c.c_int]
        x11.XDefaultGC.restype = c.c_void_p
        x11.XDefaultGC.argtypes = [c.c_void_p, c.c_int]
        x11.XFillRectangle.argtypes = [c.c_void_p, c.c_ulong, c.c_void_p, c.c_int, c.c_int,
                                       c.c_uint, c.c_uint]
        x11.XCreatePixmap.restype = c.c_ulong
        x11.XCreatePixmap.argtypes = [c.c_void_p, c.c_ulong, c.c_uint, c.c_uint, c.c_uint]
        x11.XDefaultDepth.argtypes = [c.c_void_p, c.c_int]
        x11.XSetForeground.argtypes = [c.c_void_p, c.c_void_p, c.c_ulong]
        x11.XSetWindowBackgroundPixmap.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong]
        x11.XCloseDisplay.argtypes = [c.c_void_p]
        self.display = x11.XOpenDisplay(None)
        assert self.display, "no X display"
        self.root = x11.XDefaultRootWindow(self.display)
        self.window = x11.XCreateSimpleWindow(self.display, self.root, x, y, width, height, 0, 0,
                                              0xffffff)
        atom = lambda name: x11.XInternAtom(self.display, name, 0)  # noqa: E731
        name = title.encode()
        x11.XChangeProperty(self.display, self.window, atom(b"_NET_WM_NAME"),
                            atom(b"UTF8_STRING"), 8, 0, name, len(name))
        # Content, since a blank frame is refused as an empty capture: half
        # black, half white, as a background pixmap so the server repaints
        # it after the overlays uncover it (nothing here handles Expose).
        gc = x11.XDefaultGC(self.display, 0)
        pixmap = x11.XCreatePixmap(self.display, self.window, width, height,
                                   x11.XDefaultDepth(self.display, 0))
        x11.XSetForeground(self.display, gc, 0xffffff)
        x11.XFillRectangle(self.display, pixmap, gc, 0, 0, width, height)
        x11.XSetForeground(self.display, gc, 0)
        x11.XFillRectangle(self.display, pixmap, gc, 0, 0, width // 2, height)
        x11.XSetWindowBackgroundPixmap(self.display, self.window, pixmap)
        x11.XMapWindow(self.display, self.window)
        self.stacking = atom(b"_NET_CLIENT_LIST_STACKING")
        stack = (c.c_ulong * 1)(self.window)
        x11.XChangeProperty(self.display, self.root, self.stacking, 33, 32, 0, stack, 1)
        x11.XSync(self.display, 0)

    def close(self):
        self.x11.XDeleteProperty(self.display, self.root, self.stacking)
        self.x11.XDestroyWindow(self.display, self.window)
        self.x11.XSync(self.display, 0)
        self.x11.XCloseDisplay(self.display)


class LiveCaptures:
    """Live apps, one per `--capture` run, over one settings file, History
    and event log; a new run replaces the previous process."""

    SETTINGS = {
        "settings_schema_version": 5, "onboarding_completed": True,
        "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
        "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10",
        "launch_at_login": False, "show_mini_previews": False, "auto_copy_to_clipboard": False}

    def __init__(self, binary, scratch, name):
        self.binary, self.process = binary, None
        self.root = scratch / f"{name}-history"
        self.settings = scratch / f"{name}-settings.json"
        self.settings.write_text(json.dumps(
            dict(self.SETTINGS, output_directory=str(scratch / f"{name}-exports"))))
        self.log = scratch / f"{name}.log"
        self.log.write_text("")

    def events(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()
                if line.startswith("{")]

    def outcomes(self):
        return [e for e in self.events() if e["event"] in ("captured", "capture-cancelled",
                                                           "capture-failed")]

    def run(self, kind, env):
        """Start `--capture kind`; returns the outcome count before it."""
        self.stop()
        before = len(self.outcomes())
        self.overlays = len([e for e in self.events() if e["event"] == "selector"])
        with self.log.open("a") as stream:
            self.process = subprocess.Popen(
                [self.binary, "--live", "--history-root", str(self.root),
                 "--settings-file", str(self.settings), "--capture", kind],
                env=dict(env, GSK_RENDERER="cairo", GTK_A11Y="atspi", NO_AT_BRIDGE="0"),
                stdout=stream)
        return before

    def shown(self, kind):
        """The overlay the latest run opened."""
        return wait(lambda: [e for e in self.events() if e["event"] == "selector"][self.overlays:],
                    f"the {kind} overlay")[0]

    def outcome(self, before):
        return wait(lambda: self.outcomes()[before:], "the capture outcome", timeout=30)[0]

    def app(self):
        return wait(lambda: application(self.process.pid), "the app on the accessibility bus")

    def entry(self, artifact):
        return json.loads((self.root / artifact / "metadata.json").read_text())

    def saved(self):
        return list(self.root.glob("*/metadata.json"))

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            self.process.wait(timeout=10)


def guidance(app, title):
    return wait(lambda: [n for n, (role, name) in nodes(app)
                         if role in ("status", "status bar")
                         and [c for c, (_, text) in nodes(n) if node_text(c, text) == title]],
                f"the {title!r} guidance")


def selectors(binary, scratch):
    """Screenshot Region and Window through the shared capture flow. X11: the
    display under the pointer, frozen, in an unmanaged overlay over AT-SPI; a
    click without a drag says "Click and drag", Escape cancels, a drag saves
    exactly the selection, and the Window overlay names and saves the hovered
    window. Wayland: Region selects over the Screenshot portal's still (a
    fixture) on a fullscreen surface with every Captures window unmapped and
    saves that crop; Window goes to the portal's own window picker (refused
    by the fixture, so it cancels)."""
    portal = isinstance(DESKTOP, Wayland)
    live = LiveCaptures(binary, scratch, "selectors")
    accent = (200, 90, 160)
    still = scratch / "selector still.png"
    still.write_bytes(fixture_png(Wayland.WIDTH, Wayland.HEIGHT, accent=accent))
    # X11 uses the session's fixture; the portal needs a private bus.
    desktop = (private_desktop(scratch, "selectors", still) if portal
               else contextlib.nullcontext((dict(os.environ), FIXTURE_LOG)))
    window = None if portal else X11Window("Fixture window", 60, 240, 260, 160)

    def overlay(label):
        app = live.app()
        return app, wait(lambda: find(app, "panel", label), label)[0]

    try:
        with desktop as (env, fixture_log):
            # Region: guidance, an empty click's feedback, then Escape.
            before = live.run("region", env)
            shown = live.shown("region")
            assert (shown["kind"], shown["portal"], shown["frozen"]) == ("region", portal, True)
            if portal:
                assert '"CreateSession"' not in fixture_log.read_text()
                assert "Screenshot" in fixture_log.read_text(), "no portal still was requested"
            app, _ = overlay("Capture region selector")
            guidance(app, "Drag to select a region")
            DESKTOP.click(500, 420)
            guidance(app, "Click and drag to select a region")
            DESKTOP.key("Escape")
            assert live.outcome(before)["event"] == "capture-cancelled", live.events()
            assert not live.saved(), "a cancelled overlay saved a capture"

            # Region: a drag saves exactly the selection. On Wayland it lies
            # in the still's accent block, so its pixels prove the crop.
            before = live.run("region", env)
            overlay("Capture region selector")
            if portal:
                DESKTOP.drag(900, 60, 1200, 210)
            else:
                DESKTOP.drag(100, 280, 220, 340)
            captured = live.outcome(before)
            assert captured["event"] == "captured", live.events()
            region = live.entry(captured["id"])
            size = (300, 150) if portal else (120, 60)
            assert (region["mode"], region["width"], region["height"]) == ("region", *size), region
            if portal:
                saved = GdkPixbuf.Pixbuf.new_from_file(str(live.root / captured["id"] / "capture.png"))
                at = 75 * saved.get_rowstride() + 150 * saved.get_n_channels()
                assert tuple(saved.get_pixels()[at:at + 3]) == accent, "the crop missed the still"
            print(f"selectors: Region guidance, feedback and Escape; a drag saved "
                  f"{size[0]}×{size[1]} {'from the portal still' if portal else 'from X11'}")

            # Wayland Window counts down before asking the portal. Escape
            # here must not request consent or publish another artifact.
            if portal:
                settings = json.loads(live.settings.read_text())
                live.settings.write_text(json.dumps(dict(settings, screenshot_countdown_seconds=4)))
                saved_before = len(live.saved())
                before = live.run("window", env)
                wait(lambda: find(live.app(), "frame", "Captures Screenshot Countdown"),
                     "the pre-consent countdown")
                DESKTOP.key("Escape")
                assert live.outcome(before)["event"] == "capture-cancelled", live.events()
                assert '"CreateSession"' not in fixture_log.read_text(), "cancel requested consent"
                assert len(live.saved()) == saved_before

            # Window (after the configured countdown on Wayland).
            before = live.run("window", env)
            if portal:
                assert live.outcome(before)["event"] == "capture-cancelled", live.events()
                assert '"CreateSession"' in fixture_log.read_text(), "no window picker request"
                print("selectors: Window went to the ScreenCast window picker; refusal cancelled")
            else:
                shown = live.shown("window")
                assert shown["windows"] == 1, shown
                app, group = overlay("Capture window selector")
                subprocess.run(["xdotool", "mousemove", "190", "320"], check=True)
                wait(lambda: group.get_description() == "Target: Fixture window",
                     "the hovered window target")
                guidance(app, "Select a window to continue")
                DESKTOP.click(190, 320)
                captured = live.outcome(before)
                assert captured["event"] == "captured", live.events()
                saved = live.entry(captured["id"])
                assert (saved["mode"], saved["width"], saved["height"]) == ("window", 260, 160), \
                    saved
                print("selectors: Window named the hovered window and saved it at 260×160")
    finally:
        live.stop()
        if window:
            window.close()


def new_capture(binary, scratch):
    """New Capture in Screenshot mode: shipping's bar as real controls over
    AT-SPI (Record unavailable until the GTK host records), Close cancels,
    a drag plus Enter saves exactly the region, and Full screen saves the
    display. X11 also picks the fixture window and saves it; on Wayland the
    region crops the portal still, Full screen persists that same still
    without a second request, and Window goes to the ScreenCast picker (refused,
    so it cancels)."""
    portal = isinstance(DESKTOP, Wayland)
    live = LiveCaptures(binary, scratch, "new-capture")
    accent = (90, 160, 200)
    still = scratch / "new capture still.png"
    still.write_bytes(fixture_png(Wayland.WIDTH, Wayland.HEIGHT, accent=accent))
    desktop = (private_desktop(scratch, "new-capture", still) if portal
               else contextlib.nullcontext((dict(os.environ), FIXTURE_LOG)))
    window = None if portal else X11Window("Fixture window", 60, 240, 260, 160)
    enabled = Atspi.StateType.SENSITIVE

    def bar(before):
        assert live.shown("new-capture")["kind"] == "new-capture"
        # Its own frame: the live launch also opens Preferences.
        app = wait(lambda: find(live.app(), "frame", "Captures New Capture"), "New Capture")[0]
        primary = wait(lambda: find(app, "push button", "Take screenshot"),
                       "the Take screenshot button")[0]
        return app, primary

    def segment(app, name):
        # A toggle button from GTK 4.10; 4.8 reports a push button.
        return wait(lambda: find(app, "toggle button", name) or find(app, "push button", name),
                    name)[0]

    def target(app, name):
        press(segment(app, name))

    def capture(before, size, mode):
        captured = live.outcome(before)
        assert captured["event"] == "captured", live.events()
        saved = live.entry(captured["id"])
        assert (saved["mode"], saved["width"], saved["height"]) == (mode, *size), saved
        return captured

    try:
        with desktop as (env, fixture_log):
            # The bar: shipping's controls, then Close cancels.
            before = live.run("new", env)
            app, primary = bar(before)
            for name in ("Screenshot", "Region", "Window", "Full screen"):
                assert has_state(segment(app, name), enabled), name
            for name in ("Screenshot", "Region"):
                assert has_state(segment(app, name), Atspi.StateType.PRESSED) or has_state(
                    segment(app, name), Atspi.StateType.CHECKED), f"{name} not pressed"
            assert not has_state(segment(app, "Record"), enabled), "Record before recording"
            assert [n for n, (role, name) in nodes(app) if role == "combo box"
                    and name.startswith("Region aspect ratio")], "no Aspect select"
            assert not has_state(primary, enabled), "Take screenshot before a region"
            guidance(app, "Drag to select a region")
            assert_accessible(app, "New Capture")
            press(find(app, "push button", "Close capture controls")[0])
            assert live.outcome(before)["event"] == "capture-cancelled", live.events()
            assert not live.saved(), "Close saved a capture"

            # Region: a drag enables Take screenshot; Enter saves it.
            before = live.run("new", env)
            app, primary = bar(before)
            if portal:
                DESKTOP.drag(900, 60, 1200, 210)
            else:
                DESKTOP.drag(100, 280, 220, 340)
            wait(lambda: has_state(primary, enabled), "Take screenshot after a region")
            DESKTOP.key("Return")
            captured = capture(before, (300, 150) if portal else (120, 60), "region")
            if portal:
                saved = GdkPixbuf.Pixbuf.new_from_file(str(live.root / captured["id"] / "capture.png"))
                at = 75 * saved.get_rowstride() + 150 * saved.get_n_channels()
                assert tuple(saved.get_pixels()[at:at + 3]) == accent, "the crop missed the still"

            # Full screen: the display card, then Take screenshot. On X11 the
            # saved screenshot chords switch the open bar in place (no "already
            # in progress"), and Enter on the clicked, focused Full screen
            # segment confirms instead of re-activating it.
            def registrations():
                return [e for e in live.events() if e["event"] == "shortcuts" and e["ok"]]
            registered = len(registrations())
            before = live.run("new", env)
            app, primary = bar(before)
            requests = fixture_log.read_text().count('"method": "Screenshot"')

            def pressed(name):
                node = segment(app, name)
                return has_state(node, Atspi.StateType.PRESSED) or has_state(
                    node, Atspi.StateType.CHECKED)
            if portal:
                target(app, "Full screen")
            else:
                wait(lambda: registrations()[registered:], "shortcut registration")
                DESKTOP.key("ctrl+shift+F9")
                wait(lambda: pressed("Full screen"), "the Display chord to pick Full screen")
                DESKTOP.key("ctrl+shift+F8")
                wait(lambda: pressed("Window"), "the Window chord to pick Window")
                assert not [e for e in live.events() if e["event"] == "capture-failed"], \
                    live.events()
                extents = segment(app, "Full screen").get_extents(Atspi.CoordType.WINDOW)
                DESKTOP.click(extents.x + extents.width // 2, extents.y + extents.height // 2)
                wait(lambda: pressed("Full screen"), "the clicked Full screen segment")
            wait(lambda: [n for n, (role, name) in nodes(app) if role == "combo box"
                          and name.startswith("Display")], "the Display select")
            wait(lambda: has_state(primary, enabled), "Take screenshot for Full screen")
            if portal:
                press(primary)
            else:
                assert has_state(segment(app, "Full screen"), Atspi.StateType.FOCUSED)
                DESKTOP.key("Return")
            display = (Wayland.WIDTH, Wayland.HEIGHT) if portal else (1280, 1024)
            captured = capture(before, display, "display")
            if portal:
                assert fixture_log.read_text().count('"method": "Screenshot"') == requests, \
                    "Full screen requested a second Screenshot grant"
                saved = GdkPixbuf.Pixbuf.new_from_file(str(live.root / captured["id"] / "capture.png"))
                original = GdkPixbuf.Pixbuf.new_from_file(str(still))
                # The RGB fixture becomes an RGBA History PNG; compare every
                # decoded pixel with an opaque alpha channel on both sides.
                assert saved.add_alpha(False, 0, 0, 0).get_pixels() == original.add_alpha(
                    False, 0, 0, 0).get_pixels(), "Full screen changed the still"

            if not portal:
                # Repeating Display freezes the visible controls into the next
                # still even with freeze off and a countdown configured. Compare
                # an asymmetric crop containing the bar with the visible pixels.
                settings = json.loads(live.settings.read_text())
                live.settings.write_text(json.dumps(dict(settings, freeze_screen=False,
                                                         screenshot_countdown_seconds=2)))
                before = live.run("display", env)
                app, primary = bar(before)
                wait(lambda: any(e["event"] == "shortcuts" and e["ok"]
                                 for e in live.events()), "the Display binding")
                DESKTOP.point_into(DESKTOP.window("^Captures New Capture$", live.process.pid), 40, 40)
                time.sleep(0.3)
                visible = scratch / "visible-controls.png"
                subprocess.run(["import", "-window", "root", str(visible)], check=True)
                overlays = len([e for e in live.events() if e["event"] == "selector"])
                DESKTOP.key("ctrl+shift+F9")
                wait(lambda: len([e for e in live.events() if e["event"] == "selector"]) > overlays,
                     "repeated Display to freeze the controls")
                assert len(live.outcomes()) == before, "repeat captured without confirmation"
                app, primary = bar(before)
                countdowns = len([e for e in live.events() if e["event"] == "countdown"])
                press(primary)
                captured = capture(before, display, "display")
                assert len([e for e in live.events() if e["event"] == "countdown"]) == countdowns, \
                    "explicit frozen capture showed a countdown"
                expected = GdkPixbuf.Pixbuf.new_from_file(str(visible))
                saved = GdkPixbuf.Pixbuf.new_from_file(str(live.root / captured["id"] / "capture.png"))
                # Not a blank desktop: include the left controls and their text,
                # away from the pointer and focus-dependent primary action.
                for pixels in (expected, saved):
                    assert pixels.get_n_channels() in (3, 4)
                def crop(pixels):
                    data, stride, channels = (pixels.get_pixels(), pixels.get_rowstride(),
                                              pixels.get_n_channels())
                    return bytes(v for y in range(920, 970) for x in range(220, 650)
                                 for v in data[y * stride + x * channels:][:3])
                assert crop(saved) == crop(expected), "repeat lost the visible controls pixels"

                # Cancel a confirmed countdown, restore its hidden preview,
                # then capture again in the same resident process.
                live.settings.write_text(json.dumps(dict(settings, show_mini_previews=True,
                                                         screenshot_countdown_seconds=4)))
                before = live.run("display", env)
                app, primary = bar(before)
                press(primary)
                capture(before, display, "display")
                preview = DESKTOP.window("^Captures Previews$", live.process.pid)
                saved_before = len(live.saved())
                for cancel in (True, False):
                    before = len(live.outcomes())
                    shown = len([e for e in live.events() if e["event"] == "selector"])
                    DESKTOP.key("ctrl+shift+F9")
                    wait(lambda: len([e for e in live.events() if e["event"] == "selector"]) > shown,
                         "a fresh Display selection after confirmation")
                    assert preview not in DESKTOP.visible(live.process.pid), "preview not hidden"
                    app, primary = bar(before)
                    press(primary)
                    countdown = wait(lambda: find(live.app(), "frame", "Captures Screenshot Countdown"),
                                     "the visible countdown", timeout=3)[0]
                    initial = wait(lambda: [int(node_text(n, name)) for n, (role, name) in nodes(countdown)
                                            if role == "label" and node_text(n, name).isdigit()],
                                   "countdown's remaining seconds")[0]
                    assert 1 < initial <= 4, initial
                    assert find(countdown, "alert", f"Screenshot in {initial}. Press Esc to cancel")
                    assert find(countdown, "label", "Press Esc to cancel")
                    assert_accessible(countdown, "Screenshot countdown")
                    if cancel:
                        DESKTOP.key("Escape")
                        assert live.outcome(before)["event"] == "capture-cancelled", live.events()
                        assert len(live.saved()) == saved_before, "cancelled countdown persisted"
                        wait(lambda: preview in DESKTOP.visible(live.process.pid),
                             "hidden preview restored after cancellation")
                        background = scratch / "countdown-background.png"
                        subprocess.run(["import", "-window", "root", str(background)], check=True)
                    else:
                        wait(lambda: any(node_text(n, name) == str(initial - 1)
                                         for n, (_, name) in nodes(countdown)),
                             "countdown's next second")
                        captured = capture(before, display, "display")
                        assert len(live.saved()) == saved_before + 1
                        saved = GdkPixbuf.Pixbuf.new_from_file(
                            str(live.root / captured["id"] / "capture.png"))
                        expected = GdkPixbuf.Pixbuf.new_from_file(str(background))
                        # Independent desktop pixels, away from previews: the
                        # countdown's centre and scrim must have left the screen.
                        for x, y in ((250, 320), (640, 512)):
                            at = y * saved.get_rowstride() + x * saved.get_n_channels()
                            ref = y * expected.get_rowstride() + x * expected.get_n_channels()
                            assert saved.get_pixels()[at:at + 3] == expected.get_pixels()[ref:ref + 3], \
                                "countdown pixels leaked into the capture"
                    assert not find(live.app(), "frame", "Captures Screenshot Countdown"), \
                        "countdown remained after completion"
                live.settings.write_text(json.dumps(settings))

            # Window.
            before = live.run("new", env)
            app, primary = bar(before)
            target(app, "Window")
            guidance(app, "Select a window to continue")
            if portal:
                press(primary)
                assert live.outcome(before)["event"] == "capture-cancelled", live.events()
                assert '"CreateSession"' in fixture_log.read_text(), "no window picker request"
                # The session can lock after consent but before confirmation.
                # Neither the selector watcher nor persistence may publish it.
                before = live.run("display", env)
                bar(before)
                saved_before = len(live.saved())
                lock = fixture_log.with_suffix(".locked")
                lock.touch()
                try:
                    DESKTOP.key("Return")
                    assert live.outcome(before)["event"] in ("capture-cancelled", "capture-failed")
                    assert len(live.saved()) == saved_before, "locked portal still reached History"
                finally:
                    lock.unlink()
            else:
                assert not has_state(primary, enabled), "Take screenshot before a window"
                DESKTOP.click(190, 320)
                wait(lambda: has_state(primary, enabled), "Take screenshot after a window")
                press(primary)
                capture(before, (260, 160), "window")

                # Without a window list (no _NET_CLIENT_LIST_STACKING) the
                # bar still opens, Window is disabled and Full screen saves.
                window.close()
                window = None
                before = live.run("new", env)
                app, primary = bar(before)
                assert not has_state(segment(app, "Window"), enabled), "Window without windows"
                target(app, "Full screen")
                wait(lambda: has_state(primary, enabled), "Take screenshot for Full screen")
                press(primary)
                capture(before, display, "display")
            print("new_capture: bar over AT-SPI, Close cancelled; Region (Enter), Full screen "
                  + ("saved the exact portal still without another grant; lock-after-consent "
                     "saved nothing; Window went to the ScreenCast picker"
                     if portal else "(chords switched the bar in place, Enter on the focused "
                     "segment, repeat preserved controls pixels, countdown cancellation "
                     "restored previews and allowed another capture) and the picked window "
                     "saved from X11; without a window "
                     "list Window was disabled and Full screen still saved"))
    finally:
        live.stop()
        if window:
            window.close()


def tray_menu(service):
    """The tray item's dbusmenu: its rows as (id, label, enabled), and a
    function that clicks a row by label, as a panel would."""
    from gi.repository import Gio
    bus = Gio.bus_get_sync(Gio.BusType.SESSION)
    name, _, path = service.partition("/")
    item = Gio.DBusProxy.new_sync(bus, Gio.DBusProxyFlags.NONE, None, name,
                                  "/" + path if path else "/StatusNotifierItem",
                                  "org.kde.StatusNotifierItem")
    menu = item.get_cached_property("Menu").unpack()
    reply = bus.call_sync(name, menu, "com.canonical.dbusmenu", "GetLayout",
                          GLib.Variant("(iias)", (0, -1, [])), None, 0, 5000).unpack()
    rows = []
    for child in reply[1][2]:
        identifier, props, _ = child
        rows.append((identifier, props.get("label", "-" if props.get("type") == "separator"
                                           else ""), props.get("enabled", True)))

    def click(label, exits=False):
        identifier = next(i for i, text, _ in rows if text == label)
        try:
            bus.call_sync(name, menu, "com.canonical.dbusmenu", "Event",
                          GLib.Variant("(isvu)", (identifier, "clicked", GLib.Variant("s", ""), 0)),
                          None, 0, 5000)
        except GLib.Error:
            # Quit can exit before ksni replies; panels do not wait for it.
            if not exits:
                raise
    return rows, click


def tray(binary, scratch):
    """The live app's tray: shipping's menu rows in order (captures the GTK
    host cannot run yet disabled), and real dbusmenu clicks for Capture
    History…, Screenshot Display (X11; Wayland's portal is the capture step's)
    and Quit Captures."""
    root = scratch / "tray-history"
    settings = scratch / "tray-settings.json"
    settings.write_text(json.dumps({
        "settings_schema_version": 5, "onboarding_completed": True,
        "output_directory": str(scratch / "tray-exports"),
        "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
        "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10",
        "launch_at_login": False, "show_mini_previews": False, "auto_copy_to_clipboard": False}))
    log = scratch / "tray.log"
    env = dict(os.environ, GSK_RENDERER="cairo", GTK_A11Y="atspi", NO_AT_BRIDGE="0")
    with log.open("w") as stream:
        process = subprocess.Popen([binary, "--live", "--captures-autostart", "--history-root",
                                    str(root), "--settings-file", str(settings)], env=env,
                                   stdout=stream)

    def events():
        return [json.loads(line) for line in log.read_text().splitlines() if line.startswith("{")]
    try:
        wait(lambda: any(e["event"] == "tray" for e in events()), "the tray")
        service = wait(lambda: [json.loads(line) for line in FIXTURE_LOG.read_text().splitlines()
                                if json.loads(line).get("event") == "item"
                                and json.loads(line)["sender"]], "the tray item")[-1]["service"]
        rows, click = wait(lambda: tray_menu(service), "the tray menu")
        labels = [label for _, label, _ in rows]
        assert labels == ["New Capture…", "Screenshot Region", "Screenshot Window",
                          "Screenshot Display", "Record Region", "Record Window",
                          "Record Display", "-", "Capture History…", "Open Save Location",
                          "Preferences", "Send Feedback…", "Check for Updates…", "-",
                          "Quit Captures"], labels
        enabled = [label for _, label, on in rows if on and label != "-"]
        assert enabled == ["New Capture…", "Screenshot Region", "Screenshot Window",
                           "Screenshot Display", "Capture History…", "Open Save Location",
                           "Preferences", "Send Feedback…", "Quit Captures"], enabled
        # The quiet launch shows only the launch notice, which leaves by itself.
        wait(lambda: {"event": "startup-notice-closed", "by": "timeout"} in events(),
             "the launch notice to expire", timeout=12)
        assert DESKTOP.visible(process.pid) == [], "the quiet launch left a window open"
        click("Capture History…")
        DESKTOP.window("^Capture History$", process.pid)
        if not isinstance(DESKTOP, Wayland):
            click("Screenshot Display")
            selector = DESKTOP.window("^Captures New Capture$", process.pid)
            assert not list(root.glob("*/metadata.json")), "tray Display bypassed the controls"
            DESKTOP.point_into(selector, 40, 40)
            DESKTOP.key("Return")
            wait(lambda: [e for e in events() if e["event"] == "captured"], "the tray capture")
            assert len(list(root.glob("*/metadata.json"))) == 1
        click("Quit Captures", exits=True)
        assert process.wait(timeout=10) == 0
        print(f"tray: {len(rows)} rows in shipping order; History"
              f"{', Screenshot Display' if not isinstance(DESKTOP, Wayland) else ''} "
              "and Quit through dbusmenu")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()


def setup(binary, scratch):
    """First-run setup in the live app: a fresh profile opens setup (not
    Preferences) with Linux's ready screen access; the tray shows setup
    until it finishes; Start capturing saves completion, closes the window
    and turns the global shortcuts on."""
    root = scratch / "setup-history"
    settings = scratch / "setup-settings.json"
    log = scratch / "setup.log"
    env = dict(os.environ, GSK_RENDERER="cairo", GTK_A11Y="atspi", NO_AT_BRIDGE="0")
    with log.open("w") as stream:
        process = subprocess.Popen([binary, "--live", "--history-root", str(root),
                                    "--settings-file", str(settings)], env=env, stdout=stream)

    def events(name):
        return [json.loads(line) for line in log.read_text().splitlines()
                if line.startswith("{") and json.loads(line)["event"] == name]
    try:
        assert wait(lambda: events("launch"), "the launch")[0]["action"] == "Setup"
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        window = wait(lambda: [n for n, (role, name) in nodes(app)
                               if role in ("frame", "window", "filler") and name == "Captures"],
                      "the setup window")[0]
        assert not events("shortcuts"), "shortcuts were live before setup finished"
        state = wait(lambda: events("setup"), "the setup state")[0]
        assert state == {"event": "setup", "title": "You’re ready to capture", "ready": True}, state
        wait(lambda: [n for n, (_, name) in nodes(window) if name == "Ready"], "Ready status")
        assert_accessible(window, "setup")
        start = wait(lambda: [n for n in find(window, "push button", "Start capturing")
                              if has_state(n, Atspi.StateType.SENSITIVE)], "Start capturing")[0]
        press(start)
        wait(lambda: events("setup-complete"), "setup to finish")
        assert json.loads(settings.read_text())["onboarding_completed"] is True
        registered = wait(lambda: events("shortcuts"), "shortcuts after setup")[0]
        assert registered["ok"], registered
        # Finishing setup shows the launch notice for 15 s, as shipping.
        notice = wait(lambda: events("startup-notice"), "the launch notice after setup")[0]
        assert notice["visibleMs"] == 15000, notice
        wait(lambda: not [n for n, (_, name) in nodes(app) if name == "Captures"],
             "the setup window to close")
        assert process.poll() is None, "finishing setup quit the app"
        print("setup: a fresh profile opened setup; Start capturing saved it, enabled "
              "shortcuts and showed the 15 s launch notice")
    finally:
        process.terminate()
        process.wait(timeout=10)


def shortcuts(binary, scratch):
    """Global capture shortcuts in the live app. X11: the saved Screenshot
    Display chord captures; after Preferences' file changes to another chord,
    that one captures and the old one no longer does. Wayland: the shared
    GlobalShortcuts portal owner starts without registering X11 keys (this
    headless Sway's portal has no GlobalShortcuts, so nothing is bound)."""
    root = scratch / "shortcut-history"
    settings = scratch / "shortcut-settings.json"
    values = {
        "settings_schema_version": 5, "onboarding_completed": True,
        "output_directory": str(scratch / "shortcut-exports"),
        "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
        "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10",
        "launch_at_login": False, "show_mini_previews": False, "auto_copy_to_clipboard": False}
    settings.write_text(json.dumps(values))
    log = scratch / "shortcuts.log"
    env = dict(os.environ, GSK_RENDERER="cairo", GTK_A11Y="none")
    with log.open("w") as stream:
        process = subprocess.Popen([binary, "--live", "--captures-autostart", "--history-root",
                                    str(root), "--settings-file", str(settings)], env=env,
                                   stdout=stream)

    def events(name):
        return [json.loads(line) for line in log.read_text().splitlines()
                if line.startswith("{") and json.loads(line)["event"] == name]

    def captures():
        return len(list(root.glob("*/metadata.json")))
    try:
        registered = wait(lambda: events("shortcuts"), "shortcut registration")[0]
        assert registered["ok"], registered
        if isinstance(DESKTOP, Wayland):
            print("shortcuts: the GlobalShortcuts portal owner started on Wayland")
            return
        DESKTOP.key("ctrl+shift+F9")
        selector = DESKTOP.window("^Captures New Capture$", process.pid)
        assert captures() == 0, "Display shortcut bypassed the controls"
        DESKTOP.point_into(selector, 40, 40)
        DESKTOP.key("Return")
        wait(lambda: captures() == 1, "Ctrl+Shift+F9 to capture the display")
        settings.with_suffix(".tmp").write_text(json.dumps(dict(values,
                                                                display_shortcut="Ctrl+Shift+F6")))
        settings.with_suffix(".tmp").replace(settings)
        wait(lambda: len(events("shortcuts")) == 2, "the new chord to register")
        DESKTOP.key("ctrl+shift+F9")
        time.sleep(1.5)
        assert captures() == 1, "the old chord still captured"
        DESKTOP.key("ctrl+shift+F6")
        selector = DESKTOP.window("^Captures New Capture$", process.pid)
        assert captures() == 1, "rebound Display shortcut bypassed the controls"
        DESKTOP.point_into(selector, 40, 40)
        DESKTOP.key("Return")
        wait(lambda: captures() == 2, "Ctrl+Shift+F6 to capture the display")
        print("shortcuts: the saved Display chord captured; a changed chord took over")
    finally:
        process.terminate()
        process.wait(timeout=10)


def recording(binary, scratch):
    root = scratch / "recording-history"
    write_video_entry(root, seconds=4)
    exports = root.with_name(root.name + "-exports")
    original = set(exports.iterdir())
    source_bytes = next(iter(original)).read_bytes()
    process = launch(binary, "--scene", "history", "--history-root", str(root),
                     "--settings-file", str(scratch / "preferences.json"), "--size", "1020x720")
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        press(wait(lambda: find(app, "push button", "Open video in editor"), "video Edit")[0])
        # GTK 4.8 can expose a new top level as "filler"; match it by title.
        editor = wait(lambda: [n for n, (_, name) in nodes(app) if name == "Captures Editor"],
                      "the recording editor")[0]
        start = wait(lambda: find(editor, "slider", "Trim start"), "the Trim start slider")[0]
        assert_accessible(editor, "the recording editor")
        end = find(editor, "slider", "Trim end")[0]
        wait(lambda: Atspi.Value.get_maximum_value(end) == 4000, "the 4 s duration")
        assert Atspi.Value.get_current_value(end) == 4000, "Trim end is not at the end"
        # Focus starts on Play; Tab reaches Trim start, Shift+Right moves it 1 s.
        # Without a window manager, keys go to the window under the pointer.
        DESKTOP.point_into(DESKTOP.window("Captures Editor"), 300, 20)
        DESKTOP.key("Tab", "shift+Right")
        wait(lambda: Atspi.Value.get_current_value(start) == 1000, "Trim start at 1 s")
        play = find(editor, "push button", "Play preview")[0]
        press(play)
        wait(lambda: play.get_name() == "Pause preview", "playback to start")
        press(play)
        wait(lambda: play.get_name() == "Play preview", "playback to pause")
        switch = [n for role in ("switch", "check box", "toggle button")
                  for n in find(editor, role, "Save as new file")][0]
        press(switch)
        wait(lambda: has_state(switch, Atspi.StateType.CHECKED), "Save as new file on")
        save = find(editor, "push button", "Save")[0]
        wait(lambda: has_state(save, Atspi.StateType.SENSITIVE), "Save to enable")
        press(save)
        # Save as new names the copy `<source>-edited`; exports land in a
        # hidden temporary file first, so wait for the final name.
        written = exports / f"{next(iter(original)).stem}-edited.mp4"
        wait(written.exists, "Save to write the -edited MP4", timeout=60)
        seconds = float(subprocess.run(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0",
             str(written)], capture_output=True, text=True, check=True).stdout)
        assert abs(seconds - 3.0) < 0.2, f"saved {seconds:.2f} s, expected the 3 s trim"
        assert next(iter(original)).read_bytes() == source_bytes, "Save as new changed the source"
        print(f"recording: keyboard trim, play/pause, saved {written.name} ({seconds:.2f} s)")
    finally:
        process.terminate()
        process.wait(timeout=10)


def notices(binary, scratch):
    """The update notice on shipping's fixtures over AT-SPI: Hide persists
    `show_update_changelog` and What's new restores the notes; Update now
    downloads, reaches the restart state and closes (the shared stub never
    installs anything); an error's Close dismisses it."""
    settings = scratch / "notice-settings.json"
    settings.write_text(json.dumps({
        "settings_schema_version": 5, "onboarding_completed": True, "appearance": "light",
        "output_directory": str(scratch / "notice-exports"), "region_shortcut": "Ctrl+Shift+F7",
        "window_shortcut": "Ctrl+Shift+F8", "display_shortcut": "Ctrl+Shift+F9",
        "launch_at_login": False, "show_mini_previews": False, "auto_copy_to_clipboard": False,
        "show_update_changelog": True}))
    process = launch(binary, "--scene", "update", "--update", "available",
                     "--settings-file", str(settings))
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        dialog = wait(lambda: find(app, "dialog", "Update available"), "the update notice")[0]
        assert_accessible(dialog, "update notice")
        # Pull requests are real buttons inside the note text.
        wait(lambda: find(dialog, "push button", "Open pull request 265"),
             "the pull request button in the notes")
        press(wait(lambda: find(app, "push button", "Hide"), "Hide")[0])
        reveal = wait(lambda: find(app, "push button", "What’s new"), "What's new")[0]
        assert json.loads(settings.read_text())["show_update_changelog"] is False
        press(reveal)
        wait(lambda: find(app, "push button", "Hide"), "the notes to return")
        assert json.loads(settings.read_text())["show_update_changelog"] is True
        press(find(app, "push button", "Update now")[0])
        wait(lambda: [n for n, (role, _) in nodes(app) if role == "progress bar"],
             "download progress")
        wait(lambda: find(app, "dialog", "Updated"), "the restart state", timeout=15)
        assert process.wait(timeout=10) == 0, "the notice did not close after the restart"
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=10)
    process = launch(binary, "--scene", "update", "--update", "error",
                     "--settings-file", str(settings))
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        dialog = wait(lambda: find(app, "dialog", "Update failed"), "the error notice")[0]
        assert_accessible(dialog, "update error notice")
        assert [n for n, (role, _) in nodes(dialog) if role == "alert"], "no error alert"
        assert find(dialog, "push button", "download from captur.es"), "no download fallback"
        assert find(dialog, "push button", "Try again"), "no Try again"
        press(find(dialog, "push button", "Close")[0])
        assert process.wait(timeout=10) == 0, "Close did not dismiss the notice"
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=10)
    print("notices: update notice notes, download, restart and dismissal over AT-SPI")


def crash_review(binary, scratch):
    """Crash diagnostics: a live app killed without its clean exit opens
    Send Feedback with the local review on the next visible launch. Add to
    message appends the summary, Dismiss clears the evidence, and a later
    clean quit leaves nothing to review."""
    root = scratch / "crash-history"
    settings = scratch / "crash-settings.json"
    settings.write_text(json.dumps({
        "settings_schema_version": 5, "onboarding_completed": True, "appearance": "light",
        "theme": "cobalt", "output_directory": str(scratch / "crash-exports"),
        "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
        "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10",
        "launch_at_login": False, "show_mini_previews": False, "auto_copy_to_clipboard": False}))
    env = dict(os.environ, GSK_RENDERER="cairo", GTK_A11Y="atspi", NO_AT_BRIDGE="0",
               CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER="1")
    log = scratch / "crash.log"

    def start(*extra):
        with log.open("w") as stream:
            process = subprocess.Popen([binary, "--live", "--history-root", str(root),
                                        "--settings-file", str(settings), *extra],
                                       env=env, stdout=stream)
        wait(lambda: [e for e in events() if e["event"] == "ready"], "live readiness")
        return process

    def events():
        return [json.loads(line) for line in log.read_text().splitlines() if line.startswith("{")]

    def review_requested():
        return next(e for e in events() if e["event"] == "launch")["crashReview"]

    def stop(process):
        process.terminate()
        assert process.wait(timeout=10) == 0, "SIGTERM did not quit cleanly"

    process = start("--captures-autostart")
    process.kill()
    process.wait(timeout=10)
    process = start()
    try:
        assert review_requested(), events()
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        form = wait(lambda: [n for n, (_, name) in nodes(app) if name == "Send Feedback"],
                    "Send Feedback with the review")[0]
        wait(lambda: [n for n, (role, name) in nodes(form) if role == "heading"
                      and node_text(n, name) == "Previous session did not close normally"],
             "the review title")
        assert_accessible(form, "Send Feedback")
        message = [n for n, (role, name) in nodes(form) if name == "Message"
                   and role != "label" and "Text" in n.get_interfaces()][0]
        assert Atspi.Text.get_text(message, 0, -1) == "", "the review was added unasked"
        press(find(form, "push button", "Add to message")[0])
        wait(lambda: Atspi.Text.get_text(message, 0, -1).startswith(
            "Previous session did not close normally\n"), "the summary in the message")
        press(find(form, "push button", "Dismiss diagnostics")[0])
        wait(lambda: not find(form, "push button", "Dismiss diagnostics"), "the review to close")
    finally:
        stop(process)
    process = start()
    try:
        assert not review_requested(), "dismissed evidence came back"
    finally:
        stop(process)
    process = start()
    try:
        assert not review_requested(), "a clean quit left evidence"
    finally:
        stop(process)
    print("crash review: a killed session opened its review; Add to message and Dismiss "
          "worked; clean quits left nothing")


def recording_controls(binary, scratch):
    """Real X11 recording through the opt-in HUD: separate segments survive
    pause/resume and child screenshots; cancellation/discard publish nothing.
    The green desktop shown only while paused must never enter the MP4."""
    if isinstance(DESKTOP, Wayland):
        print("recording_controls: X11-only development entry point; Wayland not connected")
        return
    profile = scratch / "recording-controls"
    profile.mkdir()
    root = profile / "history"
    settings = profile / "settings.json"
    values = {"settings_schema_version": 5, "onboarding_completed": True,
              "output_directory": str(profile / "exports"),
              "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
              "display_shortcut": "Ctrl+Shift+F9",
              "launch_at_login": False, "show_mini_previews": False,
              "auto_copy_to_clipboard": False, "screenshot_countdown_seconds": 0,
              "new_capture_shortcut": "Ctrl+Shift+F10", "freeze_screen": False,
              "recording": {"video_fps": 15, "countdown_seconds": 0,
                            "show_cursor": False, "open_editor_after_recording": False}}
    settings.write_text(json.dumps(values))
    log = profile / "events.log"

    def events():
        return [json.loads(line) for line in log.read_text().splitlines() if line.startswith("{")]

    def event(name):
        return [e for e in events() if e["event"] == name]

    def background(color):
        subprocess.run(["hsetroot", "-solid", color], check=True, stdout=subprocess.DEVNULL)

    def start(env):
        with log.open("w") as stream:
            process = subprocess.Popen([binary, "--live", "--captures-autostart",
                                        "--history-root", str(root), "--settings-file", str(settings),
                                        "--capture", "record-display"], env=env, stdout=stream)
        return process

    def button(app, name):
        # GTK 4.8 repeats stock dialog button labels (as in choose_file).
        return wait(lambda: [n for n, (role, text) in nodes(app)
                             if role == "push button" and text in (name, f"{name} {name}")
                             if has_state(n, Atspi.StateType.SENSITIVE)], name)[0]

    def stop(process):
        if process.poll() is None:
            process.terminate()
        assert process.wait(timeout=45) == 0, log.read_text()

    with private_desktop(scratch, "recording-controls") as (env, _):
        env = dict(env, GTK_A11Y="atspi", NO_AT_BRIDGE="0")
        background("#d02030")
        process = start(env)
        try:
            app = wait(lambda: application(process.pid), "recording app")
            pause = button(app, "Pause recording")
            assert_accessible(app, "Recording HUD")
            microphone = find(app, "push button", "Microphone unavailable because no microphone was selected")[0]
            assert not has_state(microphone, Atspi.StateType.SENSITIVE)
            # Escape after acceptance is no longer a discard command.
            DESKTOP.key("Escape")
            time.sleep(1.1)
            press(pause)
            resume = button(app, "Resume recording")
            background("#20d030")
            paused = wait(lambda: [e for e in event("recording-state") if e["state"] == "paused"], "paused state")[-1]["elapsedMs"]
            time.sleep(1.2)
            assert event("recording-state")[-1]["elapsedMs"] == paused, "paused timer advanced"
            background("#2030d0")
            press(resume)
            button(app, "Pause recording")
            # A screenshot borrows Escape without cancelling the recording.
            press(button(app, "Take a region screenshot"))
            selector = DESKTOP.window("^Captures Region Selector$", process.pid)
            DESKTOP.point_into(selector, 40, 40)
            DESKTOP.key("Escape")
            wait(lambda: event("capture-cancelled"), "cancelled child screenshot")
            button(app, "Pause recording")
            press(button(app, "Hide recording controls"))
            wait(lambda: not find(app, "frame", "Captures Recording Controls"), "hidden HUD")
            DESKTOP.key("ctrl+shift+F10")
            button(app, "Pause recording")
            time.sleep(1.1)
            press(button(app, "Stop recording"))
            saved = wait(lambda: event("recording-saved"), "saved MP4", timeout=45)[0]
            folder = root / saved["id"]
            media = next(folder.glob("*.mp4"))
            info = json.loads(subprocess.run(["ffprobe", "-v", "error", "-show_streams",
                                             "-of", "json", str(media)], capture_output=True,
                                            text=True, check=True).stdout)
            video = next(s for s in info["streams"] if s["codec_type"] == "video")
            assert video["width"] > 0 and video["height"] > 0
            pixels = subprocess.run(["ffmpeg", "-v", "error", "-i", str(media), "-vf",
                                     "crop=2:2:20:400,scale=1:1", "-pix_fmt", "rgb24",
                                     "-f", "rawvideo", "-"], capture_output=True, check=True).stdout
            frames = list(zip(pixels[0::3], pixels[1::3], pixels[2::3], strict=True))
            assert len(frames) >= 15, "recording was empty/too short"
            assert any(r > 150 and b < 80 for r, g, b in frames), "missing pre-pause red frames"
            assert any(b > 150 and r < 80 for r, g, b in frames), "missing resumed blue frames"
            assert not any(g > 150 and r < 80 and b < 80 for r, g, b in frames), "paused green leaked into MP4"
            assert not list((profile / "recording-recovery").glob("*/manifest.json"))
            print(f"recording_controls: {len(frames)} decoded frames, red→blue with no paused green; "
                  "Escape, child cancellation, Hide/shortcut restore and Stop passed")
        finally:
            stop(process)

        # Destructive controls ask first; Cancel keeps the take, Restart resets
        # it through a fresh countdown, and confirmed Delete publishes nothing.
        count = len(list(root.glob("*/metadata.json")))
        process = start(env)
        try:
            app = wait(lambda: application(process.pid), "second recording app")
            press(button(app, "Restart recording"))
            press(button(app, "Cancel"))
            button(app, "Pause recording")
            press(button(app, "Restart recording"))
            press(button(app, "Restart"))
            button(app, "Pause recording")
            press(button(app, "Delete recording"))
            press(button(app, "Cancel"))
            button(app, "Pause recording")
            press(button(app, "Delete recording"))
            press(button(app, "Delete"))
            wait(lambda: event("recording-discarded"), "discarded take")
            assert len(list(root.glob("*/metadata.json"))) == count
            assert not list((profile / "recording-recovery").glob("*/manifest.json"))
        finally:
            stop(process)

        values["recording"]["countdown_seconds"] = 3
        settings.write_text(json.dumps(values))
        process = start(env)
        try:
            countdown = DESKTOP.window("^Captures Recording Countdown$", process.pid)
            DESKTOP.point_into(countdown, 40, 40)
            DESKTOP.key("Escape")
            wait(lambda: event("recording-discarded"), "cancelled countdown")
            assert not any(e.get("state") == "recording" for e in events())
            assert len(list(root.glob("*/metadata.json"))) == count
            assert not list((profile / "recording-recovery").glob("*/manifest.json"))
        finally:
            stop(process)

        values["recording"]["countdown_seconds"] = 0
        settings.write_text(json.dumps(values))
        process = start(env)
        try:
            app = wait(lambda: application(process.pid), "quit-save app")
            button(app, "Pause recording")
            time.sleep(1.2)
            press(button(app, "Pause recording"))
            resume = button(app, "Resume recording")
            # A segment-open failure must preserve the paused take and its
            # Stop/Resume controls, not falsely offer destructive Retry.
            bundle = next((profile / "recording-recovery").glob("*/manifest.json")).parent
            held = bundle.with_name(bundle.name + ".held")
            bundle.rename(held)
            bundle.write_text("fixture: segment directory unavailable")
            try:
                press(resume)
                wait(lambda: event("recording-failed"), "failed resume")
                button(app, "Resume recording")
                button(app, "Stop recording")
                assert not find(app, "push button", "Retry recording")
                assert event("recording-state")[-1]["state"] == "paused"
            finally:
                bundle.unlink()
                held.rename(bundle)
        finally:
            stop(process)
        assert len(list(root.glob("*/metadata.json"))) == count + 1, "Quit lost an accepted take"
        assert event("recording-saved")[-1]["onQuit"]
        print("recording_controls: restart/delete confirmations, countdown cancellation, "
              "failed resume preserves paused media, and save-on-Quit passed")


STEPS = [history, preferences, editor, editor_close_keeps_open_text, recording, recording_controls,
         recording_output, previews, preview_overflow, feedback, open_with, live, capture, selectors,
         new_capture, tray, shortcuts, setup, crash_review, notices]
FIXTURE_LOG = None


def run(binary, scratch, only=None):
    """Every step, with a tray watcher and unlocked screen saver fixture on
    the session bus for the live-app steps."""
    global FIXTURE_LOG
    subprocess.Popen(["/usr/libexec/at-spi-bus-launcher", "--launch-immediately"],
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    FIXTURE_LOG = scratch / "desktop-fixture.log"
    FIXTURE_LOG.touch()
    fixture = subprocess.Popen([sys.executable, __file__, "--binary", binary,
                                "--desktop-fixture", str(FIXTURE_LOG)], stdout=subprocess.PIPE)
    try:
        assert select.select([fixture.stdout], [], [], 20)[0], "the desktop fixture stalled"
        assert fixture.stdout.readline().strip() == b"READY"
        for step in STEPS:
            if not only or step.__name__ in only:
                step(binary, scratch)
    finally:
        fixture.terminate()
        fixture.wait(timeout=10)


def wayland(scratch, injector):
    """Start a private headless Sway (the fixture from
    build_wayland_compositor_fixture.sh on PATH) with persistent virtual
    pointer and keyboard devices, and point every later launch at it."""
    global DESKTOP
    runtime = scratch / "runtime"
    runtime.mkdir(mode=0o700)
    # The injector's clicks are slower than a person's double-click. GDK's
    # Wayland backend reads it from GSettings: a private keyfile backend.
    (scratch / "config/glib-2.0/settings").mkdir(parents=True)
    (scratch / "config/glib-2.0/settings/keyfile").write_text(
        "[org/gnome/desktop/peripherals/mouse]\ndouble-click=1500\n")
    for key in ("DISPLAY", "WAYLAND_DISPLAY", "SWAYSOCK"):
        os.environ.pop(key, None)
    os.environ.update(XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(scratch / "config"),
                      XDG_SESSION_TYPE="wayland", XDG_CURRENT_DESKTOP="sway",
                      GDK_BACKEND="wayland", WLR_BACKENDS="headless", WLR_HEADLESS_OUTPUTS="1",
                      WLR_LIBINPUT_NO_DEVICES="1", WLR_RENDERER="pixman",
                      GSETTINGS_BACKEND="keyfile")
    config = scratch / "sway.conf"
    config.write_text(f"output HEADLESS-1 resolution {Wayland.WIDTH}x{Wayland.HEIGHT}\n"
                      "output * bg #234567 solid_color\nseat seat0 fallback true\n"
                      "default_border none\ndefault_floating_border none\n"
                      'for_window [title=".*"] floating enable\n')
    log = (scratch / "sway.log").open("w")
    sway = subprocess.Popen(["sway", "--unsupported-gpu", "-c", str(config)],
                            stdout=log, stderr=log)
    socket = wait(lambda: next((p for p in runtime.glob("wayland-*") if p.is_socket()), None),
                  "the Wayland socket")
    os.environ["WAYLAND_DISPLAY"] = socket.name
    os.environ["SWAYSOCK"] = str(wait(lambda: next(iter(runtime.glob("sway-ipc.*.sock")), None),
                                      "the Sway IPC socket"))
    devices = []
    for kind in ("pointer", "keyboard"):
        device = subprocess.Popen([str(injector), kind], stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE)
        assert select.select([device.stdout], [], [], 20)[0], f"the {kind} injector stalled"
        assert device.stdout.readline().strip() == b"READY", f"the {kind} injector failed"
        devices.append(device)
    DESKTOP = Wayland(dict(os.environ), *devices)
    return [*devices, sway]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--wayland", action="store_true",
                        help="run on a private headless Sway instead of the caller's X display")
    parser.add_argument("--injector", type=Path,
                        help="wayland_drag_probe binary (virtual pointer and keyboard)")
    parser.add_argument("--only", action="append",
                        help="run just this step (repeatable): " + ", ".join(s.__name__ for s in STEPS))
    parser.add_argument("--desktop-fixture", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--still", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.desktop_fixture:
        return desktop_fixture(args.desktop_fixture, args.still)
    # Live launches must never rebind the machine's own screenshot keys.
    os.environ["CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER"] = "1"
    assert not args.wayland or args.injector, "--wayland needs --injector"
    with tempfile.TemporaryDirectory() as scratch:
        scratch = Path(scratch)
        services = wayland(scratch, args.injector.resolve(strict=True)) if args.wayland else []
        try:
            run(args.binary, scratch, args.only)
            print(f"smoke: passed on {'Wayland (headless Sway)' if args.wayland else 'X11'}")
        finally:
            for service in services:
                service.terminate()
                service.wait(timeout=10)


if __name__ == "__main__":
    main()
