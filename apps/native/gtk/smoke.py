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

Run under `xvfb-run -a dbus-run-session --`, with `--binary` pointing at a
built `captures-gtk`. Needs python3-gi and gir1.2-atspi-2.0.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

import gi

gi.require_version("Atspi", "2.0")
gi.require_version("GdkPixbuf", "2.0")
from gi.repository import Atspi, GdkPixbuf, GLib  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from history_fixture import write_history_entries  # noqa: E402


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


def find(root, role, name):
    return [node for node, identity in nodes(root) if identity == (role, name)]


def has_state(node, state):
    try:
        return node.get_state_set().contains(state)
    except GLib.Error:
        return False


def press(node):
    names = [Atspi.Action.get_action_name(node, i) for i in range(Atspi.Action.get_n_actions(node))]
    assert names, f"{node.get_name()} has no action"
    index = names.index("click") if "click" in names else 0
    assert Atspi.Action.do_action(node, index), f"action failed on {node.get_name()}"


def launch(binary, *args):
    env = dict(os.environ, GSK_RENDERER="cairo", GTK_A11Y="atspi", NO_AT_BRIDGE="0")
    return subprocess.Popen([binary, *args], env=env, stdout=subprocess.DEVNULL)


def history(binary, scratch):
    root = scratch / "history"
    write_history_entries(root, 1000)
    process = launch(binary, "--scene", "history", "--history-root", str(root),
                     "--settings-file", str(scratch / "settings.json"), "--size", "1020x720")
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        wait(lambda: find(app, "push button", "All 1000"), "1000 loaded entries")
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
    process = launch(binary, "--settings-file", str(settings), "--size", "880x660")
    try:
        app = wait(lambda: application(process.pid), "the app on the accessibility bus")
        title = "Automatically copy captures to the clipboard"
        # GTK maps the switch role to AT-SPI "switch" or, before 4.10, "check box".
        switch = wait(lambda: [n for role in ("switch", "check box", "toggle button")
                               for n in find(app, role, title)], "the clipboard switch")[0]
        before = has_state(switch, Atspi.StateType.CHECKED)
        press(switch)
        wait(lambda: has_state(switch, Atspi.StateType.CHECKED) != before, "checked to flip")
        saved = wait(lambda: settings.exists() and json.loads(settings.read_text()), "settings file")
        assert saved["auto_copy_to_clipboard"] == (not before), saved
        print("preferences: switch role, checked state and save passed")
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
        press(wait(lambda: find(app, "push button", "Shapes · Rectangle (R)"), "Shapes")[0])
        press(wait(lambda: find(app, "push button", "Rectangle (R)"), "the Shapes flyout")[0])
        # The closing flyout keeps its input grab until it is gone.
        wait(lambda: not find(app, "push button", "Rectangle (R)"), "the flyout to close")
        # Drag across the middle of the canvas with the real pointer.
        window = subprocess.run(["xdotool", "search", "--sync", "--name",
                                 "Captures Screenshot Editor"],
                                capture_output=True, text=True, check=True).stdout.split()[0]
        geometry = dict(line.strip().split(": ", 1) for line in subprocess.run(
            ["xdotool", "getwindowgeometry", "--shell", window],
            capture_output=True, text=True, check=True).stdout.replace("=", ": ").splitlines())
        extents = canvas.get_extents(Atspi.CoordType.WINDOW)
        # Near the canvas's top-left: xvfb-run's default screen is 640×480,
        # smaller than the editor window.
        x = int(geometry["X"]) + extents.x + 150
        y = int(geometry["Y"]) + extents.y + 150
        # GTK must see the pointer enter before the press, or it drops the drag.
        subprocess.run(["xdotool", "mousemove", str(x), str(y)], check=True)
        time.sleep(0.2)
        subprocess.run(["xdotool", "mousedown", "1"], check=True)
        for step in range(1, 11):
            subprocess.run(["xdotool", "mousemove", str(x + step * 16), str(y + step * 10)],
                           check=True)
            time.sleep(0.03)
        subprocess.run(["xdotool", "mouseup", "1"], check=True)
        layer = lambda: [n for n, (role, name) in nodes(app)
                         if role == "list item" and name.startswith("Rectangle")]
        wait(layer, "a rectangle layer from the drag")
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
        press(wait(lambda: len(find(app, "push button", "Restore")) > 1
                   and find(app, "push button", "Restore"), "a second Restore")[1])
        wait(lambda: len(cards()) == 2, "two preview cards")
        # The chip shows for 2.5 s; walk only the previews window so a slow
        # runner's full-tree walk cannot miss it. The status role's AT-SPI
        # name differs across GTK releases, so match the chip by name.
        stack = wait(lambda: find(app, "frame", "Captures Previews"), "the previews window")[0]
        press(find(stack, "push button", "Copy")[0])
        chip = wait(lambda: [name for _, (_, name) in nodes(stack)
                             if name and (name == "Copied to clipboard"
                                          or name.startswith("Couldn’t copy"))],
                    "the Copied chip", timeout=10)
        assert chip == ["Copied to clipboard"], f"copy reported {chip}"
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    args = parser.parse_args()
    subprocess.Popen(["/usr/libexec/at-spi-bus-launcher", "--launch-immediately"],
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1)
    with tempfile.TemporaryDirectory() as scratch:
        scratch = Path(scratch)
        history(args.binary, scratch)
        preferences(args.binary, scratch)
        editor(args.binary, scratch)
        previews(args.binary, scratch)


if __name__ == "__main__":
    main()
