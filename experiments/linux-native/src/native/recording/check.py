#!/usr/bin/python3
"""Exercise the native recording HUD/editor in native_desktop.py's isolated X11 session."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time

NATIVE = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(NATIVE))
from native_check import assert_button_ink_alignment, find, ink_center, screen_bounds  # noqa: E402


def command(*args):
    return subprocess.check_output([str(arg) for arg in args], text=True).strip()


def xwindow_geometry(window):
    values = {}
    for line in command("xwininfo", "-id", window).splitlines():
        label, separator, value = line.strip().partition(":")
        if separator and label in ("Absolute upper-left X", "Absolute upper-left Y", "Width", "Height"):
            values[label] = int(value.strip())
    return (
        values["Absolute upper-left X"],
        values["Absolute upper-left Y"],
        values["Width"],
        values["Height"],
    )


def walk(node):
    yield node
    try:
        for child in node:
            yield from walk(child)
    except Exception:
        pass


def find_prefix(prefix, frame):
    import pyatspi

    for app in pyatspi.Registry.getDesktop(0):
        if app.name == "captures-linux-native":
            for root in [node for node in app if node.name == frame]:
                for node in walk(root):
                    if node.name.startswith(prefix):
                        return node
    return None


def timeline_range(frame="Edit recording — Captures"):
    import pyatspi

    pattern = re.compile(r"^(\d+):(\d+\.\d) – (\d+):(\d+\.\d)$")
    for app in pyatspi.Registry.getDesktop(0):
        if app.name == "captures-linux-native":
            for root in [node for node in app if node.name == frame]:
                for node in walk(root):
                    match = pattern.match(node.name)
                    if match:
                        start_minutes, start_seconds, end_minutes, end_seconds = match.groups()
                        return (
                            (int(start_minutes) * 60 + float(start_seconds)) * 1_000,
                            (int(end_minutes) * 60 + float(end_seconds)) * 1_000,
                        )
    return None


def widget_pixels(bounds):
    with tempfile.NamedTemporaryFile(suffix=".png") as image:
        command(
            "import", "-window", "root", "-crop",
            f"{bounds.width}x{bounds.height}+{bounds.x}+{bounds.y}",
            "+repage", image.name,
        )
        pixels = subprocess.check_output(["convert", image.name, "rgba:-"])
    return pixels


def changed_pixels(before, after):
    assert len(before) == len(after)
    return sum(
        any(abs(before[offset + channel] - after[offset + channel]) >= 24 for channel in range(3))
        for offset in range(0, len(before), 4)
    )


def focused_playhead_x(bounds, expected_fraction):
    pixels = widget_pixels(bounds)
    stride = bounds.width * 4
    accent_counts = []
    for x in range(bounds.width):
        count = 0
        for y in range(bounds.height):
            red, green, blue, alpha = pixels[y * stride + x * 4:y * stride + x * 4 + 4]
            if red >= 235 and 160 <= green <= 230 and blue <= 100 and alpha >= 240:
                count += 1
        accent_counts.append(count)
    expected = bounds.width * expected_fraction
    candidates = [
        x for x, count in enumerate(accent_counts)
        if abs(x - expected) <= 16 and count >= bounds.height * 0.5
    ]
    assert candidates, (expected, max(accent_counts), bounds)
    return (min(candidates) + max(candidates)) / 2


def wait(predicate, timeout=45):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError(f"Timed out waiting for {predicate!r}")


def owning_frame(node):
    while node is not None:
        try:
            if node.getRoleName() == "frame":
                return node.name
            node = node.parent
        except Exception:
            break
    raise AssertionError("Accessible control has no owning frame")


def click(name, frame=None, pointer=False):
    import pyatspi

    def ready():
        node = find(name, role="push button", frame=frame) or find(name, frame=frame)
        return node if node and node.getState().contains(pyatspi.STATE_SENSITIVE) else None

    node = wait(ready)
    if pointer:
        bounds = screen_bounds(node, frame or owning_frame(node))
        command("xdotool", "mousemove", bounds.x + bounds.width // 2, bounds.y + bounds.height // 2, "click", 1)
    else:
        assert node.queryAction().doAction(0), name
    time.sleep(0.2)


def enable_check(name, frame):
    import pyatspi

    node = wait(lambda: find(name, role="check box", frame=frame))
    bounds = screen_bounds(node, frame)
    command("xdotool", "mousemove", bounds.x + bounds.width // 2, bounds.y + bounds.height // 2, "click", 1)
    wait(lambda: node.getState().contains(pyatspi.STATE_CHECKED))


def choose(current, index, frame="Captures — Linux native"):
    node = wait(lambda: find(current, "combo box", frame))
    window = command("xdotool", "search", "--onlyvisible", "--name", frame).splitlines()[-1]
    command("xdotool", "windowactivate", "--sync", window)
    arrow = next(child for child in walk(node) if child.name == "GtkBuiltinIcon")
    bounds = screen_bounds(arrow, frame)
    command("xdotool", "mousemove", bounds.x + bounds.width // 2, bounds.y + bounds.height // 2)
    time.sleep(0.1)
    command("xdotool", "click", 1)
    wait(lambda: find('GtkTreePopover', 'filler', frame))
    # Keep keyboard focus on the combo, but close its separate popup surface.
    # GTK4 reports popup rows in coordinates that omit its placement offset;
    # opening near the footer also changes popup positioning. Closed-combo
    # Home/Down selects directly, without racing popup focus or Return dismissal.
    command('xdotool', 'key', 'Escape')
    wait(lambda: not find('GtkTreePopover', 'filler', frame))
    command('xdotool', 'key', '--delay', 100, 'Home', *(['Down'] * index))
    time.sleep(0.2)


def set_value(name, value, frame="Edit recording — Captures"):
    node = wait(lambda: find(name, frame=frame))
    node.queryValue().set_currentValue(float(value))
    time.sleep(0.1)
    assert node.queryValue().currentValue == float(value), (name, value, node.queryValue().currentValue)


def scroll_to(title, bottom):
    window = command("xdotool", "search", "--onlyvisible", "--name", title).splitlines()[-1]
    command("xdotool", "windowactivate", "--sync", window)
    x, y, width, height = xwindow_geometry(window)
    command("xdotool", "mousemove", x + width - 12, y + height // 2)
    command("xdotool", "click", "--repeat", 80, "--delay", 10, 5 if bottom else 4)
    time.sleep(0.4)


def focus_timeline(node, bounds, frame, attempts=30):
    import pyatspi

    def state():
        states = node.getState()
        return {
            "focusable": states.contains(pyatspi.STATE_FOCUSABLE),
            "focused": states.contains(pyatspi.STATE_FOCUSED),
            "name": node.name,
        }

    window = command("xdotool", "search", "--onlyvisible", "--name", frame).splitlines()[-1]
    command("xdotool", "windowactivate", "--sync", window)
    initial = state()
    command(
        "xdotool", "mousemove", bounds.x + 4,
        bounds.y + bounds.height // 2, "click", 1,
    )
    time.sleep(0.1)
    after_pointer = state()
    if node.name == "Trim start handle":
        return node
    for _ in range(attempts):
        command("xdotool", "key", "Tab")
        time.sleep(0.05)
        if node.name == "Trim start handle":
            return node
    raise AssertionError(
        "Timeline did not receive focus from pointer or Tab: "
        f"initial={initial}, after_pointer={after_pointer}, after_tabs={state()}"
    )


def capture(artifacts, name, title):
    window = wait(lambda: subprocess.run(
        ["xdotool", "search", "--onlyvisible", "--name", title], capture_output=True, text=True
    ).stdout.strip().splitlines()[-1:] or None)[0]
    time.sleep(0.4)
    x, y, width, height = xwindow_geometry(window)
    screen_width, screen_height = map(int, command("xdotool", "getdisplaygeometry").split())
    if not (0 <= x and 0 <= y and width > 0 and height > 0
            and x + width <= screen_width and y + height <= screen_height):
        raise AssertionError(f"Client {(x, y, width, height)} exceeds desktop {(screen_width, screen_height)}")
    geometry = f"{width}x{height}{x:+d}{y:+d}"
    output = artifacts / f"{name}.png"
    command("import", "-window", "root", "-crop", geometry, "+repage", output)
    actual = command("identify", "-format", "%wx%h", output)
    assert actual == f"{width}x{height}", f"Clipped client capture: {actual}, expected {width}x{height}"


def stop(process):
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


def launch(binary, profile, *arguments):
    env = dict(os.environ, CAPTURES_NATIVE_DATA=str(profile), SDL_AUDIODRIVER="dummy")
    log = (profile / "application.log").open("w+")
    process = subprocess.Popen(
        [str(binary), *map(str, arguments)], env=env, stdout=log, stderr=log, start_new_session=True
    )
    wait(lambda: find(role="frame"))
    return process, log


def stream_metadata(path):
    return json.loads(command("ffprobe", "-v", "error", "-show_streams", "-of", "json", path))["streams"]


def video_frame_hash(path, seconds):
    return command(
        "ffmpeg", "-v", "error", "-ss", seconds, "-i", path,
        "-frames:v", 1, "-f", "md5", "-",
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lab", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    args = parser.parse_args()
    os.environ.update(json.loads((args.lab / "environment.json").read_text()))
    import pyatspi  # noqa: F401 - imported after isolated session variables are installed
    import dbus

    saver = dbus.Interface(
        dbus.SessionBus().get_object("org.freedesktop.ScreenSaver", "/org/freedesktop/ScreenSaver"),
        "org.freedesktop.ScreenSaver",
    )

    args.artifacts.mkdir(parents=True, exist_ok=True)
    native = NATIVE
    binary = native / "target/release/captures-linux-native"

    # A failed earlier run may have left the shared reference at the motion
    # destination. Reset it before recording so every run captures a real move.
    reference = command(
        "xdotool", "search", "--onlyvisible", "--name",
        "Reference content — Captures comparison",
    ).splitlines()[-1]
    command("xdotool", "windowmove", reference, 210, 80)

    with tempfile.TemporaryDirectory(prefix="captures-recording-check-") as temporary:
        profile = Path(temporary)
        output = profile / "captures"
        (profile / "settings.json").write_text(json.dumps(dict(
            output_directory=str(output), show_mini_previews=True,
            include_mini_previews_in_captures=False,
            recording=dict(countdown_seconds=0, microphone_device_id="default"),
        )))
        process, log = launch(binary, profile, "--record")
        try:
            click("Full screen", "Captures — Select target")
            click("Start recording", "Captures — Select target")
            wait(lambda: find("Pause recording"))
            time.sleep(1.2)
            hud = wait(lambda: find("Captures recording controls", "frame"))
            hud_bounds = screen_bounds(hud, "Captures recording controls")
            assert hud_bounds.width <= 520 and hud_bounds.height <= 90, hud_bounds
            controls = [find(name) for name in (
                "Stop and save recording", "Pause recording", "Restart recording",
                "Take a region screenshot", "Mute microphone", "Delete recording",
                "Hide recording controls",
            )]
            assert all(controls)
            assert [screen_bounds(node, "Captures recording controls").x for node in controls] == sorted(
                screen_bounds(node, "Captures recording controls").x for node in controls
            )
            # Capture compositor output: importing an RGBA client window alone
            # drops its alpha and misrepresents transparent margins as black.
            command("import", "-window", "root", profile / "hud-desktop.png")
            command("convert", profile / "hud-desktop.png", "-crop",
                    f"{hud_bounds.width}x{hud_bounds.height}+{hud_bounds.x}+{hud_bounds.y}",
                    "+repage", args.artifacts / "recording-hud.png")

            drafts = wait(lambda: list((output / ".captures-recording-drafts").glob("*/manifest.json")))
            click("Take a region screenshot")
            wait(lambda: find("Captures — Select target", "frame"))
            wait(lambda: (value if (value := json.loads(drafts[0].read_text()))["segments"]
                          and value["segments"][0]["complete"] else None))
            command("xdotool", "key", "Escape")
            wait(lambda: json.loads(drafts[0].read_text())["state"] == "recording")
            # The callback must also resume after a successful screenshot.
            click("Take a region screenshot")
            wait(lambda: find("Captures — Select target", "frame"))
            from sys import path as python_path
            python_path.insert(0, str(native))
            from native_check import drag
            drag(180, 130, 240, 130)
            click("Capture", "Captures — Select target")
            wait(lambda: next(output.glob("*.png"), None))
            wait(lambda: json.loads(drafts[0].read_text())["state"] == "recording")
            assert not subprocess.run([
                "xdotool", "search", "--onlyvisible", "--name", "Captures — Mini previews"
            ], capture_output=True).stdout, "Screenshot must not re-show excluded previews during recording"
            # Restart gives the independent microphone assertions a fresh segment sequence.
            click("Restart recording", pointer=True)
            click("Restart", "Restart recording?", pointer=True)
            time.sleep(1.0)
            drafts = wait(lambda: list((output / ".captures-recording-drafts").glob("*/manifest.json")))
            click("Mute microphone")
            muted = wait(lambda: (value if len((value := json.loads(drafts[0].read_text()))["segments"]) >= 2
                                  and value["segments"][0]["complete"] and value["state"] == "recording" else None))
            assert muted["segments"][0]["microphone_relative_path"]
            assert muted["segments"][1]["microphone_relative_path"] is None
            click("Unmute microphone")
            unmuted = wait(lambda: (value if len((value := json.loads(drafts[0].read_text()))["segments"]) >= 3
                                    and value["segments"][1]["complete"] and value["state"] == "recording" else None))
            assert unmuted["segments"][2]["microphone_relative_path"]

            click("Pause recording")
            manifest = wait(lambda: (
                value if (value := json.loads(drafts[0].read_text()))["segments"]
                and value["state"] == "paused" else None
            ))
            assert manifest["segments"] and manifest["state"] == "paused"
            click("Resume recording")
            wait(lambda: (value if (value := json.loads(drafts[0].read_text()))["state"] == "recording" else None))
            saver.SetActive(True)
            locked = wait(lambda: (value if (value := json.loads(drafts[0].read_text()))["state"] == "paused" else None))
            assert locked["segments"][-1]["complete"]
            assert all((drafts[0].parent / segment["relative_path"]).exists() for segment in locked["segments"])
            saver.SetActive(False)
            click("Resume recording")
            wait(lambda: (value if (value := json.loads(drafts[0].read_text()))["state"] == "recording" else None))

            click("Restart recording", pointer=True)
            click("Restart", "Restart recording?", pointer=True)
            time.sleep(1.0)
            command("xdotool", "windowmove", reference, 320, 160)
            time.sleep(0.6)
            click("Hide recording controls")
            wait(lambda: find("Recording • Show controls"))
            click("Recording • Show controls")
            wait(lambda: find("Stop and save recording"))
            time.sleep(1.0)
            click("Stop and save recording")

            source = wait(lambda: next((path for path in output.glob("*.mp4") if path.stat().st_size > 1_000), None))
            wait(lambda: find("Edit recording — Captures", "frame"))
            wait(lambda: find("Ready to save.", frame="Edit recording — Captures"))
            assert not list((output / ".captures-recording-drafts").glob("*/manifest.json"))
            preview = wait(lambda: find("Recording frame preview", frame="Edit recording — Captures"))
            preview_bounds = screen_bounds(preview, "Edit recording — Captures")
            assert preview_bounds.width >= 600 and preview_bounds.height >= 350, preview_bounds
            assert find("Preview", frame="Edit recording — Captures")
            assert find("Loop preview", frame="Edit recording — Captures")
            assert find("Fit preview", frame="Edit recording — Captures")
            assert find("Show preview at 100%", frame="Edit recording — Captures")
            assert find("Saved filename", frame="Edit recording — Captures")
            assert find("Save as new file", frame="Edit recording — Captures")
            # Saved previews remain on top, as in the source app. Dismiss the
            # pile through its real UI before dragging the underlying timeline.
            pile = wait(lambda: find_prefix("Expand", "Captures — Mini previews"))
            assert pile.queryAction().doAction(0)
            click("Clear all", "Captures — Mini previews")
            wait(lambda: not find("Captures — Mini previews", "frame"))
            assert source.exists() and list(output.glob("*.png"))
            save = wait(lambda: find("Save", frame="Edit recording — Captures"))
            save_y = screen_bounds(save, "Edit recording — Captures").y
            capture(args.artifacts, "recording-editor", "Edit recording — Captures")
            editor_window = command("xdotool", "search", "--onlyvisible", "--name", "Edit recording — Captures").splitlines()[-1]
            command("xdotool", "windowactivate", "--sync", editor_window)
            scroll_to("Edit recording — Captures", bottom=True)
            assert find("Crop & size", frame="Edit recording — Captures")
            assert find("Save quality", frame="Edit recording — Captures")
            assert screen_bounds(save, "Edit recording — Captures").y == save_y
            capture(args.artifacts, "recording-editor-options", "Edit recording — Captures")

            # Pointer gestures below must target visible media, not offscreen
            # allocations retained in the accessibility tree after scrolling.
            scroll_to("Edit recording — Captures", bottom=False)
            timeline_node = find(
                "Interactive trim timeline", role="filler", frame="Edit recording — Captures"
            )
            timeline = screen_bounds(timeline_node, "Edit recording — Captures")
            source_video = next(stream for stream in stream_metadata(source) if stream["codec_type"] == "video")
            duration = float(source_video["duration"]) * 1_000
            source_hashes = [
                video_frame_hash(source, duration * fraction / 1_000)
                for fraction in (0.1, 0.9)
            ]
            assert len(set(source_hashes)) == 2, source_hashes
            initial_preview = widget_pixels(preview_bounds)
            click("Play preview", "Edit recording — Captures")
            wait(lambda: find("Pause preview", frame="Edit recording — Captures"))
            playback_changes = []
            playback_samples = min(24, max(8, int(duration / 250) + 2))
            for _ in range(playback_samples):
                time.sleep(0.25)
                playback_changes.append(changed_pixels(initial_preview, widget_pixels(preview_bounds)))
            print(f"Playback changed pixels: {playback_changes}; preview={preview_bounds.width}x{preview_bounds.height}", flush=True)
            assert max(playback_changes) > preview_bounds.width * preview_bounds.height * 0.05, playback_changes
            pause = find("Pause preview", frame="Edit recording — Captures")
            if pause and pause.getState().contains(pyatspi.STATE_SENSITIVE):
                assert pause.queryAction().doAction(0)
                time.sleep(0.2)
            else:
                assert find("Play preview", frame="Edit recording — Captures")

            timeline_node = focus_timeline(timeline_node, timeline, "Edit recording — Captures")
            command("xdotool", "key", "--repeat", 12, "--delay", 20, "Right")
            keyed_start, _ = wait(lambda: (
                value if (value := timeline_range()) and value[0] > duration * 0.08 else None
            ))
            assert abs(keyed_start - duration * 0.12) <= 75, (keyed_start, duration)
            command("xdotool", "key", "Home", "Tab")
            wait(lambda: find("Trim end handle", role="filler", frame="Edit recording — Captures"))
            command("xdotool", "key", "--repeat", 12, "--delay", 20, "Left")
            _, keyed_end = wait(lambda: (
                value if (value := timeline_range()) and value[1] < duration * 0.92 else None
            ))
            assert abs(keyed_end - duration * 0.88) <= 75, (keyed_end, duration)
            command("xdotool", "key", "End", "Tab", "Home")
            playhead = wait(lambda: find(
                "Playback position", role="filler", frame="Edit recording — Captures"
            ))
            assert playhead.getState().contains(pyatspi.STATE_FOCUSED)
            command("xdotool", "key", "--repeat", 12, "--delay", 20, "Right")
            time.sleep(0.15)
            keyed_playhead = focused_playhead_x(timeline, 0.12)
            assert abs(keyed_playhead - timeline.width * 0.12) <= 8, (keyed_playhead, timeline)
            command("xdotool", "key", "Home")
            # Use the visible filmstrip itself, not the auxiliary scales.
            drag(timeline.x + 1, timeline.y + timeline.height // 2, timeline.width // 5, 0)
            drag(timeline.x + timeline.width - 1, timeline.y + timeline.height // 2, -timeline.width // 5, 0)
            trim_a, trim_b = wait(lambda: timeline_range())
            assert .18 * duration < trim_a < .22 * duration, (trim_a, trim_b, duration, timeline)
            assert .78 * duration < trim_b < .82 * duration, (trim_a, trim_b, duration, timeline)
            scroll_to("Edit recording — Captures", bottom=True)
            enable_check("Crop recording", "Edit recording — Captures")
            set_value("Crop width", 100)
            set_value("Crop height", 100)
            scroll_to("Edit recording — Captures", bottom=False)
            overlay = screen_bounds(
                find("Interactive recording crop", role="filler", frame="Edit recording — Captures"),
                "Edit recording — Captures",
            )
            scale = min(overlay.width / source_video["width"], overlay.height / source_video["height"])
            ox = overlay.x + (overlay.width - source_video["width"] * scale) / 2
            oy = overlay.y + (overlay.height - source_video["height"] * scale) / 2
            # Start outside the small existing crop; draw reverse-direction.
            drag(round(ox + 960 * scale), round(oy + 560 * scale), round(-800 * scale), round(-480 * scale))
            actual = [find(name, frame="Edit recording — Captures").queryValue().currentValue
                      for name in ("Crop X", "Crop Y", "Crop width", "Crop height")]
            assert all(abs(a - b) <= 4 for a, b in zip(actual, (160, 80, 800, 480))), actual
            scroll_to("Edit recording — Captures", bottom=False)
            capture(args.artifacts, "recording-editor-direct", "Edit recording — Captures")
            click("Reset crop to full recording", "Edit recording — Captures")
            set_value("Crop width", 640)
            set_value("Crop height", 400)
            set_value("Crop X", 80)
            set_value("Crop Y", 50)
            scroll_to("Edit recording — Captures", bottom=True)
            choose("Output resolution", 3, "Edit recording — Captures")
            set_value("Output width", 320)
            set_value("Output height", 200)
            choose("Save quality", 1, "Edit recording — Captures")
            choose("Compression quality", 4, "Edit recording — Captures")
            capture(args.artifacts, "recording-editor-export-options", "Edit recording — Captures")
            # Changing quality automatically produces the current comparison;
            # do not invoke the hidden implementation trigger via AT-SPI.
            wait(lambda: find("Embedded compression comparison", frame="Edit recording — Captures")
                 and not find_prefix("Building compression comparison", "Edit recording — Captures"))
            scroll_to("Edit recording — Captures", bottom=False)
            comparison = find("Embedded compression comparison", frame="Edit recording — Captures")
            assert comparison.getState().contains(pyatspi.STATE_VISIBLE)
            original_bytes = source.stat().st_size
            original_size = f"{original_bytes / 1_000_000:.1f} MB" if original_bytes >= 1_000_000 else f"{original_bytes // 1_000} KB"
            assert comparison.description.startswith(f"Before · {original_size}; After · ≈ "), comparison.description
            assert not find_prefix("Comparison ready", "Edit recording — Captures")
            footer_format = screen_bounds(find("Format", "combo box", "Edit recording — Captures"), "Edit recording — Captures")
            footer_save = screen_bounds(find("Save", "push button", "Edit recording — Captures"), "Edit recording — Captures")
            frame_bounds = screen_bounds(find("Edit recording — Captures", "frame"), "Edit recording — Captures")
            filename_heading = screen_bounds(find("Filename", "label", "Edit recording — Captures"), "Edit recording — Captures")
            assert footer_format.width <= 84, footer_format
            assert footer_save.width <= 90 and footer_save.height <= 44, footer_save
            copy_toggle = screen_bounds(find('Save as new file', frame='Edit recording — Captures'), 'Edit recording — Captures')
            assert abs(copy_toggle.y + copy_toggle.height - footer_save.y - footer_save.height) <= 1, (copy_toggle, footer_save)
            assert abs(footer_save.y + footer_save.height - footer_format.y - footer_format.height) <= 1, (footer_save, footer_format)
            assert filename_heading.x - frame_bounds.x >= 32, (filename_heading, frame_bounds)
            assert not find("Change…", "push button", "Edit recording — Captures")
            bounds = screen_bounds(comparison, "Edit recording — Captures")
            handle = find(
                "Compression comparison slider, Before on the left and After on the right",
                role="push button",
                frame="Edit recording — Captures",
            )
            assert handle and handle.getState().contains(pyatspi.STATE_VISIBLE)
            assert handle.parent.getState().contains(pyatspi.STATE_VISIBLE)
            assert owning_frame(handle) == "Edit recording — Captures"
            center = screen_bounds(handle, "Edit recording — Captures")
            assert (
                bounds.x <= center.x < center.x + center.width <= bounds.x + bounds.width
                and bounds.y <= center.y < center.y + center.height <= bounds.y + bounds.height
            ), (bounds, center)
            drag(center.x + center.width//2, center.y + center.height//2, bounds.width//4, 0)
            moved = screen_bounds(handle, "Edit recording — Captures")
            fraction = (moved.x + moved.width/2 - bounds.x) / bounds.width
            assert .70 < fraction < .80, fraction
            capture(args.artifacts, "recording-editor-comparison", "Edit recording — Captures")
            shot = args.artifacts / 'recording-editor-comparison.png'
            origin = (frame_bounds.x, frame_bounds.y)
            text_center = ink_center(find('Save as new file', 'label', 'Edit recording — Captures'),
                                     'Edit recording — Captures', shot, origin)
            assert abs(text_center - copy_toggle.y - (copy_toggle.height - 1) / 2) <= .5, (text_center, copy_toggle)
            assert_button_ink_alignment(find('Save', 'push button', 'Edit recording — Captures'),
                                        'Edit recording — Captures', shot, origin)
            command("xdotool", "key", "End")
            end = screen_bounds(handle, "Edit recording — Captures")
            assert .92 < (end.x + end.width/2 - bounds.x) / bounds.width < .96
            click("Play preview", "Edit recording — Captures", pointer=True)
            # GTK4 removes hidden widgets from the live accessibility tree;
            # a retained proxy can continue reporting its last visible state.
            wait(lambda: find("Embedded compression comparison", frame="Edit recording — Captures") is None)
            assert find(
                "Compression comparison slider, Before on the left and After on the right",
                role="push button", frame="Edit recording — Captures",
            ) is None
            wait(lambda: find("Pause preview", frame="Edit recording — Captures"))
            time.sleep(.45)
            pause = find("Pause preview", frame="Edit recording — Captures")
            if pause and pause.getState().contains(pyatspi.STATE_SENSITIVE):
                assert pause.queryAction().doAction(0)
                time.sleep(0.2)
            else:
                assert find("Play preview", frame="Edit recording — Captures")
            source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
            make_copy = find("Save as new file", frame="Edit recording — Captures")
            assert make_copy.getState().contains(pyatspi.STATE_SENSITIVE)
            click("Save as new file", "Edit recording — Captures")
            wait(lambda: find("Replacing in", frame="Edit recording — Captures"))
            assert find(str(source.parent), "label", "Edit recording — Captures")
            save = find("Save", frame="Edit recording — Captures")
            wait(lambda: save.getState().contains(pyatspi.STATE_SENSITIVE))
            click("Save", "Edit recording — Captures")
            wait(lambda: find("Replace original recording?", "frame"))
            assert hashlib.sha256(source.read_bytes()).hexdigest() == source_hash
            capture(args.artifacts, "recording-replace-confirmation", "Replace original recording?")
            click("Keep original", "Replace original recording?")
            wait(lambda: not find("Replace original recording?", "frame"))
            wait(lambda: not list(output.glob(".captures-editor-export-*")))
            assert hashlib.sha256(source.read_bytes()).hexdigest() == source_hash
            destination = profile / "chosen-output"
            destination.mkdir()
            click("Change save location", "Edit recording — Captures", pointer=True)
            wait(lambda: find("Save recording to"))
            command("xdotool", "key", "ctrl+l")
            command("xdotool", "type", "--clearmodifiers", str(destination))
            # GTK4 accepts an existing folder from its location entry on Enter.
            command("xdotool", "key", "Return")
            wait(lambda: find(str(destination), "label", "Edit recording — Captures"))
            assert find("Saving to", "label", "Edit recording — Captures")
            find("Saved filename", frame="Edit recording — Captures").queryEditableText().setTextContents("parity-output")
            click("Save", "Edit recording — Captures")
            exported = wait(lambda: next(destination.glob("parity-output*.mp4"), None))
            # Publication precedes the GTK completion callback. Await that
            # callback before taking coordinates in the reflowing footer.
            wait(lambda: find("Export complete", "label", "Edit recording — Captures"))
            video = stream_metadata(exported)[0]
            assert (video["width"], video["height"]) == (320, 200), video
            exported_duration = float(video.get("duration", 0))
            assert 0.3 < exported_duration < duration / 1_000, (duration, exported_duration)

            choose("Format", 1, "Edit recording — Captures")
            click("Save", "Edit recording — Captures")
            gif = wait(lambda: next(destination.glob("parity-output*.gif"), None))
            gif_video = stream_metadata(gif)[0]
            assert gif_video["codec_name"] == "gif"
            assert (gif_video["width"], gif_video["height"]) == (320, 200)
            assert hashlib.sha256(source.read_bytes()).hexdigest() == source_hash
            print("PASS HUD: microphone mute/unmute, pause/resume, lock recovery, restart, hide/restore, private draft lifecycle")
            print("PASS editor: frame progression, keyboard/pointer trim, crop/scale, draggable comparison, visible Play dismisses stills, chosen-folder named MP4/GIF export, unchanged source")
        except Exception:
            command("import", "-window", "root", "/tmp/captures-recording-failure.png")
            log.flush(); log.seek(0); print(log.read(), flush=True)
            raise
        finally:
            stop(process)
            log.close()

    # Retain coverage of the other target/format entry points, not only display
    # MP4: asymmetric region dimensions catch accidental full-display fallback.
    for entry, target, extension in [("--gif", "Region", "gif"), ("--record", "Window", "mp4")]:
        with tempfile.TemporaryDirectory(prefix="captures-recording-target-") as temporary:
            profile = Path(temporary)
            output = profile / "captures"
            (profile / "settings.json").write_text(json.dumps(dict(
                output_directory=str(output), show_mini_previews=False,
                recording=dict(countdown_seconds=0, video_fps=15, gif_fps=15),
            )))
            process, log = launch(binary, profile, entry)
            try:
                click(target, "Captures — Select target")
                if target == "Region":
                    drag(710, 460, -380, -220)
                else:
                    command("xdotool", "mousemove", 10, 10)
                    time.sleep(.15)
                    command("xdotool", "mousemove", 650, 300)
                wait(lambda: find("Start recording").getState().contains(pyatspi.STATE_SENSITIVE))
                click("Start recording", "Captures — Select target")
                wait(lambda: find("Pause recording"))
                time.sleep(1.5)
                click("Stop and save recording")
                result = wait(lambda: next(output.glob(f"*.{extension}"), None))
                wait(lambda: find("Ready to save.", frame="Edit recording — Captures"))
                video = stream_metadata(result)[0]
                if extension == "gif":
                    assert (video["codec_name"], video["width"], video["height"]) == ("gif", 380, 220), video
                else:
                    assert video["codec_name"] == "h264" and 900 < video["width"] < 1280 and 500 < video["height"] < 800, video
                print(f"PASS recording target: {target} {extension.upper()} publishes real {video['width']}×{video['height']} media")
            finally:
                stop(process)
                log.close()

    # An interrupted Video session with GIF preference must recover as GIF,
    # through the real startup recovery UI, without losing retained segments.
    with tempfile.TemporaryDirectory(prefix="captures-recording-recovery-") as temporary:
        profile = Path(temporary)
        output = profile / "captures"
        (profile / "settings.json").write_text(json.dumps(dict(
            output_directory=str(output), show_mini_previews=False,
            recording=dict(countdown_seconds=0, video_format="gif", video_fps=15),
        )))
        process, log = launch(binary, profile, "--record")
        try:
            click("Full screen", "Captures — Select target")
            click("Start recording", "Captures — Select target")
            wait(lambda: find("Pause recording"))
            time.sleep(1.2)
            click("Pause recording")
            draft = wait(lambda: next((output / ".captures-recording-drafts").glob("*/manifest.json"), None))
            wait(lambda: json.loads(draft.read_text())["state"] == "paused")
            assert json.loads(draft.read_text())["options"]["kind"] == "gif"
        finally:
            stop(process)
            log.close()
        process, log = launch(binary, profile, "--background")
        try:
            wait(lambda: find("Recover recording", frame="Captures — Recover recordings"))
            capture(args.artifacts, "recording-recovery", "Captures — Recover recordings")
            click("Recover recording", "Captures — Recover recordings")
            recovered = wait(lambda: next(output.glob("*.gif"), None))
            wait(lambda: not draft.exists())
            stream = stream_metadata(recovered)[0]
            assert stream["codec_name"] == "gif" and stream["width"] > 0
            assert len(json.loads((profile / "history.json").read_text())) == 1
            print("PASS recovery: interrupted Video+GIF session recovers real GIF via startup UI and preserves history")
        finally:
            stop(process)
            log.close()

    with tempfile.TemporaryDirectory(prefix="captures-recording-audio-") as temporary:
        profile = Path(temporary)
        source = profile / "two-audio-streams.mp4"
        subprocess.run([
            "ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30",
            "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
            "-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000", "-t", "4",
            "-map", "0:v", "-map", "1:a", "-map", "2:a", "-metadata:s:a:0", "title=System audio",
            "-metadata:s:a:1", "title=Microphone", "-threads", "2", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", source,
        ], check=True)
        process, log = launch(binary, profile, "--open", source)
        try:
            wait(lambda: find("Edit recording — Captures", "frame"))
            wait(lambda: find("Ready to save.", frame="Edit recording — Captures"))
            scroll_to("Edit recording — Captures", bottom=True)
            click("Play preview", "Edit recording — Captures")
            wait(lambda: len(subprocess.run(["pgrep", "-P", str(process.pid), "ffplay"], capture_output=True, text=True).stdout.split()) == 2)
            enable_check("Mute system audio", "Edit recording — Captures")
            wait(lambda: len(subprocess.run(["pgrep", "-P", str(process.pid), "ffplay"], capture_output=True, text=True).stdout.split()) == 1)
            enable_check("Mute microphone", "Edit recording — Captures")
            wait(lambda: not subprocess.run(["pgrep", "-P", str(process.pid), "ffplay"], capture_output=True).stdout)
            print("PASS audio: two headless source streams play independently and each mute removes its child")
        finally:
            stop(process)
            log.close()


if __name__ == "__main__":
    main()
