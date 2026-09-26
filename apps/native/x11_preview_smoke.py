#!/usr/bin/python3
"""Real native mini previews on private X11, with disposable history/settings.

Uses the same private D-Bus session-state fixture as x11_capture_smoke. This is
software-GL input/pixel evidence, not physical-desktop or accessibility acceptance.
"""
import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import threading
import time
from urllib.parse import quote, unquote
import xml.etree.ElementTree as ET

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

from x11_capture_smoke import BACKGROUNDS, ScreenSaver


PREVIEW = "Captures Mini Preview"
SELECTOR = "Captures Region Selection"
CONTROLS = "Captures Capture Controls"
# xdotool searches legacy WM_NAME, whose em dash is not decoded as UTF-8.
EDITOR = "Screenshot editor.*"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--stack", action="store_true", help="Also exercise retained multi-card previews")
    parser.add_argument("--drag-only", action="store_true", help="Exercise real outbound XDND transfer and cancellation")
    parser.add_argument("--reduced-motion", action="store_true", help="Disable native preview motion")
    parser.add_argument("--system-motion-only", action="store_true", help="Exercise desktop motion preference refresh")
    parser.add_argument("--lifecycle", action="store_true", help="Exercise a real Xfce SNI tray and background shortcuts")
    parser.add_argument("--shortcut-editing", action="store_true", help="Also exercise live Preferences recording and persistence")
    parser.add_argument("--login-item-only", action="store_true", help="Exercise explicit autostart and hidden launch recovery")
    args = parser.parse_args()
    if args.shortcut_editing and not args.lifecycle:
        parser.error("--shortcut-editing requires --lifecycle for real global registrations")
    if args.login_item_only and not args.lifecycle:
        parser.error("--login-item-only requires --lifecycle for a real tray and disposable configuration")
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = {**os.environ, "WGPU_BACKEND": "gl", "WINIT_X11_SCALE_FACTOR": "1", "XDG_SESSION_TYPE": "x11"}
    env.pop("WAYLAND_DISPLAY", None)
    # Every process in this private desktop must use the disposable trash on
    # the same filesystem as exports. Never allow a smoke run to reach the
    # invoking user's real XDG trash.
    data_home = output / "data"
    data_home.mkdir()
    env["XDG_DATA_HOME"] = str(data_home)
    # Observe OS command delivery without launching the orb's file manager.
    # Physical file-manager selection/focus remains a separate acceptance gate.
    reveal_log = output / "revealed-folders.jsonl"
    reveal_tools = output / "reveal-tools"
    reveal_tools.mkdir()
    opener = reveal_tools / "xdg-open"
    opener.write_text("#!/usr/bin/python3\nimport json, sys\n"
        f"with open({str(reveal_log)!r}, 'a') as stream:\n"
        "    stream.write(json.dumps(sys.argv[1:]) + '\\n')\n")
    opener.chmod(0o755)
    env["PATH"] = f"{reveal_tools}:{env['PATH']}"
    if args.lifecycle:
        # Set before D-Bus starts: activated xfconfd must inherit the same
        # disposable configuration as the panel, not the orb user's home.
        env["XDG_CONFIG_HOME"] = str(output / "config")
        env["XDG_CACHE_HOME"] = str(output / "cache")
    children, logs = [], []
    loop = None

    def spawn(name, command, announce=False):
        stdout = subprocess.PIPE if announce else (output / f"{name}.stdout.log").open("w")
        stderr = (output / f"{name}.stderr.log").open("w")
        logs.append(stderr)
        if not announce:
            logs.append(stdout)
        process = subprocess.Popen(command, env=env, stdout=stdout, stderr=stderr)
        children.append(process)
        if announce:
            # Cold CI X servers can exceed ten seconds before announcing readiness.
            timeout = 60 if name == "xvfb" else 10
            assert select.select([process.stdout], [], [], timeout)[0], (
                f"{name} did not announce readiness within {timeout}s; inspect its stderr")
            endpoint = process.stdout.readline().decode().strip()
            assert endpoint, f"{name} failed; inspect stderr"
            return endpoint
        return process

    def run(*command):
        return subprocess.check_output(command, env=env, timeout=10)

    def windows(title):
        result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", f"^{title}$"],
                                env=env, capture_output=True, text=True, timeout=5)
        assert result.returncode in (0, 1), result.stderr
        return result.stdout.split()

    def shot(window, name):
        path = output / f"{name}.png"
        run("import", "-window", window, str(path))
        return path

    def window_geometry(window):
        return dict(line.split("=", 1) for line in
                    run("xdotool", "getwindowgeometry", "--shell", window).decode().splitlines())

    def active_window():
        # Openbox can briefly clear _NET_ACTIVE_WINDOW while transferring focus.
        result = subprocess.run(["xdotool", "getactivewindow"], env=env,
                                capture_output=True, text=True, timeout=5)
        assert result.returncode in (0, 1), result.stderr
        return result.stdout.strip() if result.returncode == 0 else None

    def wait(predicate, description):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if value := predicate():
                return value
            time.sleep(.05)
        shot("root", "timeout-desktop")
        raise AssertionError(f"Timed out: {description}")

    def click(window, x, y, activate=True, button=1):
        # Cross a real intermediate point to avoid winit's stale-position filter
        # after an X11 remap. Preview clicks deliberately do not activate windows.
        if activate:
            run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window)
        run("xdotool", "mousemove", "--sync", "--window", window, str(x - 1), str(y),
            "mousemove_relative", "--sync", "1", "0", "sleep", ".15", "mousedown", str(button),
            "sleep", ".15", "mouseup", str(button), "sleep", ".15")
        # XSync acknowledges X11 input, not egui's next frame. Let release open
        # the clicked recorder/navigation target before sending its next key.

    def select_region(selector, rect):
        # Direct region overlays have no toolbar: they paint centered guidance,
        # and releasing the drag commits the capture like the shipping app.
        wait(lambda: int(run("import", "-window", selector, "-crop", "640x120+320+390",
            "-format", "%k", "info:")) > 16, "painted region guidance before drag")
        x, y, width, height = rect
        run("xdotool", "windowfocus", "--sync", selector, "sleep", ".15",
            "mousemove", "--sync", "--window", selector, str(x - 1), str(y),
            "mousemove_relative", "--sync", "1", "0", "sleep", ".15")
        wait(lambda: run("xdotool", "getwindowfocus", "-f").decode().strip() == selector,
             "region selector has X input focus")

        def selection_pixel():
            return run("import", "-window", selector, "-crop",
                       f"1x1+{x + width // 2}+{y + height // 2}", "-depth", "8", "rgb:-")

        veiled_pixel = selection_pixel()
        run("xdotool", "mousedown", "1", "sleep", ".15", "mousemove", "--sync", "--window", selector,
            str(x + width), str(y + height))
        # Observe the held drag before releasing: XSync does not mean egui has
        # consumed it. The selection unveils this interior pixel; release then
        # commits. Final saved pixels still verify geometry.
        wait(lambda: selection_pixel() != veiled_pixel, "painted region selection while dragging")
        run("xdotool", "mouseup", "1", "sleep", ".15")

    def rgb(path):
        return run("convert", str(path), "-depth", "8", "rgb:-")

    def clipboard_pixels():
        value = subprocess.run(["xclip", "-selection", "clipboard", "-t", "image/png", "-o"],
                               env=env, capture_output=True, timeout=5)
        # The reset xclip owner can answer an image/png request with its plain
        # text before the asynchronous Copy takes ownership. Keep polling;
        # the caller still requires the exact full-resolution image pixels.
        if value.returncode != 0 or not value.stdout.startswith(b"\x89PNG\r\n\x1a\n"):
            return None
        return subprocess.check_output(["convert", "png:-", "-depth", "8", "rgb:-"], input=value.stdout)

    def second_action_y(entry):
        # Shipping chrome hides Copy while the clipboard still holds this
        # capture, centering Save file / Show in Folder on the card.
        return 108 if clipboard_pixels() == rgb(entry.parent / "capture.png") else 127

    def wallpaper_crop(x, y, width, height):
        # Independent pixel oracle: asymmetric desktop split at (300,270).
        colors = [bytes(color) for color in BACKGROUNDS[0]]
        return b"".join(colors[(2 if row >= 270 else 0) + (column >= 300)]
                        for row in range(y, y + height) for column in range(x, x + width))

    try:
        env["DISPLAY"] = ":" + spawn("xvfb", ["Xvfb", "-displayfd", "1", "-screen", "0",
            "1280x900x24", "-dpi", "96", "-nolisten", "tcp"], True)
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
        spawn("picom", ["picom", "--config", "/dev/null", "--backend", "xrender"])
        time.sleep(1)
        background = output / "wallpaper.ppm"
        background.write_bytes(b"P6\n1280 900\n255\n" + wallpaper_crop(0, 0, 1280, 900))
        run("hsetroot", "-fill", str(background))

        if args.system_motion_only:
            class PortalSettings(dbus.service.Object):
                value = 1
                calls = 0

                @dbus.service.method("org.freedesktop.portal.Settings", in_signature="as", out_signature="a{sa{sv}}")
                def ReadAll(self, namespaces):
                    assert list(namespaces) == ["org.freedesktop.appearance"]
                    self.calls += 1
                    return {"org.freedesktop.appearance": {} if self.value is None else {
                        "reduced-motion": dbus.UInt32(self.value, variant_level=1)}}

            portal_name = dbus.service.BusName("org.freedesktop.portal.Desktop", bus=bus, do_not_queue=True)
            portal = PortalSettings(portal_name, "/org/freedesktop/portal/desktop")
            settings = output / "motion-settings.json"
            settings.write_text(json.dumps({"settings_schema_version": 5, "onboarding_completed": True,
                                            "appearance": "dark", "theme": "mustard"}))
            original = settings.read_bytes()
            app = spawn("motion", [str(binary), "--live", "--settings-file", str(settings),
                                   "--history-root", str(output / "history"), "--quit-after", "60"])
            root = wait(lambda: windows("Captures"), "motion workspace")[0]

            def motion_events():
                events = []
                for line in (output / "motion.stdout.log").read_text().splitlines():
                    if line.startswith("{") and line.endswith("}"):
                        value = json.loads(line)
                        if value.get("event") == "motion-preference":
                            events.append(value["detail"])
                return events

            wait(lambda: motion_events() and motion_events()[-1] == {"available": True, "reduced": True},
                 "startup reads reduced motion")
            focus = spawn("motion-focus", ["xmessage", "-title", "Motion focus", "Change desktop preference"])
            other = wait(lambda: windows("Motion focus"), "external focus window")[0]
            for reported, expected in [(0, False), (1, True), (None, True), (2, False)]:
                run("xdotool", "windowactivate", "--sync", other, "windowfocus", "--sync", other, "sleep", ".3")
                portal.value = reported
                count = len(motion_events())
                run("xdotool", "windowactivate", "--sync", root, "windowfocus", "--sync", root)
                wait(lambda: len(motion_events()) > count and motion_events()[-1] == {
                    "available": reported is not None, "reduced": expected}, "foreground motion refresh")
                calls = portal.calls
                time.sleep(.5)
                assert portal.calls == calls, "idle motion preference must not poll"
                assert settings.read_bytes() == original, "desktop preference read saved app settings"
            shot(root, "motion-foreground")
            run("xdotool", "windowactivate", "--sync", root, "key", "alt+F4")
            wait(lambda: app.poll() is not None, "motion workspace closes")
            assert app.returncode == 0
            focus.terminate()
            calls = portal.calls
            fixture = spawn("motion-fixture", [str(binary), "--scene", "idle", "--quit-after", "1"])
            wait(lambda: fixture.poll() is not None, "isolated motion fixture closes")
            assert fixture.returncode == 0
            assert portal.calls == calls, "render fixtures must not read desktop motion preferences"
            (output / "result.json").write_text(json.dumps({"passed": True, "checks": [
                "startup", "foreground-toggle", "unavailable-retains", "unknown-no-preference",
                "no-idle-query", "settings-unchanged", "fixture-isolation"]}, indent=2) + "\n")
            print("PASS native system motion: startup, foreground, unavailable, unknown, no polling or writes")
            return

        if args.lifecycle:
            # A real SNI host, not a fake watcher or an XEmbed-only tray. Keep
            # all Xfce configuration and its D-Bus activation private to this run.
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
   <property name="single-row" type="bool" value="true"/>
   <property name="square-icons" type="bool" value="true"/>
  </property>
 </property>
</channel>''')
            panel = spawn("sni-panel", ["xfce4-panel", "--disable-wm-check", "--sm-client-disable"])
            wait(lambda: bus.name_has_owner("org.kde.StatusNotifierWatcher"), "real SNI watcher")

        if args.login_item_only:
            autostart = output / "config/autostart"
            for appearance in ("dark", "light"):
                history = output / f"login {appearance} % profile"
                settings = output / f"login {appearance} % settings.json"
                settings.write_text(json.dumps({"settings_schema_version": 5,
                    "onboarding_completed": True,
                    "appearance": appearance, "theme": "mustard", "launch_at_login": True,
                    "output_directory": str(output / "exports"),
                    "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
                    "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10"}))
                common = [str(binary), "--live", "--history-root", str(history),
                          "--settings-file", str(settings)]
                app = spawn(f"login-{appearance}", common)
                root = wait(lambda: windows("Captures"), "login Preferences workspace")[0]
                time.sleep(1)
                click(root, 196, 18)
                click(root, 75, 321)
                time.sleep(1)
                shot(root, f"login-{appearance}-off")
                assert not list(autostart.glob("*.desktop")), "saved setting must not register a login item"
                click(root, 857, 607)
                entry = wait(lambda: next(autostart.glob("*.desktop"), None), "explicit login registration")
                owned = entry.read_bytes()
                shot(root, f"login-{appearance}-on")
                # Exit normally, not close-to-background, then launch the real
                # Desktop Entry through GIO to test its escaping and exact argv.
                run("xdotool", "key", "ctrl+q")
                assert app.wait(timeout=10) == 0
                other_app = spawn(f"login-focus-{appearance}", ["xmessage", "-title", "Login focus fixture",
                    "-geometry", "220x70+10+10", "Keep this application focused"])
                other = wait(lambda: windows("Login focus fixture"), "login focus fixture")[0]
                run("xdotool", "windowactivate", "--sync", other, "windowfocus", "--sync", other)
                # GIO returns immediately; its launcher PID is not the app PID.
                # Use files rather than a stdout pipe inherited by the app.
                launcher = spawn(f"gio-login-{appearance}", ["gio", "launch", str(entry)])
                assert launcher.wait(timeout=10) == 0
                hidden = wait(lambda: subprocess.run(["xdotool", "search", "--name", "^Captures$"],
                    env=env, capture_output=True, text=True).stdout.split(), "hidden login window")[0]
                pid = int(run("xdotool", "getwindowpid", hidden))
                try:
                    time.sleep(1)
                    assert not windows("Captures"), "login launch showed the root"
                    assert run("xdotool", "getwindowfocus").decode().strip() == other, "login launch stole focus"
                    assert str(history).encode() in Path(f"/proc/{pid}/cmdline").read_bytes()
                    assert str(settings).encode() in Path(f"/proc/{pid}/cmdline").read_bytes()
                    result = subprocess.run(common, env=env, capture_output=True, timeout=10)
                    assert result.returncode == 0, result.stderr
                    root = wait(lambda: windows("Captures"), "relaunch restores live Preferences")[0]
                    click(root, 75, 321)
                    time.sleep(1)
                    shot(root, f"login-{appearance}-restored")
                    click(root, 857, 607)
                    wait(lambda: not entry.exists(), "explicit disable after hidden launch")
                    run("xdotool", "key", "ctrl+q")
                    wait(lambda: not Path(f"/proc/{pid}").exists(), "normal exit after login launch")
                finally:
                    # Only the PID obtained from our private X server/profile.
                    if Path(f"/proc/{pid}").exists():
                        os.kill(pid, 15)
                other_app.terminate()
                other_app.wait(timeout=5)
                assert owned.startswith(b"[Desktop Entry]\n")
                # A changed/moved binary or foreign registration must stay
                # untouched and surface a recoverable error, not a guessed Off.
                conflict = b"[Desktop Entry]\nType=Application\nName=Another login item\n"
                entry.write_bytes(conflict)
                conflict_app = spawn(f"login-conflict-{appearance}", common)
                root = wait(lambda: windows("Captures"), "conflicting login entry Preferences")[0]
                time.sleep(1)
                click(root, 196, 18)
                click(root, 75, 321)
                time.sleep(1)
                shot(root, f"login-{appearance}-conflict")
                assert entry.read_bytes() == conflict
                run("xdotool", "key", "ctrl+q")
                assert conflict_app.wait(timeout=10) == 0
                assert entry.read_bytes() == conflict
                entry.unlink()  # Remove only our deliberately foreign fixture.

            panel.terminate()
            panel.wait(timeout=5)
            wait(lambda: not bus.name_has_owner("org.kde.StatusNotifierWatcher"), "tray host removed")
            recovery = spawn("login-without-tray", common + ["--scene", "idle"])
            root = wait(lambda: windows("Captures"), "missing tray exposes login recovery window")[0]
            time.sleep(1)
            click(root, 196, 18)
            click(root, 75, 321)
            time.sleep(1)
            shot(root, "login-without-tray")
            run("xdotool", "key", "ctrl+q")
            assert recovery.wait(timeout=10) == 0
            (output / "result.json").write_text(json.dumps({"passed": True,
                "checks": ["saved setting does not register", "explicit enable/disable",
                    "GIO argv preserves profile spaces and percent", "hidden nonactivating startup",
                    "single-instance relaunch restores usable Preferences", "light/dark presentation",
                    "conflicting entry preserved", "missing tray exposes recovery workspace"],
                "scope": "Private X11/software GL; not physical sign-in or Wayland acceptance."}, indent=2))
            print("PASS native login items: explicit registration, hidden startup and relaunch")
            return

        cases = [(placement, True, False) for placement in ("bottom_left", "bottom_right", "top_left", "top_right")]
        cases += [("bottom_left", True, True), ("bottom_left", False, False)]
        if args.lifecycle or args.drag_only:
            cases = cases[:1]
        for placement, enabled, include in cases:
            prefix = f"{placement}-enabled-{enabled}-include-{include}"
            history = output / prefix / "history"
            settings = output / f"{prefix}-settings.json"
            settings.write_text(json.dumps({
                "onboarding_completed": True,
                "settings_schema_version": 5, "appearance": "dark", "theme": "mustard",
                "output_directory": str(output / prefix / "exports Café"),
                "new_capture_shortcut": "Ctrl+Shift+F10",
                "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
                "display_shortcut": "Ctrl+Shift+F9", "launch_at_login": False,
                "auto_copy_to_clipboard": False, "auto_start_on_selection": False,
                "freeze_screen": True, "show_cursor_in_screenshots": False,
                "screenshot_countdown_seconds": 1 if args.lifecycle else 0,
                "show_mini_previews": enabled,
                "include_mini_previews_in_captures": include, "mini_preview_placement": placement,
            }))
            app = spawn(prefix, [str(binary), "--live", "--history-root", str(history),
                "--settings-file", str(settings), "--quit-after", "300" if args.lifecycle else "180"]
                + (["--reduced-motion"] if args.reduced_motion else []))
            root = wait(lambda: windows("Captures"), "root workspace")[0]
            # Leave the left-hand preview/capture area unobstructed. Both root
            # capture buttons still fit on this desktop after moving the window.
            def move_root(x, y):
                # windowmove --sync returns on any location change. After a
                # remap Openbox may still be placing the workspace, so that
                # change can be its intermediate placement and our move lands
                # later, under a pointer already positioned for the old origin.
                # Wait for the client origin our frame position implies.
                run("xdotool", "windowmove", "--sync", root, str(x), str(y))

                def placed():
                    extents = run("xprop", "-id", root, "_NET_FRAME_EXTENTS").decode()
                    left, _, top, _ = map(int, extents.split("=", 1)[1].split(","))
                    info = run("xwininfo", "-id", root).decode()
                    origin = tuple(int(line.split(":", 1)[1]) for line in info.splitlines()
                                   if "Absolute upper-left" in line)
                    return origin == (x + left, y + top)

                wait(placed, f"root workspace moved to {x},{y}")

            move_root(620, 20)
            time.sleep(1)

            def entries():
                # History publishes by renaming hidden staging directories.
                return {p for p in history.glob("*/metadata.json") if not p.parent.name.startswith(".")}

            def begin():
                # Region is root-local x=575. Keep its desktop x=875 between
                # the always-on-top preview windows at x=0..340 and 940..1280,
                # including their transparent margins and expanded stacks.
                run("xdotool", "windowactivate", "--sync", root, "windowfocus", "--sync", root)
                wait(lambda: windows("Captures"), "workspace restored before positioning")
                move_root(300, 280)
                click(root, 575, 126)
                selector = wait(lambda: windows(SELECTOR), "region selector")[0]
                wait(lambda: int(run("import", "-window", selector, "-crop", "640x120+320+390",
                    "-format", "%k", "info:")) > 16, "painted region guidance")
                return selector

            def capture(rect):
                previous = entries()
                selector = begin()
                select_region(selector, rect)
                entry = wait(lambda: entries() - previous, "new persisted capture").pop()
                wait(lambda: windows("Captures") and not windows(SELECTOR), "restored root")
                # Openbox may reposition an off-screen workspace when remapping.
                # Keep it out of the pixel oracle crop again before comparing a
                # composited card with the following screenshot (which hides it).
                move_root(620, 20)
                return entry

            first = capture((140, 180, 310, 170))
            stack_entries = [first]
            assert rgb(first.parent / "capture.png") == wallpaper_crop(140, 180, 310, 170)
            if enabled:
                preview = wait(lambda: windows(PREVIEW), "mini preview")[0]
                wait(lambda: int(run("import", "-window", preview, "-format", "%k", "info:")) > 16,
                     "painted preview")
                geometry = window_geometry(preview)
                shot("root", f"{prefix}-placed")
                focus = run("xdotool", "getwindowfocus").decode().strip()
                assert focus != preview, "preview stole focus"
                expected_x = 940 if placement.endswith("right") else 0
                expected_y = 12 if placement.startswith("top") else 600
                assert (int(geometry["X"]), int(geometry["Y"])) == (expected_x, expected_y), geometry
                assert (int(geometry["WIDTH"]), int(geometry["HEIGHT"])) == (340, 240), geometry
                # Check the actual idle/hover presentation, not only clicks on
                # controls. Sample empty media beside the centered action group.
                run("xdotool", "mousemove", "--sync", "640", "440")
                time.sleep(.25)
                card_top = 52 if placement.startswith("top") else 28
                crop = f"1x1+80+{card_top + 72}"
                idle_pixel = run("import", "-window", preview, "-crop", crop, "-depth", "8", "rgb:-")
                shot(preview, f"{prefix}-chrome-idle")
                run("xdotool", "mousemove", "--sync", "--window", preview, "60", str(card_top + 72))
                time.sleep(.25)
                hover_pixel = run("import", "-window", preview, "-crop", crop, "-depth", "8", "rgb:-")
                assert all(abs(b - a * .5) <= 2 for a, b in zip(idle_pixel, hover_pixel)), (
                    "hover must dim only the media", list(idle_pixel), list(hover_pixel))
                shot(preview, f"{prefix}-chrome-hover")
                run("xdotool", "mousemove", "--sync", "640", "440")
                time.sleep(.25)
                assert run("import", "-window", preview, "-crop", crop, "-depth", "8", "rgb:-") == idle_pixel
                if args.drag_only:
                    # Reject our own source without importing it into an editor.
                    # Record the real 420ms shake and settled pointer recovery.
                    recording = subprocess.Popen(["ffmpeg", "-y", "-loglevel", "error", "-f", "x11grab",
                        "-video_size", "340x240", "-framerate", "30", "-i", env["DISPLAY"] + "+0,600",
                        "-t", "4", "-c:v", "libx264", "-pix_fmt", "yuv420p", str(output / "self-drop.mp4")],
                        env=env)
                    children.append(recording)
                    time.sleep(.5)
                    run("xdotool", "mousemove", "--sync", "--window", preview, "60", "90", "mousedown", "1",
                        "sleep", ".2", "mousemove", "--sync", "--window", preview, "90", "90", "sleep", ".5",
                        "mousemove", "--sync", "--window", preview, "200", "90", "sleep", ".3", "mouseup", "1")
                    time.sleep(.7)
                    assert windows(PREVIEW) and not windows(EDITOR), "self drop must retain, not reimport"
                    assert recording.wait(timeout=10) == 0
                    shot(preview, "self-drop-settled")
                    for mode in ("cancel", "reject", "nofinish", "disappear", "accept"):
                        if mode == "accept":
                            click(preview, 170, second_action_y(first), activate=False)
                            saved_path = Path(wait(lambda: json.loads(first.read_text()).get("saved_path"), "saved Unicode export"))
                        destination = output / f"received-{mode}.bin"
                        spawn(f"receiver-{mode}", ["/usr/bin/python3", str(Path(__file__).with_name("x11_drag_receiver.py")),
                              "--output", str(destination), "--mode", "accept" if mode == "cancel" else mode], True)
                        run("xdotool", "mousemove", "--sync", "--window", preview, "60", "90", "mousedown", "1",
                            "sleep", ".2", "mousemove", "--sync", "--window", preview, "90", "90", "sleep", ".5",
                            "mousemove", "--sync", "700", "550", "sleep", ".3")
                        events = destination.with_suffix(".jsonl")
                        wait(lambda: events.exists() and '"position"' in events.read_text(), "receiver negotiates COPY")
                        shot("root", f"outbound-drag-{mode}")
                        if mode == "cancel":
                            run("xdotool", "key", "Escape", "mouseup", "1")
                        else:
                            run("xdotool", "mouseup", "1")
                        if mode in ("accept", "nofinish"):
                            wait(destination.exists, "receiver obtains file bytes")
                            assert destination.read_bytes() == (first.parent / "capture.png").read_bytes(), "drag must transfer original PNG, not thumbnail"
                        if mode == "accept":
                            wait(lambda: not windows(PREVIEW), "accepted external COPY dismisses source preview")
                            received = next(json.loads(line) for line in events.read_text().splitlines()
                                            if json.loads(line)["event"] == "received")
                            assert Path(received["path"]) == saved_path, "existing saved file must win"
                        else:
                            time.sleep(6 if mode in ("nofinish", "disappear") else .5)
                            assert windows(PREVIEW), f"{mode} must retain source preview"
                            subprocess.run(["xclip", "-selection", "clipboard"], input=b"reset", env=env, check=True)
                            # Copy reappears once the host's 1 s clipboard check sees the reset.
                            time.sleep(1.5)
                            click(preview, 170, 89, activate=False)
                            wait(lambda: clipboard_pixels() == rgb(first.parent / "capture.png"),
                                 "pointer state recovers for exact-pixel Copy")
                    assert first.exists(), "drag dismissal does not delete History"
                    print("PASS native outbound drag: self drop, original bytes, saved Unicode path, cancellation, rejection, timeout, target loss and repeat")
                    return
                if placement == "bottom_left" and not include:
                    run("xdotool", "windowminimize", root)
                    wait(lambda: not windows("Captures"), "minimized workspace")
                    other_app = spawn("preview-action-focus", ["xmessage", "-title", "Preview action focus fixture",
                        "-geometry", "220x70+900+20", "Keep this application focused"])
                    other = wait(lambda: windows("Preview action focus fixture"), "other app focus")[0]
                    run("xdotool", "windowactivate", "--sync", other, "windowfocus", "--sync", other)
                    click(preview, 170, 89, activate=False)  # Centered Copy, not a footer.

                    def clipboard_png():
                        value = subprocess.run(["xclip", "-selection", "clipboard", "-t", "image/png", "-o"],
                                               env=env, capture_output=True, timeout=5)
                        return value.stdout if value.returncode == 0 else None

                    clipboard = wait(clipboard_png, "full-resolution clipboard image")
                    dimensions = subprocess.check_output(["identify", "-format", "%w %h", "png:-"], input=clipboard)
                    assert dimensions == b"310 170", "Copy used thumbnail dimensions"
                    pixels = subprocess.check_output(["convert", "png:-", "-depth", "8", "rgb:-"], input=clipboard)
                    assert pixels == rgb(first.parent / "capture.png"), "Copy changed full-resolution pixels"
                    assert run("xdotool", "getwindowfocus").decode().strip() == other, "Copy activated Captures"

                    click(preview, 170, second_action_y(first), activate=False)  # Centered Save file.
                    exported = Path(wait(lambda: json.loads(first.read_text()).get("saved_path"), "saved export metadata"))
                    assert rgb(exported) == rgb(first.parent / "capture.png"), "Save changed full-resolution PNG pixels"
                    export_bytes = exported.read_bytes()
                    shot("root", f"{prefix}-saved")
                    assert run("xdotool", "getwindowfocus").decode().strip() == other, "Save activated Captures"
                    assert not windows("Captures"), "Copy/Save unexpectedly restored workspace"

                    def reveals():
                        return [json.loads(line) for line in reveal_log.read_text().splitlines()] if reveal_log.exists() else []

                    time.sleep(.3)  # Allow the saved-state label to paint before another click.
                    click(preview, 170, second_action_y(first), activate=False)  # Same button is now Show in Folder.
                    wait(lambda: reveals() == [[str(exported.parent)]], "Reveal receives exact Unicode export folder")
                    assert list(exported.parent.iterdir()) == [exported], "Reveal created another export"
                    assert run("xdotool", "getwindowfocus").decode().strip() == other, "Reveal activated Captures"
                    assert not windows("Captures"), "Reveal restored workspace"
                    shot(preview, f"{prefix}-reveal")
                    preserved = entries()
                    held = exported.with_suffix(".held")
                    exported.rename(held)
                    click(preview, 170, second_action_y(first), activate=False)
                    time.sleep(.5)
                    shot(preview, f"{prefix}-reveal-missing")
                    assert reveals() == [[str(exported.parent)]], "missing export launched a file manager"
                    assert list(exported.parent.iterdir()) == [held], "missing export silently saved a replacement"
                    assert entries() == preserved and windows(PREVIEW), "missing export removed history or preview"
                    held.rename(exported)
                    click(preview, 170, second_action_y(first), activate=False)
                    wait(lambda: len(reveals()) == 2, "Reveal retries after export is restored")
                    assert reveals() == [[str(exported.parent)]] * 2

                    # With a FileManager1 implementer on the session bus, Reveal
                    # selects the exact file (shipping reveal_item_in_dir) and
                    # does not also open the folder.
                    class FileManager(dbus.service.Object):
                        calls = []

                        @dbus.service.method("org.freedesktop.FileManager1", in_signature="ass", out_signature="")
                        def ShowItems(self, uris, startup_id):
                            self.calls.append([str(uri) for uri in uris])

                    manager_name = dbus.service.BusName("org.freedesktop.FileManager1", bus=bus, do_not_queue=True)
                    manager = FileManager(manager_name, "/org/freedesktop/FileManager1")
                    click(preview, 170, second_action_y(first), activate=False)
                    wait(lambda: manager.calls == [["file://" + quote(str(exported))]],
                         "Reveal selects the exact file through FileManager1.ShowItems")
                    time.sleep(.3)
                    assert len(reveals()) == 2, "ShowItems success also opened the folder"
                    # Dropping the last BusName reference releases the name
                    # while the private bus is still connected.
                    manager.remove_from_connection()
                    del manager, manager_name

                    def private_files(entry):
                        return {path.relative_to(entry.parent): path.read_bytes()
                                for path in entry.parent.rglob("*") if path.is_file()}

                    def draft_files():
                        drafts = history.with_name("editor-drafts")
                        return {path.relative_to(drafts): path.read_bytes()
                                for path in drafts.rglob("*") if path.is_file()}

                    preserved_private = private_files(first)
                    preserved = entries()
                    preserved_drafts = draft_files()
                    click(preview, 290, 50, activate=False)  # Top-right Edit on a left-anchored card.
                    editor = wait(lambda: windows(EDITOR),
                                  "Edit opens the screenshot editor")[0]
                    wait(lambda: active_window() == editor,
                         "Edit focuses the screenshot editor")
                    wait(lambda: run("xprop", "-id", editor, "_NET_WM_NAME").decode().strip()
                         == '_NET_WM_NAME(UTF8_STRING) = "Screenshot editor — Captures"',
                         "preview editor finishes loading its screenshot")
                    assert not windows("Captures"), "Edit restored minimized workspace"
                    assert entries() == preserved and private_files(first) == preserved_private, (
                        "Edit changed the artifact or History")
                    shot("root", f"{prefix}-edit-with-preview")
                    run("xdotool", "windowactivate", "--sync", other, "windowfocus", "--sync", other)
                    assert active_window() == other
                    click(preview, 290, 50, activate=False)
                    wait(lambda: active_window() == editor,
                         "repeated Edit focuses the existing screenshot editor")
                    assert windows(EDITOR) == [editor], (
                        "repeated Edit opened a duplicate screenshot editor")
                    assert not windows("Captures"), "repeated Edit restored minimized workspace"
                    assert entries() == preserved and private_files(first) == preserved_private, (
                        "repeated Edit changed the artifact or History")
                    run("xdotool", "keydown", "Alt_L", "sleep", ".1", "key", "F4",
                        "sleep", ".1", "keyup", "Alt_L", "sleep", ".4")
                    wait(lambda: not windows(EDITOR),
                         "unmodified screenshot editor closes cleanly")
                    assert draft_files() == preserved_drafts, "unmodified Edit changed saved drafts"
                    assert windows(PREVIEW) and not windows("Captures"), (
                        "closing Edit removed the preview or restored workspace")
                    click(preview, 50, 50, activate=False)  # Saved card Close (top-left).
                    wait(lambda: not windows(PREVIEW), "Dismiss closes only the card")
                    time.sleep(.3)
                    assert entries() == preserved and exported.read_bytes() == export_bytes, "Dismiss deleted history or export"
                    assert not windows(PREVIEW), "dismissed card reappeared"

                    def settled_preview(window):
                        samples = []

                        def settled():
                            samples.append(run("import", "-window", window, "-depth", "8", "rgb:-"))
                            return samples[-1] if len(samples) >= 3 and len(set(samples[-3:])) == 1 else None

                        return wait(settled, "preview reaches a stable painted state")

                    # Trash on an unsaved card is preview-only: retain exact
                    # private capture/preview bytes and metadata.
                    unsaved = capture((140, 180, 310, 170))
                    stack_entries = [unsaved]
                    preview = wait(lambda: windows(PREVIEW), "new capture after dismissal")[0]
                    wait(lambda: int(run("import", "-window", preview, "-format", "%k", "info:")) > 16,
                         "replacement paints after dismissal")
                    run("xdotool", "windowminimize", root,
                        "windowactivate", "--sync", other, "windowfocus", "--sync", other)
                    unsaved_private = private_files(unsaved)
                    unsaved_entries = entries()
                    shot(preview, f"{prefix}-trash-unsaved")
                    click(preview, 50, 50, activate=False)  # Unsaved Delete preserves History.
                    wait(lambda: not windows(PREVIEW), "unsaved Trash dismisses preview")
                    assert entries() == unsaved_entries, "unsaved Trash removed history"
                    assert private_files(unsaved) == unsaved_private, "unsaved Trash changed private files or metadata"
                    assert run("xdotool", "getwindowfocus").decode().strip() == other, "unsaved Trash activated Captures"
                    assert not windows("Captures"), "unsaved Trash restored workspace"

                    # A missing saved export must be retryable and must not
                    # create a replacement or mutate private history.
                    trashed = capture((140, 180, 310, 170))
                    stack_entries = [trashed]
                    preview = wait(lambda: windows(PREVIEW), "saved Trash preview")[0]
                    run("xdotool", "windowminimize", root,
                        "windowactivate", "--sync", other, "windowfocus", "--sync", other)
                    run("xdotool", "mousemove", "--sync", "--window", preview, "170", "127")
                    before_save = settled_preview(preview)
                    click(preview, 170, 127, activate=False)
                    trash_export = Path(wait(lambda: json.loads(trashed.read_text()).get("saved_path"),
                                             "Trash fixture saved export metadata"))
                    wait(lambda: settled_preview(preview) != before_save, "saved preview state paints")
                    trash_export_bytes = trash_export.read_bytes()
                    other_exports = {path: path.read_bytes() for path in trash_export.parent.iterdir()
                                     if path != trash_export}
                    trash_private = private_files(trashed)
                    trash_entries = entries()
                    shot(preview, f"{prefix}-trash-saved")
                    held = trash_export.with_suffix(".held")
                    trash_export.rename(held)
                    run("xdotool", "mousemove", "--sync", "--window", preview, "84", "50")
                    before_error = settled_preview(preview)
                    click(preview, 84, 50, activate=False)
                    wait(lambda: windows(PREVIEW) and settled_preview(preview) != before_error,
                         "missing export Trash error paints")
                    shot(preview, f"{prefix}-trash-error")
                    assert {path: path.read_bytes() for path in trash_export.parent.iterdir()} == (
                        other_exports | {held: trash_export_bytes}), "failed Trash replaced or changed an export"
                    assert held.read_bytes() == trash_export_bytes, "failed Trash changed held export"
                    assert entries() == trash_entries and private_files(trashed) == trash_private, (
                        "failed Trash changed private files or metadata")
                    assert run("xdotool", "getwindowfocus").decode().strip() == other, "failed Trash activated Captures"
                    assert not windows("Captures"), "failed Trash restored workspace"

                    held.rename(trash_export)
                    click(preview, 84, 50, activate=False)
                    wait(lambda: not trash_export.exists() and not windows(PREVIEW),
                         "restored export moves to trash and closes preview")
                    assert {path: path.read_bytes() for path in trash_export.parent.iterdir()} == other_exports, (
                        "successful Trash replaced its export or changed another export")
                    assert entries() == trash_entries and private_files(trashed) == trash_private, (
                        "successful Trash changed private files or metadata")
                    trash_files = list((data_home / "Trash/files").iterdir())
                    trash_info = list((data_home / "Trash/info").glob("*.trashinfo"))
                    assert len(trash_files) == len(trash_info) == 1, "private OS trash has unexpected contents"
                    assert trash_files[0].read_bytes() == trash_export_bytes, "OS trash changed export bytes"
                    info_lines = trash_info[0].read_text().splitlines()
                    encoded_path = next(line.removeprefix("Path=") for line in info_lines if line.startswith("Path="))
                    assert unquote(encoded_path) == str(trash_export), "trashinfo Path does not name original export"
                    assert run("xdotool", "getwindowfocus").decode().strip() == other, "Trash activated Captures"
                    assert not windows("Captures"), "Trash restored workspace"

                    other_app.terminate()
                    other_app.wait(timeout=5)
                    stack_entries = [capture((140, 180, 310, 170))]
                    preview = wait(lambda: windows(PREVIEW), "fresh preview after Trash checks")[0]
                    wait(lambda: int(run("import", "-window", preview, "-format", "%k", "info:")) > 16,
                         "fresh preview paints after Trash checks")
                    print("PASS preview actions: Copy/Save/Reveal (ShowItems or folder)/Edit, Dismiss, and disposable OS Trash", flush=True)
                if placement == "bottom_left":
                    # Frozen capture must exactly include the prior composited
                    # card, or exactly omit it, depending on the stored setting.
                    time.sleep(.3)
                    before = output / f"{prefix}-before.png"
                    run("import", "-window", "root", "-crop", "360x330+8+552", str(before))
                    assert rgb(before) != wallpaper_crop(8, 552, 360, 330), "preview was not on desktop"
                    second = capture((8, 552, 360, 330))
                    stack_entries.append(second)
                    expected = rgb(before) if include else wallpaper_crop(8, 552, 360, 330)
                    assert rgb(second.parent / "capture.png") == expected, "preview capture inclusion mismatch"
                    wait(lambda: windows(PREVIEW), "replacement preview")
                saved = entries()
                begin()
                assert bool(windows(PREVIEW)) == include, "capture UI suppression ignored setting"
                run("xdotool", "key", "Escape")
                wait(lambda: windows("Captures") and windows(PREVIEW) and not windows(SELECTOR), "cancel restores preview")
                assert entries() == saved, "cancellation persisted an artifact"
                begin()
                queries = saver.queries
                saver.locked = True
                wait(lambda: saver.queries > queries and windows("Captures") and not windows(SELECTOR), "lock cancels capture")
                saver.locked = False
                wait(lambda: windows(PREVIEW), "unlock restores preview")
                assert entries() == saved, "session cancellation persisted an artifact"
                if args.stack and not include:
                    # These asymmetric captures distinguish retained cards from
                    # duplicate thumbnails and wrong per-card action routing.
                    rectangles = [(45, 55, 230, 110), (260, 240, 200, 140)]
                    while len(stack_entries) < 3:
                        rect = rectangles[len(stack_entries) - 1]
                        entry = capture(rect)
                        assert rgb(entry.parent / "capture.png") == wallpaper_crop(*rect)
                        stack_entries.append(entry)
                    preview = wait(lambda: windows(PREVIEW), "three-card preview")[0]
                    wait(lambda: int(window_geometry(preview)["HEIGHT"]) == 608,
                         "three retained cards (240 + 2 × 184)")
                    geometry = window_geometry(preview)
                    assert int(geometry["X"]) == expected_x, geometry
                    assert int(geometry["Y"]) == (12 if placement.startswith("top") else 232), geometry
                    shot(preview, f"{prefix}-three-cards")
                    assert run("xdotool", "getwindowfocus").decode().strip() != preview

                    def card_action(index, action, count=None):
                        count = len(stack_entries) if count is None else count
                        slot = count - 1 - index if placement.startswith("top") else index
                        x, local_y = (170, 61) if action == "copy" else (
                            290 if placement.endswith("right") else 50, 22)
                        y = (52 if placement.startswith("top") else 28) + slot * 184 + local_y
                        click(preview, x, y, activate=False)

                    # Oldest and newest have different dimensions and pixels;
                    # copying only the latest card cannot pass both assertions.
                    for index in (0, 2):
                        card_action(index, "copy")
                        expected_pixels = rgb(stack_entries[index].parent / "capture.png")
                        wait(lambda: clipboard_pixels() == expected_pixels, f"Copy routes to card {index}")

                    preserved = {path: (path.parent / "capture.png").read_bytes() for path in entries()}
                    card_action(1, "dismiss")
                    stack_entries.pop(1)
                    wait(lambda: int(window_geometry(preview)["HEIGHT"]) == 424, "middle dismissal leaves two cards")
                    assert all((path.parent / "capture.png").read_bytes() == data for path, data in preserved.items())

                    def toggle():
                        height = int(window_geometry(preview)["HEIGHT"])
                        click(preview, 268 if placement.endswith("right") else 72,
                              26 if placement.startswith("top") else height - 26, activate=False)

                    toggle()
                    wait(lambda: int(window_geometry(preview)["HEIGHT"]) == 264, "two-card compact pile")
                    run("xdotool", "mousemove", "0", "0")
                    time.sleep(.35)
                    # Independent CSS blend: rear depth1=.13748 over the
                    # known asymmetric capture, glass-strong-solid=(15,15,18).
                    rear_y = 220 if placement.startswith("top") else 44
                    source = BACKGROUNDS[0][2 if placement.startswith("top") else 0]
                    expected_rear = [round(s * (1 - .13748) + tint * .13748)
                                     for s, tint in zip(source, (15, 15, 18))]
                    actual_rear = run("import", "-window", preview, "-crop", f"1x1+100+{rear_y}",
                                      "-depth", "8", "rgb:-")
                    assert all(abs(a - b) <= 1 for a, b in zip(actual_rear, expected_rear)), (
                        "compact rear depth shade", list(actual_rear), expected_rear)
                    assert run("import", "-window", preview, "-crop", "1x1+170+132", "-depth", "8", "rgb:-") == bytes(
                        BACKGROUNDS[0][3]), "front card was shaded"
                    def fan_pixels(front=False):
                        # Compare only the rear peek or front interior, excluding
                        # pointer, tooltip and the rest of the changing desktop.
                        # Top piles expose the lower rounded corner at y212..215;
                        # the delayed tooltip starts at y216, below this sample.
                        crop = "230x45+55+62" if front else (
                            "20x4+28+212" if placement.startswith("top") else "230x30+55+20")
                        return run("import", "-window", preview, "-crop", crop, "-depth", "8", "rgb:-")
                    rest = fan_pixels()
                    front = fan_pixels(True)
                    fixed_frame = window_geometry(preview)
                    shot(preview, f"{prefix}-collapsed")
                    run("xdotool", "mousemove", "--window", preview, "170", "132")
                    wait(lambda: fan_pixels() != rest, "hover fans rear cards")
                    time.sleep(.35)
                    hovered = fan_pixels()
                    assert fan_pixels(True) == front, "hover moved the front card"
                    assert window_geometry(preview) == fixed_frame, "hover moved/resized the native window"
                    shot(preview, f"{prefix}-hover-fan")
                    time.sleep(.35)
                    shot(preview, f"{prefix}-hover-settled")
                    assert fan_pixels() == hovered, "stationary hover never settled"
                    run("xdotool", "mousemove", "0", "0")
                    wait(lambda: fan_pixels() == rest, "leaving restores the exact rest pose")
                    before_drag = window_geometry(preview)
                    start_x, start_y = int(before_drag["X"]), int(before_drag["Y"])
                    dx = -780 if placement.endswith("right") else 160
                    dy = 80 if placement.startswith("top") else -80
                    other_app = spawn(f"{prefix}-drag-focus", ["xmessage", "-title", "Preview drag focus",
                        "-geometry", "240x30+580+850", "Keep this app focused"])
                    other = wait(lambda: windows("Preview drag focus"), "drag focus fixture")[0]
                    run("xdotool", "windowminimize", root,
                        "windowactivate", "--sync", other, "windowfocus", "--sync", other)
                    run("xdotool", "mousemove", "--window", preview, "170", "132", "mousedown", "1")
                    for step in range(1, 9):
                        run("xdotool", "mousemove", str(start_x + 170 + dx * step // 8),
                            str(start_y + 132 + dy * step // 8))
                        time.sleep(.08)
                    def moved_to_target():
                        actual = window_geometry(preview)
                        return (int(actual["X"]), int(actual["Y"])) == (start_x + dx, start_y + dy)
                    wait(moved_to_target, "compact pile follows desktop pointer")
                    time.sleep(.3)
                    assert moved_to_target(), "stationary pointer drifted after native window movement"
                    run("xdotool", "mouseup", "1")
                    time.sleep(.2)
                    assert int(window_geometry(preview)["HEIGHT"]) == 264, "drag expanded the pile"
                    assert run("xdotool", "getwindowfocus").decode().strip() == other, "drag stole focus"
                    assert not windows("Captures"), "drag restored hidden root"
                    shot("root", f"{prefix}-dragged")
                    other_app.terminate(); other_app.wait(timeout=5)
                    run("xdotool", "windowactivate", "--sync", root, "windowfocus", "--sync", root)
                    time.sleep(.5)
                    third = capture((80, 90, 180, 100))
                    stack_entries.append(third)
                    preview = wait(lambda: windows(PREVIEW), "incoming capture retains pile")[0]
                    # Third card increases reserved peek padding: rounded
                    # 160 + 2 × (28 + 16 × 2 × 25.1 / 26) = 278 px.
                    wait(lambda: int(window_geometry(preview)["HEIGHT"]) == 278, "incoming capture stays collapsed")
                    after_arrival = window_geometry(preview)
                    assert int(after_arrival["X"]) == start_x + dx, "arrival reset dragged x"
                    assert abs(int(after_arrival["Y"]) + 59 - (start_y + dy + 52)) <= 1, "arrival moved front card"
                    begin()
                    run("xdotool", "key", "Escape")
                    preview = wait(lambda: windows(PREVIEW) and not windows(SELECTOR) and windows(PREVIEW),
                                   "cancel restores compact pile")[0]
                    assert int(window_geometry(preview)["HEIGHT"]) == 278
                    click(preview, 170, 132, activate=False)
                    wait(lambda: int(window_geometry(preview)["HEIGHT"]) == 608, "front card expands all previews")
                    assert int(window_geometry(preview)["X"]) == start_x + dx, "expansion reset dragged x"

                    if placement == "bottom_left":
                        # Overflow is not a membership cap. Reveal newest, then
                        # use the shipping "Show older captures" cue to scroll
                        # back and Copy the oldest retained capture.
                        while len(stack_entries) < 8:
                            index = len(stack_entries)
                            stack_entries.append(capture((30 + index * 9, 40, 120 + index * 7, 100)))
                        preview = wait(lambda: windows(PREVIEW), "eight-card preview")[0]
                        wait(lambda: int(window_geometry(preview)["HEIGHT"]) == 812, "bounded overflow viewport")
                        shot(preview, f"{prefix}-overflow-newest")
                        # 1476 px of cards in a 760 px viewport: four 184 px
                        # slots reach the oldest card; the cue then hides.
                        for _ in range(4):
                            click(preview, 170, 17, activate=False)
                            time.sleep(.45)
                        shot(preview, f"{prefix}-overflow-oldest")
                        click(preview, 170, 89, activate=False)
                        expected_pixels = rgb(stack_entries[0].parent / "capture.png")
                        wait(lambda: clipboard_pixels() == expected_pixels, "overflow retains actionable oldest card")

                    preserved = {path: (path.parent / "capture.png").read_bytes() for path in entries()}
                    height = int(window_geometry(preview)["HEIGHT"])
                    click(preview, 298 if placement.endswith("right") else 42,
                          26 if placement.startswith("top") else height - 26, activate=False)
                    wait(lambda: not windows(PREVIEW), "Clear all empties previews")
                    assert entries() == set(preserved), "Clear all removed history"
                    assert all((path.parent / "capture.png").read_bytes() == data for path, data in preserved.items())
                    capture((170, 190, 110, 80))
                    preview = wait(lambda: windows(PREVIEW), "later capture survives Clear all")[0]
                    wait(lambda: int(window_geometry(preview)["HEIGHT"]) == 240, "new stack after Clear all")
                    assert int(window_geometry(preview)["X"]) == expected_x, "empty stack did not reset placement"
                    print(f"PASS {placement} stack: per-card Copy/Dismiss, compact arrival/cancel/expand, nondestructive Clear all", flush=True)
            else:
                time.sleep(1)
                assert not windows(PREVIEW), "disabled previews still appeared"
            if args.lifecycle:
                watcher = dbus.Interface(bus.get_object("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher"),
                                         "org.freedesktop.DBus.Properties")
                wait(lambda: watcher.Get("org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems"),
                     "Captures registered in real SNI tray")
                assert watcher.Get("org.kde.StatusNotifierWatcher", "IsStatusNotifierHostRegistered")
                shot("root", "lifecycle-tray-visible")

                def menu_action(label, screenshot=False):
                    # Enabled shipping rows; keyboard navigation skips separators
                    # and the disabled "Check for Updates…" row.
                    labels = ["New Capture…", "Screenshot Region", "Screenshot Window",
                              "Screenshot Display", "Record Region", "Record Window",
                              "Record Display", "Capture History…", "Open Save Location",
                              "Preferences", "Send Feedback…", "Quit Captures"]
                    index = labels.index(label)
                    panel_ids = run("xdotool", "search", "--onlyvisible", "--class", "xfce4-panel").decode().split()
                    tray = next(window for window in panel_ids
                                if int(window_geometry(window)["WIDTH"]) >= 24)
                    geometry = window_geometry(tray)
                    click(tray, int(geometry["WIDTH"]) // 2, int(geometry["HEIGHT"]) // 2,
                          activate=False, button=3)
                    # Resolve the actual GTK popup, not a fixed desktop point.
                    def visible_popup():
                        popup_ids = run("xdotool", "search", "--onlyvisible", "--class", ".*").decode().split()
                        return next((window for window in popup_ids
                                     if b"_MENU" in run("xprop", "-id", window, "_NET_WM_WINDOW_TYPE")), None)

                    popup = wait(visible_popup, f"tray popup for {label}")
                    popup_geometry = window_geometry(popup)
                    print(f"Tray action {label}: tray={tray} popup={popup} geometry={popup_geometry}", flush=True)
                    shot(popup, "lifecycle-menu-" + label.lower().replace(" ", "-").replace("…", ""))
                    if screenshot:
                        shot("root", "lifecycle-open-tray-menu")
                    run("xdotool", "key", "--delay", "60", *(["Down"] * (index + 1)), "Return")
                    # The click destroys GTK's popup. Do not race its teardown
                    # with another whole-tree query; callers verify the action.

                run("xdotool", "windowactivate", "--sync", root, "key", "alt+F4")
                wait(lambda: not windows("Captures"), "close hides resident workspace")
                assert app.poll() is None and windows(PREVIEW), "close terminated app or previews"
                other_app = spawn("shortcut-focus", ["xmessage", "-title", "Shortcut focus fixture",
                    "-geometry", "220x70+850+250", "Other application remains focused"])
                other = wait(lambda: windows("Shortcut focus fixture"), "shortcut focus target")[0]
                run("xdotool", "windowactivate", "--sync", other, "windowfocus", "--sync", other)
                previous = entries()
                run("xdotool", "keydown", "ctrl+shift+F7", "sleep", ".3")
                assert not windows(SELECTOR), "capture started before shortcut release"
                run("xdotool", "keyup", "ctrl+shift+F7")
                selector = wait(lambda: windows(SELECTOR), "hidden-root region shortcut")[0]
                select_region(selector, (140, 180, 310, 170))
                entry = wait(lambda: entries() - previous, "background region artifact").pop()
                assert rgb(entry.parent / "capture.png") == wallpaper_crop(140, 180, 310, 170)
                wait(lambda: not windows(SELECTOR) and windows(PREVIEW), "background preview restored")
                assert not windows("Captures"), "background capture reopened workspace"

                run("xdotool", "windowactivate", "--sync", other, "key", "ctrl+shift+F8")
                selector = wait(lambda: windows("Captures Window Selection"), "hidden-root window shortcut")[0]
                run("xdotool", "windowfocus", "--sync", other, "key", "Escape")
                wait(lambda: not windows("Captures Window Selection") and windows(PREVIEW), "cross-app Escape restores background")
                assert not windows("Captures") and entries() == previous | {entry}

                previous = entries()
                run("xdotool", "key", "ctrl+shift+F9")
                wait(lambda: entries() - previous, "hidden-root display shortcut")
                wait(lambda: windows(PREVIEW), "display preview")
                assert not windows("Captures"), "display capture reopened workspace"

                menu_action("Capture History…", screenshot=True)  # Real GTK/DBusMenu item.
                wait(lambda: windows("Captures"), "tray History reopens workspace")
                menu_action("Preferences")
                time.sleep(.3)
                shot("root", "lifecycle-preferences")
                run("xdotool", "windowactivate", "--sync", root, "key", "ctrl+shift+F7")
                time.sleep(.5)
                assert not windows(SELECTOR), "focused Preferences did not suppress shortcut"
                run("xdotool", "windowactivate", "--sync", other, "windowfocus", "--sync", other)
                assert run("xdotool", "getwindowfocus").decode().strip() == other
                # X11 activation acknowledgement precedes delivery of egui's
                # focus event; exercise the settled focus boundary here.
                time.sleep(.3)
                run("xdotool", "key", "ctrl+shift+F7")
                wait(lambda: windows(SELECTOR), "unfocused Preferences permits background shortcut")
                run("xdotool", "key", "Escape")
                wait(lambda: not windows(SELECTOR) and windows("Captures"),
                     "cancel restores previously visible Preferences")
                run("xdotool", "windowactivate", "--sync", root, "key", "alt+F4")
                wait(lambda: not windows("Captures"), "hide Preferences")
                run("xdotool", "windowactivate", "--sync", other, "key", "ctrl+shift+F7")
                selector = wait(lambda: windows(SELECTOR), "hidden Preferences does not block background shortcut")[0]
                previous = entries()
                select_region(selector, (140, 180, 310, 170))
                countdown = wait(lambda: windows("Captures Screenshot Countdown"),
                                 "countdown from hidden Preferences")[0]
                shot(countdown, "lifecycle-hidden-preferences-countdown")
                run("xdotool", "key", "Escape")
                wait(lambda: not windows(SELECTOR) and not windows("Captures Screenshot Countdown"),
                     "cancel hidden Preferences countdown")
                assert not windows("Captures") and entries() == previous
                for label, title in [("Screenshot Region", SELECTOR), ("Screenshot Window", "Captures Window Selection")]:
                    menu_action(label)
                    selector = wait(lambda: windows(title), f"tray {label} launches from hidden Preferences")[0]
                    shot(selector, "lifecycle-selector-" + label.lower().replace(" ", "-"))
                    run("xdotool", "key", "Escape")
                    wait(lambda: not windows(title), f"cancel tray {label}")
                    assert not windows("Captures")
                menu_action("New Capture…")
                controls = wait(lambda: windows(CONTROLS), "tray New Capture opens unified controls")[0]
                shot(controls, "controls-tray-empty")
                run("xdotool", "key", "Return", "sleep", ".3")
                assert windows(CONTROLS) and entries() == previous, "empty Region captured"
                run("xdotool", "windowactivate", "--sync", other, "key", "Escape")
                wait(lambda: not windows(CONTROLS), "cross-app Escape cancels New Capture")
                assert not windows("Captures"), "New Capture cancel reopened hidden Preferences"
                menu_action("Quit Captures")
                other_app.terminate()
                other_app.wait(timeout=5)
            else:
                run("xdotool", "windowactivate", "--sync", root, "key", "alt+F4")
            try:
                assert app.wait(timeout=10) == 0, "unclean exit"
            except subprocess.TimeoutExpired:
                shot("root", "timeout-exit")
                raise
            assert not windows(PREVIEW), "preview outlived application"
            print(f"PASS {prefix}: pixels, placement/visibility, cancellation, clean exit", flush=True)
        if args.lifecycle:
            live_args = [str(binary), "--live", "--history-root", str(history),
                         "--settings-file", str(settings)]
            if args.shortcut_editing:
                # Test physical PrintScreen delivery without Openbox's external
                # scrot shortcut stealing it. Change only this private editor
                # session, not the preceding lifecycle fixture or user config.
                # Native OS shortcut takeover remains unimplemented.
                configuration = ET.parse("/etc/xdg/openbox/rc.xml")
                for keyboard in configuration.findall(".//{*}keyboard"):
                    for binding in list(keyboard):
                        if binding.get("key") == "Print":
                            keyboard.remove(binding)
                openbox_config = output / "config/openbox/rc.xml"
                openbox_config.parent.mkdir(parents=True, exist_ok=True)
                configuration.write(openbox_config, encoding="utf-8", xml_declaration=True)
                run("openbox", "--reconfigure")
                edit_settings = output / "shortcut-settings.json"
                edit_settings.write_text(settings.read_text())
                edit_args = [str(binary), "--live", "--history-root", str(history),
                             "--settings-file", str(edit_settings), "--quit-after", "180"]
                editor = spawn("shortcut-editor", edit_args)
                root = wait(lambda: windows("Captures"), "shortcut editor workspace")[0]
                run("xdotool", "windowmove", "--sync", root, "20", "60")
                time.sleep(1)

                def open_shortcuts():
                    click(root, 196, 18)
                    click(root, 98, 189)
                    time.sleep(.4)

                # Measured live root-client positions, not fixture coordinates.
                # Both normal and focused states retain the same row geometry.
                rows = [317, 353, 389, 425, 461, 497, 533]
                recorder_x = 745  # Recorders are right-aligned, as in shipping.
                paths = [("new_capture_shortcut",), ("region_shortcut",),
                         ("window_shortcut",), ("display_shortcut",),
                         ("recording", "video_shortcut"), ("recording", "window_shortcut"),
                         ("recording", "display_shortcut")]

                def stored_keys():
                    data = json.loads(edit_settings.read_text())
                    values = []
                    for path in paths:
                        value = data
                        for key in path:
                            value = value.get(key, {})
                        values.append(value if isinstance(value, str) else None)
                    return values

                def record(index, chord, expected):
                    before = stored_keys()
                    click(root, recorder_x, rows[index])
                    run("xdotool", "key", chord)
                    wait(lambda: stored_keys()[index] == expected, f"persist {paths[index]} = {expected}")
                    after = stored_keys()
                    assert all(value is None or after[i] == value
                               for i, value in enumerate(before) if i != index), (before, after)
                    assert editor.poll() is None and not windows(SELECTOR)
                    time.sleep(.3)

                open_shortcuts()
                shot(root, "shortcuts-dark-normal")
                # An alias change makes persistence observable even though this
                # chord is already globally registered. Merely suppressing its
                # callback (without releasing the OS grab) cannot pass this.
                record(1, "ctrl+shift+F7", "Control+Shift+F7")
                baseline = stored_keys()
                click(root, recorder_x, rows[0])
                run("xdotool", "keydown", "ctrl", "sleep", ".2")
                shot(root, "shortcuts-dark-recording")
                run("xdotool", "keyup", "ctrl", "key", "p", "sleep", ".2")
                shot(root, "shortcuts-dark-invalid")
                assert stored_keys() == baseline
                run("xdotool", "key", "ctrl+shift+Escape", "key", "ctrl+alt+F11", "sleep", ".4")
                assert stored_keys() == baseline, "modified Escape failed to cancel recording"

                other_app = spawn("editor-focus", ["xmessage", "-title", "Recorder focus fixture",
                    "-geometry", "220x70+1040+250", "Recorder blur target"])
                other = wait(lambda: windows("Recorder focus fixture"), "recorder blur target")[0]
                click(root, recorder_x, rows[0])
                run("xdotool", "windowactivate", "--sync", other, "windowfocus", "--sync", other)
                time.sleep(.3)
                run("xdotool", "windowactivate", "--sync", root, "windowfocus", "--sync", root)
                time.sleep(.3)
                run("xdotool", "key", "ctrl+alt+F11", "sleep", ".4")
                assert stored_keys() == baseline, "blurred recorder accepted a later key"

                # The existing Region chord is deliverable to another recorder,
                # but the settings validator must reject that duplicate.
                click(root, recorder_x, rows[2])
                run("xdotool", "key", "ctrl+shift+F7", "sleep", ".5")
                shot(root, "shortcuts-duplicate-error")
                assert stored_keys() == baseline, "duplicate shortcut was persisted"
                # The recorder keeps keyboard focus across the error banner.
                run("xdotool", "key", "space", "sleep", ".2", "key", "ctrl+f")
                wait(lambda: stored_keys()[2] == "Control+KeyF", "repair duplicate via focused recorder")
                open_shortcuts()
                for index, chord, expected in [
                    (0, "ctrl+q", "Control+KeyQ"),
                    (1, "ctrl+alt+r", "Control+Alt+KeyR"),
                    (2, "ctrl+f", "Control+KeyF"),
                    (3, "ctrl+shift+F6", "Control+Shift+F6"),
                    (4, "Print", "PrintScreen"),
                    # With NumLock off, this is the physical keypad 7 key.
                    # KP_7 makes xdotool inject a NumLock key first, which a
                    # correctly raw recorder would capture instead.
                    (5, "ctrl+KP_Home", "Control+Numpad7"),
                    (6, "ctrl+shift+alt+super+XF86AudioPrev", "Control+Shift+Alt+Super+MediaTrackPrevious"),
                ]:
                    record(index, chord, expected)
                # New Capture is now registered, so do not leave it on the
                # workspace Quit chord after proving recorder interception.
                record(0, "ctrl+alt+n", "Control+Alt+KeyN")
                expected = stored_keys()
                shot(root, "shortcuts-all-seven-edited")
                click(root, 80, 18)
                time.sleep(.3)  # Settle navigation without injecting another event.
                run("xdotool", "key", "ctrl+alt+r")
                wait(lambda: windows(SELECTOR), "first global chord after leaving Preferences")
                run("xdotool", "key", "Escape")
                wait(lambda: not windows(SELECTOR) and windows("Captures"), "navigation shortcut cancel")
                open_shortcuts()
                click(root, recorder_x, rows[0])
                click(root, 80, 18)  # Leaving Preferences must cancel the recorder.
                run("xdotool", "key", "ctrl+q")
                assert editor.wait(timeout=10) == 0, "stale recorder swallowed workspace Quit"
                assert stored_keys() == expected

                editor = spawn("shortcut-editor-restart", edit_args)
                root = wait(lambda: windows("Captures"), "restart with saved shortcuts")[0]
                run("xdotool", "windowmove", "--sync", root, "20", "60")
                time.sleep(1)
                assert stored_keys() == expected, "restart changed stored bindings"
                open_shortcuts()
                shot(root, "shortcuts-restarted")
                run("xdotool", "windowactivate", "--sync", other, "windowfocus", "--sync", other)
                time.sleep(.3)
                run("xdotool", "key", "ctrl+alt+r")
                wait(lambda: windows(SELECTOR), "edited Region chord restored after Preferences blur")
                run("xdotool", "key", "Escape")
                wait(lambda: not windows(SELECTOR) and windows("Captures"), "edited shortcut cancellation")
                run("xdotool", "windowactivate", "--sync", other, "windowfocus", "--sync", other)
                time.sleep(.3)
                run("xdotool", "key", "ctrl+alt+n")
                controls = wait(lambda: windows(CONTROLS), "edited New Capture chord after restart and blur")[0]
                shot(controls, "controls-edited-shortcut")
                run("xdotool", "key", "Escape")
                wait(lambda: not windows(CONTROLS) and windows("Captures"), "New Capture restores visible Preferences")
                # No retained child viewport may bootstrap the hidden root's
                # UI incidentally. This is the first preview after restart.
                assert not windows(PREVIEW)
                run("xdotool", "windowactivate", "--sync", root, "key", "alt+F4")
                wait(lambda: not windows("Captures"), "hide empty-preview Preferences")
                run("xdotool", "windowactivate", "--sync", other, "windowfocus", "--sync", other)
                time.sleep(.3)
                previous = entries()
                run("xdotool", "key", "ctrl+alt+r")
                selector = wait(lambda: windows(SELECTOR), "hidden root creates first selector")[0]
                select_region(selector, (140, 180, 310, 170))
                entry = wait(lambda: entries() - previous, "first background capture after restart").pop()
                preview = wait(lambda: windows(PREVIEW), "hidden root creates first mini preview")[0]
                assert not windows("Captures"), "first preview reopened hidden Preferences"
                assert rgb(entry.parent / "capture.png") == wallpaper_crop(140, 180, 310, 170)
                shot(preview, "shortcuts-first-background-preview")
                menu_action("Quit Captures")
                assert editor.wait(timeout=10) == 0
                other_app.terminate()
                other_app.wait(timeout=5)
                print("PASS shortcut editing: all seven fields, registered chord, raw keys, cancel/blur, duplicate rejection, restart and global launch", flush=True)

            # Automation completion must explicitly quit even when a real tray
            # would intercept an ordinary window close into background mode.
            for label, completion in [
                ("timed", ["--quit-after", "3"]),
                ("framebuffer", ["--screenshot", str(output / "lifecycle-framebuffer.png"),
                                 "--screenshot-after", "2"]),
            ]:
                probe = spawn(f"lifecycle-{label}-quit", live_args + completion)
                wait(lambda: watcher.Get("org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems"),
                     f"{label} automation has a real tray")
                assert probe.wait(timeout=10) == 0, f"{label} completion hid instead of quitting"
                wait(lambda: not watcher.Get("org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems"),
                     f"{label} quit unregisters tray")
                assert not windows("Captures") and not windows(PREVIEW)
            assert (output / "lifecycle-framebuffer.png").is_file()

            probe = spawn("lifecycle-tray-loss", live_args)
            root = wait(lambda: windows("Captures"), "tray-loss workspace")[0]
            wait(lambda: watcher.Get("org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems"),
                 "tray-loss registration")
            run("xdotool", "windowactivate", "--sync", root, "key", "alt+F4")
            wait(lambda: not windows("Captures"), "hide before tray host loss")
            assert probe.poll() is None
            run("xfce4-panel", "--quit")
            panel.wait(timeout=10)
            root = wait(lambda: windows("Captures"), "tray loss restores hidden root")[0]
            wait(lambda: active_window() == root,
                 "tray loss focuses recovered root")
            time.sleep(.3)
            shot(root, "lifecycle-tray-loss-recovery")
            run("xdotool", "windowactivate", "--sync", root, "key", "alt+F4")
            assert probe.wait(timeout=10) == 0, "tray loss retained close-to-hide"
            print("PASS lifecycle: real tray automation Quit and hidden-root host-loss recovery", flush=True)
        (output / "result.json").write_text(json.dumps({"passed": True, "scenarios": len(cases),
            "multiCard": args.stack, "residentLifecycle": args.lifecycle, "shortcutEditing": args.shortcut_editing,
            "checks": ["selected corner positions and dimensions", "nonactivating map",
                "minimized-root full-pixel Copy and Save without activation",
                "Save becomes Reveal; exact Unicode folder delivery, missing export and retry preserve files",
                "Reveal selects the exact file URI through FileManager1.ShowItems when available",
                "Edit opens and refocuses one editor without restoring root or changing artifact, History or drafts",
                "Dismiss preserves history and export",
                "new capture after dismissal", "exact inclusion and exclusion pixels",
                "Escape and simulated-lock restoration", "clean exit"] +
                ([] if args.lifecycle else ["disabled previews"]) +
                (["three-card retention and per-card Copy", "middle-card dismissal preserves files",
                  "collapsed arrival/cancel/front-card expand", "eight-card overflow cue scrolls to oldest",
                  "Clear all preserves files and later arrivals"] if args.stack else []) +
                (["real SNI menu History/Preferences/Quit", "close-to-background keeps previews",
                  "region/window/display global shortcuts", "release-only launch", "hidden root stays hidden",
                  "focused Preferences suppression and unfocused/hidden Preferences launch",
                  "hidden Preferences countdown and cancellation",
                  "tray region/window capture from hidden Preferences",
                  "tray New Capture, empty-region guard and cross-app Escape preserve hidden root",
                  "timed/framebuffer completion quits with real tray",
                  "tray host loss restores/focuses hidden root and restores normal close"] if args.lifecycle else []) +
                (["all seven shortcut persistence paths", "registered chord reaches recorder",
                  "modifier/invalid rendering and modified Escape/blur cancellation",
                  "duplicate save rejection", "Ctrl-F/Q interception while recording",
                  "raw PrintScreen/keypad/Super/media input", "leaving Preferences restores Quit",
                  "restart persistence and edited global launch after blur",
                  "edited New Capture launches unified controls after restart and blur",
                  "hidden root creates first selector and mini preview without reopening"] if args.shortcut_editing else []),
            "scope": "Private X11/software GL, simulated session; not hardware, real lock, Wayland or accessibility acceptance."}, indent=2))
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
