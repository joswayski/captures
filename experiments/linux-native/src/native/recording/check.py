#!/usr/bin/python3
"""Exercise the native recording HUD/editor in native_desktop.py's isolated X11 session."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time


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


def find(name=None, role=None, frame=None):
    import pyatspi

    for app in pyatspi.Registry.getDesktop(0):
        if app.name != "captures-linux-native":
            continue
        roots = [app] if frame is None else [node for node in app if node.name == frame]
        for root in roots:
            for node in walk(root):
                try:
                    if (name is None or node.name == name) and (role is None or node.getRoleName() == role):
                        if frame is not None or node.getState().contains(pyatspi.STATE_SHOWING):
                            return node
                except Exception:
                    pass
    return None


def find_all(role, frame):
    import pyatspi

    nodes = []
    for app in pyatspi.Registry.getDesktop(0):
        if app.name == "captures-linux-native":
            for root in [node for node in app if node.name == frame]:
                nodes.extend(node for node in walk(root) if node.getRoleName() == role)
    return nodes


def find_prefix(prefix, frame):
    import pyatspi

    for app in pyatspi.Registry.getDesktop(0):
        if app.name == "captures-linux-native":
            for root in [node for node in app if node.name == frame]:
                for node in walk(root):
                    if node.name.startswith(prefix):
                        return node
    return None


def wait(predicate, timeout=45):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError(f"Timed out waiting for {predicate!r}")


def click(name, frame=None, pointer=False):
    node = wait(lambda: find(name, frame=frame))
    if pointer:
        import pyatspi

        bounds = node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
        command("xdotool", "mousemove", bounds.x + bounds.width // 2, bounds.y + bounds.height // 2, "click", 1)
    else:
        assert node.queryAction().doAction(0), name
    time.sleep(0.2)


def choose(current, index, frame="Captures — Linux native"):
    node = wait(lambda: find(current, "combo box", frame))
    window = command("xdotool", "search", "--onlyvisible", "--name", frame).splitlines()[-1]
    command("xdotool", "windowactivate", "--sync", window)
    node.queryAction().doAction(0)
    time.sleep(0.1)
    command("xdotool", "key", "Home", *(["Down"] * index), "Return")
    time.sleep(0.2)


def set_value(name, value, frame="Edit recording — Captures"):
    node = wait(lambda: find(name, frame=frame))
    node.queryValue().set_currentValue(float(value))
    time.sleep(0.1)


def capture(artifacts, name, title):
    window = wait(lambda: subprocess.run(
        ["xdotool", "search", "--onlyvisible", "--name", title], capture_output=True, text=True
    ).stdout.strip().splitlines()[-1:] or None)[0]
    time.sleep(0.4)
    x, y, width, height = xwindow_geometry(window)
    geometry = f"{width}x{height}{x:+d}{y:+d}"
    command("import", "-window", "root", "-crop", geometry, "+repage", artifacts / f"{name}.png")


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
    native = Path(__file__).resolve().parents[3]
    binary = native / "target/release/captures-linux-native"

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
            hud_bounds = hud.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            assert hud_bounds.width <= 520 and hud_bounds.height <= 90, hud_bounds
            controls = [find(name) for name in (
                "Stop and save recording", "Pause recording", "Restart recording",
                "Take a region screenshot", "Mute microphone", "Delete recording",
                "Hide recording controls",
            )]
            assert all(controls)
            assert [node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS).x for node in controls] == sorted(
                node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS).x for node in controls
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
            click("OK", pointer=True)
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
            manifest = wait(lambda: (value if (value := json.loads(drafts[0].read_text()))["segments"] else None))
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
            click("OK", pointer=True)
            time.sleep(1.0)
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
            preview_bounds = preview.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
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
            save_y = save.queryComponent().getExtents(pyatspi.DESKTOP_COORDS).y
            capture(args.artifacts, "recording-editor", "Edit recording — Captures")
            editor_window = command("xdotool", "search", "--onlyvisible", "--name", "Edit recording — Captures").splitlines()[-1]
            command("xdotool", "windowactivate", "--sync", editor_window)
            scrollbars = find_all("scroll bar", "Edit recording — Captures")
            vertical = max(scrollbars, key=lambda node: node.queryValue().maximumValue)
            vertical.queryValue().set_currentValue(vertical.queryValue().maximumValue)
            time.sleep(0.4)
            assert find("Crop & size", frame="Edit recording — Captures")
            assert find("Save quality", frame="Edit recording — Captures")
            assert save.queryComponent().getExtents(pyatspi.DESKTOP_COORDS).y == save_y
            capture(args.artifacts, "recording-editor-options", "Edit recording — Captures")

            # Pointer gestures below must target visible media, not offscreen
            # allocations retained in the accessibility tree after scrolling.
            vertical.queryValue().set_currentValue(0)
            time.sleep(0.4)
            position = wait(lambda: find("Playback position", frame="Edit recording — Captures"))
            before = float(position.queryValue().currentValue)
            click("Play preview", "Edit recording — Captures")
            time.sleep(0.9)
            after = float(position.queryValue().currentValue)
            assert after > before + 300, (before, after)
            click("Pause preview", "Edit recording — Captures")

            duration = float(wait(lambda: find("Trim end", frame="Edit recording — Captures")).queryValue().maximumValue)
            timeline_node = find("Interactive trim timeline", frame="Edit recording — Captures")
            assert timeline_node.queryComponent().grabFocus()
            command("xdotool", "key", "Right")
            assert find("Trim start", frame="Edit recording — Captures").queryValue().currentValue > 0
            command("xdotool", "key", "Home", "Tab", "Left")
            assert find("Trim end", frame="Edit recording — Captures").queryValue().currentValue < duration
            command("xdotool", "key", "End", "Tab", "Home", "Right")
            assert position.queryValue().currentValue > 0
            command("xdotool", "key", "Home")
            # Use the visible filmstrip itself, not the auxiliary scales.
            timeline = timeline_node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            drag(timeline.x + 1, timeline.y + timeline.height // 2, timeline.width // 5, 0)
            drag(timeline.x + timeline.width - 1, timeline.y + timeline.height // 2, -timeline.width // 5, 0)
            trim_a = find("Trim start", frame="Edit recording — Captures").queryValue().currentValue
            trim_b = find("Trim end", frame="Edit recording — Captures").queryValue().currentValue
            assert .18 * duration < trim_a < .22 * duration, (trim_a, trim_b, duration, timeline)
            assert .78 * duration < trim_b < .82 * duration, (trim_a, trim_b, duration, timeline)
            set_value("Trim start", min(200, duration / 5))
            set_value("Trim end", max(700, duration - 200))
            click("Crop recording", "Edit recording — Captures")
            set_value("Crop width", 100)
            set_value("Crop height", 100)
            overlay = find("Interactive recording crop", frame="Edit recording — Captures").queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            scale = min(overlay.width / 1280, overlay.height / 800)
            ox = overlay.x + (overlay.width - 1280 * scale) / 2
            oy = overlay.y + (overlay.height - 800 * scale) / 2
            # Start outside the small existing crop; draw reverse-direction.
            drag(round(ox + 960 * scale), round(oy + 560 * scale), round(-800 * scale), round(-480 * scale))
            actual = [find(name, frame="Edit recording — Captures").queryValue().currentValue
                      for name in ("Crop X", "Crop Y", "Crop width", "Crop height")]
            assert all(abs(a - b) <= 4 for a, b in zip(actual, (160, 80, 800, 480))), actual
            vertical.queryValue().set_currentValue(0)
            capture(args.artifacts, "recording-editor-direct", "Edit recording — Captures")
            click("Reset crop to full recording", "Edit recording — Captures")
            set_value("Crop width", 640)
            set_value("Crop height", 400)
            set_value("Crop X", 80)
            set_value("Crop Y", 50)
            vertical.queryValue().set_currentValue(vertical.queryValue().maximumValue)
            time.sleep(0.4)
            resolution = wait(lambda: find("Output resolution", "combo box", "Edit recording — Captures"))
            resolution.queryAction().doAction(0)
            command("xdotool", "key", "End", "Return")
            set_value("Output width", 320)
            set_value("Output height", 200)
            quality = wait(lambda: find("Save quality", "combo box", "Edit recording — Captures"))
            quality.queryAction().doAction(0)
            command("xdotool", "key", "End", "Return")
            click("Compare compression before and after", "Edit recording — Captures")
            wait(lambda: find_prefix("Comparison ready", "Edit recording — Captures"))
            scrollbars = find_all("scroll bar", "Edit recording — Captures")
            vertical = max(scrollbars, key=lambda node: node.queryValue().maximumValue)
            vertical.queryValue().set_currentValue(0)
            time.sleep(0.4)
            comparison = find("Embedded compression comparison", frame="Edit recording — Captures")
            assert comparison.getState().contains(pyatspi.STATE_SHOWING)
            bounds = comparison.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            handle = find("Compression comparison slider, Before on the left and After on the right", frame="Edit recording — Captures")
            center = handle.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            drag(center.x + center.width//2, center.y + center.height//2, bounds.width//4, 0)
            moved = handle.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            fraction = (moved.x + moved.width/2 - bounds.x) / bounds.width
            assert .70 < fraction < .80, fraction
            capture(args.artifacts, "recording-editor-comparison", "Edit recording — Captures")
            command("xdotool", "key", "End")
            end = handle.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            assert .92 < (end.x + end.width/2 - bounds.x) / bounds.width < .96
            click("Play preview", "Edit recording — Captures", pointer=True)
            wait(lambda: not comparison.getState().contains(pyatspi.STATE_SHOWING))
            start_playback = position.queryValue().currentValue
            time.sleep(.45)
            assert position.queryValue().currentValue > start_playback + 100
            click("Pause preview", "Edit recording — Captures")
            source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
            make_copy = find("Save as new file", frame="Edit recording — Captures")
            assert make_copy.getState().contains(pyatspi.STATE_SENSITIVE)
            click("Save as new file", "Edit recording — Captures")
            wait(lambda: find(f"Replacing in  {source.parent}", frame="Edit recording — Captures"))
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
            command("xdotool", "key", "Return")
            click("Choose", "Save recording to", pointer=True)
            wait(lambda: find(f"Saving to  {destination}", frame="Edit recording — Captures"))
            find("Saved filename", frame="Edit recording — Captures").queryEditableText().setTextContents("parity-output")
            click("Save", "Edit recording — Captures")
            exported = wait(lambda: next(destination.glob("parity-output*.mp4"), None))
            video = stream_metadata(exported)[0]
            assert (video["width"], video["height"]) == (320, 200), video
            exported_duration = float(video.get("duration", 0))
            assert 0.3 < exported_duration < duration / 1_000, (duration, exported_duration)

            format_combo = wait(lambda: find("Format", "combo box", "Edit recording — Captures"))
            format_combo.queryAction().doAction(0)
            command("xdotool", "key", "Home", "Down", "Return")
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
                    command("xdotool", "mousemove", 10, 10, "mousemove", 650, 300)
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
            click("Play preview", "Edit recording — Captures")
            wait(lambda: len(subprocess.run(["pgrep", "-P", str(process.pid), "ffplay"], capture_output=True, text=True).stdout.split()) == 2)
            click("Mute system audio", "Edit recording — Captures")
            wait(lambda: len(subprocess.run(["pgrep", "-P", str(process.pid), "ffplay"], capture_output=True, text=True).stdout.split()) == 1)
            click("Mute microphone", "Edit recording — Captures")
            wait(lambda: not subprocess.run(["pgrep", "-P", str(process.pid), "ffplay"], capture_output=True).stdout)
            print("PASS audio: two headless source streams play independently and each mute removes its child")
        finally:
            stop(process)
            log.close()


if __name__ == "__main__":
    main()
