#!/usr/bin/python3
"""Record native controls on a private X11 desktop; never the caller's display.

Requires system Python dbus/gi, Xvfb, Openbox, picom, hsetroot, xdotool,
ImageMagick, FFmpeg and FFprobe. Uses actual input and persisted media, no app hook.
"""
import argparse
import json
import os
from pathlib import Path
import select
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
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    env.update(WGPU_BACKEND="gl", WINIT_X11_SCALE_FACTOR="1", XDG_SESSION_TYPE="x11")
    for variable, directory in (("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
                                ("XDG_DATA_HOME", "data"), ("XDG_RUNTIME_DIR", "runtime")):
        path = output / directory
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
        process = subprocess.Popen(command, env=env, stdout=stdout, stderr=stderr)
        children.append(process)
        if announce:
            if not select.select([process.stdout], [], [], 10)[0]:
                raise RuntimeError(f"{name} did not announce its endpoint")
            endpoint = process.stdout.readline().decode().strip()
            assert endpoint, f"{name} failed; inspect stderr"
            return endpoint
        return process

    def run(*command):
        return subprocess.check_output(command, env=env, timeout=30)

    def windows(title):
        result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", f"^{title}$"],
                                env=env, capture_output=True, text=True, timeout=5)
        assert result.returncode in (0, 1), result.stderr
        return result.stdout.split()

    def shot(window, name):
        run("import", "-window", window, str(output / f"{name}.png"))

    def wait(predicate, description):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if value := predicate():
                return value
            time.sleep(.05)
        shot("root", "timeout-desktop")
        raise AssertionError(f"Timed out: {description}")

    def click(window, x, y):
        run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
            "mousemove", "--sync", "--window", window, str(x - 1), str(y),
            "mousemove_relative", "--sync", "1", "0", "sleep", ".15", "mousedown", "1",
            "sleep", ".15", "mouseup", "1")

    def manifest():
        files = list((output / "recording-recovery").glob("*/manifest.json"))
        assert len(files) <= 1, "a second recording started before the first ended"
        try:
            return json.loads(files[0].read_text()) if files else None
        except FileNotFoundError:
            # Discard/publication can remove the bundle between glob and read.
            return None

    def select_recording(name):
        selector = wait(lambda: windows("Captures Capture Controls"), "capture controls")[0]
        wait(lambda: int(run("import", "-window", selector, "-crop", "1280x96+0+804",
                             "-format", "%k", "info:")) > 16, "painted controls")
        click(selector, 405, 811)
        shot(selector, name)
        # egui must observe a held pointer, not press/release in one input batch.
        run("xdotool", "mousemove", "--sync", "--window", selector, "140", "180",
            "sleep", ".1", "mousedown", "1", "sleep", ".2",
            "mousemove", "--sync", "--window", selector, "450", "350",
            "sleep", ".2", "mouseup", "1", "key", "Return")

    def running_hud():
        wait(lambda: (value := manifest()) and value["state"] == "recording", "durable Recording")
        hud = wait(lambda: windows("Captures Recording Controls"), "running HUD")[0]
        time.sleep(.5)
        return hud

    def history():
        return set((output / "history").glob("*/metadata.json"))

    def finished(expected_count):
        wait(lambda: len(history()) == expected_count, "History publication count")
        wait(lambda: not windows("Captures Recording Controls"), "HUD removal")
        wait(lambda: manifest() is None, "recovery source cleanup")
        # Disk cleanup precedes the worker reply. Only the event-thread finish
        # restores the previously visible root and releases the capture flow.
        wait(lambda: windows("Captures"), "workspace restoration after worker completion")

    try:
        env["DISPLAY"] = ":" + spawn("xvfb", ["Xvfb", "-displayfd", "1", "-screen", "0",
            "1280x900x24", "-dpi", "96", "-nolisten", "tcp", "-noreset"], True)
        address = spawn("dbus", ["dbus-daemon", "--session", "--nofork", "--print-address=1"], True)
        env["DBUS_SESSION_BUS_ADDRESS"] = env["DBUS_SYSTEM_BUS_ADDRESS"] = address
        DBusGMainLoop(set_as_default=True)
        bus = dbus.bus.BusConnection(address)
        name = dbus.service.BusName("org.freedesktop.ScreenSaver", bus=bus, do_not_queue=True)
        saver = ScreenSaver(name, "/org/freedesktop/ScreenSaver")
        loop = GLib.MainLoop()
        threading.Thread(target=loop.run, daemon=True).start()
        spawn("openbox", ["openbox", "--sm-disable"])
        spawn("picom", ["picom", "--config", "/dev/null", "--backend", "xrender"])
        time.sleep(1)
        run("hsetroot", "-solid", "#c02040")
        settings = output / "settings.json"
        settings.write_text(json.dumps({
            "settings_schema_version": 5, "appearance": "dark", "theme": "mustard",
            "output_directory": str(output / "exports"),
            "new_capture_shortcut": "Ctrl+Shift+F10", "region_shortcut": "Ctrl+Shift+F7",
            "window_shortcut": "Ctrl+Shift+F8", "display_shortcut": "Ctrl+Shift+F9",
            "launch_at_login": False,
            "auto_copy_to_clipboard": False, "auto_start_on_selection": False,
            "freeze_screen": True, "screenshot_countdown_seconds": 0,
            "recording": {"video_fps": 15, "countdown_seconds": 3, "show_cursor": False,
                          "highlight_clicks": False, "capture_system_audio": False,
                          "microphone_device_id": None, "open_editor_after_recording": False},
        }))
        app = spawn("app", [str(binary), "--live", "--history-root", str(output / "history"),
                            "--settings-file", str(settings), "--quit-after", "120"])
        root = wait(lambda: windows("Captures"), "capture workspace")[0]
        time.sleep(1)
        click(root, 341, 141)
        select_recording("recording-selector")
        countdown = wait(lambda: windows("Captures Recording Countdown"), "recording countdown")[0]
        shot(countdown, "recording-countdown")
        hud = running_hud()
        shot(hud, "hud-running")
        # Escape only cancels before engine handoff, not an accepted recording.
        run("xdotool", "key", "Escape")
        time.sleep(.3)
        assert manifest()["state"] == "recording"
        assert windows("Captures Recording Controls")
        click(hud, 178, 54)
        wait(lambda: (value := manifest()) and value["state"] == "paused", "pause completed")
        hud = wait(lambda: windows("Captures Recording Controls"), "paused HUD")[0]
        time.sleep(.3)
        shot(hud, "hud-paused")
        run("hsetroot", "-solid", "#2070c0")
        click(hud, 178, 54)
        wait(lambda: (value := manifest()) and value["state"] == "recording", "resume completed")
        hud = wait(lambda: windows("Captures Recording Controls"), "resumed HUD")[0]
        time.sleep(.5)
        click(hud, 142, 54)
        metadata = wait(lambda: list((output / "history").glob("*/metadata.json")), "History publication")
        assert len(metadata) == 1
        entry = json.loads(metadata[0].read_text())
        assert entry["kind"] == "video" and entry["mime_type"] == "video/mp4", entry
        assert (entry["width"], entry["height"]) == (310, 170), entry
        assert entry["target"]["rect"] == {"x": 140, "y": 180, "width": 310, "height": 170}
        assert entry["saved_path"] is None and not entry["has_microphone_audio"]
        media = metadata[0].parent / "media.mp4"
        frames = run("ffmpeg", "-v", "error", "-i", str(media), "-vf", "crop=2:2:40:40",
                     "-f", "rawvideo", "-pix_fmt", "rgb24", "-")
        assert len(frames) >= 24
        for actual, expected in ((frames[:3], (192, 32, 64)), (frames[-3:], (32, 112, 192))):
            assert all(abs(a - e) <= 6 for a, e in zip(actual, expected)), (actual, expected)
        assert (metadata[0].parent / "preview.png").is_file()
        finished(1)
        published = history()

        # A countdown Escape discards only its prepared bundle, never an earlier take.
        run("xdotool", "key", "ctrl+shift+F10")
        select_recording("cancel-selector")
        wait(lambda: windows("Captures Recording Countdown"), "cancellable countdown")
        run("xdotool", "key", "Escape")
        wait(lambda: not windows("Captures Recording Countdown"), "countdown cancellation")
        finished(1)
        assert history() == published
        assert not windows("Captures Capture Controls")

        # Explicit Discard removes a started take, leaving the prior MP4 untouched.
        run("xdotool", "key", "ctrl+shift+F10")
        select_recording("discard-selector")
        hud = running_hud()
        click(hud, 358, 54)
        finished(1)
        assert history() == published and media.is_file()

        # A real session-adapter poll sees our simulated lock. Accepted content
        # must be finalized rather than mistaken for a cancelled preparation.
        run("xdotool", "key", "ctrl+shift+F10")
        select_recording("lock-selector")
        running_hud()
        saver.locked = True
        finished(2)
        preserved = history() - published
        assert len(preserved) == 1
        locked_entry = json.loads(next(iter(preserved)).read_text())
        assert locked_entry["kind"] == "video" and locked_entry["duration_ms"] > 0
        locked_media = next(iter(preserved)).parent / "media.mp4"
        run("ffmpeg", "-v", "error", "-i", str(locked_media), "-f", "null", "-")
        saver.locked = False

        # Closing the child window is not permission to destroy an accepted take.
        published = history()
        run("xdotool", "key", "ctrl+shift+F10")
        select_recording("close-selector")
        hud = running_hud()
        run("xdotool", "windowactivate", "--sync", hud, "key", "alt+F4")
        finished(3)
        closed = history() - published
        assert len(closed) == 1
        closed_media = next(iter(closed)).parent / "media.mp4"
        run("ffmpeg", "-v", "error", "-i", str(closed_media), "-f", "null", "-")
        time.sleep(.3)
        shot(wait(lambda: windows("Captures"), "recording History")[0], "recording-history")

        assert saver.queries > 0 and app.poll() is None
        (output / "acceptance.json").write_text(json.dumps({
            "region": entry["target"]["rect"], "duration_ms": entry["duration_ms"],
            "first_pixel": list(frames[:3]), "last_pixel": list(frames[-3:]),
            "pause_resume": True, "history_publication": True,
            "running_escape_ignored": True, "countdown_escape_discarded": True,
            "explicit_discard": True, "session_lock_preserved": True, "child_close_saved": True,
        }, indent=2))
        print("PASS native recording: selector/countdown, pause/resume, MP4 pixels, History, "
              "Escape scope, explicit discard, session-lock/child-close preservation and source cleanup")
    finally:
        if loop is not None:
            loop.quit()
        for process in reversed(children):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
        for stream in logs:
            stream.close()


if __name__ == "__main__":
    main()
