#!/usr/bin/python3
"""AT-SPI smoke test for the GTK4 host: real widgets on a private D-Bus and
accessibility bus under Xvfb, driven only through AT-SPI actions.

History: a 1000-entry on-disk fixture is reachable by name and role, keeps far
fewer card widgets than entries alive (virtualized), filters, and deletes one
entry from disk through the two-step Delete. Preferences: a switch row exposes
the switch role and checked state, and toggling it saves the setting.

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
from gi.repository import Atspi, GLib  # noqa: E402

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


if __name__ == "__main__":
    main()
