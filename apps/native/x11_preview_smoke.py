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

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

from x11_capture_smoke import BACKGROUNDS, ScreenSaver


PREVIEW = "Captures Mini Preview"
SELECTOR = "Captures Region Selection"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--stack", action="store_true", help="Also exercise retained multi-card previews")
    parser.add_argument("--lifecycle", action="store_true", help="Exercise a real Xfce SNI tray and background shortcuts")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = {**os.environ, "WGPU_BACKEND": "gl", "WINIT_X11_SCALE_FACTOR": "1", "XDG_SESSION_TYPE": "x11"}
    env.pop("WAYLAND_DISPLAY", None)
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
            assert select.select([process.stdout], [], [], 10)[0], f"{name} did not announce endpoint"
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

    def wait(predicate, description):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if value := predicate():
                return value
            time.sleep(.05)
        shot("root", "timeout-desktop")
        raise AssertionError(f"Timed out: {description}")

    def click(window, x, y, activate=True):
        # Cross a real intermediate point to avoid winit's stale-position filter
        # after an X11 remap. Preview clicks deliberately do not activate windows.
        if activate:
            run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window)
        run("xdotool", "mousemove", "--sync", "--window", window, str(x - 1), str(y),
            "mousemove_relative", "--sync", "1", "0", "sleep", ".15", "mousedown", "1",
            "sleep", ".15", "mouseup", "1")

    def rgb(path):
        return run("convert", str(path), "-depth", "8", "rgb:-")

    def clipboard_pixels():
        value = subprocess.run(["xclip", "-selection", "clipboard", "-t", "image/png", "-o"],
                               env=env, capture_output=True, timeout=5)
        if value.returncode != 0:
            return None
        return subprocess.check_output(["convert", "png:-", "-depth", "8", "rgb:-"], input=value.stdout)

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

        cases = [(placement, True, False) for placement in ("bottom_left", "bottom_right", "top_left", "top_right")]
        cases += [("bottom_left", True, True), ("bottom_left", False, False)]
        if args.lifecycle:
            cases = cases[:1]
        for placement, enabled, include in cases:
            prefix = f"{placement}-enabled-{enabled}-include-{include}"
            history = output / prefix / "history"
            settings = output / f"{prefix}-settings.json"
            settings.write_text(json.dumps({
                "settings_schema_version": 5, "appearance": "dark", "theme": "mustard",
                "output_directory": str(output / prefix / "exports"),
                "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
                "display_shortcut": "Ctrl+Shift+F9", "launch_at_login": False,
                "auto_copy_to_clipboard": False, "auto_start_on_selection": False,
                "freeze_screen": True, "show_cursor_in_screenshots": False,
                "screenshot_countdown_seconds": 0, "show_mini_previews": enabled,
                "include_mini_previews_in_captures": include, "mini_preview_placement": placement,
            }))
            app = spawn(prefix, [str(binary), "--live", "--history-root", str(history),
                "--settings-file", str(settings), "--quit-after", "300" if args.lifecycle else "180"])
            root = wait(lambda: windows("Captures"), "root workspace")[0]
            # Leave the left-hand preview/capture area unobstructed. Both root
            # capture buttons still fit on this desktop after moving the window.
            run("xdotool", "windowmove", "--sync", root, "620", "20")
            time.sleep(1)

            def entries():
                return set(history.glob("*/metadata.json"))

            def begin():
                # Keep the capture control outside all four always-on-top cards.
                run("xdotool", "windowmove", "--sync", root, "400", "280")
                click(root, 467, 141)
                selector = wait(lambda: windows(SELECTOR), "region selector")[0]
                wait(lambda: int(run("import", "-window", selector, "-crop", "1280x96+0+804",
                    "-format", "%k", "info:")) > 16, "painted region controls")
                return selector

            def capture(rect):
                previous = entries()
                selector = begin()
                x, y, width, height = rect
                run("xdotool", "windowfocus", "--sync", selector, "mousemove", "--window", selector,
                    str(x), str(y), "sleep", ".1", "mousedown", "1", "sleep", ".15", "mousemove",
                    "--window", selector, str(x + width), str(y + height), "sleep", ".15", "mouseup", "1",
                    "key", "Return")
                entry = wait(lambda: entries() - previous, "new persisted capture").pop()
                wait(lambda: windows("Captures") and not windows(SELECTOR), "restored root")
                # Openbox may reposition an off-screen workspace when remapping.
                # Keep it out of the pixel oracle crop again before comparing a
                # composited card with the following screenshot (which hides it).
                run("xdotool", "windowmove", "--sync", root, "620", "20")
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
                if placement == "bottom_left" and not include:
                    run("xdotool", "windowminimize", root)
                    wait(lambda: not windows("Captures"), "minimized workspace")
                    other_app = spawn("preview-action-focus", ["xmessage", "-title", "Preview action focus fixture",
                        "-geometry", "220x70+900+20", "Keep this application focused"])
                    other = wait(lambda: windows("Preview action focus fixture"), "other app focus")[0]
                    run("xdotool", "windowactivate", "--sync", other, "windowfocus", "--sync", other)
                    click(preview, 63, 169, activate=False)  # Padded card Copy center.

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

                    click(preview, 122, 169, activate=False)  # Padded card Save center.
                    exported = Path(wait(lambda: json.loads(first.read_text()).get("saved_path"), "saved export metadata"))
                    assert rgb(exported) == rgb(first.parent / "capture.png"), "Save changed full-resolution PNG pixels"
                    export_bytes = exported.read_bytes()
                    shot("root", f"{prefix}-saved")
                    assert run("xdotool", "getwindowfocus").decode().strip() == other, "Save activated Captures"
                    assert not windows("Captures"), "Copy/Save unexpectedly restored workspace"
                    click(preview, 189, 169, activate=False)  # Padded card History center.
                    wait(lambda: windows("Captures"), "History restores workspace")
                    preserved = entries()
                    click(preview, 288, 169, activate=False)  # Padded card Dismiss center.
                    wait(lambda: not windows(PREVIEW), "Dismiss closes only the card")
                    time.sleep(.3)
                    assert entries() == preserved and exported.read_bytes() == export_bytes, "Dismiss deleted history or export"
                    assert not windows(PREVIEW), "dismissed card reappeared"
                    other_app.terminate()
                    other_app.wait(timeout=5)
                    stack_entries = [capture((140, 180, 310, 170))]
                    preview = wait(lambda: windows(PREVIEW), "new capture after dismissal")[0]
                    wait(lambda: int(run("import", "-window", preview, "-format", "%k", "info:")) > 16,
                         "replacement paints after dismissal")
                    print("PASS preview actions: full-pixel Copy/Save without activation, History restores, Dismiss preserves files", flush=True)
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

                    def card_action(index, x, count=None):
                        count = len(stack_entries) if count is None else count
                        slot = count - 1 - index if placement.startswith("top") else index
                        y = (52 if placement.startswith("top") else 28) + slot * 184 + 141
                        click(preview, x, y, activate=False)

                    # Oldest and newest have different dimensions and pixels;
                    # copying only the latest card cannot pass both assertions.
                    for index in (0, 2):
                        card_action(index, 63)
                        expected_pixels = rgb(stack_entries[index].parent / "capture.png")
                        wait(lambda: clipboard_pixels() == expected_pixels, f"Copy routes to card {index}")

                    preserved = {path: (path.parent / "capture.png").read_bytes() for path in entries()}
                    card_action(1, 288)
                    stack_entries.pop(1)
                    wait(lambda: int(window_geometry(preview)["HEIGHT"]) == 424, "middle dismissal leaves two cards")
                    assert all((path.parent / "capture.png").read_bytes() == data for path, data in preserved.items())

                    def toggle():
                        height = int(window_geometry(preview)["HEIGHT"])
                        click(preview, 65, 26 if placement.startswith("top") else height - 26, activate=False)

                    toggle()
                    wait(lambda: int(window_geometry(preview)["HEIGHT"]) == 264, "two-card compact pile")
                    shot(preview, f"{prefix}-collapsed")
                    third = capture((80, 90, 180, 100))
                    stack_entries.append(third)
                    preview = wait(lambda: windows(PREVIEW), "incoming capture retains pile")[0]
                    # Third card increases reserved peek padding: rounded
                    # 160 + 2 × (28 + 16 × 2 × 25.1 / 26) = 278 px.
                    wait(lambda: int(window_geometry(preview)["HEIGHT"]) == 278, "incoming capture stays collapsed")
                    begin()
                    run("xdotool", "key", "Escape")
                    preview = wait(lambda: windows(PREVIEW) and not windows(SELECTOR) and windows(PREVIEW),
                                   "cancel restores compact pile")[0]
                    assert int(window_geometry(preview)["HEIGHT"]) == 278
                    click(preview, 170, 132, activate=False)
                    wait(lambda: int(window_geometry(preview)["HEIGHT"]) == 608, "front card expands all previews")

                    if placement == "bottom_left":
                        # Overflow is not a membership cap. Reveal newest, then
                        # scroll back and Copy the oldest retained capture.
                        while len(stack_entries) < 8:
                            index = len(stack_entries)
                            stack_entries.append(capture((30 + index * 9, 40, 120 + index * 7, 100)))
                        preview = wait(lambda: windows(PREVIEW), "eight-card preview")[0]
                        wait(lambda: int(window_geometry(preview)["HEIGHT"]) == 812, "bounded overflow viewport")
                        shot(preview, f"{prefix}-overflow-newest")
                        run("xdotool", "mousemove", "--window", preview, "170", "400", "click", "--repeat", "30", "--delay", "30", "4")
                        time.sleep(.5)
                        shot(preview, f"{prefix}-overflow-oldest")
                        click(preview, 63, 169, activate=False)
                        expected_pixels = rgb(stack_entries[0].parent / "capture.png")
                        wait(lambda: clipboard_pixels() == expected_pixels, "overflow retains actionable oldest card")

                    preserved = {path: (path.parent / "capture.png").read_bytes() for path in entries()}
                    height = int(window_geometry(preview)["HEIGHT"])
                    click(preview, 250, 26 if placement.startswith("top") else height - 26, activate=False)
                    wait(lambda: not windows(PREVIEW), "Clear all empties previews")
                    assert entries() == set(preserved), "Clear all removed history"
                    assert all((path.parent / "capture.png").read_bytes() == data for path, data in preserved.items())
                    capture((170, 190, 110, 80))
                    preview = wait(lambda: windows(PREVIEW), "later capture survives Clear all")[0]
                    wait(lambda: int(window_geometry(preview)["HEIGHT"]) == 240, "new stack after Clear all")
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

                def menu_action(index, screenshot=False):
                    panel_ids = run("xdotool", "search", "--onlyvisible", "--class", "xfce4-panel").decode().split()
                    tray = next(window for window in panel_ids
                                if int(window_geometry(window)["WIDTH"]) >= 24)
                    geometry = window_geometry(tray)
                    run("xdotool", "mousemove", "--window", tray, str(int(geometry["WIDTH"]) // 2),
                        str(int(geometry["HEIGHT"]) // 2), "click", "3", "sleep", ".4")
                    if screenshot:
                        shot("root", "lifecycle-open-tray-menu")
                    run("xdotool", "key", "Home")
                    for _ in range(index):
                        run("xdotool", "key", "Down")
                    run("xdotool", "key", "Return")

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
                run("xdotool", "windowfocus", "--sync", selector, "mousemove", "--window", selector,
                    "140", "180", "mousedown", "1", "sleep", ".15", "mousemove", "--window", selector,
                    "450", "350", "sleep", ".15", "mouseup", "1", "key", "Return")
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

                menu_action(3, screenshot=True)  # Real GTK/DBusMenu History item.
                wait(lambda: windows("Captures"), "tray History reopens workspace")
                menu_action(4)
                time.sleep(.3)
                shot("root", "lifecycle-preferences")
                run("xdotool", "windowactivate", "--sync", root, "key", "ctrl+shift+F7")
                time.sleep(.5)
                assert not windows(SELECTOR), "focused Preferences did not suppress shortcut"
                run("xdotool", "windowactivate", "--sync", other, "key", "ctrl+shift+F7")
                wait(lambda: windows(SELECTOR), "unfocused Preferences permits background shortcut")
                run("xdotool", "key", "Escape")
                wait(lambda: not windows(SELECTOR) and windows("Captures"),
                     "cancel restores previously visible Preferences")
                run("xdotool", "windowactivate", "--sync", root, "key", "alt+F4")
                wait(lambda: not windows("Captures"), "hide Preferences")
                run("xdotool", "windowactivate", "--sync", other, "key", "ctrl+shift+F7")
                wait(lambda: windows(SELECTOR), "hidden Preferences does not block background shortcut")
                run("xdotool", "key", "Escape")
                wait(lambda: not windows(SELECTOR), "cancel hidden Preferences capture")
                assert not windows("Captures")
                menu_action(1)
                wait(lambda: windows(SELECTOR), "tray region launches from hidden Preferences")
                run("xdotool", "key", "Escape")
                wait(lambda: not windows(SELECTOR), "cancel tray region capture")
                assert not windows("Captures")
                menu_action(6)
                other_app.terminate()
                other_app.wait(timeout=5)
            else:
                run("xdotool", "windowactivate", "--sync", root, "key", "alt+F4")
            assert app.wait(timeout=10) == 0, "unclean exit"
            assert not windows(PREVIEW), "preview outlived application"
            print(f"PASS {prefix}: pixels, placement/visibility, cancellation, clean exit", flush=True)
        if args.lifecycle:
            live_args = [str(binary), "--live", "--history-root", str(history),
                         "--settings-file", str(settings)]
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
            wait(lambda: run("xdotool", "getactivewindow").decode().strip() == root,
                 "tray loss focuses recovered root")
            time.sleep(.3)
            shot(root, "lifecycle-tray-loss-recovery")
            run("xdotool", "windowactivate", "--sync", root, "key", "alt+F4")
            assert probe.wait(timeout=10) == 0, "tray loss retained close-to-hide"
            print("PASS lifecycle: real tray automation Quit and hidden-root host-loss recovery", flush=True)
        (output / "result.json").write_text(json.dumps({"passed": True, "scenarios": len(cases),
            "multiCard": args.stack, "residentLifecycle": args.lifecycle,
            "checks": ["selected corner positions and dimensions", "nonactivating map",
                "minimized-root full-pixel Copy and Save without activation",
                "History restores minimized workspace", "Dismiss preserves history and export",
                "new capture after dismissal", "exact inclusion and exclusion pixels",
                "Escape and simulated-lock restoration", "clean exit"] +
                ([] if args.lifecycle else ["disabled previews"]) +
                (["three-card retention and per-card Copy", "middle-card dismissal preserves files",
                  "collapsed arrival/cancel/front-card expand", "eight-card overflow retains oldest",
                  "Clear all preserves files and later arrivals"] if args.stack else []) +
                (["real SNI menu History/Preferences/Quit", "close-to-background keeps previews",
                  "region/window/display global shortcuts", "release-only launch", "hidden root stays hidden",
                  "focused Preferences suppression and unfocused/hidden Preferences launch",
                  "tray region capture from hidden Preferences",
                  "timed/framebuffer completion quits with real tray",
                  "tray host loss restores/focuses hidden root and restores normal close"] if args.lifecycle else []),
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
