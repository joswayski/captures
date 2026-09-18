#!/usr/bin/python3
"""Real X11 region capture against a private desktop and simulated session state.

Requires system Python's dbus/gi, Xvfb, Openbox, picom, xdotool, hsetroot,
xmessage and ImageMagick. Never uses the caller's display or D-Bus session.
No production capture/session bypass exists in the application.
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


BACKGROUNDS = (
    ((36, 52, 68), (173, 114, 39), (29, 99, 144), (90, 38, 118)),
    ((113, 40, 60), (27, 134, 93), (180, 91, 35), (71, 53, 149)),
)


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
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    children, logs = [], []
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
        for title in ("Captures", "Captures Region Selection", "Captures Screenshot Countdown"):
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
        run("xdotool", "windowactivate", "--sync", window, "mousemove", "--sync", "--window",
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

        for countdown in (0, 1):
            prefix = f"countdown-{countdown}"
            history = output / prefix / "history"
            settings = output / f"{prefix}-settings.json"
            settings.write_text(json.dumps({
                "settings_schema_version": 5, "appearance": "dark", "theme": "mustard",
                "output_directory": str(output / prefix / "exports"),
                "region_shortcut": "Super+Shift+S", "window_shortcut": "Alt+PrintScreen",
                "display_shortcut": "Shift+PrintScreen", "launch_at_login": False,
                "auto_copy_to_clipboard": False, "auto_start_on_selection": False,
                "freeze_screen": True, "show_cursor_in_screenshots": False,
                "screenshot_countdown_seconds": countdown,
            }))
            background(0)
            app = spawn(prefix, [str(binary), "--live", "--history-root", str(history),
                                 "--settings-file", str(settings), "--quit-after", "90"])
            root = wait(lambda: windows("Captures"), "capture workspace")[0]
            time.sleep(2)
            screenshot(root, f"{prefix}-workspace")

            def begin_region():
                click(root, 467, 141)
                selector = wait(lambda: windows("Captures Region Selection"), "region selector")[0]
                if windows("Captures"):
                    raise RuntimeError("capture workspace was not hidden")
                # Mapping precedes the first GL paint. Do not inject a complete
                # drag into an unpainted window during cold texture preparation.
                wait(lambda: int(run("import", "-window", selector, "-crop", "336x52+472+824",
                                     "-format", "%k", "info:")) > 16, "selector toolbar paint")
                run("xdotool", "windowfocus", "--sync", selector, "mousemove", "--window",
                    selector, "140", "180", "sleep", ".1", "mousedown", "1", "sleep", ".2", "mousemove",
                    "--window", selector, "450", "350", "sleep", ".2", "mouseup", "1")
                return selector

            def entries():
                return {p for p in history.glob("*/metadata.json") if not p.parent.name.startswith(".")}

            for capture_index in (0, 1):
                captured = entries()
                capture_prefix = f"{prefix}-capture-{capture_index}"
                # Reverse A/B on the second capture to reject a reused session,
                # frozen texture or artifact from the first capture.
                background(capture_index)
                selector = begin_region()
                screenshot(selector, f"{capture_prefix}-selection")
                background(1 - capture_index)
                run("xdotool", "key", "Return")
                if countdown:
                    clock = wait(lambda: windows("Captures Screenshot Countdown"), "countdown")[0]
                    screenshot(clock, f"{capture_prefix}-clock")

                new_entries = wait(lambda: entries() - captured, "persisted capture")
                wait(lambda: windows("Captures"), "workspace restored")
                assert len(new_entries) == 1 and len(entries()) == capture_index + 1
                metadata = new_entries.pop()
                entry = json.loads(metadata.read_text())
                assert (entry["mode"], entry["width"], entry["height"]) == ("region", 310, 170), entry
                pixels = run("convert", str(metadata.parent / "capture.png"), "-depth", "8", "RGBA:-")
                # Frozen zero-delay keeps the pre-selection background; any
                # countdown must use the changed desktop.
                source = 1 - capture_index if countdown else capture_index
                colors = [bytes((*color, 255)) for color in BACKGROUNDS[source]]
                # Selection [140,180]→[450,350] crosses (300,270): 160/150 columns,
                # 90/80 rows. Shifted or transposed crops must fail too.
                expected = (colors[0] * 160 + colors[1] * 150) * 90
                expected += (colors[2] * 160 + colors[3] * 150) * 80
                assert pixels == expected, "wrong source/crop, cursor, or captured UI pixels"
                screenshot(root, f"{capture_prefix}-captured")

            begin_region()
            focus = spawn(f"{prefix}-focus", ["xmessage", "-title", "Captures X11 focus fixture",
                                               "-geometry", "220x70+900+20", "Disposable keyboard focus"])
            other = wait(lambda: windows("Captures X11 focus fixture"), "other application")[0]
            run("xdotool", "windowfocus", "--sync", other)
            assert run("xdotool", "getwindowfocus").decode().strip() == other
            run("xdotool", "key", "Escape")
            wait(lambda: not windows("Captures Region Selection") and windows("Captures"), "global Escape")
            assert len(entries()) == 2, "cancelled selection persisted a capture"
            focus.terminate()
            focus.wait(timeout=5)

            begin_region()
            queries = saver.queries
            saver.locked = True
            wait(lambda: saver.queries > queries and not windows("Captures Region Selection")
                 and windows("Captures"), "session-lock cancellation")
            saver.locked = False
            time.sleep(.5)
            assert not windows("Captures Region Selection"), "unlock resumed a cancelled selector"
            assert len(entries()) == 2, "locked capture persisted pixels"
            screenshot(root, f"{prefix}-cancelled")
            run("xdotool", "windowactivate", "--sync", root, "key", "alt+F4")
            assert app.wait(timeout=10) == 0
            assert len(entries()) == 2, "cancelled capture persisted during shutdown"
            print(f"PASS {prefix}: two captures with exact pixels/metadata, cross-app Escape, lock/unlock, clean exit", flush=True)
        (output / "result.json").write_text(json.dumps({
            "passed": True, "screenSaverQueries": saver.queries,
            "scope": "Real capture/persistence and X11 input on private Xvfb; simulated session state, software GL; not hardware, OS login/lock, Wayland or accessibility acceptance.",
        }, indent=2))
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
