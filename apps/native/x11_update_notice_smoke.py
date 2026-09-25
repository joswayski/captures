#!/usr/bin/env python3
"""Update notice fixture on private X11, driven by the shared stub status source.

No updater, download, install, relaunch or URL open happens: the workbench
reports link actions as events. Requires Xvfb, Openbox, xdotool and ImageMagick.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time

CARD_WIDTH = 400
FRAME_PAD = 28
CARET = 8
STILLS = {
    "available": ("Update available", 434),
    "single": ("Update available", 290),
    "closing": ("Update available", 480),
    "manual": ("Update available", 290),
    "downloading": ("Updating Captures", 224),
    "restarting": ("Updated", 224),
    "error": ("Update failed", 264),
    "checking": ("Checking for updates", 224),
    "up-to-date": ("You’re up to date", 224),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = {**os.environ, "WGPU_BACKEND": "gl", "WINIT_X11_SCALE_FACTOR": "1", "XDG_SESSION_TYPE": "x11"}
    env.pop("WAYLAND_DISPLAY", None)
    children = []
    with (output / "processes.log").open("w") as log:
        def spawn(command, announce=False):
            child = subprocess.Popen(command, env=env, stdout=subprocess.PIPE if announce else log, stderr=log)
            children.append(child)
            return child

        def run(*command):
            return subprocess.check_output(command, env=env, stderr=log, timeout=15)

        def wait(predicate, description, timeout=20):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if result := predicate():
                    return result
                time.sleep(.05)
            run("import", "-window", "root", str(output / "timeout.png"))
            raise AssertionError(description)

        class App:
            def __init__(self, name, *extra):
                self.name = name
                self.events = queue.Queue()
                self.seen = []
                self.process = spawn([str(binary), "--scene", "update", "--quit-after", "90", *extra], True)
                threading.Thread(target=self.read, daemon=True).start()

            def read(self):
                for line in self.process.stdout:
                    try:
                        self.events.put(json.loads(line))
                    except json.JSONDecodeError:
                        pass

            def event(self, name, predicate=lambda detail: True, timeout=20):
                deadline = time.monotonic() + timeout
                while time.monotonic() < deadline:
                    for event in self.seen:
                        if event["event"] == name and predicate(event["detail"]):
                            self.seen.remove(event)
                            return event["detail"]
                    try:
                        self.seen.append(self.events.get(timeout=.1))
                    except queue.Empty:
                        pass
                run("import", "-window", "root", str(output / f"{self.name}-timeout.png"))
                raise AssertionError(f"{self.name}: no {name} event; saw {[e['event'] for e in self.seen]}")

            def report(self, title=None):
                return self.event("update-notice", lambda d: d is not None and (title is None or d["title"] == title))

            def windows(self):
                result = subprocess.run(["xdotool", "search", "--all", "--onlyvisible", "--pid", str(self.process.pid),
                                         "--name", "^Captures Update$"], env=env, capture_output=True, text=True, timeout=5)
                assert result.returncode in (0, 1)
                return result.stdout.splitlines()

            def window(self):
                return wait(self.windows, f"{self.name}: notice window")[0]

            def size(self, window):
                shell = dict(line.split("=", 1) for line in run("xdotool", "getwindowgeometry", "--shell", window).decode().split())
                return int(shell["WIDTH"]), int(shell["HEIGHT"])

            def wait_size(self, window, height):
                expected = (CARD_WIDTH + FRAME_PAD * 2, height)
                return wait(lambda: self.size(window) == expected or None,
                            f"{self.name}: window size {self.size(window)} != {expected}")

            def click(self, window, x, y):
                run("xdotool", "windowactivate", "--sync", window, "mousemove", "--sync", "--window", window,
                    str(x - 1), str(y), "mousemove_relative", "--sync", "1", "0", "sleep", ".15",
                    "mousedown", "1", "sleep", ".1", "mouseup", "1")

            def key(self, window, key):
                run("xdotool", "windowactivate", "--sync", window, "key", "--clearmodifiers", key)

            def shot(self, window, name):
                time.sleep(.4)
                run("import", "-window", window, str(output / f"{name}.png"))

            def close(self):
                if self.process.poll() is None:
                    self.process.terminate()
                self.process.wait(timeout=10)

        def window_height(card_height, caret=True):
            return card_height + FRAME_PAD + (CARET if caret else FRAME_PAD)

        try:
            server = spawn(["Xvfb", "-displayfd", "1", "-screen", "0", "1280x900x24", "-nolisten", "tcp"], True)
            env["DISPLAY"] = ":" + server.stdout.readline().decode().strip()
            run("xdpyinfo")
            spawn(["openbox", "--sm-disable"])
            wait(lambda: b"window id" in run("xprop", "-root", "_NET_SUPPORTING_WM_CHECK"), "window manager ready")

            # Every shipping state renders with shared copy and the shipping card height.
            stills = [(state, "dark", "top") for state in STILLS]
            stills += [("available", "light", "top"), ("error", "light", "top"),
                       ("single", "dark", "bottom"), ("single", "dark", "none")]
            for state, appearance, tray in stills:
                name = f"{state}-{appearance}-{tray}"
                app = App(name, "--update-state", state, "--appearance", appearance, "--update-tray", tray)
                title, card = STILLS[state]
                report = app.report(title)
                assert report["cardHeight"] == card, (name, report)
                expected_caret = "none" if tray == "none" else tray
                assert report["caret"] == expected_caret, (name, report)
                window = app.window()
                app.wait_size(window, window_height(card, tray != "none"))
                app.shot(window, name)
                app.close()
                print(f"PASS still {name}: {title!r}, {card}px card, caret {expected_caret}", flush=True)

            # Hide / What's new persist and resize; Update now simulates the
            # download and restart; Escape is ignored while busy.
            app = App("flow", "--update-state", "available")
            report = app.report("Update available")
            assert report["notes"]["stacked"] and report["notes"]["groups"] == ["2026.08.27.5", "2026.08.27.3"], report
            assert report["footer"] == {"dismiss": "Later", "primary": "Update now", "enabled": True}, report
            window = app.window()
            app.wait_size(window, window_height(434))
            app.click(window, 346, 153)  # "#265" in the first stacked group.
            assert app.event("update-notice-action")["action"] == "open_pull_request"
            opened = app.event("update-notice-open-url")
            assert opened == {"kind": "pull_request", "url": "https://github.com/joswayski/captures/pull/265",
                              "opened": False}, opened
            # The notes box starts under the 64px header; Hide sits at its top right.
            app.click(window, FRAME_PAD + CARD_WIDTH - 16 - 12 - 14, CARET - 1 + 67 + 12 + 8)
            assert app.event("update-notice-action")["action"] == "hide_notes"
            report = app.report()
            assert report["notes"] is None and report["revealNotes"] and report["cardHeight"] == 168, report
            app.wait_size(window, window_height(168))
            app.shot(window, "flow-compact")
            app.click(window, FRAME_PAD + CARD_WIDTH - 16 - 36, CARET - 1 + 16 + 18)
            assert app.event("update-notice-action")["action"] == "show_notes"
            report = app.report()
            assert report["notes"] is not None and report["cardHeight"] == 434, report
            app.wait_size(window, window_height(434))
            height = window_height(434)
            app.click(window, FRAME_PAD + CARD_WIDTH - 16 - 52, height - FRAME_PAD - 16 - 16)
            assert app.event("update-notice-action")["action"] == "install"
            report = app.report("Updating Captures")
            assert report["dismissBlocked"] and report["footer"] is None, report
            app.wait_size(window, window_height(224))
            app.key(window, "Escape")
            assert app.event("update-notice-action")["action"] == "dismiss"
            app.event("update-notice-dismiss-blocked")
            assert app.windows(), "Escape dismissed a busy notice"
            app.shot(window, "flow-downloading")
            report = app.report("Updated")
            assert report["restart"] == "Reopening in 3 seconds…", report
            app.shot(window, "flow-restarting")
            app.event("update-notice-restart-simulated", timeout=10)
            wait(lambda: not app.windows(), "restart simulation left the notice open")
            app.close()
            print("PASS flow: PR link reported, hide/show notes, stub install progress, busy Escape, restart countdown", flush=True)

            # Error: the download link is reported (not opened), Try again
            # reinstalls, and Escape dismisses a notice that is not busy.
            app = App("error", "--update-state", "error")
            report = app.report("Update failed")
            assert report["footer"] == {"dismiss": "Close", "primary": "Try again", "enabled": True}, report
            window = app.window()
            app.wait_size(window, window_height(264))
            app.key(window, "Escape")
            app.event("update-notice-dismissed")
            wait(lambda: not app.windows(), "Escape left an idle notice open")
            app.close()
            app = App("retry", "--update-state", "error")
            app.report("Update failed")
            window = app.window()
            app.wait_size(window, window_height(264))
            app.click(window, 178, 141)  # "download from captur.es".
            assert app.event("update-notice-action")["action"] == "open_download_page"
            opened = app.event("update-notice-open-url")
            assert opened["kind"] == "download" and not opened["opened"], opened
            height = window_height(264)
            app.click(window, FRAME_PAD + CARD_WIDTH - 16 - 52, height - FRAME_PAD - 16 - 16)
            assert app.event("update-notice-action")["action"] == "install"
            app.report("Updating Captures")
            app.close()
            print("PASS error: Escape dismisses, download link reported, Try again reinstalls through the stub", flush=True)

            (output / "acceptance.json").write_text(json.dumps({
                "passed": True, "stills": [f"{s}-{a}-{t}" for s, a, t in stills],
                "updaterConnected": False,
                "scope": "Private X11/software GL; stub status source; no hardware/AT acceptance.",
            }, indent=2))
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()


if __name__ == "__main__":
    main()
