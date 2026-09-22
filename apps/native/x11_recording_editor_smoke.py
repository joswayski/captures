#!/usr/bin/python3
"""Real frame/geometry/audio/export input on a disposable private X11 desktop."""
import argparse
from array import array
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import re
import select
import shutil
import subprocess
import threading
import time

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

from x11_capture_smoke import ScreenSaver


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--appearance", choices=("light", "dark"), default="dark")
    parser.add_argument("--audio", action="store_true", help="Exercise separate system/microphone export controls")
    parser.add_argument("--presets", action="store_true", help="Exercise output presets on a portrait source")
    parser.add_argument("--crop-aspect", action="store_true", help="Exercise locked/unlocked numeric crop dimensions")
    parser.add_argument("--estimate", action="store_true", help="Exercise exact size estimates and missing-source retry")
    parser.add_argument("--timeline", action="store_true", help="Exercise graphical trim staging, keyboard input and export")
    parser.add_argument("--thumbnails", action="store_true", help="Exercise source thumbnails, cancellation, failure/retry and trim")
    parser.add_argument("--playback", action="store_true", help="Exercise silent motion, pause/resume, trim EOF, failure and close")
    args = parser.parse_args()
    if args.thumbnails:
        args.timeline = True
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env.update(WGPU_BACKEND="gl", WINIT_X11_SCALE_FACTOR="1", XDG_SESSION_TYPE="x11")
    for variable in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR"):
        path = output / variable.lower()
        path.mkdir(mode=0o700)
        env[variable] = str(path)
    children, logs = [], []
    loop = None

    def spawn(name, command, announce=False):
        stdout = subprocess.PIPE if announce else (output / f"{name}.jsonl").open("w")
        stderr = (output / f"{name}.stderr.txt").open("w")
        logs.append(stderr)
        if not announce:
            logs.append(stdout)
        child = subprocess.Popen(command, env=env, stdout=stdout, stderr=stderr)
        children.append(child)
        if announce:
            assert select.select([child.stdout], [], [], 10)[0], name
            return child.stdout.readline().decode().strip()
        return child

    def run(*command):
        return subprocess.check_output(command, env=env, timeout=40)

    def windows(title):
        found = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", f"^{re.escape(title)}"],
                               env=env, capture_output=True, text=True, timeout=5)
        assert found.returncode in (0, 1), found.stderr
        return found.stdout.split()

    def shot(window, name):
        if window != "root":
            idle(window)
        time.sleep(.5)
        run("import", "-window", window, str(output / f"{name}.png"))

    def wait(predicate, message):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if value := predicate():
                return value
            time.sleep(.1)
        shot("root", "timeout-desktop")
        raise AssertionError(message)

    def idle(window):
        # Apply starts on the UI thread; the old idle title can remain visible
        # briefly after the injected click. A single title read can type into
        # disabled controls, losing the next destination under software-GL load.
        since = None

        def settled():
            nonlocal since
            if "Working…" in run("xdotool", "getwindowname", window).decode():
                since = None
            elif since is None:
                since = time.monotonic()
            return since is not None and time.monotonic() - since >= .5

        wait(settled, "worker presentation settled")

    def click(window, x, y):
        idle(window)
        run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
            "sleep", ".2", "mousemove", "--sync", "--window", window, str(x), str(y),
            "sleep", ".2", "mousedown", "1", "sleep", ".15", "mouseup", "1", "sleep", ".3")

    def field(window, x, y, value):
        click(window, x, y)
        run("xdotool", "key", "ctrl+a")
        run("xdotool", "type", "--clearmodifiers", "--delay", "35", "--", str(value))
        run("xdotool", "key", "Return", "sleep", ".3")

    def close(window):
        run("xdotool", "windowactivate", "--sync", window, "key", "alt+F4", "sleep", ".5")

    def dominant(path, channel, at=None):
        if at is None:
            pixel = run("convert", str(path), "-crop", "1x1+480+220", "-depth", "8", "rgb:-")
        else:
            pixel = run("ffmpeg", "-v", "error", "-ss", str(at), "-i", str(path),
                        "-frames:v", "1", "-vf", "crop=2:2:160:90", "-f", "rawvideo", "-pix_fmt", "rgb24", "-")[:3]
        assert len(pixel) == 3 and pixel[channel] > 90, (path, pixel)
        assert all(pixel[channel] > pixel[i] + 40 for i in range(3) if i != channel), (path, pixel)

    try:
        env["DISPLAY"] = ":" + spawn("xvfb", ["Xvfb", "-displayfd", "1", "-screen", "0", "1280x1200x24", "-dpi", "96", "-nolisten", "tcp"], True)
        address = spawn("dbus", ["dbus-daemon", "--session", "--nofork", "--print-address=1"], True)
        env["DBUS_SESSION_BUS_ADDRESS"] = env["DBUS_SYSTEM_BUS_ADDRESS"] = address
        DBusGMainLoop(set_as_default=True)
        bus = dbus.bus.BusConnection(address)
        name = dbus.service.BusName("org.freedesktop.ScreenSaver", bus=bus, do_not_queue=True)
        saver = ScreenSaver(name, "/org/freedesktop/ScreenSaver")
        loop = GLib.MainLoop()
        thread = threading.Thread(target=loop.run, daemon=True)
        thread.start()
        spawn("openbox", ["openbox", "--sm-disable"])
        history = output / "history"
        artifact_id = "032135f1-11e4-4a47-893d-2368c079a6ba"
        artifact = history / artifact_id
        artifact.mkdir(parents=True)
        source = output / "source.mp4"
        source_width, source_height = (640, 1440) if args.presets else (320, 180)
        source_size = f"{source_width}x{source_height}"
        segment_seconds = 2 if args.playback else 1
        audio_inputs = []
        audio_filters = ""
        audio_maps = []
        if args.audio:
            # Real recordings can retain a playback mix followed by separate
            # system/mic tracks. Unequal stereo tones catch swapped tracks,
            # accidental reuse of the mix, ignored gains and ignored mono.
            audio_inputs = ["-f", "lavfi", "-i",
                "aevalsrc=0.1*sin(2*PI*440*t)|0.05*sin(2*PI*440*t):s=48000:d=3",
                "-f", "lavfi", "-i",
                "aevalsrc=0.04*sin(2*PI*880*t)|0.12*sin(2*PI*880*t):s=48000:d=3"]
            audio_filters = (";[3:a]asplit=2[system][s];[4:a]asplit=2[mic][m];"
                             "[s][m]amix=inputs=2:normalize=0[mixed]")
            audio_maps = ["-map", "[mixed]", "-map", "[system]", "-map", "[mic]", "-c:a", "aac", "-b:a", "256k"]
        run("ffmpeg", "-v", "error", "-f", "lavfi", "-i", f"color=red:s={source_size}:r=10:d={segment_seconds}",
            "-f", "lavfi", "-i", f"color=green:s={source_size}:r=10:d={segment_seconds}",
            "-f", "lavfi", "-i", f"color=blue:s={source_size}:r=10:d={segment_seconds}",
            *audio_inputs, "-filter_complex",
            "[0:v][1:v][2:v]concat=n=3:v=1:a=0,drawbox=x=15:y=10:w=45:h=25:color=white:t=fill[v]" + audio_filters,
            "-map", "[v]", *audio_maps, "-c:v", "mpeg4", "-q:v", "2", str(source))
        run("ffmpeg", "-v", "error", "-i", str(source), "-frames:v", "1", str(artifact / "preview.png"))
        metadata = artifact / "metadata.json"
        metadata.write_text(json.dumps({
            "id": artifact_id, "kind": "video", "preview_url": "", "full_url": "",
            "width": source_width, "height": source_height, "size_bytes": source.stat().st_size,
            "created_at": datetime.now(timezone.utc).isoformat(), "mode": None,
            "saved_path": str(source), "mime_type": "video/mp4", "duration_ms": 3000 * segment_seconds,
            "target": {"type": "display", "display_id": "fixture"},
            "has_system_audio": args.audio, "has_microphone_audio": args.audio, "dropped_frames": 0,
        }))
        original, original_metadata = source.read_bytes(), metadata.read_bytes()
        exports = output / "exports"
        exports.mkdir()
        settings = output / "settings.json"
        settings.write_text(json.dumps({"settings_schema_version": 5, "appearance": args.appearance,
            "theme": "mustard", "output_directory": str(exports), "launch_at_login": False,
            "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
            "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10",
            "auto_copy_to_clipboard": False, "show_mini_previews": False}))
        if args.thumbnails:
            # Delay only the real sprite command to exercise cancellation without
            # racing a tiny fixture. Pixel generation still uses real FFmpeg.
            tools = output / "tools"
            tools.mkdir()
            started = output / "thumbnail-calls.txt"
            allowed = output / "allow-thumbnails"
            ffmpeg = shutil.which("ffmpeg")
            assert ffmpeg
            wrapper = tools / "ffmpeg"
            wrapper.write_text(
                "#!/usr/bin/python3\nimport os, sys, time\nfrom pathlib import Path\n"
                "if any('tile=' in arg for arg in sys.argv[1:]):\n"
                f"    with Path({str(started)!r}).open('a') as log: log.write('call\\n')\n"
                f"    while not Path({str(allowed)!r}).exists(): time.sleep(.05)\n"
                f"os.execv({ffmpeg!r}, [{ffmpeg!r}, *sys.argv[1:]])\n")
            wrapper.chmod(0o755)
            env["PATH"] = str(tools) + os.pathsep + env["PATH"]
        app = spawn("app", [str(binary), "--live", "--history-root", str(history),
                    "--settings-file", str(settings), "--quit-after", "900"])
        root = wait(lambda: windows("Captures"), "History")[0]
        time.sleep(1)
        shot(root, "history")
        click(root, 795, 191)
        editor = wait(lambda: windows("Recording editor"), "recording editor opens")[0]
        run("xdotool", "windowmove", "--sync", editor, "80", "60")
        run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
        if args.thumbnails:
            wait(started.exists, "initial source thumbnail request")
            run("xdotool", "windowactivate", "--sync", editor, "windowfocus", "--sync", editor,
                "mousemove", "--sync", "--window", editor, "80", "794", "sleep", ".5")
            run("import", "-window", editor, str(output / "thumbnails-loading.png"))
            # Deliberately bypass idle(): this cancels an accepted running job.
            run("xdotool", "mousedown", "1", "sleep", ".15", "mouseup", "1", "sleep", ".3")
            shot(editor, "thumbnails-cancelled")
            allowed.touch()
            missing = output / "temporarily-moved.mp4"
            source.rename(missing)
            try:
                click(editor, 250, 794)
                shot(editor, "thumbnails-missing-source")
            finally:
                missing.rename(source)
            click(editor, 250, 794)
            shot(editor, "thumbnails-retried")
            assert started.read_text().splitlines() == ["call"] * 3
            assert len(list(history.glob("*/metadata.json"))) == 1 and not list(exports.iterdir())
            for x, channel in ((120, 0), (500, 1), (890, 2)):
                pixel = run("convert", str(output / "thumbnails-retried.png"),
                            "-crop", f"1x1+{x}+563", "-depth", "8", "rgb:-")
                assert len(pixel) == 3 and pixel[channel] > 90, (x, pixel)
                assert all(pixel[channel] > pixel[i] + 40 for i in range(3) if i != channel), (x, pixel)
        wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(), "decode")
        shot(editor, "original")
        dominant(output / "original.png", 0)
        run("xdotool", "windowminimize", root, "sleep", ".5")
        estimate_expectations = {}
        if args.playback:
            def motion_click():
                # Pause must work during an active decoder; never wait for idle.
                run("xdotool", "mousemove", "--sync", "--window", editor, "192", "57",
                    "mousedown", "1", "sleep", ".08", "mouseup", "1")

            def playing():
                return "Working…" in run("xdotool", "getwindowname", editor).decode()

            def position():
                click(editor, 136, 520)
                run("xdotool", "key", "ctrl+a", "ctrl+c", "sleep", ".1")
                return int(run("xclip", "-selection", "clipboard", "-o").strip())

            field(editor, 98, 598, 1500)
            field(editor, 211, 598, 4500)
            motion_click()
            time.sleep(.3)
            assert not playing(), "unapplied trim gates Play"
            click(editor, 793, 882)
            shot(editor, "playback-accepted")
            dominant(output / "playback-accepted.png", 0)
            # Record real presentation rather than turning fixture PNGs into a video.
            recording = spawn("playback-capture", ["ffmpeg", "-v", "error", "-f", "x11grab",
                "-framerate", "15", "-video_size", "960x900", "-i", env["DISPLAY"] + "+80,60",
                "-t", "12", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
                str(output / "playback-motion.mp4")])
            motion_click()
            wait(playing, "Play owns decoder")
            time.sleep(.9)
            run("import", "-window", editor, str(output / "playback-running.png"))
            dominant(output / "playback-running.png", 1)
            motion_click()
            idle(editor)
            shot(editor, "playback-paused")
            paused_at = position()
            assert 2000 <= paused_at < 4000, paused_at
            # Leave field focus before comparing frozen frame+playhead pixels.
            click(editor, 700, 470)
            shot(editor, "playback-paused-stable-a")
            time.sleep(.8)
            shot(editor, "playback-paused-stable-b")
            a = run("convert", str(output / "playback-paused-stable-a.png"), "-crop", "940x410+8+85", "rgba:-")
            b = run("convert", str(output / "playback-paused-stable-b.png"), "-crop", "940x410+8+85", "rgba:-")
            assert a == b, "Pause has no late frame or advancing timestamp"
            motion_click()
            wait(playing, "resume decoder")
            idle(editor)
            shot(editor, "playback-ended")
            ended_at = position()
            assert 4400 <= ended_at < 4500, ended_at
            dominant(output / "playback-ended.png", 2)
            motion_click()
            wait(playing, "EOF replay decoder")
            time.sleep(.2)
            # Losing focus must cancel without losing accepted edits.
            run("xdotool", "windowmap", root, "windowactivate", "--sync", root)
            idle(editor)
            run("xdotool", "windowactivate", "--sync", editor)
            replay_at = position()
            assert 1500 <= replay_at < 3000, replay_at
            assert recording.wait(timeout=20) == 0
            missing = output / "temporarily-moved.mp4"
            source.rename(missing)
            try:
                motion_click()
                idle(editor)
                shot(editor, "playback-error")
                dominant(output / "playback-error.png", 0)
                assert position() == 0, "failure restores accepted still and source position"
            finally:
                missing.rename(source)
            motion_click()
            wait(playing, "failure retry decoder")
            time.sleep(.7)
            motion_click()
            idle(editor)
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            shot(editor, "playback-minimum-paused")
            assert len(list(history.glob("*/metadata.json"))) == 1 and not list(exports.iterdir())
            assert source.read_bytes() == original and metadata.read_bytes() == original_metadata
            motion_click()
            wait(playing, "minimum Play")
            run("import", "-window", editor, str(output / "playback-minimum-running.png"))
            close(editor)
            idle(editor)
            shot(editor, "playback-close-confirmation")
            assert windows("Recording editor"), "accepted unsaved edits still require discard"
            # Keep editing, then save the accepted edit (not a playback range).
            run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
            click(editor, 60, 794)
            destination = exports / "playback-trim.mp4"
            field(editor, 360, 838, destination)
            click(editor, 899, 882)
            wait(lambda: len(list(history.glob("*/metadata.json"))) == 2, "save after playback")
            info = json.loads(run("ffprobe", "-v", "error", "-show_format", "-of", "json", str(destination)))
            assert abs(float(info["format"]["duration"]) - 3) < .15, info
            dominant(destination, 0, .1)
            dominant(destination, 2, 2.7)
            motion_click()
            wait(playing, "saved replay")
            close(editor)
            wait(lambda: not windows("Recording editor"), "clean close waits for decoder")
            close(root)
            wait(lambda: app.poll() is not None, "playback quit")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({"passed": True,
                "appearance": args.appearance, "paused_at_ms": paused_at, "ended_at_ms": ended_at,
                "replay_at_ms": replay_at, "checks": ["staged-play-gate", "temporal-motion",
                    "pause-stable", "resume", "exclusive-trim-end", "replay", "focus-pause",
                    "failure-restores-still", "retry", "minimum-layout", "dirty-close",
                    "accepted-export-duration-colors", "source-immutable", "clean-close"]}, indent=2) + "\n")
            print("PASS silent playback: real motion, pause/resume/EOF, failure/retry, close, accepted export and immutable source")
            return
        if args.timeline:
            def read_time(x):
                click(editor, x, 598)
                run("xdotool", "key", "ctrl+a", "ctrl+c", "sleep", ".2")
                return int(run("xclip", "-selection", "clipboard", "-o").strip())

            def drag(x, delta, cancel=False):
                run("xdotool", "mousemove", "--sync", "--window", editor, str(x), "563",
                    "mousedown", "1", "sleep", ".15", "mousemove_relative", "--sync", "--",
                    str(delta), "0", "sleep", ".2")
                if cancel:
                    run("xdotool", "key", "Escape", "mousemove_relative", "--sync", "--", "100", "0")
                run("xdotool", "mouseup", "1", "sleep", ".2")

            # Grab inside each grip, rather than at the interval boundary.
            # Time is measured from that original pointer, not absolute x.
            drag(55, 2)
            assert read_time(98) == 0, "subthreshold drag cannot jump trim start"
            drag(55, 300)
            start = read_time(98)
            assert 1000 <= start <= 1100, ("start drag", start)
            drag(938, -200)
            end = read_time(211)
            assert 2250 <= end <= 2400, ("end drag", end)
            drag(355, 15, cancel=True)
            cancelled_start = read_time(98)
            assert 45 <= cancelled_start - start <= 60, (start, cancelled_start)
            # A click focuses a handle without changing its value. Keyboard
            # adjustment must happen once, even across egui layout passes.
            click(editor, 370, 563)
            run("xdotool", "key", "Right", "sleep", ".2")
            start = read_time(98)
            assert start == cancelled_start + 1, ("focused keyboard step", start, cancelled_start)
            shot(editor, "timeline-staged")
            dominant(output / "timeline-staged.png", 0)
            destination = exports / "timeline.mp4"
            field(editor, 360, 838, destination)
            click(editor, 899, 882)
            assert not destination.exists() and len(list(history.glob("*/metadata.json"))) == 1
            click(editor, 793, 882)
            field(editor, 136, 520, 1500)
            click(editor, 222, 520)
            shot(editor, "timeline-applied")
            dominant(output / "timeline-applied.png", 1)
            click(editor, 899, 882)
            wait(lambda: len(list(history.glob("*/metadata.json"))) == 2, "timeline export published")
            info = json.loads(run("ffprobe", "-v", "error", "-show_format", "-of", "json", str(destination)))
            expected_seconds = (end - start) / 1000
            assert 1.1 < expected_seconds < 1.4
            assert abs(float(info["format"]["duration"]) - expected_seconds) < .15, info
            dominant(destination, 1, .1)
            dominant(destination, 2, 1.0)
            click(editor, 87, 882)
            click(editor, 793, 882)
            click(editor, 899, 882)
            gif = destination.with_suffix(".gif")
            wait(lambda: len(list(history.glob("*/metadata.json"))) == 3, "timeline GIF published")
            dominant(gif, 1, .1)
            dominant(gif, 2, 1.0)
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            shot(editor, "timeline-minimum-saved")
            assert source.read_bytes() == original and metadata.read_bytes() == original_metadata
            if args.thumbnails:
                assert started.read_text().splitlines() == ["call"] * 3, "edits/seek/export never regenerate source thumbnails"
            close(editor)
            wait(lambda: not windows("Recording editor"), "saved timeline editor closes")
            close(root)
            wait(lambda: app.poll() is not None, "timeline quit")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
                "trim_start_ms": start, "trim_end_ms": end,
                "checks": ["subthreshold-click", "start-drag", "end-drag", "escape-retains-last-stage",
                    "focused-keyboard-step", "accepted-frame-retained", "unapplied-save-gate",
                    "source-relative-seek", "mp4-duration", "mp4-green-blue", "gif-green-blue",
                    "history-publication", "minimum-controls", "immutable-source", "saved-close-quit"]}, indent=2) + "\n")
            print("PASS recording timeline: pointer/keyboard staging, cancellation, save gate, MP4/GIF pixels, immutable source")
            return
        if args.estimate:
            click(editor, 681, 882)
            shot(editor, "estimate-original")
            estimate_expectations["estimate-original.png"] = f"{len(original)} bytes (exact)"
            assert len(list(history.glob("*/metadata.json"))) == 1 and not list(exports.iterdir())
            missing = output / "temporarily-moved.mp4"
            source.rename(missing)
            try:
                click(editor, 681, 882)
                shot(editor, "estimate-missing-source")
            finally:
                missing.rename(source)
            click(editor, 681, 882)
            shot(editor, "estimate-retried")
            estimate_expectations["estimate-retried.png"] = f"{len(original)} bytes (exact)"
            if args.audio:
                run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
                field(editor, 201, 866, 50)
                click(editor, 793, 1082)
                click(editor, 681, 1082)
                shot(editor, "estimate-audio-approximate")
                estimate_expectations["estimate-audio-approximate.png"] = f"≈ {len(original)} bytes"
                field(editor, 201, 866, 100)
                click(editor, 793, 1082)
                run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
        if args.crop_aspect:
            run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
            click(editor, 22, 683)
            field(editor, 208, 727, source_width // 2)
            shot(editor, "locked-width-staged")

            def save_crop(name, expected):
                path = exports / f"{name}.mp4"
                field(editor, 360, 1038, path)
                click(editor, 899, 1082)
                assert not path.exists(), "unapplied crop gates save"
                click(editor, 793, 1082)
                shot(editor, f"{name}-preview")
                count = len(list(history.glob("*/metadata.json")))
                click(editor, 899, 1082)
                wait(lambda: len(list(history.glob("*/metadata.json"))) == count + 1, name)
                info = json.loads(run("ffprobe", "-v", "error", "-show_streams", "-of", "json", str(path)))
                stream = next(s for s in info["streams"] if s["codec_type"] == "video")
                assert (stream["width"], stream["height"]) == expected, (name, stream)

            save_crop("locked-width", (source_width // 2, source_height // 2 // 2 * 2))
            field(editor, 309, 727, source_height)
            save_crop("locked-height", (source_width, source_height))
            click(editor, 205, 683)  # Unlock. Only width changes now.
            field(editor, 208, 727, 160)
            save_crop("unlocked-width", (160, source_height))
            # Relocking uses the current crop ratio, not the original source.
            click(editor, 205, 683)
            field(editor, 309, 727, source_height // 2)
            save_crop("relocked-height", (80, source_height // 2 // 2 * 2))
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            run("xdotool", "mousemove", "--window", editor, "690", "380", "click", "--repeat", "12", "--delay", "60", "5", "sleep", ".5")
            shot(editor, "minimum-locked-crop")
            assert source.read_bytes() == original and metadata.read_bytes() == original_metadata
            close(editor)
            wait(lambda: not windows("Recording editor"), "saved editor closes")
            close(root)
            wait(lambda: app.poll() is not None, "quit")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
                "estimate_label_expectations_for_visual_inspection": estimate_expectations,
                "checks": ["locked-width", "locked-height", "unlocked-width", "relocked-current-ratio",
                    "unapplied-save-gate", "export-dimensions", "history-publication", "minimum-controls",
                    "immutable-source", "saved-close-and-quit"]}, indent=2) + "\n")
            print("PASS recording crop aspect: locked width/height, unlocked, relocked, save gate, exports, immutable source")
            return
        # Numeric fields exercise exact source-relative times, independently of
        # slider geometry and the trim start.
        field(editor, 136, 520, 1500)
        click(editor, 222, 520)
        shot(editor, "seek-green")
        dominant(output / "seek-green.png", 1)
        field(editor, 211, 598, 1100)
        field(editor, 98, 598, 2600)
        click(editor, 793, 882)  # Apply edits stays in the fixed save bar.
        shot(editor, "invalid-trim")
        dominant(output / "invalid-trim.png", 1)
        run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
        shot(editor, "minimum-error")
        run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
        field(editor, 98, 598, 1100)
        field(editor, 227, 598, 2300)
        click(editor, 793, 882)
        shot(editor, "trimmed")
        dominant(output / "trimmed.png", 1)
        if args.estimate:
            click(editor, 681, 882)
            shot(editor, "estimate-trimmed")
            assert len(list(history.glob("*/metadata.json"))) == 1 and not list(exports.iterdir())
        close(root)
        assert app.poll() is None and windows("Recording editor"), "dirty editor blocks quit"
        shot(editor, "quit-guard")
        close(editor)
        assert windows("Recording editor"), "dirty editor requires confirmation"
        shot(editor, "close-confirmation")
        click(editor, 55, 794)  # Keep editing, immediately above the fixed destination row.
        run("xdotool", "windowminimize", root, "sleep", ".5")
        destination = exports / "trimmed.mp4"
        field(editor, 360, 838, destination)
        click(editor, 899, 882)
        wait(lambda: len(list(history.glob("*/metadata.json"))) == 2, "export published in History")
        info = json.loads(run("ffprobe", "-v", "error", "-show_format", "-show_streams", "-of", "json", str(destination)))
        assert abs(float(info["format"]["duration"]) - 1.2) < .15, info
        assert (info["streams"][0]["width"], info["streams"][0]["height"]) == (source_width, source_height)
        dominant(destination, 1, .2)
        dominant(destination, 2, 1.0)
        shot(editor, "saved")
        saved_bytes = destination.read_bytes()
        if args.estimate:
            estimate_expectations["estimate-trimmed.png"] = f"{len(saved_bytes)} bytes (exact)"
        click(editor, 899, 882)
        shot(editor, "collision")
        assert destination.read_bytes() == saved_bytes and len(list(history.glob("*/metadata.json"))) == 2
        click(editor, 87, 882)
        if args.estimate:
            click(editor, 681, 882)  # Unapplied GIF must not reuse the MP4 estimate.
            shot(editor, "estimate-staged-format")
        click(editor, 899, 882)  # Format changes cannot save unaccepted preview settings.
        assert not destination.with_suffix(".gif").exists()
        click(editor, 793, 882)
        if args.estimate:
            click(editor, 681, 882)
            shot(editor, "estimate-gif")
        click(editor, 899, 882)
        wait(lambda: len(list(history.glob("*/metadata.json"))) == 3, "GIF published in History")
        gif = destination.with_suffix(".gif")
        if args.estimate:
            estimate_expectations["estimate-gif.png"] = f"{gif.stat().st_size} bytes (exact)"
        dominant(gif, 1, .2)
        dominant(gif, 2, 1.0)
        shot(editor, "gif-saved")

        # Asymmetric crop and independently sized output catch ignored origins,
        # resize-only implementations, and accidental loss of the accepted trim.
        run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
        click(editor, 22, 683)  # Crop recording.
        click(editor, 205, 683)  # Unlock for independent asymmetric crop fields.
        field(editor, 51, 727, 10)
        field(editor, 114, 727, 6)
        field(editor, 208, 727, source_width)  # Valid width alone, invalid with X=10.
        field(editor, 309, 727, 90)
        click(editor, 793, 1082)
        shot(editor, "invalid-crop")
        dominant(output / "invalid-crop.png", 1)
        crop_destination = exports / "cropped.mp4"
        click(editor, 33, 1082)  # MP4.
        field(editor, 360, 1038, crop_destination)
        click(editor, 899, 1082)  # Save remains gated while the crop is unapplied.
        assert not crop_destination.exists() and len(list(history.glob("*/metadata.json"))) == 3
        field(editor, 208, 727, 160)
        click(editor, 22, 771)  # Custom output size.
        field(editor, 78, 815, 81)
        field(editor, 170, 815, 61)
        shot(editor, "crop-staged")
        click(editor, 793, 1082)
        shot(editor, "cropped")
        dominant(output / "cropped.png", 1)
        # Frame is 80x60 after shared even rounding, fitted into the 380px-tall
        # preview. Both samples lie inside the translated/scaled white box;
        # omitting the crop or either origin makes at least one sample green.
        for x, y in ((253, 110), (353, 180)):
            pixel = run("convert", str(output / "cropped.png"), "-crop", f"1x1+{x}+{y}", "-depth", "8", "rgb:-")
            assert len(pixel) == 3 and min(pixel) > 210, (x, y, pixel)
        run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
        run("xdotool", "mousemove", "--window", editor, "690", "380", "click", "--repeat", "12", "--delay", "60", "5", "sleep", ".5")
        shot(editor, "minimum-crop-controls")
        run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
        run("xdotool", "mousemove", "--window", editor, "690", "380", "click", "--repeat", "20", "--delay", "60", "4", "sleep", ".5")
        click(editor, 899, 1082)
        wait(lambda: len(list(history.glob("*/metadata.json"))) == 4, "cropped MP4 in History")
        click(editor, 87, 1082)
        click(editor, 793, 1082)
        click(editor, 899, 1082)
        wait(lambda: len(list(history.glob("*/metadata.json"))) == 5, "cropped GIF in History")
        for path in (crop_destination, crop_destination.with_suffix(".gif")):
            info = json.loads(run("ffprobe", "-v", "error", "-show_format", "-show_streams", "-of", "json", str(path)))
            assert abs(float(info["format"]["duration"]) - 1.2) < .15, info
            stream = info["streams"][0]
            assert (stream["width"], stream["height"]) == (80, 60), info
            assert stream.get("sample_aspect_ratio", "1:1") in ("1:1", "N/A"), info
            frame = run("ffmpeg", "-v", "error", "-ss", "0.2", "-i", str(path),
                        "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-")
            assert len(frame) == 80 * 60 * 3, (path, len(frame))
            for x, y in ((4, 4), (20, 15)):
                pixel = frame[(y * 80 + x) * 3:(y * 80 + x) * 3 + 3]
                assert min(pixel) > 210, (path, x, y, pixel)
            pixel = frame[(30 * 80 + 40) * 3:(30 * 80 + 40) * 3 + 3]
            assert pixel[1] > 90 and pixel[1] > max(pixel[0], pixel[2]) + 40, (path, pixel)
        shot(editor, "crop-saved")
        run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
        shot(editor, "minimum-saved")

        # Request wider-than-encoder output without a huge frame allocation.
        # MP4 fits the shared encoder limit; GIF retains the explicit dimensions.
        run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
        run("xdotool", "mousemove", "--window", editor, "690", "380", "click", "--repeat", "20", "--delay", "60", "4", "sleep", ".5")
        field(editor, 227, 598, 1300)
        field(editor, 78, 815, 4001)
        field(editor, 170, 815, 601)
        click(editor, 33, 1082)
        large_destination = exports / "encoder-sized.mp4"
        field(editor, 360, 1038, large_destination)
        click(editor, 793, 1082)
        shot(editor, "mp4-encoder-preview")
        click(editor, 899, 1082)
        wait(lambda: len(list(history.glob("*/metadata.json"))) == 6, "encoder-sized MP4 in History")
        click(editor, 87, 1082)
        click(editor, 899, 1082)
        assert not large_destination.with_suffix(".gif").exists()
        shot(editor, "format-staged")
        click(editor, 793, 1082)
        shot(editor, "gif-sized-preview")
        click(editor, 899, 1082)
        wait(lambda: len(list(history.glob("*/metadata.json"))) == 7, "explicit-sized GIF in History")
        for path, width, height in ((large_destination, 3840, 576), (large_destination.with_suffix(".gif"), 4000, 600)):
            info = json.loads(run("ffprobe", "-v", "error", "-show_format", "-show_streams", "-of", "json", str(path)))
            stream = info["streams"][0]
            assert (stream["width"], stream["height"]) == (width, height), info
            assert abs(float(info["format"]["duration"]) - .2) < .1, info
            frame = run("ffmpeg", "-v", "error", "-i", str(path), "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-")
            assert len(frame) == width * height * 3
            white = frame[(80 * width + 400) * 3:(80 * width + 400) * 3 + 3]
            green = frame[(300 * width + 2000) * 3:(300 * width + 2000) * 3 + 3]
            assert min(white) > 210, (path, white)
            assert green[1] > 90 and green[1] > max(green[0], green[2]) + 40, (path, green)
        shot(editor, "format-saved")
        audio_checks = []
        if args.audio:
            click(editor, 22, 683)  # Remove crop and resize to expose audio rows.
            click(editor, 22, 727)
            field(editor, 227, 598, 2300)
            click(editor, 33, 1082)
            field(editor, 201, 866, 25)
            field(editor, 231, 910, 175)
            shot(editor, "audio-staged")
            pending = exports / "pending-audio.mp4"
            field(editor, 360, 1038, pending)
            click(editor, 899, 1082)
            assert not pending.exists(), "unapplied audio must gate save"
            click(editor, 793, 1082)
            shot(editor, "audio-applied")

            def audio_export(filename):
                path = exports / filename
                count = len(list(history.glob("*/metadata.json")))
                field(editor, 360, 1038, path)
                click(editor, 899, 1082)
                wait(lambda: len(list(history.glob("*/metadata.json"))) == count + 1, filename)
                info = json.loads(run("ffprobe", "-v", "error", "-show_streams", "-of", "json", str(path)))
                return path, [s for s in info["streams"] if s["codec_type"] == "audio"]

            def assert_tones(path, channels, expected):
                samples = array("f", run("ffmpeg", "-v", "error", "-ss", "0.3", "-i", str(path),
                    "-map", "0:a:0", "-t", "0.2", "-ar", "48000", "-f", "f32le", "-"))
                assert len(samples) == 9600 * channels, (path, len(samples))
                for channel in range(channels):
                    values = samples[channel::channels]
                    for frequency, amplitude in zip((440, 880), expected[channel]):
                        # Phase-independent sinusoid projection: integer periods
                        # independently measure each generated tone, not total RMS.
                        real = sum(v * math.cos(2 * math.pi * frequency * i / 48000) for i, v in enumerate(values))
                        imaginary = sum(v * math.sin(2 * math.pi * frequency * i / 48000) for i, v in enumerate(values))
                        measured = 2 * math.hypot(real, imaginary) / len(values)
                        assert abs(measured - amplitude) < max(.001, amplitude * .12), (path, channel, frequency, measured, amplitude)

            mixed, streams = audio_export("volume.mp4")
            assert len(streams) == 1 and streams[0]["channels"] == 2, streams
            # System stays stereo, microphone is centered by the shared mixer.
            assert_tones(mixed, 2, ((.025, .14), (.0125, .14)))
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            run("xdotool", "mousemove", "--window", editor, "690", "380", "click", "--repeat", "15", "--delay", "60", "5", "sleep", ".5")
            shot(editor, "minimum-audio-controls")
            run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
            run("xdotool", "mousemove", "--window", editor, "690", "380", "click", "--repeat", "20", "--delay", "60", "4", "sleep", ".5")
            click(editor, 87, 1082)
            click(editor, 793, 1082)
            shot(editor, "gif-audio-disabled")
            click(editor, 244, 866)  # Disabled mute must not change the MP4 settings.
            _, streams = audio_export("audio-free.gif")
            assert not streams
            click(editor, 33, 1082)
            click(editor, 793, 1082)
            restored, streams = audio_export("audio-restored.mp4")
            assert len(streams) == 1 and streams[0]["channels"] == 2, streams
            assert_tones(restored, 2, ((.025, .14), (.0125, .14)))
            click(editor, 274, 910)  # Mute microphone, retain system gain/stereo.
            click(editor, 793, 1082)
            system_only, streams = audio_export("system-only.mp4")
            assert len(streams) == 1 and streams[0]["channels"] == 2, streams
            assert_tones(system_only, 2, ((.025, 0), (.0125, 0)))
            click(editor, 244, 866)
            click(editor, 274, 910)
            click(editor, 22, 946)  # Microphone only, mono.
            click(editor, 793, 1082)
            microphone_only, streams = audio_export("microphone-only.mp4")
            assert len(streams) == 1 and streams[0]["channels"] == 1, streams
            assert_tones(microphone_only, 1, ((0, .14 * math.sqrt(2)),))
            shot(editor, "microphone-mono")
            click(editor, 274, 910)  # Both muted removes the audio stream.
            click(editor, 793, 1082)
            _, streams = audio_export("muted.mp4")
            assert not streams
            shot(editor, "audio-muted")
            for path, flags in ((system_only, (True, False)), (microphone_only, (False, True))):
                entry = next(json.loads(p.read_text()) for p in history.glob("*/metadata.json")
                             if json.loads(p.read_text()).get("saved_path") == str(path))
                assert (entry["has_system_audio"], entry["has_microphone_audio"]) == flags, entry
            audio_checks = ["audio-save-gate", "independent-track-gains", "minimum-audio-controls",
                "gif-no-audio", "gif-retains-mp4-audio", "microphone-mute", "system-mute",
                "mono-output", "both-muted-no-stream", "audio-history-flags"]
        preset_checks = []
        if args.presets:
            if not args.audio:
                click(editor, 22, 683)  # Clear previous crop/custom size.
                click(editor, 22, 727)
                field(editor, 227, 598, 2300)
                click(editor, 33, 1082)
            for name, choice_y, expected in (("720", 852, (320, 720)), ("1080", 808, (480, 1080)), ("original", 764, (640, 1440))):
                click(editor, 300, 727)
                shot(editor, f"preset-{name}-menu")
                click(editor, 280, choice_y)
                path = exports / f"preset-{name}.mp4"
                field(editor, 360, 1038, path)
                click(editor, 899, 1082)
                assert not path.exists(), "unapplied preset must gate save"
                click(editor, 793, 1082)
                shot(editor, f"preset-{name}-preview")
                count = len(list(history.glob("*/metadata.json")))
                click(editor, 899, 1082)
                wait(lambda: len(list(history.glob("*/metadata.json"))) == count + 1, name)
                info = json.loads(run("ffprobe", "-v", "error", "-show_streams", "-of", "json", str(path)))
                stream = next(s for s in info["streams"] if s["codec_type"] == "video")
                assert (stream["width"], stream["height"]) == expected, (name, stream)
                dominant(path, 1, .2)
                dominant(path, 2, 1.0)
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            run("xdotool", "mousemove", "--window", editor, "690", "380", "click", "--repeat", "15", "--delay", "60", "5", "sleep", ".5")
            shot(editor, "minimum-resolution-controls")
            preset_checks = ["720p-preset", "1080p-preset", "original-preset", "preset-save-gate", "preset-export-pixels", "minimum-resolution-controls"]
        assert source.read_bytes() == original and metadata.read_bytes() == original_metadata
        close(editor)
        wait(lambda: not windows("Recording editor"), "saved editor closes")
        close(root)
        wait(lambda: app.poll() is not None, "quit")
        assert app.returncode == 0
        (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
            "estimate_label_expectations_for_visual_inspection": estimate_expectations,
            "checks": ["history-open", "decoded-frame", "source-relative-seek", "trim-preview",
                "failed-trim-retains-frame", "minimum-error", "dirty-quit-guard", "close-confirmation",
                "save-new", "duration", "dimensions", "export-green", "export-blue", "collision",
                "gif-green", "gif-blue", "minimum-saved", "immutable-source", "saved-close-and-quit",
                "worker-completion-with-minimized-root", "invalid-crop-retains-frame", "unapplied-save-gate",
                "cropped-preview", "minimum-crop-controls", "crop-preserves-trim", "even-output-dimensions",
                "mp4-crop-origin-and-resize", "gif-crop-origin-and-resize", "format-save-gate",
                "mp4-encoder-dimensions", "gif-explicit-dimensions", "format-specific-pixels"] + audio_checks + preset_checks,
            "source_sha256": hashlib.sha256(original).hexdigest()}, indent=2) + "\n")
        print(f"PASS recording editor: seeks, trim/crop/resize, MP4/GIF pixels, {len(audio_checks)} audio / {len(preset_checks)} preset checks, History, immutable source")
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
