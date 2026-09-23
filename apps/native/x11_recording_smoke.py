#!/usr/bin/python3
"""Record native controls on a private X11 desktop; never the caller's display.

Requires system Python dbus/gi/Xlib, Xvfb, Openbox, picom, hsetroot, xdotool,
ImageMagick, FFmpeg and FFprobe. Uses actual input and persisted media, no app hook.
"""
import argparse
import json
import math
import os
from pathlib import Path
import re
import select
import struct
import subprocess
import threading
import time

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib
from Xlib import X, display, protocol
from Xlib.ext import shape

from x11_capture_smoke import ScreenSaver


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--restart-only", action="store_true",
                        help="stop after running/paused Restart and replacement-media checks")
    parser.add_argument("--hide-controls-only", action="store_true",
                        help="exercise real-SNI Hide/restore, tray loss and finalized media")
    parser.add_argument("--screenshot-only", action="store_true",
                        help="exercise running/paused region screenshots without replacing the take")
    parser.add_argument("--ready-notice-only", action="store_true",
                        help="exercise recording-ready save/retry/reveal, expiry and dismissal")
    parser.add_argument("--appearance", choices=("dark", "light"), default="dark")
    parser.add_argument("--virtual-microphone", action="store_true",
                        help="use a disposable PulseAudio microphone to verify live meter and mute segments")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    env.pop("WAYLAND_DISPLAY", None)
    if args.virtual_microphone:
        for variable in ("PULSE_SERVER", "PULSE_COOKIE", "PULSE_RUNTIME_PATH"):
            env.pop(variable, None)
    env.update(WGPU_BACKEND="gl", WINIT_X11_SCALE_FACTOR="1", XDG_SESSION_TYPE="x11")
    for variable, directory in (("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
                                ("XDG_DATA_HOME", "data"), ("XDG_RUNTIME_DIR", "runtime")):
        path = output / directory
        path.mkdir(mode=0o700)
        env[variable] = str(path)
    children, logs = [], []
    loop = None
    panel = None

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
        result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name",
                                 f"^{re.escape(title)}$"],
                                env=env, capture_output=True, text=True, timeout=5)
        assert result.returncode in (0, 1), result.stderr
        return result.stdout.split()

    def shot(window, name):
        run("import", "-window", window, str(output / f"{name}.png"))

    def meter_pixels(window):
        # The fixed-size live HUD's meter track only; exclude adjacent buttons.
        pixels = run("import", "-window", window, "-crop", "28x4+272+51", "-depth", "8", "rgb:-")
        return sum(min(pixels[index:index + 3]) > 220 for index in range(0, len(pixels), 3))

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

    def window_geometry(window):
        values = {}
        for line in run("xdotool", "getwindowgeometry", "--shell", window).decode().splitlines():
            if "=" in line:
                key, value = line.split("=", 1)
                values[key] = value
        return values

    def menu_action(label):
        labels = ["New Capture", "Show recording controls", "Capture display",
                  "Capture region", "Capture window", "History", "Preferences",
                  "Open output folder", "Quit Captures"]
        panel_ids = run("xdotool", "search", "--onlyvisible", "--class", "xfce4-panel").decode().split()
        tray = next(window for window in panel_ids if int(window_geometry(window)["WIDTH"]) >= 24)
        geometry = window_geometry(tray)
        run("xdotool", "mousemove", "--window", tray, str(int(geometry["WIDTH"]) // 2),
            str(int(geometry["HEIGHT"]) // 2), "click", "3", "sleep", ".4")
        popup_ids = run("xdotool", "search", "--onlyvisible", "--class", ".*").decode().split()
        popup = next(window for window in popup_ids
                     if b"_MENU" in run("xprop", "-id", window, "_NET_WM_WINDOW_TYPE"))
        popup_geometry = window_geometry(popup)
        run("xdotool", "mousemove", "--window", popup,
            str(int(popup_geometry["WIDTH"]) // 2),
            str(int((labels.index(label) + .5) * int(popup_geometry["HEIGHT"]) / len(labels))),
            "click", "1")

    def manifest():
        files = list((output / "recording-recovery").glob("*/manifest.json"))
        assert len(files) <= 1, "a second recording started before the first ended"
        try:
            return json.loads(files[0].read_text()) if files else None
        except FileNotFoundError:
            # Discard/publication can remove the bundle between glob and read.
            return None

    def select_recording(name, shortcuts=False):
        selector = wait(lambda: windows("Captures Capture Controls"), "capture controls")[0]
        wait(lambda: int(run("import", "-window", selector, "-crop", "1280x96+0+804",
                             "-format", "%k", "info:")) > 16, "painted controls")
        if shortcuts:
            shot(selector, "record-window-shortcut")
        # A fresh New Capture starts in Screenshot mode, whose toolbar is
        # shorter than Record's. Select mode and target explicitly through the
        # registered shortcut instead of clicking a previous layout's row.
        run("xdotool", "key", "ctrl+alt+r", "sleep", ".6")
        shot(selector, name)
        # Observe the actual selected area: the unified controls toolbar does
        # not necessarily change its pixels when a region becomes valid.
        before = run("import", "-window", selector, "-crop", "310x170+140+180",
                     "-depth", "8", "rgb:-")
        # egui must observe a held pointer, not press/release in one input batch.
        # Like the capture smoke, focus the mapped selector before injecting
        # input so the window manager does not consume the press for activation.
        run("xdotool", "windowfocus", "--sync", selector,
            "mousemove", "--sync", "--window", selector, "140", "180",
            "sleep", ".1", "mousedown", "1", "sleep", ".2",
            "mousemove", "--sync", "--window", selector, "450", "350",
            "sleep", ".2", "mouseup", "1")
        wait(lambda: run("import", "-window", selector, "-crop", "310x170+140+180",
                         "-depth", "8", "rgb:-") != before,
             "painted recording selection before confirmation")
        if shortcuts:
            # Switching across every target must retain this asymmetric region,
            # not replace the child or accidentally start a Display recording.
            for chord in ("ctrl+alt+d", "ctrl+alt+w", "ctrl+alt+r"):
                run("xdotool", "key", chord, "sleep", ".2")
                assert windows("Captures Capture Controls") == [selector]
                assert manifest() is None and not history()
            # Do not drag this corner again: the retained selection correctly
            # treats that as a NW resize and collapses it toward the SE corner.
            # Validate the preserved target after confirmation instead.
        # Confirmation destroys the selector before asynchronous preparation
        # necessarily writes its manifest. Never reactivate that retired XID or
        # repeat Return while waiting for the accepted recording to start.
        run("xdotool", "windowactivate", "--sync", selector, "key", "Return")
        wait(manifest, "recording confirmation reaches preparation")
        assert manifest()["options"]["target"]["rect"] == {
            "x": 140, "y": 180, "width": 310, "height": 170,
        }, "recording must retain the selected region across target shortcuts"

    def running_hud():
        wait(lambda: (value := manifest()) and value["state"] == "recording", "durable Recording")
        hud = wait(lambda: windows("Captures Recording Controls"), "running HUD")[0]
        time.sleep(.5)
        return hud

    def restart(hud, name):
        click(hud, 218, 54)
        confirmation = wait(lambda: windows("Restart recording?"), "restart confirmation")[0]
        shot(confirmation, name)
        click(confirmation, 104, 115)

    def history():
        return set((output / "history").glob("*/metadata.json"))

    def finished(expected_count):
        wait(lambda: len(history()) == expected_count, "History publication count")
        wait(lambda: not windows("Captures Recording Controls"), "HUD removal")
        wait(lambda: not windows("Captures Recording Region"), "region guide removal")
        wait(lambda: manifest() is None, "recovery source cleanup")
        # Disk cleanup precedes the worker reply. Only the event-thread finish
        # restores the previously visible root and releases the capture flow.
        wait(lambda: windows("Captures"), "workspace restoration after worker completion")
        time.sleep(.3)

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
        if args.hide_controls_only or args.ready_notice_only:
            config = output / "config/xfce4/xfconf/xfce-perchannel-xml/xfce4-panel.xml"
            config.parent.mkdir(parents=True)
            config.write_text('''<?xml version="1.0" encoding="UTF-8"?>
<channel name="xfce4-panel" version="1.0">
 <property name="configver" type="int" value="2"/>
 <property name="panels" type="array"><value type="int" value="1"/>
  <property name="panel-1" type="empty">
   <property name="position" type="string" value="p=0;x=1200;y=24"/>
   <property name="position-locked" type="bool" value="true"/>
   <property name="disable-struts" type="bool" value="true"/>
   <property name="length" type="uint" value="1"/>
   <property name="length-adjust" type="bool" value="true"/>
   <property name="size" type="uint" value="32"/>
   <property name="plugin-ids" type="array"><value type="int" value="1"/></property>
  </property>
 </property>
 <property name="plugins" type="empty">
  <property name="plugin-1" type="string" value="systray">
   <property name="hide-new-items" type="bool" value="false"/>
   <property name="icon-size" type="uint" value="24"/>
  </property>
 </property>
</channel>''')
            panel = spawn("sni-panel", ["xfce4-panel", "--disable-wm-check", "--sm-client-disable"])
            wait(lambda: bus.name_has_owner("org.kde.StatusNotifierWatcher"), "real SNI watcher")
        if args.virtual_microphone:
            spawn("pulseaudio", ["pulseaudio", "--daemonize=no", "--exit-idle-time=-1"])
            wait(lambda: subprocess.run(["pactl", "info"], env=env, capture_output=True).returncode == 0,
                 "PulseAudio virtual microphone server")
            run("pactl", "load-module", "module-null-sink", "sink_name=captures",
                "sink_properties=device.description=CapturesVirtualMicrophone")
            run("pactl", "set-default-source", "captures.monitor")
            tone = output / "microphone-tone.wav"
            run("ffmpeg", "-v", "error", "-f", "lavfi", "-i",
                "sine=frequency=730:sample_rate=48000", "-af", "volume=4", "-t", "120", str(tone))
            spawn("microphone-tone", ["paplay", "--device=captures", str(tone)])
        time.sleep(1)
        if args.screenshot_only:
            wallpaper = output / "asymmetric-wallpaper.png"
            run("convert", "-size", "1280x900", "xc:#c02040", "-fill", "#20a050",
                "-draw", "rectangle 140,180 294,349", "-fill", "#2070c0",
                "-draw", "rectangle 295,180 449,349", str(wallpaper))
            run("hsetroot", "-fill", str(wallpaper))
        else:
            run("hsetroot", "-solid", "#c02040")
        settings = output / "settings.json"
        settings.write_text(json.dumps({
            "settings_schema_version": 5, "appearance": args.appearance, "theme": "mustard",
            "output_directory": str(output / "exports"),
            "new_capture_shortcut": "Ctrl+Shift+F10", "region_shortcut": "Ctrl+Shift+F7",
            "window_shortcut": "Ctrl+Shift+F8", "display_shortcut": "Ctrl+Shift+F9",
            "launch_at_login": False,
            "auto_copy_to_clipboard": False, "auto_start_on_selection": False,
            "freeze_screen": True,
            "screenshot_countdown_seconds": 2 if args.screenshot_only else 0,
            "recording": {"video_fps": 15, "countdown_seconds": 3, "show_cursor": False,
                          "video_shortcut": "Ctrl+Alt+R", "window_shortcut": "Ctrl+Alt+W",
                          "display_shortcut": "Ctrl+Alt+D",
                          "highlight_clicks": False, "capture_system_audio": False,
                          "microphone_device_id": "default" if args.virtual_microphone else None,
                          "open_editor_after_recording": False},
        }))
        if args.ready_notice_only:
            # Observe the exact OS-launch argument without opening a file manager.
            # Export itself still uses the real Rust worker and filesystem.
            tools = output / "tools"
            tools.mkdir()
            opener = tools / "xdg-open"
            opener.write_text('#!/bin/sh\nprintf \'%s\\n\' "$@" > "$CAPTURES_TEST_REVEAL"\n')
            opener.chmod(0o755)
            env["PATH"] = str(tools) + os.pathsep + env["PATH"]
            env["CAPTURES_TEST_REVEAL"] = str(output / "revealed-path.txt")
        app = spawn("app", [str(binary), "--live", "--history-root", str(output / "history"),
                            "--settings-file", str(settings), "--quit-after", "120"])
        root = wait(lambda: windows("Captures"), "capture workspace")[0]
        time.sleep(1)
        run("xdotool", "key", "ctrl+alt+w")
        select_recording("recording-selector", shortcuts=not args.screenshot_only)
        countdown = wait(lambda: windows("Captures Recording Countdown"), "recording countdown")[0]
        guide = wait(lambda: windows("Captures Recording Region"), "countdown region guide")[0]
        shot(countdown, "recording-countdown")
        run("xdotool", "key", "ctrl+alt+d")
        hud = running_hud()
        shot(hud, "hud-running")
        if args.virtual_microphone:
            run("xdotool", "mousemove", "0", "0")
            wait(lambda: meter_pixels(hud) >= 20, "nonzero live microphone meter with root hidden")
            high = meter_pixels(hud)
            shot(hud, "meter-audible")
            run("pactl", "set-source-volume", "captures.monitor", "75%")
            wait(lambda: 0 < meter_pixels(hud) < high - 8, "smaller meter for quieter live audio")
            low = meter_pixels(hud)
            shot(hud, "meter-quieter")
            run("pactl", "set-source-volume", "captures.monitor", "0%")
            wait(lambda: meter_pixels(hud) == 0, "empty meter for silent unmuted microphone")
            shot(hud, "meter-silent")
            run("pactl", "set-source-volume", "captures.monitor", "100%")
            wait(lambda: meter_pixels(hud) >= high - 4, "meter resumes without reopening recording")
            shot(hud, "meter-restored")
            print(f"PASS microphone meter live pixels: audible {high}, quieter {low}, silent 0, restored", flush=True)
        assert windows("Captures Recording Region") == [guide]
        shot("root", "recording-region-running")
        # The guide must really be click-through, not just omit UI handlers.
        connection = display.Display(env["DISPLAY"])
        try:
            window = connection.create_resource_object("window", int(guide))
            assert not window.shape_get_rectangles(shape.SK.Input).rectangles
        finally:
            connection.close()
        # Probe every inner edge, where an inward/antialiased border would leak.
        for x, y in [(140, 180), (449, 180), (140, 349), (449, 349), (295, 265)]:
            pixel = run("import", "-window", "root", "-crop", f"1x1+{x}+{y}",
                        "-depth", "8", "rgb:-")
            expected = ((32, 160, 80) if x < 295 else (32, 112, 192)) \
                if args.screenshot_only else (192, 32, 64)
            assert tuple(pixel[:3]) == expected, (x, y, pixel)
        border = run("import", "-window", "root", "-crop", "1x1+139+220", "-depth", "8", "rgb:-")
        assert tuple(border[:3]) == (255, 202, 40), border
        veil = run("import", "-window", "root", "-crop", "1x1+100+220", "-depth", "8", "rgb:-")
        assert 0 < veil[0] < 192, veil
        if args.screenshot_only:
            session_id = manifest()["session_id"]
            for chord in ("ctrl+alt+r", "ctrl+alt+w", "ctrl+alt+d", "ctrl+shift+F7"):
                run("xdotool", "key", chord, "sleep", ".2")
                assert manifest()["session_id"] == session_id

            seen_selectors = set()

            def start_screenshot():
                seen_selectors.update(windows("Captures Region Selection"))
                controls = wait(lambda: windows("Captures Recording Controls"),
                                "recording controls before screenshot")[0]
                click(controls, 258, 54)
                for _ in range(3):
                    selector = wait(
                        lambda: [window for window in windows("Captures Region Selection")
                                 if window not in seen_selectors],
                        "new recording screenshot selector")[0]
                    seen_selectors.add(selector)
                    try:
                        wait(lambda: int(run("import", "-window", selector, "-format", "%k", "info:")) > 16,
                             "painted recording screenshot selector")
                    except subprocess.CalledProcessError:
                        continue
                    time.sleep(.3)
                    if selector in windows("Captures Region Selection"):
                        return selector
                raise AssertionError("recording screenshot selector did not stay active")

            def select_screenshot_region(selector, start, end, screenshot=None):
                for attempt in range(10):
                    run("xdotool", "mousemove", "--sync", *map(str, start),
                        "sleep", ".2", "mousedown", "1", "sleep", ".3", "mousemove",
                        "--sync", *map(str, end), "sleep", ".2", "mouseup", "1")
                    time.sleep(.2)
                    if screenshot and attempt == 0:
                        shot("root", screenshot)
                    run("xdotool", "key", "Return")
                    time.sleep(.2)
                    countdown = windows("Captures Screenshot Countdown")
                    if countdown:
                        return countdown[0]
                raise AssertionError("recording screenshot region was not confirmed")

            # Escape from selection owns only the child; the accepted take and
            # its passive guide remain alive.
            selector = start_screenshot()
            shot(selector, "recording-screenshot-selector-cancel")
            run("xdotool", "key", "Escape")
            hud = wait(lambda: windows("Captures Recording Controls"),
                       "HUD restored after screenshot selection cancellation")[0]
            assert manifest()["session_id"] == session_id and not history()
            time.sleep(1)  # Let the X11 hotkey worker release Escape before rearming it.

            # Persist one asymmetric region with fresh pixels and show its real
            # mini preview while the original recording keeps running.
            selector = start_screenshot()
            countdown = select_screenshot_region(
                selector, (140, 180), (450, 350), "recording-screenshot-selector-running")
            shot(countdown, "recording-screenshot-countdown-running")
            screenshot_metadata = wait(
                lambda: [path for path in history()
                         if json.loads(path.read_text())["kind"] == "screenshot"],
                "independent screenshot publication")
            assert len(screenshot_metadata) == 1
            screenshot_entry = json.loads(screenshot_metadata[0].read_text())
            assert (screenshot_entry["mode"], screenshot_entry["width"],
                    screenshot_entry["height"]) == ("region", 310, 170), screenshot_entry
            pixels = run("convert", str(screenshot_metadata[0].parent / "capture.png"),
                         "-depth", "8", "RGBA:-")
            assert pixels[:4] == bytes((32, 160, 80, 255)), pixels[:4]
            assert pixels[-4:] == bytes((32, 112, 192, 255)), pixels[-4:]
            preview = wait(lambda: windows("Captures Mini Preview"), "recording screenshot preview")[0]
            shot("root", "recording-screenshot-preview")
            assert manifest()["session_id"] == session_id

            # Pause is unchanged by the child. Escape during its countdown
            # removes only that child and publishes no second screenshot.
            hud = wait(lambda: windows("Captures Recording Controls"), "HUD after screenshot")[0]
            click(hud, 178, 54)
            wait(lambda: manifest()["state"] == "paused", "pause after screenshot")
            hud = wait(lambda: windows("Captures Recording Controls"), "paused HUD")[0]
            selector = start_screenshot()
            select_screenshot_region(selector, (530, 150), (810, 330))
            run("xdotool", "key", "Escape")
            hud = wait(lambda: windows("Captures Recording Controls"),
                       "paused HUD restored after countdown cancellation")[0]
            assert manifest()["state"] == "paused" and manifest()["session_id"] == session_id
            assert len(history()) == 1
            # Match selection cancellation above: the X11 hotkey worker must
            # observe Escape release before the restored HUD receives input.
            time.sleep(1)

            click(hud, 142, 54)
            finished(2)
            metadata = [path for path in history()
                        if json.loads(path.read_text())["kind"] == "video"]
            assert len(metadata) == 1
            media = metadata[0].parent / "media.mp4"
            run("ffmpeg", "-v", "error", "-i", str(media), "-f", "null", "-")
            assert screenshot_metadata[0].parent.exists() and media.is_file()
            acceptance = {
                "running_screenshot": True, "paused_countdown_escape": True,
                "selection_escape": True, "single_screenshot_publication": True,
                "mini_preview": True, "same_recording_session": True,
                "asymmetric_region": [310, 170], "actual_pixels": True,
                "recording_decoded": True, "recovery_cleanup": True,
                "x11_selector_visible_in_recording": True,
            }
            (output / "acceptance-recording-screenshot.json").write_text(
                json.dumps(acceptance, indent=2))
            print("PASS native recording Screenshot: running publish/preview, selection and paused "
                  "countdown Escape, asymmetric pixels, same take, decode and cleanup")
            return
        if args.ready_notice_only:
            def notice_click(window, x, y):
                # Notifications do not activate the root or accept WM activation.
                # Like click(), deliver a position event even for repeated clicks.
                run("xdotool", "mousemove", "--sync", "--window", window, str(x - 1), str(y),
                    "mousemove_relative", "--sync", "1", "0",
                    "sleep", ".15", "mousedown", "1", "sleep", ".15", "mouseup", "1")
                time.sleep(.3)

            def stop_with_notice(hud, count):
                click(hud, 142, 54)
                wait(lambda: len(history()) == count, "recording publication")
                notice = wait(lambda: windows("Recording ready"), "recording-ready notice")[0]
                wait(lambda: manifest() is None, "finalization cleanup")
                assert not windows("Captures Recording Controls")
                assert not windows("Captures Recording Region")
                assert run("xdotool", "getwindowfocus").decode().strip() != notice
                time.sleep(.4)
                return notice

            notice = stop_with_notice(hud, 1)
            shot(notice, "notice-ready")
            metadata = next(iter(history()))
            media = metadata.parent / "media.mp4"
            run("ffmpeg", "-v", "error", "-i", str(media), "-f", "null", "-")
            original = media.read_bytes()
            # Close the root normally into its real SNI tray: the notice must
            # continue accepting actions and expiring with no visible root.
            connection = display.Display(env["DISPLAY"])
            try:
                window = connection.create_resource_object("window", int(root))
                window.send_event(protocol.event.ClientMessage(
                    window=window, client_type=connection.intern_atom("WM_PROTOCOLS"),
                    data=(32, [connection.intern_atom("WM_DELETE_WINDOW"), X.CurrentTime, 0, 0, 0])))
                connection.sync()
            finally:
                connection.close()
            wait(lambda: not windows("Captures"), "root hidden in tray")
            exports = output / "exports"
            exports.write_text("blocked output directory")
            notice_click(notice, 300, 89)
            wait(lambda: windows("Could not save recording"), "save failure presented")
            shot(notice, "notice-save-error")
            assert json.loads(metadata.read_text())["saved_path"] is None
            assert media.read_bytes() == original and windows("Could not save recording")
            exports.unlink()
            notice_click(notice, 300, 89)
            wait(lambda: windows("Recording saved"), "saved state presented")
            saved = Path(wait(lambda: json.loads(metadata.read_text()).get("saved_path"), "notice export"))
            assert saved.parent == exports and saved.read_bytes() == original
            assert len(list(exports.iterdir())) == 1
            shot(notice, "notice-saved")
            saved.unlink()
            notice_click(notice, 300, 89)
            wait(lambda: windows("Could not show recording"), "missing export error presented")
            assert not (output / "revealed-path.txt").exists()
            shot(notice, "notice-reveal-error")
            saved.write_bytes(original)
            notice_click(notice, 300, 89)
            wait(lambda: not windows("Could not show recording"), "successful reveal dismissal")
            wait(lambda: (output / "revealed-path.txt").exists(), "OS reveal launcher")
            assert (output / "revealed-path.txt").read_text().strip() == str(exports)
            assert len(history()) == 1 and saved.read_bytes() == original

            for count, action in [(2, "expiry"), (3, "dismiss"), (4, "new-capture")]:
                run("xdotool", "key", "ctrl+shift+F10")
                select_recording(f"notice-{action}-selector")
                hud = running_hud()
                time.sleep(.4)
                notice = stop_with_notice(hud, count)
                assert not windows("Captures"), "background completion must not show root"
                if action == "expiry":
                    time.sleep(13)
                    assert windows("Recording ready"), "notice expired too early"
                    wait(lambda: not windows("Recording ready"), "15.2-second expiry with hidden root")
                elif action == "dismiss":
                    notice_click(notice, 423, 18)
                    wait(lambda: not windows("Recording ready"), "explicit notice dismissal")
                else:
                    run("xdotool", "key", "ctrl+shift+F10")
                    wait(lambda: windows("Captures Capture Controls"), "capture after notice")
                    assert not windows("Recording ready"), "notice leaked into next capture"
                    run("xdotool", "key", "Escape")
                    wait(lambda: not windows("Captures Capture Controls"), "capture cancellation")
                assert len(history()) == count and saved.read_bytes() == original
                assert manifest() is None
            menu_action("Quit Captures")
            assert app.wait(timeout=10) == 0
            (output / "acceptance-ready-notice.json").write_text(json.dumps({
                "appearance": args.appearance, "real_recording_decode": True,
                "nonactivating": True, "hidden_root_actions_and_expiry": True,
                "save_error_retry": True, "byte_identical_export": True,
                "missing_export_reveal_error": True, "reveal_path": str(exports),
                "expiry_preserves_history": True, "dismiss_preserves_history": True,
                "new_capture_clears_notice": True, "clean_exit": True,
                "scope": "Private X11/software GL; OS reveal launcher intercepted; no hardware/AT acceptance",
            }, indent=2))
            print("PASS recording notice: actual finalization, save failure/retry, byte-identical export, "
                  "reveal path/error, hidden-root expiry, dismiss, next capture and clean exit")
            return
        if args.hide_controls_only:
            watcher = dbus.Interface(
                bus.get_object("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher"),
                "org.freedesktop.DBus.Properties")
            wait(lambda: watcher.Get("org.kde.StatusNotifierWatcher",
                                     "RegisteredStatusNotifierItems"),
                 "Captures registered in real SNI tray")
            bundle = next((output / "recording-recovery").glob("*/manifest.json")).parent
            click(hud, 398, 54)
            wait(lambda: not windows("Captures Recording Controls"), "running HUD hidden")
            assert windows("Captures Recording Region") == [guide]
            shot("root", "recording-region-hidden-hud")
            notice = wait(lambda: windows("Recording controls hidden"), "temporary hidden notice")[0]
            shot(notice, "recording-controls-hidden-running")
            before = manifest()
            assert (before["state"] == "recording" and len(before["segments"]) == 1
                    and not before["segments"][0]["complete"])
            run("xdotool", "key", "ctrl+shift+F9", "ctrl+alt+r", "sleep", ".3")
            assert (not windows("Captures Capture Controls")
                    and manifest()["session_id"] == before["session_id"])
            run("xdotool", "key", "ctrl+shift+F10")
            hud = wait(lambda: windows("Captures Recording Controls"),
                       "New Capture shortcut restores HUD")[0]
            assert manifest()["session_id"] == before["session_id"] and not history()

            click(hud, 178, 54)
            wait(lambda: (value := manifest()) and value["state"] == "paused", "pause completed")
            hud = wait(lambda: windows("Captures Recording Controls"), "paused HUD")[0]
            click(hud, 398, 54)
            wait(lambda: not windows("Captures Recording Controls"), "paused HUD hidden")
            menu_action("Show recording controls")
            hud = wait(lambda: windows("Captures Recording Controls"),
                       "real tray action restores paused HUD")[0]
            assert (manifest()["state"] == "paused"
                    and manifest()["session_id"] == before["session_id"])
            assert windows("Captures Recording Region") == [guide]
            shot(hud, "recording-controls-restored-paused")

            click(hud, 398, 54)
            wait(lambda: not windows("Captures Recording Controls"), "HUD hidden before tray loss")
            run("xfce4-panel", "--quit")
            panel.wait(timeout=10)
            hud = wait(lambda: windows("Captures Recording Controls"),
                       "tray-host loss restores controls")[0]
            root = wait(lambda: windows("Captures"), "tray-host loss restores workspace")[0]
            shot("root", "recording-controls-tray-loss-restored")
            assert (manifest()["state"] == "paused"
                    and manifest()["session_id"] == before["session_id"])

            click(hud, 142, 54)
            metadata = wait(lambda: list((output / "history").glob("*/metadata.json")),
                            "hidden/restored recording publication")
            assert len(metadata) == 1
            media = metadata[0].parent / "media.mp4"
            run("ffmpeg", "-v", "error", "-i", str(media), "-f", "null", "-")
            pixels = run("ffmpeg", "-v", "error", "-i", str(media), "-frames:v", "1",
                         "-f", "rawvideo", "-pix_fmt", "rgb24", "-")
            # OpenH264 quantizes edge blocks: a separate encode of a uniform
            # RGB(192,32,64) buffer has up to 18 levels of corner error and
            # matches this recording byte-for-byte after decoding. The raw
            # composited edge probes above stay exact; this codec tolerance
            # still rejects either the dim veil or accent border in the MP4.
            for x, y in [(0, 0), (309, 0), (0, 169), (309, 169), (155, 85)]:
                offset = (y * 310 + x) * 3
                actual = pixels[offset:offset + 3]
                assert len(actual) == 3 and all(abs(a - e) <= 20 for a, e in
                    zip(actual, (192, 32, 64))), (x, y, actual)
            finished(1)
            assert manifest() is None and not bundle.exists()
            acceptance = {
                "running_hide": True, "paused_hide": True,
                "new_capture_shortcut_restore": True, "real_sni_restore": True,
                "tray_host_loss_restore": True, "same_session": True,
                "busy_shortcuts_suppressed": True, "history_publication": True,
                "decoded_output": True, "source_cleanup": True,
                "region_guide_preserved": True, "region_guide_click_through": True,
                "region_guide_clean_inner_edges": True, "region_guide_cleanup": True,
            }
            (output / "acceptance-hide-controls.json").write_text(
                json.dumps(acceptance, indent=2))
            print("PASS native recording Hide: running/paused preservation, New Capture and real "
                  "SNI restore, tray-host-loss recovery, finalized decode and cleanup")
            return
        run("xdotool", "key", "ctrl+alt+r", "ctrl+shift+F9")
        assert not windows("Captures Capture Controls")
        # Escape only cancels before engine handoff, not an accepted recording.
        run("xdotool", "key", "Escape")
        time.sleep(.3)
        assert manifest()["state"] == "recording"
        assert windows("Captures Recording Controls")
        if args.virtual_microphone:
            click(hud, 318, 54)
            wait(lambda: (value := manifest()) and value["state"] == "recording"
                 and value["options"]["audio"]["microphone_muted"]
                 and len(value["segments"]) == 2, "running microphone mute segment")
            wait(lambda: meter_pixels(hud) == 0, "muted meter clears the previous sample")
            shot(hud, "hud-muted")
            click(hud, 318, 54)
            wait(lambda: (value := manifest()) and value["state"] == "recording"
                 and not value["options"]["audio"]["microphone_muted"]
                 and len(value["segments"]) == 3, "running microphone unmute segment")
            wait(lambda: meter_pixels(hud) >= 20, "unmuted meter resumes live samples")
            shot(hud, "hud-unmuted")
        click(hud, 178, 54)
        wait(lambda: (value := manifest()) and value["state"] == "paused", "pause completed")
        time.sleep(.3)
        hud = wait(lambda: windows("Captures Recording Controls"), "paused HUD")[0]
        if args.virtual_microphone:
            wait(lambda: meter_pixels(hud) == 0, "paused meter clears the previous sample")
        shot(hud, "hud-paused")

        # Paused Restart replaces the accepted take and rearms Escape for its
        # stored countdown. Cancelling that countdown discards the replacement.
        restart(hud, "restart-confirmation-paused")
        countdown = wait(lambda: windows("Captures Recording Countdown"),
                         "paused restart countdown")[0]
        wait(lambda: (value := manifest()) and value["state"] == "countdown"
             and not value["segments"], "paused restart reset")
        shot(countdown, "recording-restart-countdown")
        run("xdotool", "key", "Escape")
        wait(lambda: not windows("Captures Recording Countdown"), "restart countdown cancellation")
        finished(0)
        assert not history()
        time.sleep(.5)

        # Running Restart also drops the old segment. The final MP4 must contain
        # only blue replacement pixels, not the red media recorded before Restart.
        run("xdotool", "key", "ctrl+shift+F10")
        select_recording("running-restart-selector")
        hud = running_hud()
        restart(hud, "restart-confirmation-running")
        wait(lambda: (value := manifest()) and value["state"] == "countdown"
             and not value["segments"], "running restart reset")
        run("hsetroot", "-solid", "#2070c0")
        hud = running_hud()
        time.sleep(.5)
        if args.virtual_microphone:
            click(hud, 318, 54)
            wait(lambda: (value := manifest()) and value["state"] == "recording"
                 and value["options"]["audio"]["microphone_muted"]
                 and len(value["segments"]) == 2, "published recording mute segment")
            time.sleep(.6)
            click(hud, 318, 54)
            wait(lambda: (value := manifest()) and value["state"] == "recording"
                 and not value["options"]["audio"]["microphone_muted"]
                 and len(value["segments"]) == 3, "published recording unmute segment")
            segments = manifest()["segments"]
            assert segments[0]["microphone_relative_path"]
            assert segments[1]["microphone_relative_path"] is None
            assert segments[2]["microphone_relative_path"]
            mute_start = segments[0]["duration_ms"] / 1000
            mute_end = mute_start + segments[1]["duration_ms"] / 1000
            print(f"Microphone mute interval: {mute_start:.3f}–{mute_end:.3f}s")
            bundle = next((output / "recording-recovery").glob("*/manifest.json")).parent
            microphone = bundle / segments[2]["microphone_relative_path"]
            # Stream startup is not sample delivery. Require actual buffered PCM
            # before Stop; a broken unmute must time out rather than pass on metadata.
            wait(lambda: microphone.stat().st_size > 192000, "unmuted microphone sample delivery")
        click(hud, 142, 54)
        metadata = wait(lambda: list((output / "history").glob("*/metadata.json")), "History publication")
        assert len(metadata) == 1
        entry = json.loads(metadata[0].read_text())
        assert entry["kind"] == "video" and entry["mime_type"] == "video/mp4", entry
        assert (entry["width"], entry["height"]) == (310, 170), entry
        assert entry["target"]["rect"] == {"x": 140, "y": 180, "width": 310, "height": 170}
        assert entry["saved_path"] is None
        assert entry["has_microphone_audio"] == args.virtual_microphone, entry
        media = metadata[0].parent / "media.mp4"
        frames = run("ffmpeg", "-v", "error", "-i", str(media), "-vf", "crop=2:2:40:40",
                     "-f", "rawvideo", "-pix_fmt", "rgb24", "-")
        assert len(frames) >= 24
        for actual, expected in ((frames[:3], (32, 112, 192)), (frames[-3:], (32, 112, 192))):
            assert all(abs(a - e) <= 6 for a, e in zip(actual, expected)), (actual, expected)
        microphone_rms = None
        if args.virtual_microphone:
            pcm = run("ffmpeg", "-v", "error", "-i", str(media), "-map", "0:a:0",
                      "-ac", "1", "-ar", "24000", "-f", "f32le", "-")
            samples = [sample[0] for sample in struct.iter_unpack("<f", pcm)]
            def rms(begin, end):
                window = samples[int(begin * 24000):int(end * 24000)]
                assert len(window) >= 2400, "need at least 100 ms away from AAC boundaries"
                return math.sqrt(sum(value * value for value in window) / len(window))
            # Inspect the interior of the first take, not its padded encoder-drain tail.
            microphone_rms = [rms(.2, mute_start / 2),
                              rms(mute_start + .2, mute_end - .2),
                              rms(mute_end + .2, len(samples) / 24000 - .1)]
            assert microphone_rms[0] > .01 and microphone_rms[2] > .01, microphone_rms
            assert microphone_rms[1] < .001, microphone_rms
            print(f"PASS microphone waveform RMS: audible/muted/audible {microphone_rms}")
        assert (metadata[0].parent / "preview.png").is_file()
        finished(1)
        published = history()
        if args.restart_only:
            (output / "acceptance-restart.json").write_text(json.dumps({
                "region": entry["target"]["rect"], "duration_ms": entry["duration_ms"],
                "first_pixel": list(frames[:3]), "last_pixel": list(frames[-3:]),
                "paused_restart": True, "running_restart": True,
                "virtual_microphone_mute": args.virtual_microphone,
                "microphone_meter_pixels": {"audible": high, "quieter": low, "silent": 0}
                    if args.virtual_microphone else None,
                "microphone_rms": microphone_rms,
                "restart_countdown_escape_discarded": True,
                "replacement_only_media": True, "source_cleanup": True,
            }, indent=2))
            print("PASS native recording Restart: paused/running replacement, countdown Escape, "
                  "replacement-only decoded pixels and source cleanup")
            return

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
        run("xdotool", "key", "ctrl+alt+d")
        selector = wait(lambda: windows("Captures Capture Controls"), "Record Display shortcut")[0]
        time.sleep(.5)
        shot(selector, "record-display-shortcut")
        assert manifest() is None, "the Display shortcut must not start recording"
        run("xdotool", "key", "Return")
        hud = running_hud()
        assert manifest()["options"]["target"]["type"] == "display"
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

        # A screenshot key must leave Record mode, not merely change its target.
        published = history()
        run("xdotool", "key", "ctrl+alt+r")
        selector = wait(lambda: windows("Captures Capture Controls"), "Record Region shortcut")[0]
        time.sleep(.5)
        # On this fixed 1280x900 fixture only Record's taller toolbar covers
        # (250,770); (100,770) is the same solid desktop outside either toolbar.
        # Compare pixels before moving the pointer: stale child painting used
        # to persist until a mouse event even though the mode had changed.
        probe = "%[pixel:p{250,770}]|%[pixel:p{100,770}]"
        before = run("import", "-window", selector, "-format", probe, "info:").split(b"|")
        assert before[0] != before[1], "Record shortcut did not present recording controls"
        run("xdotool", "key", "ctrl+shift+F7")
        assert windows("Captures Capture Controls") == [selector]

        def screenshot_toolbar_painted():
            after = run("import", "-window", selector, "-format", probe, "info:").split(b"|")
            return after[0] == after[1]

        # Observe asynchronous shortcut delivery and deferred-child painting,
        # without a second key or pointer event that could hide a missed repaint.
        wait(screenshot_toolbar_painted, "screenshot shortcut repaints child without pointer input")
        shot(selector, "record-to-screenshot-shortcut")
        run("xdotool", "mousemove", "--sync", "--window", selector, "140", "180",
            "sleep", ".1", "mousedown", "1", "sleep", ".2",
            "mousemove", "--sync", "--window", selector, "450", "350",
            "sleep", ".2", "mouseup", "1", "key", "Return")
        finished(4)
        screenshot = history() - published
        assert len(screenshot) == 1
        image = next(iter(screenshot)).parent / "capture.png"
        assert run("identify", "-format", "%wx%h", str(image)) == b"310x170"

        assert saver.queries > 0 and app.poll() is None
        # This fixture has no tray host: closing the root requests application
        # exit. Shutdown must drain accepted media, not merely stop the HUD.
        published = history()
        run("xdotool", "key", "ctrl+shift+F10")
        select_recording("quit-selector")
        running_hud()
        # Address the root directly: mapping it then Alt+F4 can still close the
        # focused HUD instead. xdotool windowclose destroys the drawable rather
        # than delivering the normal WM protocol request needed for media drain.
        connection = display.Display(env["DISPLAY"])
        try:
            window = connection.create_resource_object("window", int(root))
            window.send_event(protocol.event.ClientMessage(
                window=window, client_type=connection.intern_atom("WM_PROTOCOLS"),
                data=(32, [connection.intern_atom("WM_DELETE_WINDOW"), X.CurrentTime, 0, 0, 0])))
            connection.sync()
        finally:
            connection.close()
        try:
            assert app.wait(timeout=10) == 0, "unclean recording shutdown"
        except subprocess.TimeoutExpired:
            shot("root", "timeout-recording-exit")
            raise
        assert len(history()) == 5 and manifest() is None
        saved_on_quit = history() - published
        assert len(saved_on_quit) == 1
        run("ffmpeg", "-v", "error", "-i", str(next(iter(saved_on_quit)).parent / "media.mp4"),
            "-f", "null", "-")
        assert not windows("Captures Recording Controls")
        (output / "acceptance.json").write_text(json.dumps({
            "region": entry["target"]["rect"], "duration_ms": entry["duration_ms"],
            "first_pixel": list(frames[:3]), "last_pixel": list(frames[-3:]),
            "pause_resume": True, "history_publication": True,
            "running_escape_ignored": True, "countdown_escape_discarded": True,
            "paused_restart": True, "running_restart": True,
            "restart_countdown_escape_discarded": True,
            "explicit_discard": True, "session_lock_preserved": True, "child_close_saved": True,
            "application_quit_saved": True,
            "recording_shortcuts": True, "shortcut_mode_switch": True,
        }, indent=2))
        print("PASS native recording: recording shortcuts/mode switching, selector/countdown, pause/resume/restart, replacement-only MP4 pixels, History, "
              "Escape scope, explicit discard, lock/child-close/application-quit preservation and source cleanup")
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
