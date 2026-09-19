#!/usr/bin/python3
"""Real X11 region/window capture on a private desktop and simulated session state.

Requires system Python's dbus/gi, Xvfb, Openbox, picom, xdotool, hsetroot,
xmessage and ImageMagick. Never uses the caller's display or D-Bus session.
No production capture/session bypass exists in the application.
"""
import argparse
import ctypes as c
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


BACKGROUNDS = (
    ((36, 52, 68), (173, 114, 39), (29, 99, 144), (90, 38, 118)),
    ((113, 40, 60), (27, 134, 93), (180, 91, 35), (71, 53, 149)),
)


class WindowFixture:
    """Ordinary WM-managed X11 client; an owned background pixmap survives expose.

    No application test hook or additional GUI toolkit is needed. All Xlib calls
    stay on the script's main thread and connect only to the private Xvfb server.
    """

    def __init__(self, display, title, x, y, width=421, height=237):
        self.xlib = c.CDLL("libX11.so.6")
        signatures = {
            "XOpenDisplay": (c.c_void_p, [c.c_char_p]),
            "XDefaultRootWindow": (c.c_ulong, [c.c_void_p]),
            "XCreateSimpleWindow": (c.c_ulong, [c.c_void_p, c.c_ulong, c.c_int, c.c_int,
                                               c.c_uint, c.c_uint, c.c_uint, c.c_ulong, c.c_ulong]),
            "XInternAtom": (c.c_ulong, [c.c_void_p, c.c_char_p, c.c_int]),
            "XChangeProperty": (c.c_int, [c.c_void_p, c.c_ulong, c.c_ulong, c.c_ulong,
                                          c.c_int, c.c_int, c.c_void_p, c.c_int]),
            "XStoreName": (c.c_int, [c.c_void_p, c.c_ulong, c.c_char_p]),
            "XCreatePixmap": (c.c_ulong, [c.c_void_p, c.c_ulong, c.c_uint, c.c_uint, c.c_uint]),
            "XCreateGC": (c.c_void_p, [c.c_void_p, c.c_ulong, c.c_ulong, c.c_void_p]),
            "XSetForeground": (c.c_int, [c.c_void_p, c.c_void_p, c.c_ulong]),
            "XFillRectangle": (c.c_int, [c.c_void_p, c.c_ulong, c.c_void_p, c.c_int,
                                         c.c_int, c.c_uint, c.c_uint]),
            "XSetWindowBackgroundPixmap": (c.c_int, [c.c_void_p, c.c_ulong, c.c_ulong]),
            "XClearWindow": (c.c_int, [c.c_void_p, c.c_ulong]),
            "XMapRaised": (c.c_int, [c.c_void_p, c.c_ulong]),
            "XUnmapWindow": (c.c_int, [c.c_void_p, c.c_ulong]),
            "XSync": (c.c_int, [c.c_void_p, c.c_int]),
            "XCloseDisplay": (c.c_int, [c.c_void_p]),
        }
        for name, (result, arguments) in signatures.items():
            function = getattr(self.xlib, name)
            function.restype, function.argtypes = result, arguments
        self.display = self.xlib.XOpenDisplay(display.encode())
        assert self.display, "private X11 connection failed"
        self.width, self.height = width, height
        self.window = self.xlib.XCreateSimpleWindow(self.display,
            self.xlib.XDefaultRootWindow(self.display), x, y, width, height, 0, 0, 0)
        self.xlib.XStoreName(self.display, self.window, title.encode())
        name = self.xlib.XInternAtom(self.display, b"_NET_WM_NAME", 0)
        utf8 = self.xlib.XInternAtom(self.display, b"UTF8_STRING", 0)
        self.xlib.XChangeProperty(self.display, self.window, name, utf8, 8, 0,
                                  c.c_char_p(title.encode()), len(title.encode()))
        # Suppress decorations, not WM management/enumeration. Bounds and saved
        # pixels should match the independently specified client, not a theme.
        atom = self.xlib.XInternAtom(self.display, b"_MOTIF_WM_HINTS", 0)
        hints = (c.c_ulong * 5)(2, 0, 0, 0, 0)
        self.xlib.XChangeProperty(self.display, self.window, atom, atom, 32, 0, hints, 5)
        self.pixmap = self.xlib.XCreatePixmap(self.display, self.window, width, height, 24)
        self.gc = self.xlib.XCreateGC(self.display, self.pixmap, 0, None)
        self.paint(0)

    def paint(self, index):
        for (x, y, w, h), (red, green, blue) in zip(
                ((0, 0, 157, 89), (157, 0, self.width - 157, 89),
                 (0, 89, 157, self.height - 89), (157, 89, self.width - 157, self.height - 89)),
                BACKGROUNDS[index]):
            self.xlib.XSetForeground(self.display, self.gc, red << 16 | green << 8 | blue)
            self.xlib.XFillRectangle(self.display, self.pixmap, self.gc, x, y, w, h)
        self.xlib.XSetWindowBackgroundPixmap(self.display, self.window, self.pixmap)
        self.xlib.XClearWindow(self.display, self.window)
        self.xlib.XSync(self.display, 0)

    def show(self):
        self.xlib.XMapRaised(self.display, self.window)
        self.xlib.XSync(self.display, 0)

    def hide(self):
        self.xlib.XUnmapWindow(self.display, self.window)
        self.xlib.XSync(self.display, 0)

    def close(self):
        # Closing the connection frees its window, pixmap and GC together.
        self.xlib.XCloseDisplay(self.display)


class ScreenSaver(dbus.service.Object):
    locked = False
    queries = 0

    @dbus.service.method("org.freedesktop.ScreenSaver", out_signature="b")
    def GetActive(self):
        self.queries += 1
        return self.locked


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--controls", action="store_true",
                        help="Exercise the same capture oracles through New Capture controls")
    parser.add_argument("--target-shortcuts", action="store_true",
                        help="Switch New Capture targets using registered global shortcuts")
    args = parser.parse_args()
    if args.target_shortcuts and not args.controls:
        parser.error("--target-shortcuts requires --controls")
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    children, logs, fixtures = [], [], []
    loop = None
    env = os.environ.copy()
    # Native children must not fall back to the caller's Wayland connection.
    env.pop("WAYLAND_DISPLAY", None)
    env.update(WGPU_BACKEND="gl", WINIT_X11_SCALE_FACTOR="1", XDG_SESSION_TYPE="x11")

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
                raise RuntimeError(f"{name} did not announce its private endpoint")
            endpoint = process.stdout.readline().decode().strip()
            if not endpoint:
                raise RuntimeError(f"{name} failed; see its stderr")
            return endpoint
        return process

    def run(*command):
        return subprocess.check_output(command, env=env, timeout=10)

    def windows(title, visible=True):
        result = subprocess.run(["xdotool", "search", *(["--onlyvisible"] if visible else []),
                                 "--name", f"^{title}$"], env=env, capture_output=True, text=True, timeout=5)
        if result.returncode not in (0, 1):
            raise RuntimeError(result.stderr)
        return result.stdout.split()

    def wait(predicate, description):
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            result = predicate()
            if result:
                return result
            time.sleep(.05)
        for title in ("Captures", "Captures Region Selection", "Captures Window Selection",
                      "Captures Capture Controls", "Captures Screenshot Countdown"):
            for window in windows(title):
                screenshot(window, f"timeout-{title}")
                print(run("xdotool", "getwindowgeometry", window).decode(), flush=True)
                print(run("xwininfo", "-id", window).decode(), flush=True)
        print(run("xdotool", "getmouselocation").decode(), flush=True)
        screenshot("root", "timeout-desktop")
        raise RuntimeError(f"Timed out: {description}")

    def click(window, x, y):
        # Cross an intermediate point as a physical pointer would. After remap,
        # winit's X11 duplicate-position filter can suppress a one-step warp to
        # the exact pre-hide coordinate, leaving egui with the remap position.
        run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
            "mousemove", "--sync", "--window",
            window, str(x - 1), str(y), "mousemove_relative", "--sync", "1", "0",
            "sleep", ".2", "mousedown", "1", "sleep", ".2", "mouseup", "1")

    def screenshot(window, name):
        run("import", "-window", window, str(output / f"{name}.png"))

    def background(index):
        colors = [bytes(color) for color in BACKGROUNDS[index]]
        # Deliberately asymmetric quadrants, split at desktop pixel (300,270).
        pixels = (colors[0] * 300 + colors[1] * 980) * 270
        pixels += (colors[2] * 300 + colors[3] * 980) * 630
        path = output / f"background-{index}.ppm"
        path.write_bytes(b"P6\n1280 900\n255\n" + pixels)
        # Set an owned root pixmap: xsetroot's solid background is ignored by picom.
        run("hsetroot", "-fill", str(path))

    try:
        display = spawn("xvfb", ["Xvfb", "-displayfd", "1", "-screen", "0", "1280x900x24",
                                   "-dpi", "96", "-nolisten", "tcp"], announce=True)
        env["DISPLAY"] = f":{display}"
        address = spawn("dbus", ["dbus-daemon", "--session", "--nofork", "--print-address=1"], announce=True)
        env["DBUS_SESSION_BUS_ADDRESS"] = address
        env["DBUS_SYSTEM_BUS_ADDRESS"] = address
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

        cases = [("region", True, 0, False, False), ("region", True, 1, False, False),
                 ("window", True, 0, False, False), ("window", True, 1, False, False),
                 ("window", False, 0, False, False), ("window", True, 0, True, False),
                 ("window", True, 0, False, True)]
        for mode, freeze, countdown, occluded, auto_start in cases:
            prefix = f"{mode}-freeze-{freeze}-countdown-{countdown}-occluded-{occluded}-auto-{auto_start}"
            title = "Captures Capture Controls" if args.controls else f"Captures {mode.title()} Selection"
            fixture = cover = None
            if mode == "window":
                fixture = WindowFixture(env["DISPLAY"], "Captures X11 pixel fixture", 610, 290)
                fixtures.append(fixture)
                fixture.show()
                wait(lambda: windows("Captures X11 pixel fixture"), "fixture mapping")
                run("xdotool", "windowmove", str(fixture.window), "610", "290")
                if occluded:
                    cover = WindowFixture(env["DISPLAY"], "Captures X11 cover fixture",
                                          820, 390, 180, 110)
                    fixtures.append(cover)
                    cover.paint(1)
                    cover.show()
                    wait(lambda: windows("Captures X11 cover fixture"), "cover mapping")
                    run("xdotool", "windowmove", str(cover.window), "820", "390")
            history = output / prefix / "history"
            settings = output / f"{prefix}-settings.json"
            settings.write_text(json.dumps({
                "settings_schema_version": 5, "appearance": "dark", "theme": "mustard",
                "output_directory": str(output / prefix / "exports"),
                "new_capture_shortcut": "Ctrl+Shift+F10",
                "region_shortcut": "Ctrl+Shift+F7" if args.target_shortcuts else "Super+Shift+S",
                "window_shortcut": "Ctrl+Shift+F8" if args.target_shortcuts else "Alt+PrintScreen",
                "display_shortcut": "Ctrl+Shift+F9" if args.target_shortcuts else "Shift+PrintScreen",
                "launch_at_login": False,
                "auto_copy_to_clipboard": False, "auto_start_on_selection": auto_start,
                "freeze_screen": freeze, "show_cursor_in_screenshots": False,
                "screenshot_countdown_seconds": countdown,
            }))
            background(0)
            app = spawn(prefix, [str(binary), "--live", "--history-root", str(history),
                                 "--settings-file", str(settings), "--quit-after", "90"])
            root = wait(lambda: windows("Captures"), "capture workspace")[0]
            time.sleep(2)
            screenshot(root, f"{prefix}-workspace")
            checked_toolbar_drag = False

            def select_target(selector, target):
                if args.target_shortcuts:
                    key = {"region": "F7", "window": "F8", "display": "F9"}[target]
                    run("xdotool", "key", f"ctrl+shift+{key}", "sleep", ".2")
                    assert windows(title) == [selector], "target shortcut replaced or captured the selector"
                else:
                    click(selector, {"region": 492, "window": 568, "display": 653}[target], 811)

            def begin_selection(full_display=False):
                nonlocal checked_toolbar_drag
                click(root, 341 if args.controls else (575 if mode == "region" else 696), 141)
                selector = wait(lambda: windows(title), f"{mode} selector")[0]
                if windows("Captures"):
                    raise RuntimeError("capture workspace was not hidden")
                # Mapping precedes the first GL paint. Do not inject a complete
                # drag into an unpainted window during cold texture preparation.
                paint_crop = "1280x96+0+804" if mode == "region" or args.controls else "1280x120+0+0"
                wait(lambda: int(run("import", "-window", selector, "-crop", paint_crop,
                                     "-format", "%k", "info:")) > 16, "selector controls paint")
                if args.controls and mode == "region" and not checked_toolbar_drag:
                    previous = entries()
                    # The trailing footer is noninteractive. Drag to both
                    # bounds, then return to the original 26px-bottom position.
                    for start, end, label in [
                        ((1010, 850), (0, 0), "top-left"),
                        ((813, 79), (1279, 899), "bottom-right"),
                        ((1207, 859), (1010, 850), "restored"),
                    ]:
                        run("xdotool", "windowfocus", "--sync", selector,
                            "mousemove", "--window", selector, *map(str, start),
                            "sleep", ".2", "mousedown", "1", "sleep", ".2",
                            "mousemove", "--window", selector, *map(str, end),
                            "sleep", ".2", "mouseup", "1", "sleep", ".2")
                        screenshot(selector, f"{prefix}-toolbar-{label}")
                    run("xdotool", "key", "Return", "sleep", ".3")
                    assert windows(title) and entries() == previous, "toolbar drag created a region"
                    checked_toolbar_drag = True
                if args.target_shortcuts:
                    previous = entries()
                    run("xdotool", "key", "ctrl+shift+F10", "sleep", ".2")
                    assert windows(title) == [selector] and entries() == previous, "New Capture re-entered"
                    if auto_start:
                        select_target(selector, "display")
                        assert entries() == previous, "keyboard Full screen armed automatic capture"
                if args.controls and full_display:
                    select_target(selector, "display")
                    return selector
                if args.controls and mode == "window":
                    select_target(selector, "window")
                if mode == "region":
                    run("xdotool", "windowfocus", "--sync", selector, "mousemove", "--window",
                        selector, "140", "180", "sleep", ".1", "mousedown", "1", "sleep", ".2", "mousemove",
                        "--window", selector, "450", "350", "sleep", ".2", "mouseup", "1")
                elif not auto_start:
                    # This point belongs to the target even with the covering
                    # window present. Clicking must latch it before Enter.
                    click(selector, 660, 360)
                    time.sleep(.15)
                    assert selector in windows(title), "manual window selection closed before confirmation"
                else:
                    # Hover only: the capture loop will click after changing
                    # the underlying pixels. Cancellation tests must not click.
                    run("xdotool", "windowfocus", "--sync", selector, "mousemove",
                        "--window", selector, "660", "360", "sleep", ".2")
                if args.controls and not auto_start:
                    # Returning to each target must retain its settled choice.
                    # The independent pixel/size oracle below catches a reset
                    # or accidental capture of the temporary Full screen target.
                    select_target(selector, "display")
                    select_target(selector, mode)
                    if args.target_shortcuts and mode == "window":
                        # Keyboard Region/Display clear the settled window,
                        # unlike explicit toolbar target toggles.
                        previous = entries()
                        run("xdotool", "key", "Return", "sleep", ".3")
                        assert windows(title) == [selector] and entries() == previous
                        click(selector, 660, 360)
                        select_target(selector, "window")  # Same-target shortcut retains this choice.
                    # Release queues the target change; let the compositor
                    # present it before collecting the selection screenshot.
                    time.sleep(.2)
                return selector

            def entries():
                return {p for p in history.glob("*/metadata.json") if not p.parent.name.startswith(".")}

            for capture_index in (0, 1):
                captured = entries()
                capture_prefix = f"{prefix}-capture-{capture_index}"
                # Reverse A/B on the second capture to reject a reused session,
                # frozen texture or artifact from the first capture.
                background(capture_index)
                if fixture:
                    fixture.paint(capture_index)
                selector = begin_selection()
                screenshot(selector, f"{capture_prefix}-selection")
                if mode == "window" and not auto_start:
                    # Confirm the clicked target, not the later hovered desktop.
                    run("xdotool", "mousemove", "--window", selector, "100", "700", "sleep", ".3")
                    assert entries() == captured, "click captured despite automatic start being disabled"
                background(1 - capture_index)
                if fixture:
                    fixture.paint(1 - capture_index)
                if auto_start:
                    click(selector, 660, 360)  # Deliberately no Enter.
                else:
                    run("xdotool", "key", "Return")
                if countdown:
                    clock = wait(lambda: windows("Captures Screenshot Countdown"), "countdown")[0]
                    screenshot(clock, f"{capture_prefix}-clock")
                    if args.target_shortcuts:
                        # Once confirmed, target keys cannot change the target
                        # or queue another capture behind the existing flow.
                        run("xdotool", "key", "ctrl+shift+F9", "ctrl+shift+F8")

                new_entries = wait(lambda: entries() - captured, "persisted capture")
                wait(lambda: windows("Captures"), "workspace restored")
                assert len(new_entries) == 1 and len(entries()) == capture_index + 1
                metadata = new_entries.pop()
                entry = json.loads(metadata.read_text())
                size = (310, 170) if mode == "region" else (421, 237)
                assert (entry["mode"], entry["width"], entry["height"]) == (mode, *size), entry
                pixels = run("convert", str(metadata.parent / "capture.png"), "-depth", "8", "RGBA:-")
                # Frozen zero-delay keeps the pre-selection background; any
                # countdown must use the changed desktop.
                # An occluded frozen target cannot use the desktop crop; the
                # native-surface fallback necessarily reads the current client.
                source = 1 - capture_index if countdown or not freeze or occluded else capture_index
                colors = [bytes((*color, 255)) for color in BACKGROUNDS[source]]
                # Selection [140,180]→[450,350] crosses (300,270): 160/150 columns,
                # 90/80 rows. Shifted or transposed crops must fail too.
                left, right, top, bottom = (160, 150, 90, 80) if mode == "region" else (157, 264, 89, 148)
                expected = (colors[0] * left + colors[1] * right) * top
                expected += (colors[2] * left + colors[3] * right) * bottom
                assert pixels == expected, "wrong source/crop, cursor, or captured UI pixels"
                screenshot(root, f"{capture_prefix}-captured")

            saved_count = 2
            if fixture and freeze and not countdown and not occluded and not auto_start:
                fixture.hide()
                background(0)
                captured = entries()
                # Direct window picking falls back on empty desktop; unified
                # controls expose a dedicated Full screen target instead.
                selector = begin_selection(full_display=args.controls)
                screenshot(selector, f"{prefix}-display-selection")
                background(1)
                run("xdotool", "key", "Return")
                new_entries = wait(lambda: entries() - captured, "display capture from window picker")
                wait(lambda: windows("Captures"), "display capture workspace restored")
                assert len(new_entries) == 1
                metadata = new_entries.pop()
                entry = json.loads(metadata.read_text())
                assert (entry["mode"], entry["width"], entry["height"]) == ("display", 1280, 900), entry
                pixels = run("convert", str(metadata.parent / "capture.png"), "-depth", "8", "RGBA:-")
                colors = [bytes((*color, 255)) for color in BACKGROUNDS[0]]
                expected = (colors[0] * 300 + colors[1] * 980) * 270
                expected += (colors[2] * 300 + colors[3] * 980) * 630
                assert pixels == expected, "display fallback lost frozen pixels or captured selector UI"
                saved_count += 1
                assert len(entries()) == saved_count
                fixture.show()

            if fixture and countdown:
                begin_selection()
                run("xdotool", "key", "Return")
                wait(lambda: windows("Captures Screenshot Countdown"), "vanishing-target countdown")
                fixture.hide()
                wait(lambda: not windows("Captures Screenshot Countdown") and windows("Captures"),
                     "vanished-target failure restores workspace")
                assert len(entries()) == saved_count, "vanished target captured stale or replacement pixels"
                screenshot(root, f"{prefix}-vanished-target")
                fixture.show()

            begin_selection()
            focus = spawn(f"{prefix}-focus", ["xmessage", "-title", "Captures X11 focus fixture",
                                               "-geometry", "220x70+900+20", "Disposable keyboard focus"])
            other = wait(lambda: windows("Captures X11 focus fixture"), "other application")[0]
            run("xdotool", "windowfocus", "--sync", other)
            assert run("xdotool", "getwindowfocus").decode().strip() == other
            run("xdotool", "key", "Escape")
            wait(lambda: not windows(title) and windows("Captures"), "global Escape")
            assert len(entries()) == saved_count, "cancelled selection persisted a capture"
            focus.terminate()
            focus.wait(timeout=5)

            begin_selection()
            queries = saver.queries
            saver.locked = True
            wait(lambda: saver.queries > queries and not windows(title)
                 and windows("Captures"), "session-lock cancellation")
            saver.locked = False
            time.sleep(.5)
            assert not windows(title), "unlock resumed a cancelled selector"
            assert len(entries()) == saved_count, "locked capture persisted pixels"
            screenshot(root, f"{prefix}-cancelled")
            run("xdotool", "windowactivate", "--sync", root, "key", "alt+F4")
            assert app.wait(timeout=10) == 0
            assert len(entries()) == saved_count, "cancelled capture persisted during shutdown"
            for window_fixture in (cover, fixture):
                if window_fixture:
                    window_fixture.close()
                    fixtures.remove(window_fixture)
            print(f"PASS {prefix}: {saved_count} captures with exact pixels/metadata, cross-app Escape, lock/unlock, clean exit", flush=True)
        (output / "result.json").write_text(json.dumps({
            "passed": True, "screenSaverQueries": saver.queries,
            "scenarios": len(cases),
            "newCaptureControls": args.controls,
            "targetShortcuts": args.target_shortcuts,
            "savedCaptures": sum(1 for _ in output.glob("*/history/*/metadata.json")),
            "scope": "Real capture/persistence and X11 input on private Xvfb; simulated session state, software GL; not hardware, OS login/lock, Wayland or accessibility acceptance.",
        }, indent=2))
    finally:
        for fixture in fixtures:
            fixture.close()
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
