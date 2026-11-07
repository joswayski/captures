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
built `captures-gtk`. Needs python3-gi and gir1.2-atspi-2.0.
"""
import argparse
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
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


def choose_file(app, title, accept, path):
    """Pick `path` in GTK's own file chooser: open its Location field (Ctrl+L
    once the dialog has focus), paste the whole path, then press `accept`.
    Typing key by key races the field's inline autocompletion on GTK 4.14,
    and AT-SPI text edits do not reach it."""
    dialog = subprocess.run(["xdotool", "search", "--sync", "--name", f"^{title}$"],
                            capture_output=True, text=True, check=True).stdout.split()[0]
    # GTK 4.8 names the stock button "Add Add"; GTK 4.14 names it "Add".
    button = wait(lambda: [n for n, (role, name) in nodes(app)
                           if role == "push button" and name in (accept, f"{accept} {accept}")],
                  f"the {title} dialog")[0]
    subprocess.run(["xclip", "-selection", "clipboard"], input=str(path), text=True, check=True)
    subprocess.run(["xdotool", "windowfocus", "--sync", dialog], check=True)
    subprocess.run(["xdotool", "key", "ctrl+l"], check=True)
    time.sleep(0.5)
    subprocess.run(["xdotool", "key", "ctrl+a", "ctrl+v"], check=True)
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
        window = subprocess.run(["xdotool", "search", "--sync", "--onlyvisible",
                                 "--pid", str(process.pid), "--name", "^Preferences$"],
                                capture_output=True, text=True, check=True).stdout.split()[0]
        subprocess.run(["xdotool", "windowfocus", "--sync", window], check=True)
        subprocess.run(["xdotool", "key", "ctrl+shift+4"], check=True)
        wait(lambda: saved_value("region_shortcut") == "Control+Shift+Digit4", "the shortcut")
        wait(lambda: not has_state(region, Atspi.StateType.PRESSED), "the recorder to stop")
        print("preferences: selects, the microphone list and the shortcut recorder saved")

        # Find: Ctrl+F opens a search landmark; the live count follows the
        # query and Ctrl+G, and Escape closes it.
        def find_count():
            return [node_text(n, name) for n, (role, name) in nodes(app)
                    if role in ("status bar", "status")]

        subprocess.run(["xdotool", "key", "ctrl+f"], check=True)
        wait(lambda: [n for role in ("text", "entry") for n in find(app, role, "Find settings")],
             "the Find settings field")
        subprocess.run(["xdotool", "type", "--delay", "40", "shortcut"], check=True)
        wait(lambda: "1 of 2" in find_count(), "Find to count two matches")
        subprocess.run(["xdotool", "key", "ctrl+g"], check=True)
        wait(lambda: "2 of 2" in find_count(), "Ctrl+G to step to the second match")
        subprocess.run(["xdotool", "key", "shift+F3"], check=True)
        wait(lambda: "1 of 2" in find_count(), "Shift+F3 to step back")
        subprocess.run(["xdotool", "key", "Escape"], check=True)
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
        subprocess.run(["xdotool", "mousemove", str(x + 80), str(y + 50)], check=True)
        time.sleep(0.2)
        subprocess.run(["xdotool", "click", "1"], check=True)
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
        subprocess.run(["xdotool", "mousemove", str(x + 40), str(y + 60)], check=True)
        time.sleep(0.2)
        subprocess.run(["xdotool", "click", "1"], check=True)
        field = wait(lambda: find(app, "text", "Edit text on canvas"), "the inline text field")[0]
        wait(lambda: has_state(field, Atspi.StateType.FOCUSED), "the field to take focus")
        subprocess.run(["xdotool", "type", "--delay", "40", "Smoke label"], check=True)
        subprocess.run(["xdotool", "key", "Return"], check=True)
        subprocess.run(["xdotool", "type", "--delay", "40", "second"], check=True)
        typed = Atspi.Text.get_text(field, 0, -1)
        assert typed == "Smoke label\nsecond", f"the field holds {typed!r}"
        subprocess.run(["xdotool", "key", "Escape"], check=True)
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
        subprocess.run(["xdotool", "mousemove", str(x + 300), str(y + 120)], check=True)
        time.sleep(0.2)
        subprocess.run(["xdotool", "click", "1"], check=True)
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
            subprocess.run(["xdotool", "mousemove", str(x), str(y)], check=True)
            time.sleep(0.2)
            subprocess.run(["xdotool", "mousedown", "1"], check=True)
            for step in range(1, 11):
                subprocess.run(["xdotool", "mousemove", str(x + step * 16), str(y + step * 6)],
                               check=True)
                time.sleep(0.03)
            subprocess.run(["xdotool", "mouseup", "1"], check=True)
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
        subprocess.run(["xdotool", "key", "Escape"], check=True)
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
        subprocess.run(["xdotool", "windowmove", window, "-500", "0"], check=True)
        time.sleep(0.5)

        def row_center(row):
            extents = row.get_extents(Atspi.CoordType.WINDOW)
            return extents.x - 500 + extents.width // 2, extents.y + extents.height // 2

        def rows():
            return [name for _, (role, name) in nodes(app)
                    if role == "list item" and " · " in name]

        logo = find(app, "list item", "Pasted logo.png · Image")[0]
        x, y = row_center(logo)
        subprocess.run(["xdotool", "mousemove", str(x), str(y)], check=True)
        time.sleep(0.2)
        subprocess.run(["xdotool", "click", "--repeat", "2", "--delay", "80", "1"], check=True)
        wait(lambda: find(app, "text", "Rename layer"), "the Rename layer field")
        time.sleep(0.5)  # The row's entry takes focus on idle.
        subprocess.run(["xdotool", "key", "ctrl+a"], check=True)
        subprocess.run(["xdotool", "type", "--delay", "20", "  Logo  "], check=True)
        subprocess.run(["xdotool", "key", "Return"], check=True)
        wait(lambda: find(app, "list item", "Logo · Image"), "the trimmed new layer name")
        before = rows()
        assert before[0] == "Logo · Image", f"unexpected layer order {before}"
        below = find(app, "list item", before[1])[0]
        x, y = row_center(find(app, "list item", "Logo · Image")[0])
        _, target_y = row_center(below)
        target_y += below.get_extents(Atspi.CoordType.WINDOW).height // 4
        subprocess.run(["xdotool", "mousemove", str(x), str(y)], check=True)
        time.sleep(0.2)
        subprocess.run(["xdotool", "mousedown", "1"], check=True)
        for step in range(1, 11):
            subprocess.run(["xdotool", "mousemove", str(x),
                            str(y + (target_y - y) * step // 10)], check=True)
            time.sleep(0.03)
        subprocess.run(["xdotool", "mouseup", "1"], check=True)
        wait(lambda: rows()[:2] == [before[1], "Logo · Image"],
             f"the drag to move Logo below {before[1]}")
        press(find(app, "push button", "Undo")[0])
        wait(lambda: rows()[:2] == before[:2], "Undo to restore the layer order")
        subprocess.run(["xdotool", "windowmove", window, "0", "0"], check=True)
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


def close_window(window):
    """Ask `window` to close with WM_DELETE_WINDOW, as a taskbar or a Wayland
    compositor does: unlike a window manager's Alt+F4 there is no keyboard
    grab, so a focused text field does not commit on focus loss first."""
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
    x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long,
                               ctypes.c_void_p]
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
            return subprocess.run(["xdotool", "search", "--sync", "--name",
                                   "Captures Screenshot Editor"],
                                  capture_output=True, text=True, check=True).stdout.split()[0]

        def label(window, dx, words):
            tool = find(app, "push button", "Text (T)")[0]
            press(tool)
            wait(lambda: has_state(tool, Atspi.StateType.PRESSED), "the Text tool")
            subprocess.run(["xdotool", "windowfocus", "--sync", window], check=True)
            # Near the canvas's top-left: xvfb-run's default screen is 640×480.
            canvas = find(app, "image", "Screenshot canvas")[0]
            extents = canvas.get_extents(Atspi.CoordType.WINDOW)
            subprocess.run(["xdotool", "mousemove", "--window", window,
                            str(extents.x + 120 + dx), str(extents.y + 120)], check=True)
            time.sleep(0.2)
            subprocess.run(["xdotool", "click", "1"], check=True)
            field = wait(lambda: find(app, "text", "Edit text on canvas"), "the inline field")[0]
            wait(lambda: has_state(field, Atspi.StateType.FOCUSED), "the field to take focus")
            subprocess.run(["xdotool", "type", "--delay", "40", words], check=True)
            wait(lambda: find(app, "list item", f"{words} · Text"), f"the {words} layer")

        window = open_editor()
        label(window, 0, "abc")
        subprocess.run(["xdotool", "key", "Escape"], check=True)
        wait(lambda: not find(app, "text", "Edit text on canvas"), "Escape to commit abc")
        label(window, 150, "OPEN")
        close_window(window)
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
        window = subprocess.run(["xdotool", "search", "--sync", "--name", "^Send Feedback$"],
                                capture_output=True, text=True, check=True).stdout.split()[0]
        subprocess.run(["xdotool", "mousemove", "--window", window, "20", "20"], check=True)
        time.sleep(0.2)
        subprocess.run(["xdotool", "key", "Tab", "Tab", "Tab", "Tab"], check=True)
        subprocess.run(["xdotool", "type", "--delay", "40", "Toolbar overlaps"], check=True)
        subprocess.run(["xdotool", "key", "Tab"], check=True)
        subprocess.run(["xdotool", "type", "--delay", "40", "@captures"], check=True)
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
        subprocess.run(["xdotool", "key", "shift+Tab", "shift+Tab"], check=True)
        subprocess.run(["xdotool", "type", "--delay", "40", "Again"], check=True)
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
        window = subprocess.run(["xdotool", "search", "--sync", "--name", "Captures Editor"],
                                capture_output=True, text=True, check=True).stdout.split()[0]
        subprocess.run(["xdotool", "mousemove", "--window", window, "300", "20"], check=True)
        time.sleep(0.2)
        subprocess.run(["xdotool", "key", "Tab", "shift+Right"], check=True)
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
        editor_close_keeps_open_text(args.binary, scratch)
        recording(args.binary, scratch)
        recording_output(args.binary, scratch)
        previews(args.binary, scratch)
        preview_overflow(args.binary, scratch)
        feedback(args.binary, scratch)


if __name__ == "__main__":
    main()
