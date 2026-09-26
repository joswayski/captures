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
    parser.add_argument("--external-media", action="store_true", help="Open a mixed external batch, preserve staged alias edits and export WebM safely as MP4")
    parser.add_argument("--audio", action="store_true", help="Exercise separate system/microphone export controls")
    parser.add_argument("--presets", action="store_true", help="Exercise output presets on a portrait source")
    parser.add_argument("--crop-aspect", action="store_true", help="Exercise locked/unlocked numeric crop dimensions")
    parser.add_argument("--estimate", action="store_true", help="Exercise exact size estimates and missing-source retry")
    parser.add_argument("--estimate-delta", action="store_true", help="Render exact and sampled size deltas against an immutable source")
    parser.add_argument("--comparison", action="store_true", help="Exercise encoded before/after, hide, failure/retry and immutable identity")
    parser.add_argument("--replace-original", action="store_true", help="Exercise confirmed replacement, cancellation and same-session rebase")
    parser.add_argument("--timeline", action="store_true", help="Exercise graphical trim staging, keyboard input and export")
    parser.add_argument("--thumbnails", action="store_true", help="Exercise source thumbnails, cancellation, failure/retry and trim")
    parser.add_argument("--playback", action="store_true", help="Exercise silent motion, pause/resume, trim EOF, failure and close")
    parser.add_argument("--sound", action="store_true", help="Exercise opt-in playback through an isolated PulseAudio sink (not physical audio acceptance)")
    parser.add_argument("--graphical-crop", action="store_true", help="Exercise source-view crop handles, cache, staging and export")
    parser.add_argument("--preview-scale", action="store_true", help="Exercise display-only Fit/100% and bounded preview scrolling")
    parser.add_argument("--gif-frame-rate", action="store_true", help="Exercise staged GIF cadence and real exported frame counts")
    parser.add_argument("--gif-quality", action="store_true", help="Exercise quality-dependent GIF palette output")
    parser.add_argument("--gif-width", action="store_true", help="Exercise GIF width caps without compounding accepted dimensions")
    parser.add_argument("--maximum-size", action="store_true", help="Exercise accepted size caps, encoder retries and immutable publication")
    args = parser.parse_args()
    if args.sound:
        args.audio = True
    if args.thumbnails:
        args.timeline = True
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env.update(WGPU_BACKEND="gl", WINIT_X11_SCALE_FACTOR="1", XDG_SESSION_TYPE="x11",
               CAPTURES_NATIVE_LAYOUT_PROBE="1")
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
            # Cold CI X servers can exceed ten seconds before announcing readiness.
            timeout = 60 if name == "xvfb" else 10
            assert select.select([child.stdout], [], [], timeout)[0], (
                f"{name} did not announce readiness within {timeout}s; inspect its stderr")
            return child.stdout.readline().decode().strip()
        return child

    def run(*command):
        return subprocess.check_output(command, env=env, timeout=40)

    def file_size(size):
        # Shipping formatFileSize: decimal units, one decimal below 100.
        value, units = size, ["B", "KB", "MB", "GB"]
        unit = 0
        while value >= 1000 and unit < len(units) - 1:
            value /= 1000
            unit += 1
        if unit == 0:
            return f"{size} B"
        return f"{value:.{0 if value >= 100 else 1}f} {units[unit]}"

    def expected_estimate(size, exact=True):
        # Derive the delta from measured file bytes using integer arithmetic,
        # independently of the UI's floating-point formatter.
        percent = ((size - len(original)) * 200 + len(original)) // (2 * len(original))
        suffix = f" · {'−' if percent < 0 else '+'}{abs(percent)}%" if percent else ""
        return f"{'≈ ' if not exact else ''}{file_size(size)}{suffix}"

    def windows(title):
        found = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", f"^{re.escape(title)}"],
                               env=env, capture_output=True, text=True, timeout=5)
        assert found.returncode in (0, 1), found.stderr
        return found.stdout.split()

    def active_window():
        result = subprocess.run(["xdotool", "getactivewindow"], env=env,
                                capture_output=True, text=True, timeout=5)
        # Openbox can temporarily unset _NET_ACTIVE_WINDOW while changing focus.
        # Keep polling until the expected window actually owns focus.
        assert result.returncode in (0, 1), result.stderr
        return result.stdout.strip() if result.returncode == 0 else None

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
        # Alt+F4 goes to whichever client Openbox has focused; while it briefly
        # clears the active window during activation the key closes nothing.
        run("xdotool", "windowactivate", "--sync", window)
        # A modal child may legitimately own focus, so only wait for Openbox to
        # finish the transfer rather than for this exact window.
        wait(lambda: active_window() is not None, "close target owns focus")
        run("xdotool", "key", "alt+F4", "sleep", ".5")

    # The editor reports named control rectangles (CAPTURES_NATIVE_LAYOUT_PROBE)
    # as `recording-editor-layout` events on stdout. Interactions target those
    # names instead of hard-coded coordinates, and scroll the page into view.
    layout_log = {"name": "app", "offset": 0, "latest": {}, "order": []}
    window_artifacts = {}

    def refresh_layout():
        path = output / f"{layout_log['name']}.jsonl"
        if not path.exists():
            return
        with path.open() as log:
            log.seek(layout_log["offset"])
            while line := log.readline():
                if not line.endswith("\n"):
                    break
                layout_log["offset"] += len(line.encode())
                try:
                    event = json.loads(line)
                except ValueError:
                    continue
                if event.get("event") != "recording-editor-layout":
                    continue
                detail = event["detail"]
                artifact = detail["artifact_id"]
                if artifact not in layout_log["order"]:
                    layout_log["order"].append(artifact)
                layout_log["latest"][artifact] = detail["controls"] or {}

    def switch_layout_log(name):
        layout_log.update(name=name, offset=0, latest={}, order=[])
        window_artifacts.clear()

    def artifact_for(window):
        if window not in window_artifacts:
            def unclaimed():
                refresh_layout()
                return next((artifact for artifact in reversed(layout_log["order"])
                             if artifact not in window_artifacts.values()), None)
            window_artifacts[window] = wait(unclaimed, "editor layout probe")
        return window_artifacts[window]

    def controls(window):
        refresh_layout()
        return layout_log["latest"].get(artifact_for(window), {})

    def control(window, name, prefix=False):
        def found():
            values = controls(window)
            if prefix:
                return next((values[key] for key in values if key.startswith(name)), None)
            return values.get(name)
        return wait(found, f"control {name}")

    def visible_rect(window, name, prefix=False, whole_control=False):
        """Scroll the page until the named control is visible, then return it."""
        for attempt in range(60):
            x0, y0, x1, y1, cx0, cy0, cx1, cy1 = control(window, name, prefix)
            left, top, right, bottom = max(x0, cx0), max(y0, cy0), min(x1, cx1), min(y1, cy1)
            # A control that fits its clip must be wholly visible: callers map
            # fractions of the returned rect onto the control, so a clipped
            # rect would misplace them. Taller controls (or a wheel step that
            # keeps overshooting) settle for the visible part.
            # Nested clips shrink as the page scrolls, so measure against the
            # page viewport for controls inside it (not the fixed footer).
            page = control(window, "Page")
            vy0, vy1 = cy0, cy1
            if cy0 >= page[5] - 1 and cy1 <= page[7] + 1:
                vy0, vy1 = page[5], page[7]
            fits = whole_control and y1 - y0 <= vy1 - vy0 + 1 and attempt < 30
            whole = y0 >= vy0 - 1 and y1 <= vy1 + 1
            if right - left >= 2 and bottom - top >= 2 and (whole or not fits):
                return left, top, right, bottom
            wheel = "5" if (y1 > vy1 if fits else (y0 + y1) / 2 > cy1) else "4"
            run("xdotool", "mousemove", "--sync", "--window", window, str(page[0] + 6),
                str((page[1] + page[3]) // 2), "click", wheel, "sleep", ".35")
        raise AssertionError(f"{name} never scrolled into view")

    def center(window, name, prefix=False):
        left, top, right, bottom = visible_rect(window, name, prefix)
        return (left + right) // 2, (top + bottom) // 2

    def press(window, name, prefix=False):
        idle(window)
        click(window, *center(window, name, prefix))

    def fill(window, name, value):
        press(window, name)
        run("xdotool", "key", "ctrl+a")
        run("xdotool", "type", "--clearmodifiers", "--delay", "35", "--", str(value))
        run("xdotool", "key", "Return", "sleep", ".3")

    def choose(window, name, item):
        """Open a select and click an item, matched by label prefix."""
        press(window, name)
        click(window, *center(window, f"{name}/{item}", prefix=True))

    def set_destination(window, path):
        # The footer edits the file stem; its folder and format add the rest.
        assert path.parent == exports, path
        fill(window, "Filename", path.stem)

    def volume(window, track, value):
        # Shipping RangeSlider (0-200%, whole percents): a press focuses the
        # track, then Home and one Right per percent set the exact value.
        x0, y0, x1, y1 = visible_rect(window, f"{track} volume")
        click(window, (x0 + x1) // 2, (y0 + y1) // 2)
        run("xdotool", "key", "Home")
        if value:
            run("xdotool", "key", "--delay", "8", "--repeat", str(value), "Right")
        run("xdotool", "sleep", ".3")

    def raw_press(window, name):
        """Click without waiting for idle, e.g. to cancel running work."""
        x, y = center(window, name)
        run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
            "mousemove", "--sync", "--window", window, str(x), str(y), "sleep", ".3",
            "mousedown", "1", "sleep", ".15", "mouseup", "1", "sleep", ".3")

    def blur(window):
        """Click the page gutter so no field keeps focus."""
        page = control(window, "Page")
        click(window, page[0] + 6, page[1] + 40)

    def image_point(window, fx, fy):
        # Fractions of a clipped rect would misplace the point, so scroll the
        # whole image into view first.
        x0, y0, x1, y1 = visible_rect(window, "Preview image", whole_control=True)
        return round(x0 + (x1 - x0) * fx), round(y0 + (y1 - y0) * fy)

    def region(window, name, inset=0):
        """ImageMagick crop geometry for a visible control in window screenshots."""
        x0, y0, x1, y1 = visible_rect(window, name)
        x0, y0, x1, y1 = x0 + inset, y0 + inset, x1 - inset, y1 - inset
        return f"{x1 - x0}x{y1 - y0}+{x0}+{y0}", (x0, y0, x1 - x0, y1 - y0)

    def dominant(path, channel, at=None, window=None):
        if at is None:
            # Away from the centered play button and the fixture's white box.
            x, y = image_point(window or editor, .75, .75)
            # Measuring may scroll the page; retake the shot at that position.
            run("import", "-window", window or editor, str(path))
            pixel = run("convert", str(path), "-crop", f"1x1+{x}+{y}", "-depth", "8", "rgb:-")
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
        if args.sound:
            socket = output / "pulse.sock"
            env["PULSE_SERVER"] = "unix:" + str(socket)
            audio_server = spawn("pulse", ["pulseaudio", "-n", "--daemonize=no", "--use-pid-file=no",
                "--exit-idle-time=-1", "--log-target=stderr",
                f"--load=module-native-protocol-unix socket={socket} auth-anonymous=1",
                "--load=module-null-sink sink_name=captures_preview rate=48000 channels=2"])
            wait(socket.exists, "isolated PulseAudio socket")
            run("pactl", "set-default-sink", "captures_preview")
            alsa = output / "alsa.conf"
            alsa.write_text('</usr/share/alsa/alsa.conf>\npcm.!default { type pulse }\nctl.!default { type pulse }\n')
            env["ALSA_CONFIG_PATH"] = str(alsa)
        history = output / "history"
        artifact_id = "032135f1-11e4-4a47-893d-2368c079a6ba"
        artifact = history / artifact_id
        artifact.mkdir(parents=True)
        source = output / "source.mp4"
        source_width, source_height = (640, 360) if args.maximum_size or args.gif_quality or args.comparison else (1600, 900) if args.preview_scale or args.gif_width else (640, 1440) if args.presets else (320, 180)
        source_size = f"{source_width}x{source_height}"
        segment_seconds = 12 if args.estimate_delta else 2 if args.playback or args.sound else 1
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
        if args.maximum_size or args.gif_quality or args.comparison:
            run("ffmpeg", "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30:duration=4",
                "-c:v", "mpeg4", "-q:v", "2", "-an", str(source))
        else:
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
            "saved_path": str(source), "mime_type": "video/mp4", "duration_ms": 4000 if args.maximum_size or args.gif_quality or args.comparison else 3000 * segment_seconds,
            "target": {"type": "display", "display_id": "fixture"},
            "has_system_audio": args.audio, "has_microphone_audio": args.audio, "dropped_frames": 0,
        }))
        if args.replace_original:
            shutil.copyfile(source, artifact / "media.mp4")
        original, original_metadata = source.read_bytes(), metadata.read_bytes()
        exports = output / "exports"
        exports.mkdir()
        settings = output / "settings.json"
        settings.write_text(json.dumps({"settings_schema_version": 5, "appearance": args.appearance,
            "onboarding_completed": True,
            "theme": "mustard", "output_directory": str(exports), "launch_at_login": False,
            "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
            "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10",
            "auto_copy_to_clipboard": False, "show_mini_previews": False}))
        if args.thumbnails or args.graphical_crop or args.maximum_size or args.comparison or args.replace_original:
            # Delay only the requested frame/export command to exercise
            # cancellation without racing a tiny fixture. Pixels still use FFmpeg.
            tools = output / "tools"
            tools.mkdir()
            operation = "comparison" if args.comparison else "export" if args.maximum_size or args.replace_original else "thumbnails" if args.thumbnails else "source-frame"
            started = output / f"{operation}-calls.txt"
            allowed = output / f"allow-{operation}"
            predicate = "'.captures-replace-' in arg" if args.replace_original else "'captures-export-comparison-' in arg" if args.comparison else "'-attempt-' in arg" if args.maximum_size else "'tile=' in arg" if args.thumbnails else "'source-frame-' in arg"
            if args.maximum_size or args.comparison or args.replace_original:
                allowed.touch()
            ffmpeg = shutil.which("ffmpeg")
            assert ffmpeg
            wrapper = tools / "ffmpeg"
            wrapper.write_text(
                "#!/usr/bin/python3\nimport os, sys, time\nfrom pathlib import Path\n"
                f"if any({predicate} for arg in sys.argv[1:]):\n"
                f"    with Path({str(started)!r}).open('a') as log: log.write('call\\n')\n"
                f"    while not Path({str(allowed)!r}).exists(): time.sleep(.05)\n"
                f"os.execv({ffmpeg!r}, [{ffmpeg!r}, *sys.argv[1:]])\n")
            wrapper.chmod(0o755)
            env["PATH"] = str(tools) + os.pathsep + env["PATH"]
        app_command = [str(binary), "--live", "--history-root", str(history),
                       "--settings-file", str(settings), "--quit-after", "900"]
        open_arguments = []
        if args.external_media:
            animation = output / "Animation é.gif"
            webm = output / "WebM source.mp4"  # Deliberately misleading suffix.
            still = output / "Still.png"
            alias = output / "Animation alias.gif"
            broken = output / "Broken.png"
            run("ffmpeg", "-v", "error", "-i", str(source), str(animation))
            run("ffmpeg", "-v", "error", "-i", str(source), "-c:v", "libvpx-vp9",
                "-an", "-f", "webm", str(webm))
            shutil.copyfile(artifact / "preview.png", still)
            alias.symlink_to(animation)
            broken.write_bytes(b"not an image")
            source_bytes = {path: path.read_bytes() for path in [source, animation, webm, still]}
            shutil.rmtree(artifact)  # The app, not the fixture, must publish History.

            # Hold the second external open while staging an edit in the first
            # editor. All probes remain real. Releasing it lets the final alias
            # refocus that editor, which must retain the unaccepted trim value.
            tools = output / "open-tools"
            tools.mkdir()
            started = output / "second-open-started"
            allowed = output / "allow-second-open"
            webm_started = output / "webm-open-started"
            webm_allowed = output / "allow-webm-open"
            ffprobe = shutil.which("ffprobe")
            assert ffprobe
            wrapper = tools / "ffprobe"
            wrapper.write_text(
                "#!/usr/bin/python3\nimport os, sys, time\nfrom pathlib import Path\n"
                f"if {str(source)!r} in sys.argv[1:]:\n"
                f"    Path({str(started)!r}).touch()\n"
                f"    while not Path({str(allowed)!r}).exists(): time.sleep(.05)\n"
                f"if {str(webm)!r} in sys.argv[1:]:\n"
                f"    Path({str(webm_started)!r}).touch()\n"
                f"    while not Path({str(webm_allowed)!r}).exists(): time.sleep(.05)\n"
                f"os.execv({ffprobe!r}, [{ffprobe!r}, *sys.argv[1:]])\n")
            wrapper.chmod(0o755)
            env["PATH"] = str(tools) + os.pathsep + env["PATH"]
            for index, path in enumerate([animation, source, webm, still, broken, alias]):
                open_arguments.extend(["--open-image" if index == 3 else "--open-media", str(path)])

            def opened_entries():
                return [json.loads(path.read_text()) for path in history.glob("*/metadata.json")
                        if not path.parent.name.startswith(".")]

        app = spawn("app", app_command + open_arguments)
        root = wait(lambda: windows("Captures"), "History")[0]
        if args.external_media:
            wait(started.exists, "second external open blocked after first dispatch")
            editor = wait(lambda: windows("Recording editor"), "external GIF editor")[0]
            run("xdotool", "windowmove", "--sync", editor, "80", "60",
                "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
            shot(editor, "external-gif-decoded")
            dominant(output / "external-gif-decoded.png", 0)
            gif_entry = next(value for value in opened_entries() if value["kind"] == "gif")
            gif_metadata = history / gif_entry["id"] / "metadata.json"
            gif_before_alias = gif_metadata.read_bytes()
            fill(editor, "Start (ms)", 1100)
            shot(editor, "external-gif-staged")
            allowed.touch()
            wait(webm_started.exists, "WebM held while checking MP4 reference controls")
            mp4_editor = wait(lambda: next((window for window in windows("Recording editor") if window != editor), None),
                              "external MP4 editor")
            run("xdotool", "windowmove", "--sync", mp4_editor, "80", "60",
                "windowsize", "--sync", mp4_editor, "960", "900", "sleep", ".5")
            shot(mp4_editor, "external-mp4-decoded")
            dominant(output / "external-mp4-decoded.png", 0, window=mp4_editor)
            press(mp4_editor, "Replace original…")
            shot(mp4_editor, "external-mp4-replace-disabled")
            webm_allowed.touch()
            entries = wait(lambda: values if len(values := opened_entries()) == 4 else None,
                           "four imported artifacts without alias duplicate")
            wait(lambda: len(windows("Recording editor")) == 3 and len(windows("Screenshot editor")) == 1,
                 "mixed recording and screenshot editor routing")
            wait(lambda: active_window() == editor,
                 "final canonical alias refocuses the original GIF editor")
            shot(editor, "external-gif-alias-staged")
            assert gif_metadata.read_bytes() == gif_before_alias, "alias must not republish History"
            for path, kind, mime in [(animation, "gif", "image/gif"), (source, "video", "video/mp4"),
                                     (webm, "video", "video/webm"), (still, "screenshot", "image/png")]:
                entry = next(value for value in entries if value["saved_path"] == str(path))
                assert (entry["kind"], entry["mime_type"], entry["width"], entry["height"]) == (kind, mime, 320, 180), entry
                if kind != "screenshot":
                    assert not list((history / entry["id"]).glob("media.*")), "reference must not own source bytes"

            # Saving the carried trim gives independent evidence that alias
            # focus preserved staging, rather than only reusing a window ID.
            press(editor, "Apply edits")
            trimmed = exports / "external-gif-trim.mp4"
            set_destination(editor, trimmed)
            press(editor, "Save new copy")
            wait(lambda: len(opened_entries()) == 5, "trimmed GIF source exported to new MP4")
            probe = json.loads(run("ffprobe", "-v", "error", "-show_format", "-show_streams", "-of", "json", str(trimmed)))
            assert abs(float(probe["format"]["duration"]) - 1.9) <= .15, probe
            dominant(trimmed, 1, .2)
            shot(editor, "external-gif-saved")
            for window in windows("Recording editor") + windows("Screenshot editor"):
                close(window)
            wait(lambda: not windows("Recording editor") and not windows("Screenshot editor"), "clean editor close")
            run("xdotool", "windowsize", "--sync", root, "760", "540", "sleep", ".5")
            shot(root, "external-media-error-minimum")
            close(root)
            wait(lambda: app.poll() is not None, "mixed batch quit")
            assert app.returncode == 0

            # A closed canonical reference must reload the same ID. Its default
            # Preserve MP4 export must encode MP4, not rename/copy WebM bytes.
            webm_id = next(value["id"] for value in entries if value["saved_path"] == str(webm))
            switch_layout_log("reopened")
            app = spawn("reopened", app_command + ["--open-media", str(webm)])
            root = wait(lambda: windows("Captures"), "reopened History")[0]
            editor = wait(lambda: windows("Recording editor"), "closed external WebM reopened")[0]
            run("xdotool", "windowmove", "--sync", editor, "80", "60",
                "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
            shot(editor, "external-webm-decoded")
            dominant(output / "external-webm-decoded.png", 0)
            assert len(opened_entries()) == 5
            assert next(value["id"] for value in opened_entries() if value["saved_path"] == str(webm)) == webm_id
            # A reference has no retained recovery copy, so Replace original is
            # disabled even though saved_path points to a writable external file.
            press(editor, "Replace original…")
            shot(editor, "external-reference-replace-disabled")
            destination = exports / "webm-as-mp4.mp4"
            set_destination(editor, destination)
            press(editor, "Save new copy")
            wait(lambda: len(opened_entries()) == 6, "WebM exported to a distinct MP4 History artifact")
            probe = json.loads(run("ffprobe", "-v", "error", "-show_format", "-show_streams", "-of", "json", str(destination)))
            assert "mp4" in probe["format"]["format_name"].split(","), probe
            assert next(stream for stream in probe["streams"] if stream["codec_type"] == "video")["codec_name"] == "h264", probe
            assert abs(float(probe["format"]["duration"]) - 3) <= .15, probe
            dominant(destination, 2, 2.4)
            assert destination.read_bytes() != source_bytes[webm]
            for path, content in source_bytes.items():
                assert path.read_bytes() == content, f"source changed: {path}"
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            shot(editor, "external-webm-saved-minimum")
            close(editor)
            wait(lambda: not windows("Recording editor"), "saved WebM clean close")
            close(root)
            wait(lambda: app.poll() is not None, "external-media quit")
            assert app.returncode == 0
            checks = ["empty-history-mixed-batch", "gif-decoded", "canonical-alias-focus",
                "staged-trim-survives-alias", "per-kind-editor-routing", "actual-container-not-suffix",
                "reference-only-history", "alias-no-history-rewrite", "per-file-error-continuation",
                "closed-reference-same-id", "webm-decoded", "reference-replace-disabled",
                "webm-preserve-export-is-h264-mp4", "export-duration-pixels", "source-bytes-unchanged",
                "minimum-error-and-saved", "clean-close-and-quit"]
            (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
                "checks": checks}, indent=2) + "\n")
            print(f"PASS external media: {len(checks)} checks, GIF/MP4/WebM/still batch and safe MP4 export")
            return
        time.sleep(1)
        shot(root, "history")
        click(root, 105, 590)  # First History card: Edit, below the two-line action row.
        editor = wait(lambda: windows("Recording editor"), "recording editor opens")[0]
        run("xdotool", "windowmove", "--sync", editor, "80", "60")
        run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
        if args.thumbnails:
            wait(started.exists, "initial source thumbnail request")
            cancel_x, cancel_y = center(editor, "Cancel thumbnails")
            run("xdotool", "windowactivate", "--sync", editor, "windowfocus", "--sync", editor,
                "mousemove", "--sync", "--window", editor, str(cancel_x), str(cancel_y), "sleep", ".5")
            run("import", "-window", editor, str(output / "thumbnails-loading.png"))
            # Deliberately bypass idle(): this cancels an accepted running job.
            run("xdotool", "mousedown", "1", "sleep", ".15", "mouseup", "1", "sleep", ".3")
            shot(editor, "thumbnails-cancelled")
            allowed.touch()
            missing = output / "temporarily-moved.mp4"
            source.rename(missing)
            try:
                press(editor, "Retry thumbnails")
                shot(editor, "thumbnails-missing-source")
            finally:
                missing.rename(source)
            press(editor, "Retry thumbnails")
            idle(editor)
            track = visible_rect(editor, "Timeline track")
            shot(editor, "thumbnails-retried")
            assert started.read_text().splitlines() == ["call"] * 3
            assert len(list(history.glob("*/metadata.json"))) == 1 and not list(exports.iterdir())
            for fraction, channel in ((.1, 0), (.5, 1), (.9, 2)):
                x = round(track[0] + (track[2] - track[0]) * fraction)
                y = (track[1] + track[3]) // 2
                pixel = run("convert", str(output / "thumbnails-retried.png"),
                            "-crop", f"1x1+{x}+{y}", "-depth", "8", "rgb:-")
                assert len(pixel) == 3 and pixel[channel] > 90, (x, pixel)
                assert all(pixel[channel] > pixel[i] + 40 for i in range(3) if i != channel), (x, pixel)
        wait(lambda: "Working…" not in run("xdotool", "getwindowname", editor).decode(), "decode")
        shot(editor, "original")
        if args.replace_original:
            recovery = artifact / "media.mp4"
            run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
            fill(editor, "Start (ms)", 1100)
            fill(editor, "End (ms)", 2300)
            press(editor, "Crop recording")
            press(editor, "Lock aspect ratio")
            for name, value in (("Crop X", 10), ("Crop Y", 6), ("Crop width", 160), ("Crop height", 90)):
                fill(editor, name, value)
            choose(editor, "Output resolution", "Custom")
            fill(editor, "Output width", 81)
            fill(editor, "Output height", 61)
            press(editor, "Apply edits")
            shot(editor, "replace-accepted")
            press(editor, "Replace original…")
            shot(editor, "replace-confirmation")
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            shot(editor, "replace-confirmation-minimum")
            run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
            press(editor, "Cancel replacement")
            shot(editor, "replace-declined")
            assert source.read_bytes() == original == recovery.read_bytes()
            assert metadata.read_bytes() == original_metadata

            # Gate a real export subprocess, then cancel after its start marker.
            allowed.unlink()
            calls = len(started.read_text().splitlines()) if started.exists() else 0
            press(editor, "Replace original…")
            press(editor, "Replace original")
            wait(lambda: started.exists() and len(started.read_text().splitlines()) > calls, "replacement encoder started")
            # The child marker can precede the UI's progress event. Let that
            # row settle before targeting Cancel, without waiting for idle.
            time.sleep(1)
            cancel_x, cancel_y = center(editor, "Cancel export")
            run("xdotool", "windowactivate", "--sync", editor, "windowfocus", "--sync", editor,
                "mousemove", "--sync", "--window", editor, str(cancel_x), str(cancel_y), "sleep", ".5")
            run("import", "-window", editor, str(output / "replace-running.png"))
            run("xdotool", "mousedown", "1", "sleep", ".15", "mouseup", "1", "sleep", ".3")
            shot(editor, "replace-cancelled")
            assert source.read_bytes() == original == recovery.read_bytes()
            assert metadata.read_bytes() == original_metadata
            assert not list(output.glob(".captures-replace-*"))
            allowed.touch()

            press(editor, "Replace original…")
            press(editor, "Replace original")
            wait(lambda: source.read_bytes() != original, "original replaced")
            shot(editor, "replace-rebased")
            new_bytes = source.read_bytes()
            assert recovery.read_bytes() == new_bytes
            updated = json.loads(metadata.read_text())
            before = json.loads(original_metadata)
            for key in ("id", "created_at", "target", "dropped_frames", "saved_path"):
                assert updated[key] == before[key], (key, updated)
            assert (updated["width"], updated["height"]) == (80, 60)
            assert abs(updated["duration_ms"] - 1200) <= 150
            assert len(list(history.glob("*/metadata.json"))) == 1
            # This same open editor must use the new duration/geometry for seek
            # and save; stale original dimensions or a phantom dirty state fails.
            fill(editor, "Position (ms)", 1000)
            press(editor, "Seek")
            shot(editor, "replace-seek")
            dominant(output / "replace-seek.png", 2)
            copy = exports / "after-replace.mp4"
            set_destination(editor, copy)
            press(editor, "Save new copy")
            wait(copy.exists, "same-session save after replacement")
            wait(lambda: len(list(history.glob("*/metadata.json"))) == 2, "same-session copy published in History")
            info = json.loads(run("ffprobe", "-v", "error", "-show_format", "-show_streams", "-of", "json", str(copy)))
            assert (info["streams"][0]["width"], info["streams"][0]["height"]) == (80, 60)
            assert abs(float(info["format"]["duration"]) - 1.2) < .15
            frame = run("ffmpeg", "-v", "error", "-ss", "0.2", "-i", str(copy), "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-")
            assert len(frame) == 80 * 60 * 3
            for x, y in ((4, 4), (20, 15)):
                assert min(frame[(y * 80 + x) * 3:(y * 80 + x) * 3 + 3]) > 210
            green = frame[(30 * 80 + 40) * 3:(30 * 80 + 40) * 3 + 3]
            assert green[1] > max(green[0], green[2]) + 40
            assert source.read_bytes() == new_bytes == recovery.read_bytes()
            assert len(list(history.glob("*/metadata.json"))) == 2
            assert not list(output.glob(".captures-replace-*"))
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            shot(editor, "replace-saved-minimum")
            close(editor)
            wait(lambda: not windows("Recording editor"), "rebased editor closes without dirty warning")
            close(root)
            wait(lambda: app.poll() is not None, "replacement quit")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
                "checks": ["exact-path-confirmation", "decline-preserves-source", "in-flight-cancel", "cancel-cleanup",
                    "same-id-history", "permanent-recovery-bytes", "asymmetric-crop-resize-pixels", "same-session-seek-save", "clean-close"]}, indent=2) + "\n")
            print("PASS replacement: confirmation, cancel, source/History rebase, real edited pixels and same-session save")
            return
        if args.comparison:
            press(editor, "Compare")
            shot(editor, "comparison-mp4")
            assert started.exists(), "Compare must invoke the real media tool"
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            shot(editor, "comparison-minimum")
            press(editor, "Hide compare")  # Hide restores the accepted still and ordinary toolbar.
            missing = output / "temporarily-moved.mp4"
            source.rename(missing)
            try:
                press(editor, "Compare")
                shot(editor, "comparison-error-minimum")
            finally:
                missing.rename(source)
            press(editor, "Compare")
            shot(editor, "comparison-retry-minimum")
            press(editor, "Hide compare")
            run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
            calls = len(started.read_text().splitlines())
            allowed.unlink()
            press(editor, "Compare")
            wait(lambda: len(started.read_text().splitlines()) > calls, "comparison child starts")
            run("import", "-window", editor, str(output / "comparison-pending.png"))
            # Bypass idle: cancellation interrupts the running tool process.
            cancel_x, cancel_y = center(editor, "Cancel comparison")
            run("xdotool", "mousemove", "--window", editor, str(cancel_x), str(cancel_y), "mousedown", "1",
                "sleep", ".15", "mouseup", "1")
            idle(editor)
            allowed.touch()
            shot(editor, "comparison-cancelled")
            choose(editor, "Format", ".gif")
            press(editor, "Apply edits")
            press(editor, "Compare")
            shot(editor, "comparison-gif")
            press(editor, "100%")  # 100% avoids interpolation in the pixel oracle.
            click(editor, *image_point(editor, .02, .4))
            shot(editor, "comparison-gif-encoded")
            click(editor, *image_point(editor, .98, .4))
            shot(editor, "comparison-gif-before")
            # Sample one side only: clear of the divider, the corner badges and
            # the play button that sits below center while comparing.
            x0, y0, x1, y1 = visible_rect(editor, "Preview image")
            width, height = x1 - x0 - 32, y1 - y0 - 140
            colors = {}
            for side in ("before", "encoded"):
                pixels = run("convert", str(output / f"comparison-gif-{side}.png"),
                             "-crop", f"{width}x{height}+{x0 + 16}+{y0 + 30}", "-depth", "8", "rgb:-")
                assert len(pixels) == width * height * 3
                colors[side] = len(set(zip(pixels[0::3], pixels[1::3], pixels[2::3])))
            assert colors["before"] > colors["encoded"] and colors["encoded"] <= 256, colors
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            press(editor, "Fit")  # Fit
            click(editor, *image_point(editor, .1, .4))
            shot(editor, "comparison-gif-minimum")
            press(editor, "Hide compare")
            run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
            choose(editor, "Format", ".mp4")
            press(editor, "Apply edits")
            choose(editor, "Quality mode", "Maximum file size")
            press(editor, "Apply edits")
            press(editor, "Compare")
            shot(editor, "comparison-maximum")
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            shot(editor, "comparison-maximum-minimum")
            press(editor, "Hide compare")
            run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
            assert source.read_bytes() == original and metadata.read_bytes() == original_metadata
            assert len(list(history.glob("*/metadata.json"))) == 1 and not list(exports.iterdir())
            close(editor)
            assert windows("Recording editor"), "comparison must not mark accepted Maximum edits saved"
            shot(editor, "comparison-dirty-close")
            press(editor, "Discard edits and close")  # Explicitly discard the unsaved Maximum setting.
            wait(lambda: not windows("Recording editor"), "explicit discard closes comparison editor")
            close(root)
            wait(lambda: app.poll() is not None, "comparison quit")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
                "displayed_colors": colors, "checks": ["mp4", "gif-palette-pixels", "split-pointer", "100-percent",
                    "minimum-controls", "missing-source-retry", "running-child-cancel", "maximum-first-attempt-label",
                    "immutable-source-history", "no-export-publication", "dirty-close-retained"]}, indent=2) + "\n")
            print(f"PASS encoded comparison: displayed colors {colors}, cancel/retry, Maximum, immutable source and History")
            return
        if not (args.maximum_size or args.gif_quality):
            dominant(output / "original.png", 0)
        run("xdotool", "windowminimize", root, "sleep", ".5")
        estimate_expectations = {}
        if args.sound:
            def motion_click():
                x, y = center(editor, "Play preview")
                run("xdotool", "mousemove", "--window", editor, str(x), str(y), "sleep", ".05",
                    "mousedown", "1", "sleep", ".08", "mouseup", "1")

            def playing():
                return "Working…" in run("xdotool", "getwindowname", editor).decode()

            def capture_playback(name):
                pcm = output / f"{name}.f32"
                monitor = spawn(name, ["ffmpeg", "-v", "error", "-f", "pulse", "-i",
                    "captures_preview.monitor", "-t", "8", "-ar", "48000", "-ac", "2",
                    "-f", "f32le", str(pcm)])
                time.sleep(.5)
                motion_click()
                wait(playing, "playback started")
                time.sleep(1)
                inputs = run("pactl", "list", "short", "sink-inputs").splitlines()
                assert len(inputs) == (0 if name == "sound-default-off" else 1), inputs
                run("import", "-window", editor, str(output / f"{name}-running.png"))
                idle(editor)
                assert not run("pactl", "list", "short", "sink-inputs").strip(), "EOF releases audio output"
                assert monitor.wait(timeout=15) == 0
                return array("f", pcm.read_bytes())

            silent = capture_playback("sound-default-off")
            assert silent and max(abs(v) for v in silent) < .00001, "Sound defaults off"
            press(editor, "Sound")
            audible = capture_playback("sound-on")
            assert max(abs(v) for v in audible) > .05, "Sound reaches the default virtual sink"
            # Independently measure both asymmetric source tones, rather than accepting noise.
            measured = []
            for frequency in (440, 880):
                amplitudes = []
                for offset in range(0, len(audible) - 19200, 19200):
                    values = audible[offset:offset + 19200:2]
                    real = sum(v * math.cos(2 * math.pi * frequency * i / 48000) for i, v in enumerate(values))
                    imaginary = sum(v * math.sin(2 * math.pi * frequency * i / 48000) for i, v in enumerate(values))
                    amplitudes.append(2 * math.hypot(real, imaginary) / len(values))
                measured.append(max(amplitudes))
            assert all(value > .02 for value in measured), measured
            shot(editor, "sound-ended")
            dominant(output / "sound-ended.png", 2)
            assert max(abs(v) for v in audible[-48000:]) < .00001, "short audio drains to silence"

            press(editor, "Loop preview")  # Loop reopens audio only after decoder/output teardown.
            motion_click()
            wait(playing, "audible loop starts")
            def output_stream():
                streams = run("pactl", "list", "short", "sink-inputs").splitlines()
                assert len(streams) <= 1, "loop never opens overlapping output streams"
                return streams[0].split()[0] if streams else None
            first_stream = wait(output_stream, "first lap opens audio output")
            wait(lambda: (stream := output_stream()) and stream != first_stream,
                 "loop closes and reopens the audio output for the next lap")
            assert playing(), "audible loop retains worker ownership across EOF"
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            run("import", "-window", editor, str(output / "sound-minimum-running.png"))
            motion_click()
            idle(editor)
            assert not run("pactl", "list", "short", "sink-inputs").strip(), "Pause releases audio output"
            shot(editor, "sound-minimum-on-paused")
            run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
            press(editor, "Loop preview")

            # Default-device absence is visible failure, not an implicit silent fallback.
            audio_server.terminate()
            audio_server.wait(timeout=5)
            motion_click()
            idle(editor)
            shot(editor, "sound-device-error")
            dominant(output / "sound-device-error.png", 0)
            press(editor, "Sound")
            motion_click()
            wait(playing, "explicit Sound-off retry works without an audio server")
            time.sleep(.7)
            motion_click()
            idle(editor)
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            shot(editor, "sound-minimum-paused")
            run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
            press(editor, "Sound")  # Request Sound again, but GIF must not open a device.
            choose(editor, "Format", ".gif")
            press(editor, "Apply edits")
            idle(editor)  # Apply and Play share the Working title; finish Apply first.
            motion_click()
            wait(playing, "GIF with Sound selected stays playable without a device")
            def gif_motion():
                path = output / "sound-gif-silent.png"
                x, y = image_point(editor, .75, .75)  # May scroll; measure before the shot.
                run("import", "-window", editor, str(path))
                pixel = run("convert", str(path), "-crop", f"1x1+{x}+{y}", "-depth", "8", "rgb:-")
                return len(pixel) == 3 and pixel[1] > max(pixel[0], pixel[2]) + 40
            wait(gif_motion, "silent GIF actually advances from red to green without a device")
            motion_click()
            idle(editor)
            assert source.read_bytes() == original and metadata.read_bytes() == original_metadata
            assert len(list(history.glob("*/metadata.json"))) == 1 and not list(exports.iterdir())
            close(editor)
            shot(editor, "sound-close-confirmation")
            (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
                "virtual_sink_tone_amplitudes": measured,
                "checks": ["default-silent", "opt-in-real-output", "both-source-tones", "short-audio-video-eof",
                    "audible-loop-reopen", "one-output-stream", "pause-eof-release-output",
                    "device-failure", "explicit-silent-retry", "minimum-layout", "gif-no-device",
                    "immutable-source-history", "no-export"]}, indent=2) + "\n")
            print("PASS Sound preview: default-off, virtual audio output, EOF, device error/retry and GIF without a device")
            return
        if args.gif_width:
            run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
            choose(editor, "Format", ".gif")
            shot(editor, "gif-width-default-staged")
            dimensions = {}
            # Save the default, then increase it: reusing accepted 800px output
            # as the base would silently prevent the 1200px export.
            for maximum, expected in ((800, (800, 450)), (1200, (1200, 674)), (320, (320, 180))):
                if maximum != 800:
                    press(editor, "Maximum width")
                    shot(editor, f"gif-width-menu-{maximum}")
                    click(editor, *center(editor, f"Maximum width/{maximum} px"))
                destination = exports / f"width-{maximum}.gif"
                set_destination(editor, destination)
                press(editor, "Save new copy")
                assert not destination.exists(), "staged width cannot save"
                press(editor, "Apply edits")
                shot(editor, f"gif-width-{maximum}-accepted")
                press(editor, "Save new copy")
                wait(destination.exists, f"{maximum}px GIF export")
                idle(editor)
                stream = json.loads(run("ffprobe", "-v", "error", "-select_streams", "v:0",
                    "-show_entries", "stream=width,height", "-of", "json", str(destination)))["streams"][0]
                assert (stream["width"], stream["height"]) == expected, stream
                dimensions[str(maximum)] = stream
            choose(editor, "Format", ".mp4")
            press(editor, "Apply edits")
            mp4 = exports / "restored.mp4"
            set_destination(editor, mp4)
            press(editor, "Save new copy")
            wait(mp4.exists, "uncapped MP4 export")
            idle(editor)
            stream = json.loads(run("ffprobe", "-v", "error", "-select_streams", "v:0",
                "-show_entries", "stream=width,height", "-of", "json", str(mp4)))["streams"][0]
            assert (stream["width"], stream["height"]) == (1600, 900), stream
            choose(editor, "Format", ".gif")
            press(editor, "Apply edits")
            restored = exports / "restored.gif"
            set_destination(editor, restored)
            press(editor, "Save new copy")
            wait(restored.exists, "remembered GIF width export")
            idle(editor)
            restored_stream = json.loads(run("ffprobe", "-v", "error", "-select_streams", "v:0",
                "-show_entries", "stream=width,height", "-of", "json", str(restored)))["streams"][0]
            assert (restored_stream["width"], restored_stream["height"]) == (320, 180), restored_stream
            shot(editor, "gif-width-restored")
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            run("xdotool", "mousemove", "--window", editor, "450", "410",
                "click", "--repeat", "25", "--delay", "40", "5", "sleep", ".5")
            shot(editor, "gif-width-minimum")
            press(editor, "Maximum width")  # Saved-status row reduces the scrolling viewport.
            shot(editor, "gif-width-minimum-menu")
            run("xdotool", "key", "Escape")
            assert source.read_bytes() == original and metadata.read_bytes() == original_metadata
            assert len(list(history.glob("*/metadata.json"))) == 6
            close(editor)
            wait(lambda: not windows("Recording editor"), "saved GIF width closes cleanly")
            close(root)
            wait(lambda: app.poll() is not None, "GIF width quit")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
                "dimensions": dimensions, "mp4_dimensions": stream,
                "checks": ["staged-save-gate", "default-800", "increase-without-compounding", "decrease-320",
                    "mp4-base-restored", "gif-choice-retained", "minimum-controls", "immutable-source-history", "clean-close"]}, indent=2) + "\n")
            print(f"PASS GIF width: {dimensions}, MP4 restored to 1600x900, immutable source")
            return
        if args.gif_quality:
            run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
            choose(editor, "Format", ".gif")
            colors = {}
            # GIF offers Compress/Maximum only, as shipping does; Compress
            # lists the Tiny through Highest presets.
            for quality, label in (("tiny", "Tiny"), ("high", "High")):
                press(editor, "Quality")
                shot(editor, f"gif-quality-menu-{quality}")
                click(editor, *center(editor, f"Quality/{label}"))
                destination = exports / f"{quality}.gif"
                set_destination(editor, destination)
                press(editor, "Save new copy")
                assert not destination.exists(), "staged quality cannot save"
                press(editor, "Apply edits")
                shot(editor, f"gif-quality-{quality}-accepted")
                press(editor, "Save new copy")
                wait(destination.exists, f"{quality} GIF export")
                idle(editor)  # Destination publication precedes History completion.
                pixels = run("ffmpeg", "-v", "error", "-i", str(destination), "-frames:v", "1",
                             "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1")
                assert len(pixels) == 640 * 360 * 3
                colors[quality] = len(set(zip(pixels[0::3], pixels[1::3], pixels[2::3])))
            assert 32 < colors["tiny"] <= 64, colors
            assert 128 < colors["high"] <= 256, colors
            assert source.read_bytes() == original and metadata.read_bytes() == original_metadata
            assert len(list(history.glob("*/metadata.json"))) == 3
            close(editor)
            wait(lambda: not windows("Recording editor"), "saved GIF quality closes cleanly")
            close(root)
            wait(lambda: app.poll() is not None, "GIF quality quit")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
                "decoded_colors": colors, "checks": ["staged-save-gate", "quality-palette-output",
                    "immutable-source-history", "clean-close"]}, indent=2) + "\n")
            print(f"PASS GIF quality: decoded colors {colors}, Apply/save and immutable source")
            return
        if args.gif_frame_rate:
            run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
            choose(editor, "Format", ".gif")
            shot(editor, "gif-frame-rate-default")
            press(editor, "Frame rate")
            shot(editor, "gif-frame-rate-menu")
            click(editor, *center(editor, "Frame rate/8 FPS"))  # 8 FPS.
            eight = exports / "eight.gif"
            set_destination(editor, eight)
            press(editor, "Save new copy")
            assert not eight.exists(), "staged format/FPS cannot save"
            press(editor, "Apply edits")
            press(editor, "Save new copy")
            wait(eight.exists, "8 FPS GIF export")
            shot(editor, "gif-frame-rate-eight")
            press(editor, "Frame rate")
            click(editor, *center(editor, "Frame rate/24 FPS"))  # 24 FPS.
            twenty_four = exports / "twenty-four.gif"
            set_destination(editor, twenty_four)
            missing = output / "temporarily-moved.mp4"
            source.rename(missing)
            try:
                press(editor, "Apply edits")
                shot(editor, "gif-frame-rate-failed-apply")
                dominant(output / "gif-frame-rate-failed-apply.png", 0)
                press(editor, "Save new copy")
                assert not twenty_four.exists(), "failed FPS Apply cannot save staged settings"
            finally:
                missing.rename(source)
            press(editor, "Apply edits")
            press(editor, "Save new copy")
            wait(twenty_four.exists, "24 FPS GIF export after retry")
            shot(editor, "gif-frame-rate-twenty-four")
            cadences = {}
            for path, fps in ((eight, 8), (twenty_four, 24)):
                stream = json.loads(run("ffprobe", "-v", "error", "-count_frames", "-select_streams", "v:0",
                    "-show_entries", "stream=nb_read_frames,duration,width,height", "-of", "json", str(path)))["streams"][0]
                assert int(stream["nb_read_frames"]) == fps * 3, stream
                assert abs(float(stream["duration"]) - 3) < .03, stream
                assert (stream["width"], stream["height"]) == (320, 180), stream
                dominant(path, 1, at=1.5)
                dominant(path, 2, at=2.5)
                cadences[str(fps)] = stream
            choose(editor, "Format", ".mp4")
            press(editor, "Apply edits")
            shot(editor, "gif-frame-rate-mp4")
            choose(editor, "Format", ".gif")
            shot(editor, "gif-frame-rate-restored")
            press(editor, "Apply edits")
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            run("xdotool", "mousemove", "--window", editor, "450", "410",
                "click", "--repeat", "25", "--delay", "40", "5", "sleep", ".5")
            shot(editor, "gif-frame-rate-minimum")
            press(editor, "Frame rate")
            shot(editor, "gif-frame-rate-minimum-menu")
            run("xdotool", "key", "Escape")
            assert source.read_bytes() == original and metadata.read_bytes() == original_metadata
            assert len(list(history.glob("*/metadata.json"))) == 3
            assert set(exports.iterdir()) == {eight, twenty_four}
            close(editor)
            wait(lambda: not windows("Recording editor"), "restored saved GIF cadence closes cleanly")
            close(root)
            wait(lambda: app.poll() is not None, "GIF frame-rate quit")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
                "cadences": cadences, "checks": ["staged-save-gate", "failed-apply-retry", "8-and-24-fps-frame-counts",
                    "duration-dimensions-colors", "mp4-switch-restores-gif-cadence", "minimum-controls",
                    "immutable-source-history", "distinct-saved-history", "clean-close"]}, indent=2) + "\n")
            print("PASS GIF frame rate: 24/72 frames over 3s, Apply/retry/save, MP4 roundtrip and immutable source")
            return
        if args.maximum_size:
            run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
            choose(editor, "Quality mode", "Maximum file size")
            shot(editor, "maximum-initial")
            fill(editor, "Maximum file size value", ".0999999")
            press(editor, "Apply edits")
            shot(editor, "maximum-invalid")
            assert not list(exports.iterdir())
            fill(editor, "Maximum file size value", ".1")
            fill(editor, "End (ms)", 800)
            press(editor, "Apply edits")
            shot(editor, "maximum-accepted")
            limited = exports / "limited.mp4"
            set_destination(editor, limited)
            press(editor, "Save new copy")
            wait(limited.exists, "capped MP4 export")
            assert limited.stat().st_size <= 100_000
            choose(editor, "Format", ".gif")
            shot(editor, "maximum-gif-staged")
            press(editor, "Frame rate")
            click(editor, *center(editor, "Frame rate/30 FPS"))  # 30 FPS requested; budget retries can lower it.
            press(editor, "Apply edits")
            shot(editor, "maximum-gif-accepted")
            press(editor, "Save new copy")
            gif = limited.with_suffix(".gif")
            wait(gif.exists, "capped GIF retry export")
            assert gif.stat().st_size <= 100_000
            info = json.loads(run("ffprobe", "-v", "error", "-show_streams", "-of", "json", str(gif)))
            assert (info["streams"][0]["width"], info["streams"][0]["height"]) == (320, 180), info
            shot(editor, "maximum-gif-saved")
            for name in ("maximum-gif-accepted", "maximum-gif-saved"):
                pixels = run("convert", str(output / f"{name}.png"), "-crop", "960x380+0+85", "-depth", "8", "rgb:-")
                if name == "maximum-gif-accepted":
                    accepted_pixels = pixels
                else:
                    assert pixels == accepted_pixels, "saving retry output never replaces the accepted preview"
            saved = next(json.loads(p.read_text()) for p in history.glob("*/metadata.json")
                         if json.loads(p.read_text()).get("saved_path") == str(gif))
            assert (saved["width"], saved["height"], saved["size_bytes"]) == (320, 180, gif.stat().st_size)

            cancelled = exports / "cancelled.gif"
            set_destination(editor, cancelled)
            attempts_before = len(started.read_text().splitlines())
            allowed.unlink()
            press(editor, "Save new copy")
            wait(lambda: len(started.read_text().splitlines()) > attempts_before, "blocked export attempt")
            time.sleep(1)
            run("import", "-window", editor, str(output / "maximum-cancelling.png"))
            # Bypass idle while the worker is deliberately paused inside FFmpeg.
            raw_press(editor, "Cancel export")
            idle(editor)
            allowed.touch()
            shot(editor, "maximum-cancelled")
            assert not cancelled.exists() and not list(exports.glob(".captures-*"))
            fill(editor, "End (ms)", 4000)
            press(editor, "Apply edits")
            failed = exports / "unattainable.gif"
            set_destination(editor, failed)
            press(editor, "Save new copy")
            idle(editor)
            shot(editor, "maximum-unattainable")
            assert not failed.exists()
            assert len(list(history.glob("*/metadata.json"))) == 3
            assert source.read_bytes() == original and metadata.read_bytes() == original_metadata
            fill(editor, "End (ms)", 800)
            press(editor, "Apply edits")
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            run("xdotool", "mousemove", "--window", editor, "450", "410",
                "click", "--repeat", "25", "--delay", "40", "5", "sleep", ".5")
            shot(editor, "maximum-minimum")
            press(editor, "File size unit")
            shot(editor, "maximum-minimum-units")
            run("xdotool", "key", "Escape")
            close(editor)
            wait(lambda: not windows("Recording editor"), "restored saved limit closes cleanly")
            close(root)
            wait(lambda: app.poll() is not None, "maximum size quit")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
                "mp4_bytes": limited.stat().st_size, "gif_bytes": gif.stat().st_size,
                "gif_saved_dimensions": [320, 180], "preview_dimensions": [640, 360],
                "checks": ["minimum-cap-validation", "capped-mp4", "gif-retry-dimensions", "preview-retained-on-save",
                    "history-actual-output-metadata", "in-flight-cancel-cleanup", "unattainable-no-publication",
                    "immutable-source-history", "minimum-controls", "clean-restored-close"]}, indent=2) + "\n")
            print("PASS maximum size: capped MP4/GIF, retry dimensions, cancel/unattainable cleanup, immutable source and History")
            return
        if args.preview_scale:
            def marker_size(name):
                pixels = run("convert", str(output / f"{name}.png"), "-crop", "960x380+0+85",
                             "-depth", "8", "rgb:-")
                red = [(i // 3 % 960, i // 3 // 960) for i in range(0, len(pixels), 3)
                       if pixels[i] > max(pixels[i + 1], pixels[i + 2]) + 40]
                assert red, "red source preview"
                left, right = min(x for x, _ in red), max(x for x, _ in red)
                top, bottom = min(y for _, y in red), max(y for _, y in red)
                # Count only white enclosed by red in its row and column: the
                # preview's rounded corners reveal the (light) viewport there.
                rows, columns = {}, {}
                for x, y in red:
                    low, high = rows.get(y, (x, x)); rows[y] = (min(low, x), max(high, x))
                    low, high = columns.get(x, (y, y)); columns[x] = (min(low, y), max(high, y))
                white = [(x, y) for y in range(top, bottom + 1) for x in range(left, right + 1)
                         if min(pixels[(y * 960 + x) * 3:(y * 960 + x) * 3 + 3]) > 210
                         and y in rows and rows[y][0] < x < rows[y][1]
                         and x in columns and columns[x][0] < y < columns[x][1]]
                if not white:
                    return (0, 0)
                return (max(x for x, _ in white) - min(x for x, _ in white) + 1,
                        max(y for _, y in white) - min(y for _, y in white) + 1)

            shot(editor, "scale-fit")
            fit_size = marker_size("scale-fit")
            assert 0 < fit_size[0] < 30 and 0 < fit_size[1] < 20, fit_size
            missing = output / "temporarily-moved.mp4"
            source.rename(missing)
            try:
                press(editor, "100%")
                shot(editor, "scale-actual")
                actual = marker_size("scale-actual")
                assert abs(actual[0] - 45) <= 1 and abs(actual[1] - 25) <= 1, actual
                run("xdotool", "mousemove", "--window", editor, "500", "230",
                    "click", "--repeat", "5", "--delay", "100", "5", "sleep", ".5")
                shot(editor, "scale-scrolled")
                assert marker_size("scale-scrolled") == (0, 0), "inner scrolling moves source pixels"
                press(editor, "Fit")
                shot(editor, "scale-fit-again")
                assert marker_size("scale-fit-again") == fit_size
                press(editor, "100%")
                shot(editor, "scale-actual-reset")
                assert marker_size("scale-actual-reset") == actual, "Fit/100% resets the preview scroll"
                run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
                shot(editor, "scale-actual-minimum")
                press(editor, "Fit")
                shot(editor, "scale-fit-minimum")
            finally:
                missing.rename(source)
            assert source.read_bytes() == original and metadata.read_bytes() == original_metadata
            assert len(list(history.glob("*/metadata.json"))) == 1 and not list(exports.iterdir())
            close(editor)
            wait(lambda: not windows("Recording editor"), "display-only scaling closes without dirty prompt")
            close(root)
            wait(lambda: app.poll() is not None, "preview scale quit")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
                "checks": ["fit-ratio", "actual-pixel-scale", "inner-scroll", "fit-restores",
                           "actual-scroll-reset", "minimum-controls", "no-decode", "immutable-source-history", "clean-close"]}, indent=2) + "\n")
            print("PASS recording preview scale: Fit/100%, bounded scroll, no decode or dirty state")
            return
        if args.graphical_crop:
            run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
            fill(editor, "Position (ms)", 1500)
            press(editor, "Seek")
            press(editor, "Crop recording")
            press(editor, "Lock aspect ratio")  # Independent dimensions.
            for name, value in (("Crop X", 80), ("Crop Y", 40), ("Crop width", 160), ("Crop height", 80)):
                fill(editor, name, value)
            press(editor, "Apply edits")
            preview_regions = {}

            def preview_shot(name):
                # Scroll the image into view and remember where it was shot:
                # its top quarter, clear of the play control, which dims while
                # a staged crop awaits Apply.
                visible_rect(editor, "Preview image", whole_control=True)
                shot(editor, name)
                # Measure after the shot settles so the probe matches its frame.
                x0, y0, x1, y1 = visible_rect(editor, "Preview image", whole_control=True)
                # Inset past the rounded, antialiased edges, which vary with
                # the page's scroll offset.
                x0, y0, x1 = x0 + 8, y0 + 8, x1 - 8
                preview_regions[name] = f"{x1 - x0}x{(y1 - y0) // 4}+{x0}+{y0}"

            def preview_pixels(name):
                return run("convert", str(output / f"{name}.png"), "-crop", preview_regions[name], "rgba:-")

            preview_shot("crop-accepted-before-source")
            press(editor, "Adjust crop")
            wait(started.exists, "full-source request started")
            wait(lambda: "Working…" in run("xdotool", "getwindowname", editor).decode(),
                 "source loading controls presented")
            time.sleep(.5)
            run("import", "-window", editor, str(output / "crop-source-loading.png"))
            # Bypass idle(): Cancel must interrupt the blocked frame extraction.
            raw_press(editor, "Cancel source preview")
            preview_shot("crop-source-cancelled")
            allowed.touch()
            missing = output / "temporarily-moved.mp4"
            source.rename(missing)
            try:
                press(editor, "Adjust crop")
                shot(editor, "crop-source-error")
            finally:
                missing.rename(source)
            press(editor, "Adjust crop")
            # Bring the whole full-source image into view before sampling it;
            # the crop card's button can leave the preview scrolled away.
            x, y = image_point(editor, .9, .9)
            shot(editor, "crop-source-ready")
            pixel = run("convert", str(output / "crop-source-ready.png"), "-crop", f"1x1+{x}+{y}",
                        "-depth", "8", "rgb:-")
            assert pixel[1] > max(pixel[0], pixel[2]) + 10, ("full-source preview must be visible", pixel)
            assert "Crop size" in controls(editor), "the crop box shows its size badge"

            def drag_source(start, delta):
                x0, y0, x1, y1 = visible_rect(editor, "Preview image")
                sx, sy = (x1 - x0) / 320, (y1 - y0) / 180
                assert abs(sx - sy) < .03, (x0, y0, x1, y1)
                x, y = round(x0 + start[0] * sx), round(y0 + start[1] * sy)
                dx, dy = round(delta[0] * sx), round(delta[1] * sy)
                run("xdotool", "mousemove", "--window", editor, str(x), str(y),
                    "mousedown", "1", "sleep", ".15", "mousemove_relative", "--sync", "--",
                    str(dx), str(dy), "sleep", ".2", "mouseup", "1", "sleep", ".2")

            def crop_values():
                values = []
                for name in ("Crop X", "Crop Y", "Crop width", "Crop height"):
                    subprocess.run(["xclip", "-selection", "clipboard", "-i"], env=env,
                                   input=b"waiting", check=True, timeout=5)
                    click(editor, *center(editor, name))
                    run("xdotool", "key", "ctrl+a", "ctrl+c")
                    def copied():
                        result = subprocess.run(["xclip", "-selection", "clipboard", "-o"],
                                                env=env, capture_output=True, timeout=5)
                        value = result.stdout.strip()
                        return value if result.returncode == 0 and value.isdigit() else None
                    values.append(int(wait(copied, "fresh crop field")))
                    run("xdotool", "key", "Return")
                # Let the final Return finish text editing before another drag;
                # a press during pending input intentionally commits, not drags.
                idle(editor)
                return tuple(values)

            drag_source((160, 80), (-50, -20))
            assert (actual := crop_values()) == (30, 20, 160, 80), actual
            drag_source((190, 100), (30, 20))
            assert (actual := crop_values()) == (30, 20, 190, 100), actual
            preview_shot("crop-source-staged")
            destination = exports / "graphical-crop.mp4"
            set_destination(editor, destination)
            press(editor, "Save new copy")
            assert not destination.exists() and len(list(history.glob("*/metadata.json"))) == 1
            press(editor, "Adjust crop")  # Done restores the unchanged accepted crop.
            preview_shot("crop-done-accepted")
            assert preview_pixels("crop-source-cancelled") == preview_pixels("crop-accepted-before-source"), preview_regions
            assert preview_pixels("crop-done-accepted") == preview_pixels("crop-accepted-before-source")
            source.rename(missing)
            try:
                press(editor, "Adjust crop")  # Cached pixels work even when source is temporarily absent.
                preview_shot("crop-source-cached")
                assert preview_pixels("crop-source-cached") == preview_pixels("crop-source-staged")
            finally:
                missing.rename(source)
            press(editor, "Apply edits")
            shot(editor, "crop-applied")
            press(editor, "Save new copy")
            wait(lambda: len(list(history.glob("*/metadata.json"))) == 2, "graphical crop export")
            info = json.loads(run("ffprobe", "-v", "error", "-show_streams", "-of", "json", str(destination)))
            video = next(s for s in info["streams"] if s["codec_type"] == "video")
            assert (video["width"], video["height"]) == (190, 100), video
            frame = run("ffmpeg", "-v", "error", "-ss", "1.5", "-i", str(destination),
                        "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-")
            white = frame[(5 * 190 + 5) * 3:(5 * 190 + 5) * 3 + 3]
            green = frame[(50 * 190 + 100) * 3:(50 * 190 + 100) * 3 + 3]
            assert len(frame) == 190 * 100 * 3 and min(white) > 210, white
            assert green[1] > max(green[0], green[2]) + 40, green
            press(editor, "Adjust crop")
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            shot(editor, "crop-source-minimum")
            press(editor, "Done cropping (preview)")  # Done is accessible beside the preview at minimum size.
            shot(editor, "crop-done-minimum")
            run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
            fill(editor, "Position (ms)", 500)
            press(editor, "Seek")
            press(editor, "Adjust crop")
            # Sample inside the staged crop; the default point is in the dimmed exclusion.
            x, y = image_point(editor, .4, .4)
            shot(editor, "crop-source-after-seek")
            pixel = run("convert", str(output / "crop-source-after-seek.png"), "-crop", f"1x1+{x}+{y}",
                        "-depth", "8", "rgb:-")
            assert pixel[0] > 90 and pixel[0] > max(pixel[1], pixel[2]) + 40, pixel
            assert started.read_text().splitlines() == ["call"] * 4, "only cancel, failure, retry and changed-position loads"
            assert source.read_bytes() == original and metadata.read_bytes() == original_metadata
            close(editor)
            wait(lambda: not windows("Recording editor"), "saved crop closes")
            close(root)
            wait(lambda: app.poll() is not None, "graphical crop quit")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
                "checks": ["source-loading-cancel", "source-failure-retry", "source-letterbox", "interior-move", "corner-resize",
                    "numeric-stage-sync", "unapplied-save-gate", "done-restores-accepted", "source-cache",
                    "export-dimensions-pixels", "minimum-source-controls", "seek-invalidates-source", "immutable-source", "saved-close"]}, indent=2) + "\n")
            print("PASS graphical recording crop: source view, move/resize, staging, export pixels and immutable source")
            return
        if args.playback:
            def motion_click():
                # Pause must work during an active decoder; never wait for idle.
                # Do not wait for a motion event when already over this button.
                x, y = center(editor, "Play preview")
                run("xdotool", "mousemove", "--window", editor, str(x), str(y), "sleep", ".05",
                    "mousedown", "1", "sleep", ".08", "mouseup", "1")

            def loop_click():
                x, y = center(editor, "Loop preview")
                run("xdotool", "mousemove", "--window", editor, str(x), str(y), "sleep", ".05",
                    "mousedown", "1", "sleep", ".08", "mouseup", "1")

            def playing():
                return "Working…" in run("xdotool", "getwindowname", editor).decode()

            def position():
                # A fixed sleep can return an older clipboard value under load.
                subprocess.run(["xclip", "-selection", "clipboard", "-i"], env=env,
                    input=b"waiting for playback position", check=True, timeout=5)
                press(editor, "Position (ms)")
                run("xdotool", "key", "ctrl+a", "ctrl+c")
                def copied_position():
                    result = subprocess.run(["xclip", "-selection", "clipboard", "-o"],
                        env=env, capture_output=True, timeout=5)
                    value = result.stdout.strip()
                    return value if result.returncode == 0 and value.isdigit() else None
                return int(wait(copied_position, "fresh playback position clipboard value"))

            fill(editor, "Start (ms)", 1500)
            fill(editor, "End (ms)", 4500)
            motion_click()
            time.sleep(.3)
            assert not playing(), "unapplied trim gates Play"
            press(editor, "Apply edits")
            shot(editor, "playback-accepted")
            dominant(output / "playback-accepted.png", 0)
            # Record real presentation rather than turning fixture PNGs into a video.
            recording = spawn("playback-capture", ["ffmpeg", "-v", "error", "-f", "x11grab",
                "-framerate", "15", "-video_size", "960x900", "-i", env["DISPLAY"] + "+80,60",
                "-t", "18", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
                str(output / "playback-motion.mp4")])
            motion_click()
            wait(playing, "Play owns decoder")
            def motion_color(channel, name):
                # Decoder startup is not presentation time. Observe an actual
                # temporal transition instead of assuming fixed startup latency.
                path = output / f"{name}.png"
                x, y = image_point(editor, .75, .75)  # May scroll; measure before the shot.
                run("import", "-window", editor, str(path))
                pixel = run("convert", str(path), "-crop", f"1x1+{x}+{y}", "-depth", "8", "rgb:-")
                return len(pixel) == 3 and pixel[channel] > 90 and all(
                    pixel[channel] > pixel[i] + 40 for i in range(3) if i != channel)
            wait(lambda: motion_color(1, "playback-running"), "real playback crosses from red to green")
            dominant(output / "playback-running.png", 1)
            motion_click()
            idle(editor)
            shot(editor, "playback-paused")
            paused_at = position()
            assert 2000 <= paused_at < 4000, paused_at
            # Leave field focus before comparing frozen frame+playhead pixels.
            blur(editor)
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
            assert recording.wait(timeout=20) == 0
            loop_recording = spawn("loop-capture", ["ffmpeg", "-v", "error", "-f", "x11grab",
                "-framerate", "15", "-video_size", "960x900", "-i", env["DISPLAY"] + "+80,60",
                "-t", "14", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
                str(output / "playback-loop-motion.mp4")])
            motion_click()
            wait(playing, "loop playback starts")
            loop_click()  # Turn on while already playing, not only before Play.
            wait(lambda: motion_color(2, "loop-first-end"), "loop reaches blue trim end")
            wait(lambda: motion_color(0, "loop-restart"), "loop returns to red accepted trim start")
            assert playing(), "loop must retain worker ownership across EOF"
            motion_click()
            idle(editor)
            shot(editor, "loop-paused")
            motion_click()
            wait(playing, "loop resumes after Pause")
            loop_click()  # Turn off while active; stop at this lap's exclusive end.
            idle(editor)
            assert 4400 <= position() < 4500
            shot(editor, "loop-disabled-ended")
            assert loop_recording.wait(timeout=20) == 0
            motion_click()
            wait(playing, "EOF replay decoder")
            time.sleep(.2)
            # Losing focus must cancel without losing accepted edits.
            run("xdotool", "windowmap", root, "windowactivate", "--sync", root)
            idle(editor)
            run("xdotool", "windowactivate", "--sync", editor)
            replay_at = position()
            assert 1500 <= replay_at < 3000, replay_at
            loop_click()  # A decoder failure must not restart even with looping enabled.
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
            loop_click()
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
            loop_click()  # Close must cancel looping just like single-pass playback.
            # The title changes in the click's layout pass; capture after the
            # first decoder frame redraw, not that transient old Play label.
            time.sleep(.4)
            run("import", "-window", editor, str(output / "playback-minimum-running.png"))
            close(editor)
            idle(editor)
            shot(editor, "playback-close-confirmation")
            assert windows("Recording editor"), "accepted unsaved edits still require discard"
            # Keep editing, then save the accepted edit (not a playback range).
            run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
            press(editor, "Keep editing")
            destination = exports / "playback-trim.mp4"
            set_destination(editor, destination)
            press(editor, "Save new copy")
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
                    "loop-active-toggle", "loop-trim-restart", "loop-pause-resume", "loop-disable-eof",
                    "failure-restores-still", "retry", "minimum-layout", "dirty-close",
                    "accepted-export-duration-colors", "source-immutable", "clean-close"]}, indent=2) + "\n")
            print("PASS silent playback: real motion, pause/resume/EOF, failure/retry, close, accepted export and immutable source")
            return
        if args.timeline:
            def read_time(name):
                click(editor, *center(editor, name))
                run("xdotool", "key", "ctrl+a", "ctrl+c", "sleep", ".2")
                return int(run("xclip", "-selection", "clipboard", "-o").strip())

            def drag(name, delta, cancel=False):
                # Grab the probed grip itself, rather than the interval boundary.
                x, y = center(editor, name)
                run("xdotool", "mousemove", "--sync", "--window", editor, str(x), str(y),
                    "mousedown", "1", "sleep", ".15", "mousemove_relative", "--sync", "--",
                    str(delta), "0", "sleep", ".2")
                if cancel:
                    run("xdotool", "key", "Escape", "mousemove_relative", "--sync", "--", "100", "0")
                run("xdotool", "mouseup", "1", "sleep", ".2")

            # Time is measured from the original pointer, not absolute x; the
            # ~850px track at 960px makes each pixel about 3.5 ms.
            track = visible_rect(editor, "Timeline track")
            assert 780 <= track[2] - track[0] <= 900, track
            drag("Trim start", 2)
            assert read_time("Start (ms)") == 0, "subthreshold drag cannot jump trim start"
            drag("Trim start", 300)
            start = read_time("Start (ms)")
            assert 1000 <= start <= 1100, ("start drag", start)
            drag("Trim end", -200)
            end = read_time("End (ms)")
            assert 2250 <= end <= 2400, ("end drag", end)
            drag("Trim start", 15, cancel=True)
            cancelled_start = read_time("Start (ms)")
            assert 45 <= cancelled_start - start <= 60, (start, cancelled_start)
            # A click focuses a handle without changing its value. Keyboard
            # adjustment must happen once, even across egui layout passes.
            press(editor, "Trim start")
            run("xdotool", "key", "Right", "sleep", ".2")
            start = read_time("Start (ms)")
            assert start == cancelled_start + 1, ("focused keyboard step", start, cancelled_start)
            shot(editor, "timeline-staged")
            dominant(output / "timeline-staged.png", 0)
            destination = exports / "timeline.mp4"
            set_destination(editor, destination)
            press(editor, "Save new copy")
            assert not destination.exists() and len(list(history.glob("*/metadata.json"))) == 1
            press(editor, "Apply edits")
            fill(editor, "Position (ms)", 1500)
            press(editor, "Seek")
            shot(editor, "timeline-applied")
            dominant(output / "timeline-applied.png", 1)
            press(editor, "Save new copy")
            wait(lambda: len(list(history.glob("*/metadata.json"))) == 2, "timeline export published")
            info = json.loads(run("ffprobe", "-v", "error", "-show_format", "-of", "json", str(destination)))
            expected_seconds = (end - start) / 1000
            assert 1.1 < expected_seconds < 1.4
            assert abs(float(info["format"]["duration"]) - expected_seconds) < .15, info
            dominant(destination, 1, .1)
            dominant(destination, 2, 1.0)
            choose(editor, "Format", ".gif")
            press(editor, "Apply edits")
            press(editor, "Save new copy")
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
        if args.estimate_delta:
            press(editor, "Estimate size")
            shot(editor, "delta-original-zero")
            choose(editor, "Format", ".gif")
            shot(editor, "delta-staged")
            press(editor, "Apply edits")
            press(editor, "Estimate size")
            shot(editor, "delta-sampled")
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            shot(editor, "delta-sampled-minimum")
            run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
            fill(editor, "Start (ms)", 1000)
            fill(editor, "End (ms)", 4000)
            press(editor, "Apply edits")
            press(editor, "Estimate size")
            shot(editor, "delta-exact")
            assert len(list(history.glob("*/metadata.json"))) == 1 and not list(exports.iterdir())
            run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
            shot(editor, "delta-exact-minimum")
            # A real minimum-window Estimate click checks the longer label did
            # not steal the button's input region; a missing source must fail.
            missing = output / "temporarily-moved.mp4"
            source.rename(missing)
            try:
                press(editor, "Estimate size")
                shot(editor, "delta-estimate-error-minimum")
            finally:
                missing.rename(source)
            press(editor, "Estimate size")
            shot(editor, "delta-retry-minimum")
            run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
            destination = exports / "delta.gif"
            set_destination(editor, destination)
            press(editor, "Save new copy")
            wait(lambda: len(list(history.glob("*/metadata.json"))) == 2, "delta export in History")
            idle(editor)  # History publication precedes accepted save identity on the UI thread.
            size = destination.stat().st_size
            expected = expected_estimate(size)
            assert '%' in expected, "fixture must discriminate a nonzero delta"
            assert source.read_bytes() == original and metadata.read_bytes() == original_metadata
            close(editor)
            wait(lambda: not windows("Recording editor"), "estimate and save retain clean close")
            close(root)
            wait(lambda: app.poll() is not None, "delta quit")
            assert app.returncode == 0
            (output / "result.json").write_text(json.dumps({"passed": True, "appearance": args.appearance,
                "source_bytes": len(original), "saved_bytes": size,
                "exact_label_expectation_for_visual_inspection": expected,
                "checks": ["zero-original", "staged", "sampled-normal-minimum", "exact-normal-minimum",
                    "minimum-estimate-error-retry", "no-estimate-publication", "immutable-source-history", "clean-close"]}, indent=2) + "\n")
            print(f"PASS estimate delta exports: source {len(original)} bytes, saved {size}, expected {expected}")
            return
        if args.estimate:
            press(editor, "Estimate size")
            shot(editor, "estimate-original")
            estimate_expectations["estimate-original.png"] = expected_estimate(len(original))
            assert len(list(history.glob("*/metadata.json"))) == 1 and not list(exports.iterdir())
            missing = output / "temporarily-moved.mp4"
            source.rename(missing)
            try:
                press(editor, "Estimate size")
                shot(editor, "estimate-missing-source")
            finally:
                missing.rename(source)
            press(editor, "Estimate size")
            shot(editor, "estimate-retried")
            estimate_expectations["estimate-retried.png"] = expected_estimate(len(original))
            if args.audio:
                run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
                volume(editor, "System audio", 50)
                press(editor, "Apply edits")
                press(editor, "Estimate size")
                shot(editor, "estimate-audio-approximate")
                estimate_expectations["estimate-audio-approximate.png"] = expected_estimate(len(original), exact=False)
                volume(editor, "System audio", 100)
                press(editor, "Apply edits")
                run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
        if args.crop_aspect:
            run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
            press(editor, "Crop recording")
            fill(editor, "Crop width", source_width // 2)
            shot(editor, "locked-width-staged")

            def save_crop(name, expected):
                path = exports / f"{name}.mp4"
                set_destination(editor, path)
                press(editor, "Save new copy")
                assert not path.exists(), "unapplied crop gates save"
                press(editor, "Apply edits")
                shot(editor, f"{name}-preview")
                count = len(list(history.glob("*/metadata.json")))
                press(editor, "Save new copy")
                wait(lambda: len(list(history.glob("*/metadata.json"))) == count + 1, name)
                info = json.loads(run("ffprobe", "-v", "error", "-show_streams", "-of", "json", str(path)))
                stream = next(s for s in info["streams"] if s["codec_type"] == "video")
                assert (stream["width"], stream["height"]) == expected, (name, stream)

            save_crop("locked-width", (source_width // 2, source_height // 2 // 2 * 2))
            fill(editor, "Crop height", source_height)
            save_crop("locked-height", (source_width, source_height))
            press(editor, "Lock aspect ratio")  # Unlock. Only width changes now.
            fill(editor, "Crop width", 160)
            save_crop("unlocked-width", (160, source_height))
            # Relocking uses the current crop ratio, not the original source.
            press(editor, "Lock aspect ratio")
            fill(editor, "Crop height", source_height // 2)
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
        fill(editor, "Position (ms)", 1500)
        press(editor, "Seek")
        shot(editor, "seek-green")
        dominant(output / "seek-green.png", 1)
        fill(editor, "End (ms)", 1100)
        fill(editor, "Start (ms)", 2600)
        press(editor, "Apply edits")  # Apply edits stays in the fixed save bar.
        shot(editor, "invalid-trim")
        dominant(output / "invalid-trim.png", 1)
        run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
        shot(editor, "minimum-error")
        run("xdotool", "windowsize", "--sync", editor, "960", "900", "sleep", ".5")
        fill(editor, "Start (ms)", 1100)
        fill(editor, "End (ms)", 2300)
        press(editor, "Apply edits")
        shot(editor, "trimmed")
        dominant(output / "trimmed.png", 1)
        if args.estimate:
            press(editor, "Estimate size")
            shot(editor, "estimate-trimmed")
            assert len(list(history.glob("*/metadata.json"))) == 1 and not list(exports.iterdir())
        close(root)
        assert app.poll() is None and windows("Recording editor"), "dirty editor blocks quit"
        shot(editor, "quit-guard")
        close(editor)
        assert windows("Recording editor"), "dirty editor requires confirmation"
        shot(editor, "close-confirmation")
        press(editor, "Keep editing")  # Keep editing, immediately above the fixed destination row.
        run("xdotool", "windowminimize", root, "sleep", ".5")
        destination = exports / "trimmed.mp4"
        set_destination(editor, destination)
        press(editor, "Save new copy")
        wait(lambda: len(list(history.glob("*/metadata.json"))) == 2, "export published in History")
        info = json.loads(run("ffprobe", "-v", "error", "-show_format", "-show_streams", "-of", "json", str(destination)))
        assert abs(float(info["format"]["duration"]) - 1.2) < .15, info
        assert (info["streams"][0]["width"], info["streams"][0]["height"]) == (source_width, source_height)
        dominant(destination, 1, .2)
        dominant(destination, 2, 1.0)
        shot(editor, "saved")
        assert "Show in Folder" in controls(editor), "a successful copy offers Show in Folder"
        saved_bytes = destination.read_bytes()
        if args.estimate:
            estimate_expectations["estimate-trimmed.png"] = expected_estimate(len(saved_bytes))
        press(editor, "Save new copy")
        shot(editor, "collision")
        assert destination.read_bytes() == saved_bytes and len(list(history.glob("*/metadata.json"))) == 2
        choose(editor, "Format", ".gif")
        if args.estimate:
            press(editor, "Estimate size")  # Unapplied GIF must not reuse the MP4 estimate.
            shot(editor, "estimate-staged-format")
        press(editor, "Save new copy")  # Format changes cannot save unaccepted preview settings.
        assert not destination.with_suffix(".gif").exists()
        press(editor, "Apply edits")
        if args.estimate:
            press(editor, "Estimate size")
            shot(editor, "estimate-gif")
        press(editor, "Save new copy")
        wait(lambda: len(list(history.glob("*/metadata.json"))) == 3, "GIF published in History")
        gif = destination.with_suffix(".gif")
        if args.estimate:
            estimate_expectations["estimate-gif.png"] = expected_estimate(gif.stat().st_size)
        dominant(gif, 1, .2)
        dominant(gif, 2, 1.0)
        shot(editor, "gif-saved")

        # Asymmetric crop and independently sized output catch ignored origins,
        # resize-only implementations, and accidental loss of the accepted trim.
        run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
        press(editor, "Crop recording")  # Crop recording.
        press(editor, "Lock aspect ratio")  # Unlock for independent asymmetric crop fields.
        fill(editor, "Crop X", 10)
        fill(editor, "Crop Y", 6)
        fill(editor, "Crop width", source_width)  # Valid width alone, invalid with X=10.
        fill(editor, "Crop height", 90)
        press(editor, "Apply edits")
        shot(editor, "invalid-crop")
        dominant(output / "invalid-crop.png", 1)
        crop_destination = exports / "cropped.mp4"
        choose(editor, "Format", ".mp4")  # MP4.
        set_destination(editor, crop_destination)
        press(editor, "Save new copy")  # Save remains gated while the crop is unapplied.
        assert not crop_destination.exists() and len(list(history.glob("*/metadata.json"))) == 3
        fill(editor, "Crop width", 160)
        choose(editor, "Output resolution", "Custom")  # Custom output size.
        fill(editor, "Output width", 81)
        fill(editor, "Output height", 61)
        shot(editor, "crop-staged")
        press(editor, "Apply edits")
        shot(editor, "cropped")
        dominant(output / "cropped.png", 1)
        # Frame is 80x60 after shared even rounding, fitted into the preview.
        # Both samples (output pixels 8,6 and 20,12) lie inside the translated
        # white box; omitting the crop or either origin makes one sample green.
        samples = [image_point(editor, .1, .1), image_point(editor, .25, .2)]
        shot(editor, "cropped-samples")
        for x, y in samples:
            pixel = run("convert", str(output / "cropped-samples.png"), "-crop", f"1x1+{x}+{y}", "-depth", "8", "rgb:-")
            assert len(pixel) == 3 and min(pixel) > 210, (x, y, pixel)
        run("xdotool", "windowsize", "--sync", editor, "760", "580", "sleep", ".5")
        run("xdotool", "mousemove", "--window", editor, "690", "380", "click", "--repeat", "12", "--delay", "60", "5", "sleep", ".5")
        shot(editor, "minimum-crop-controls")
        run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
        run("xdotool", "mousemove", "--window", editor, "690", "380", "click", "--repeat", "20", "--delay", "60", "4", "sleep", ".5")
        press(editor, "Save new copy")
        wait(lambda: len(list(history.glob("*/metadata.json"))) == 4, "cropped MP4 in History")
        choose(editor, "Format", ".gif")
        press(editor, "Apply edits")
        press(editor, "Save new copy")
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
        # MP4 fits the shared encoder limit. GIF applies its default 800px cap
        # to the even-normalized custom base (4000x600), producing 800x120.
        run("xdotool", "windowsize", "--sync", editor, "960", "1100", "sleep", ".5")
        run("xdotool", "mousemove", "--window", editor, "690", "380", "click", "--repeat", "20", "--delay", "60", "4", "sleep", ".5")
        fill(editor, "End (ms)", 1300)
        fill(editor, "Output width", 4001)
        fill(editor, "Output height", 601)
        choose(editor, "Format", ".mp4")
        large_destination = exports / "encoder-sized.mp4"
        set_destination(editor, large_destination)
        press(editor, "Apply edits")
        shot(editor, "mp4-encoder-preview")
        press(editor, "Save new copy")
        wait(lambda: len(list(history.glob("*/metadata.json"))) == 6, "encoder-sized MP4 in History")
        choose(editor, "Format", ".gif")
        press(editor, "Save new copy")
        assert not large_destination.with_suffix(".gif").exists()
        shot(editor, "format-staged")
        press(editor, "Apply edits")
        shot(editor, "gif-sized-preview")
        press(editor, "Save new copy")
        wait(lambda: len(list(history.glob("*/metadata.json"))) == 7, "width-capped GIF in History")
        for path, width, height in ((large_destination, 3840, 576), (large_destination.with_suffix(".gif"), 800, 120)):
            info = json.loads(run("ffprobe", "-v", "error", "-show_format", "-show_streams", "-of", "json", str(path)))
            stream = info["streams"][0]
            assert (stream["width"], stream["height"]) == (width, height), info
            assert abs(float(info["format"]["duration"]) - .2) < .1, info
            frame = run("ffmpeg", "-v", "error", "-i", str(path), "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-")
            assert len(frame) == width * height * 3
            # Probe the same asymmetric white marker and green background in
            # each output's coordinates, never beyond the capped GIF's bounds.
            white_index = ((height * 2 // 15) * width + width // 10) * 3
            green_index = ((height // 2) * width + width // 2) * 3
            white = frame[white_index:white_index + 3]
            green = frame[green_index:green_index + 3]
            assert min(white) > 210, (path, white)
            assert green[1] > 90 and green[1] > max(green[0], green[2]) + 40, (path, green)
        shot(editor, "format-saved")
        audio_checks = []
        if args.audio:
            press(editor, "Crop recording")  # Remove crop and resize to expose audio rows.
            choose(editor, "Output resolution", "Original")
            fill(editor, "End (ms)", 2300)
            choose(editor, "Format", ".mp4")
            volume(editor, "System audio", 25)
            volume(editor, "Microphone", 175)
            shot(editor, "audio-staged")
            pending = exports / "pending-audio.mp4"
            set_destination(editor, pending)
            press(editor, "Save new copy")
            assert not pending.exists(), "unapplied audio must gate save"
            press(editor, "Apply edits")
            shot(editor, "audio-applied")

            def audio_export(filename):
                path = exports / filename
                count = len(list(history.glob("*/metadata.json")))
                set_destination(editor, path)
                press(editor, "Save new copy")
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
            choose(editor, "Format", ".gif")
            press(editor, "Apply edits")
            shot(editor, "gif-audio-disabled")
            # GIFs show only the shipping audio note; retained MP4 settings are untouched.
            assert "System audio" not in controls(editor), "GIF must replace the audio rows"
            _, streams = audio_export("audio-free.gif")
            assert not streams
            choose(editor, "Format", ".mp4")
            press(editor, "Apply edits")
            restored, streams = audio_export("audio-restored.mp4")
            assert len(streams) == 1 and streams[0]["channels"] == 2, streams
            assert_tones(restored, 2, ((.025, .14), (.0125, .14)))
            press(editor, "Microphone")  # Mute microphone, retain system gain/stereo.
            press(editor, "Apply edits")
            system_only, streams = audio_export("system-only.mp4")
            assert len(streams) == 1 and streams[0]["channels"] == 2, streams
            assert_tones(system_only, 2, ((.025, 0), (.0125, 0)))
            press(editor, "System audio")
            press(editor, "Microphone")
            press(editor, "Convert to mono")  # Microphone only, mono.
            press(editor, "Apply edits")
            microphone_only, streams = audio_export("microphone-only.mp4")
            assert len(streams) == 1 and streams[0]["channels"] == 1, streams
            assert_tones(microphone_only, 1, ((0, .14 * math.sqrt(2)),))
            shot(editor, "microphone-mono")
            press(editor, "Microphone")  # Both muted removes the audio stream.
            press(editor, "Apply edits")
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
                press(editor, "Crop recording")  # Clear previous crop/custom size.
                choose(editor, "Output resolution", "Original")
                fill(editor, "End (ms)", 2300)
                choose(editor, "Format", ".mp4")
            for name, label, expected in (("720", "720p maximum", (320, 720)), ("1080", "1080p maximum", (480, 1080)), ("original", "Original", (640, 1440))):
                press(editor, "Output resolution")
                shot(editor, f"preset-{name}-menu")
                click(editor, *center(editor, f"Output resolution/{label}", prefix=True))
                path = exports / f"preset-{name}.mp4"
                set_destination(editor, path)
                press(editor, "Save new copy")
                assert not path.exists(), "unapplied preset must gate save"
                press(editor, "Apply edits")
                shot(editor, f"preset-{name}-preview")
                count = len(list(history.glob("*/metadata.json")))
                press(editor, "Save new copy")
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
