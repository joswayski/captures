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
        recording(args.binary, scratch)
        previews(args.binary, scratch)


if __name__ == "__main__":
    main()
