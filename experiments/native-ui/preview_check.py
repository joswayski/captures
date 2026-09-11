#!/usr/bin/python3
"""Pointer-driven native mini-preview parity checks in native_desktop.py's X11 session."""
import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


def command(*args):
    return subprocess.check_output([str(arg) for arg in args], text=True).strip()


def walk(node):
    yield node
    try:
        for child in node:
            yield from walk(child)
    except Exception:
        pass


def find(name=None, prefix=None, role=None, frame=None):
    import pyatspi
    for app in pyatspi.Registry.getDesktop(0):
        if app.name != "captures-linux-native":
            continue
        roots = [app] if frame is None else [node for node in app if node.name == frame]
        for root in roots:
            for node in walk(root):
                try:
                    matches_name = name is None or node.name == name
                    matches_prefix = prefix is None or node.name.startswith(prefix)
                    if (matches_name and matches_prefix and
                            (role is None or node.getRoleName() == role) and
                            node.getState().contains(pyatspi.STATE_SHOWING)):
                        return node
                except Exception:
                    pass
    return None


def wait(predicate, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError(f"Timed out waiting for {predicate!r}")


def bounds(node):
    import pyatspi
    return node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)


def pointer_click(node):
    rect = bounds(node)
    command("xdotool", "mousemove", rect.x + rect.width // 2, rect.y + rect.height // 2, "click", 1)
    time.sleep(0.18)


def pointer_hover(node):
    rect = bounds(node)
    command("xdotool", "mousemove", "--sync", rect.x + rect.width // 2, rect.y + rect.height // 2)
    time.sleep(0.25)


def pointer_drag(node, destination, source_fraction=(0.5, 0.5)):
    rect = bounds(node)
    start = (
        rect.x + int(rect.width * source_fraction[0]),
        rect.y + int(rect.height * source_fraction[1]),
    )
    print(f"drag {node.name!r}: {start} -> {destination}", flush=True)
    command("xdotool", "mousemove", "--sync", *start, "mousedown", 1)
    for step in range(1, 15):
        x = start[0] + (destination[0] - start[0]) * step // 14
        y = start[1] + (destination[1] - start[1]) * step // 14
        command("xdotool", "mousemove", "--sync", x, y)
        time.sleep(0.018)
    time.sleep(0.25)
    command("xdotool", "mouseup", 1)
    time.sleep(0.35)


def self_drop(node):
    rect = bounds(node)
    x = rect.x + int(rect.width * 0.8)
    y = rect.y + int(rect.height * 0.72)
    command("xdotool", "mousemove", x, y, "mousedown", 1)
    command("xdotool", "mousemove", x + 70, y - 20)
    time.sleep(0.12)
    command("xdotool", "mousemove", x, y, "mouseup", 1)


@contextmanager
def external_drop_target(marker):
    code = r'''
import sys
import gi
gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, Gtk
window = Gtk.Window(title="Preview external drop target")
window.set_default_size(220, 160)
window.move(1000, 300)
window.add(Gtk.Label(label="Drop capture here"))
window.drag_dest_set(
    Gtk.DestDefaults.ALL,
    [Gtk.TargetEntry.new("text/uri-list", Gtk.TargetFlags(0), 0)],
    Gdk.DragAction.COPY,
)
def received(_widget, context, _x, _y, data, _info, timestamp):
    with open(sys.argv[1], "w", encoding="utf-8") as output:
        output.write("\n".join(data.get_uris() or []))
    Gtk.drag_finish(context, True, False, timestamp)
def motion(_widget, context, _x, _y, timestamp):
    with open(sys.argv[1] + ".motion", "w", encoding="utf-8") as output:
        output.write(str(context.list_targets()))
    Gdk.drag_status(context, Gdk.DragAction.COPY, timestamp)
    return True
window.connect("drag-motion", motion)
window.connect("drag-data-received", received)
window.show_all()
Gtk.main()
'''
    process = subprocess.Popen(["/usr/bin/python3", "-c", code, str(marker)], env=os.environ)
    try:
        window = wait(lambda: subprocess.run(
            ["xdotool", "search", "--onlyvisible", "--name", "Preview external drop target"],
            capture_output=True,
            text=True,
        ).stdout.strip())
        window_id = window.splitlines()[-1]
        geometry = command("xdotool", "getwindowgeometry", "--shell", window_id)
        values = dict(line.split("=", 1) for line in geometry.splitlines() if "=" in line)
        time.sleep(0.4)
        yield (
            int(values["X"]) + int(values["WIDTH"]) // 2,
            int(values["Y"]) + int(values["HEIGHT"]) // 2,
        )
    finally:
        process.terminate()
        process.wait(timeout=5)


def screenshot(path):
    time.sleep(0.2)
    command("import", "-window", "root", path)


@contextmanager
def launch(binary, images, profile):
    env = dict(os.environ, CAPTURES_NATIVE_DATA=str(profile))
    profile.mkdir(parents=True, exist_ok=True)
    (profile / "settings.json").write_text(json.dumps(dict(mini_preview_placement=1)))
    log_path = profile / "preview.log"
    with log_path.open("w+") as log:
        process = subprocess.Popen(
            [str(binary), "--previews", *map(str, images)],
            env=env,
            stdout=log,
            stderr=log,
            start_new_session=True,
        )
        try:
            wait(lambda: find("Captures — Mini previews", role="frame"))
            wait(lambda: find(prefix="Expand ", role="push button"))
            time.sleep(0.4)
            yield process
        except Exception:
            log.seek(0)
            print(log.read(), flush=True)
            screenshot(profile / "failure.png")
            raise
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def make_sources(directory):
    specs = [
        ("red-wide.png", "#e74c3c", "RED 640x240", "640x240"),
        ("green-tall.png", "#27ae60", "GREEN 280x620", "280x620"),
        ("blue-square.png", "#2878d0", "BLUE 420x420", "420x420"),
        ("amber-small.png", "#d99016", "AMBER 300x180", "300x180"),
    ]
    paths = []
    for filename, color, label, size in specs:
        path = directory / filename
        subprocess.run(
            ["convert", "-size", size, f"xc:{color}", "-fill", "white", "-gravity", "center",
             "-pointsize", "28", "-annotate", "0", label, str(path)],
            check=True,
        )
        paths.append(path)
    return paths


def check_layout_and_actions(binary, images, artifacts, profile):
    with launch(binary, images, profile):
        command("xdotool", "mousemove", 640, 400)
        screenshot(artifacts / "after-preview-bottom-left-collapsed.png")
        frame = find("Captures — Mini previews", role="frame")
        frame_rect = bounds(frame)
        assert frame_rect.width == 340, f"preview frame width {frame_rect.width}, expected 340"
        pile = wait(lambda: find(prefix="Expand ", role="push button"))
        pile_rect = bounds(pile)
        assert (pile_rect.width, pile_rect.height) == (284, 160), pile_rect
        pointer_hover(pile)
        screenshot(artifacts / "after-preview-bottom-left-collapsed-hover.png")
        # The full-size native window is shaped to the cards: this point is in
        # its transparent middle but over the reference-content window.
        reference = command("xdotool", "search", "--onlyvisible", "--name", "Reference content").splitlines()[-1]
        command("xdotool", "windowactivate", "--sync", reference)
        command("xdotool", "mousemove", 470, 300, "click", 1)
        active = command("xdotool", "getactivewindow")
        assert active == reference, (
            f"transparent preview area intercepted input: active={active} "
            f"({command('xdotool', 'getwindowname', active)!r}), expected={reference}"
        )

        corners = {
            "top-left": (175, 130),
            "top-right": (1105, 130),
            "bottom-right": (1105, 670),
            "bottom-left": (175, 670),
        }
        for name, destination in corners.items():
            pile = wait(lambda: find(prefix="Expand ", role="push button"))
            pointer_drag(pile, destination)
            screenshot(artifacts / f"after-preview-{name}-collapsed.png")

        pile = wait(lambda: find(prefix="Expand ", role="push button"))
        assert pile.queryAction().doAction(0), "AT-SPI activation was rejected"
        wait(lambda: find("Preview amber-small.png"))
        command("xdotool", "mousemove", 640, 400)
        time.sleep(0.25)
        assert find("Close amber-small.png", role="push button") is None, (
            "per-card chrome must remain hidden without hover"
        )
        screenshot(artifacts / "after-preview-expanded-idle.png")
        pointer_hover(wait(lambda: find("Preview amber-small.png")))
        wait(lambda: find("Close amber-small.png", role="push button"))
        screenshot(artifacts / "after-preview-expanded-hover.png")

        blue_card = wait(lambda: find("Preview blue-square.png"))
        self_drop(blue_card)
        screenshot(artifacts / "after-preview-self-drop-reject.png")
        assert find("Close blue-square.png", role="push button"), "self-drop dismissed its card"

        # Exercise a non-front card by identity, not by an AT-SPI action behind
        # another window. Pointer coordinates must hit the visible GTK button.
        pointer_hover(wait(lambda: find("Preview green-tall.png")))
        pointer_click(wait(lambda: find("Copy green-tall.png", role="push button")))
        assert find("Copy green-tall.png", role="push button") is None
        pointer_hover(wait(lambda: find("Preview amber-small.png")))
        assert find("Copy amber-small.png", role="push button"), "copy state leaked across cards"

        green = images[1]
        pointer_hover(wait(lambda: find("Preview green-tall.png")))
        pointer_click(wait(lambda: find("Close green-tall.png", role="push button")))
        wait(lambda: find("Close green-tall.png", role="push button") is None)
        assert green.exists(), "Close deleted the selected card's file"
        screenshot(artifacts / "after-preview-per-card-close.png")

        red = images[0]
        marker = profile / "external-drop.txt"
        with external_drop_target(marker) as destination:
            pointer_drag(wait(lambda: find("Preview red-wide.png")), destination, (0.8, 0.72))
            try:
                wait(marker.exists)
            except AssertionError:
                motion = marker.with_suffix(".txt.motion")
                raise AssertionError(
                    f"external drop was not received; drag-motion={motion.exists()}"
                )
        assert "red-wide.png" in marker.read_text(), "file drag exported the wrong card"
        wait(lambda: find("Close red-wide.png", role="push button") is None)
        assert red.exists(), "successful external file drag deleted the source"

        pointer_click(wait(lambda: find("Clear all", role="push button")))
        wait(lambda: find("Captures — Mini previews", role="frame") is None, timeout=5)
        assert all(path.exists() for path in images), "Clear all deleted a source/history file"


def check_delete_animation(binary, images, artifacts, profile):
    victim = images[-1]
    with launch(binary, images, profile):
        pointer_click(wait(lambda: find(prefix="Expand ", role="push button")))
        pointer_hover(wait(lambda: find(f"Preview {victim.name}")))
        delete = wait(lambda: find(f"Delete {victim.name}", role="push button"))
        clip = subprocess.Popen(
            ["ffmpeg", "-v", "error", "-y", "-f", "x11grab", "-video_size", "1280x800",
             "-framerate", "30", "-i", os.environ["DISPLAY"], "-t", "4", "-c:v", "libx264",
             "-preset", "ultrafast", "-tune", "zerolatency", "-pix_fmt", "yuv420p",
             "-progress", "pipe:1", "-stats_period", "0.1", str(artifacts / "after-preview-delete-dust.mp4")],
            stdout=subprocess.PIPE,
            text=True,
        )
        # The action happens only after FFmpeg confirms a real encoded frame.
        for line in clip.stdout:
            if line.startswith("frame=") and int(line.split("=")[1]) > 0:
                break
        else:
            raise AssertionError("FFmpeg did not encode a first frame")
        pointer_click(delete)
        pointer_click(wait(lambda: find("Delete", role="push button", frame=None)))
        time.sleep(0.38)
        screenshot(artifacts / "after-preview-delete-dust.png")
        wait(lambda: not victim.exists(), timeout=5)
        clip.wait(timeout=8)
        assert clip.returncode == 0
        assert all(path.exists() for path in images[:-1]), "Delete targeted the wrong card"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lab", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    args = parser.parse_args()
    os.environ.update(json.loads((args.lab / "environment.json").read_text()))
    import pyatspi  # noqa: F401 -- import after isolated session environment
    args.artifacts.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="captures-preview-check-") as temporary:
        root = Path(temporary)
        sources = make_sources(root)
        check_layout_and_actions(args.binary.resolve(), sources, args.artifacts, root / "profile-a")
        # Clear all intentionally preserves these files, so reuse them for the
        # separately proven destructive path.
        check_delete_animation(args.binary.resolve(), sources, args.artifacts, root / "profile-b")
    print(
        "PASS preview: four placements, click-through, per-card identity/copy/close, "
        "correct external file drag, nondestructive clear, confirmed dust delete",
        flush=True,
    )


if __name__ == "__main__":
    main()
