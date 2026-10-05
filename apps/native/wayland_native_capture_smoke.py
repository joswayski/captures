#!/usr/bin/python3
"""Native screenshot → portal → History on disposable headless Wayland.

Real portal acquisition plus cancelled/failed responses. Desktop session state
is a private D-Bus fixture, not physical session-lock acceptance.
"""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import sys
import tempfile
import threading
import time

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

from wayland_screenshot_smoke import DESKTOP, ready, rgba, stop, wait_owner
from x11_capture_smoke import ScreenSaver

WIDTH, HEIGHT = 1280, 900
EXPECTED = bytes((35, 69, 103, 255)) * (WIDTH * HEIGHT)


def wait(predicate, description, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.1)
    raise RuntimeError(f"Timed out: {description}")


def windows(env):
    tree = json.loads(subprocess.check_output(["swaymsg", "-t", "get_tree"], env=env))
    def leaves(node):
        for child in node.get("nodes", []) + node.get("floating_nodes", []):
            yield from leaves(child)
        if node.get("pid") is not None:
            yield node
    return list(leaves(tree))


def host_cases(binary, pointer, root, env, bus, front, store, saver, screenshots):
    def click(x, y):
        pointer.stdin.write(f"{x} {y} {WIDTH} {HEIGHT}\n".encode())
        pointer.stdin.flush()
        assert select.select([pointer.stdout], [], [], 5)[0], "input probe did not finish"
        assert pointer.stdout.readline().strip() == b"CLICKED"

    store.SetPermission("screenshot", True, "screenshot", "", ["yes"])
    for mode, appearance in (("real", "dark"), ("real", "light"), ("cancel", "dark"),
                             ("failure", "light"), ("lock", "dark"), ("quit", "light")):
        if mode == "cancel":
            stop(front)
            wait(lambda: not bus.name_has_owner(DESKTOP), "real portal released its bus name")
        profile = root / f"{mode}-{appearance}"
        profile.mkdir()
        saver.locked = False
        source = profile / "editor-source.png"
        subprocess.run(["convert", "-size", "37x23", "xc:#81426b", str(source)], check=True)
        original = source.read_bytes()
        fixture_log = profile / "portal.jsonl"
        fixture = None
        if mode != "real":
            fixture = subprocess.Popen([sys.executable, str(Path(__file__).with_name("wayland_screenshot_smoke.py")),
                                        "--fixture", "wait" if mode in ("lock", "quit") else mode,
                                        "--uri", source.as_uri(), "--log", str(fixture_log)],
                                       env=env, stdout=subprocess.PIPE)
            ready(fixture)
        settings = profile / "settings.json"
        settings.write_text(json.dumps({"onboarding_completed": True, "settings_schema_version": 5,
                                       "appearance": appearance, "auto_copy_to_clipboard": False,
                                       "launch_at_login": False, "screenshot_countdown_seconds": 0,
                                       "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
                                       "display_shortcut": "Ctrl+Shift+F9",
                                       "output_directory": str(profile / "exports")}))
        log = (profile / "host.log").open("w")
        app = subprocess.Popen([binary, "--live", "--open-history", "--open-preferences",
                                "--open-media", str(source),
                                "--settings-file", str(settings), "--history-root", str(profile / "history"),
                                "--quit-after", "12" if mode == "quit" else "60"],
                               env=env, stdout=log, stderr=log)
        try:
            def shown():
                assert app.poll() is None, (profile / "host.log").read_text()
                return {node["name"] for node in windows(env)} >= {
                    "Capture History", "Captures Preferences", "Captures Screenshot Editor"}
            wait(shown, "History, Preferences and editor mapped")
            time.sleep(1)
            def shot(name):
                path = (screenshots if screenshots else profile) / f"native-wayland-{mode}-{appearance}-{name}.png"
                subprocess.run(["grim", str(path)], env=env, check=True, timeout=5)
                return path
            def layouts(name):
                events = [json.loads(line) for line in (profile / "host.log").read_text().splitlines()
                          if line.startswith("{") and line.endswith("}")]
                return [event["detail"] for event in events if event["event"] == name]
            def button():
                data = layouts("portal-screenshot-layout")
                # Ignore a cached layout from before Sway's resize configure.
                return (data[-1]["button"] if data and data[-1]["enabled"]
                        and data[-1]["viewport_size"] == [880, 640] else None)
            def card_action(artifact_id, action):
                data = [item for item in layouts("history-action-layout")
                        if item["id"] == artifact_id and item["action"] == action]
                return data[-1] if data else None
            def focus_history():
                history = next(node for node in windows(env) if node["name"] == "Capture History")
                # Wayland does not promise the old position after unmapping.
                # Arrange the private scene for input and readable review captures.
                subprocess.run(["swaymsg", f'[con_id={history["id"]}] floating enable, '
                                'resize set 880 640, move position 20 20, focus'],
                               env=env, check=True, stdout=subprocess.DEVNULL)
                time.sleep(.5)
                return next(node for node in windows(env) if node["name"] == "Capture History")
            history = focus_history()
            x1, y1, x2, y2 = wait(button, "enabled screenshot button")
            shot("before")
            rect = history["rect"]
            before = set((profile / "history").glob("*/metadata.json"))
            assert len(before) == 1, "The editor import must be ready before capturing"
            click(int(rect["x"] + (x1 + x2) / 2), int(rect["y"] + (y1 + y2) / 2))
            def requests():
                return [json.loads(line) for line in fixture_log.read_text().splitlines()] if fixture_log.exists() else []
            if mode == "real":
                entries = wait(lambda: set((profile / "history").glob("*/metadata.json")) - before,
                               "portal screenshot saved")
                assert len(entries) == 1
                captured = next(iter(entries)).parent / "capture.png"
                pixels = rgba(captured)
                if pixels != EXPECTED and screenshots:
                    shutil.copyfile(captured, screenshots / f"native-wayland-{appearance}-wrong-pixels.png")
                assert pixels == EXPECTED, "Captured app windows or wrong desktop pixels"
            else:
                wait(lambda: requests(), "host invoked the private portal")
                assert len([event for event in requests() if event["event"] == "request"]) == 1
                if mode in ("lock", "quit"):
                    wait(lambda: not windows(env), "all three app windows unmapped")
                    assert rgba(shot("pending")) == EXPECTED
                    if mode == "lock":
                        saver.locked = True
                    else:
                        # Forwarded media must remain queued, without remapping an
                        # editor or root over the desktop while capture is pending.
                        subprocess.run([binary, "--live", "--settings-file", str(settings),
                                        "--history-root", str(profile / "history"), "--open-media", str(source)],
                                       env=env, check=True, timeout=10, stdout=subprocess.DEVNULL)
                        time.sleep(.5)
                        assert not windows(env), "forwarded media remapped capture-excluded windows"
                        assert app.wait(timeout=20) == 0, "deadline-driven normal quit failed"
                    wait(lambda: any(event["event"] == "close" for event in requests()), "pending Request.Close")
            if mode == "failure":
                dialog = wait(lambda: next((node for node in windows(env) if node["name"] == "Captures"), None),
                              "portal failure dialog")
                subprocess.run(["swaymsg", f'[con_id={dialog["id"]}] move position 450 180, focus'],
                               env=env, check=True, stdout=subprocess.DEVNULL)
                # The tree reports mapping before the compositor presents the
                # first frame. Keep the dialog above the restored editor for review.
                time.sleep(.5)
                shot("failure-dialog")
                rect = next(node for node in windows(env) if node["id"] == dialog["id"])["rect"]
                click(rect["x"] + rect["width"] - 60, rect["y"] + rect["height"] - 36)
                wait(lambda: not any(node["name"] == "Captures" for node in windows(env)), "OK dismisses failure")
            if mode != "quit":
                wait(shown, "History, Preferences and editor restored")
                history = focus_history()
                wait(button, "capture released the busy gate at the restored size")
            if mode == "real":
                # Exercise the restored surfaces again, not merely their map state.
                before_repeat = set((profile / "history").glob("*/metadata.json"))
                x1, y1, x2, y2 = wait(button, "enabled repeated screenshot")
                rect = history["rect"]
                click(int(rect["x"] + (x1 + x2) / 2), int(rect["y"] + (y1 + y2) / 2))
                entries = wait(lambda: set((profile / "history").glob("*/metadata.json")) - before_repeat,
                               "repeated portal screenshot saved")
                assert len(entries) == 1
                assert rgba(next(iter(entries)).parent / "capture.png") == EXPECTED
                wait(shown, "all three windows restored after repeated capture")
                history = focus_history()
                wait(button, "repeated capture released the busy gate at the restored size")
                artifact_id = json.loads(next(iter(entries)).read_text())["id"]
                restore = wait(lambda: card_action(artifact_id, "Restore"), "portal screenshot Restore state")
                assert not restore["enabled"], "Wayland floating-preview Restore must be visibly unavailable"
                edit = wait(lambda: card_action(artifact_id, "Edit"), "portal screenshot Edit action")
                assert edit["enabled"], "Editing must not depend on unavailable preview placement"
                previous_editors = {node["id"] for node in windows(env) if node["name"] == "Captures Screenshot Editor"}
                x1, y1, x2, y2 = edit["rect"]
                rect = history["rect"]
                click(int(rect["x"] + (x1 + x2) / 2), int(rect["y"] + (y1 + y2) / 2))
                editor = wait(lambda: next((node for node in windows(env)
                                           if node["name"] == "Captures Screenshot Editor"
                                           and node["id"] not in previous_editors), None),
                              "History Edit opened the new portal screenshot")
                # A focused tiled window stays below floating History in Sway.
                # Arrange this review state above the restored fixture windows.
                subprocess.run(["swaymsg", f'[con_id={editor["id"]}] floating enable, '
                                'resize set 1160 820, move position 60 40, focus'],
                               env=env, check=True, stdout=subprocess.DEVNULL)
                time.sleep(1)
                shot("editor")
                focus_history()
            if mode != "real":
                assert set((profile / "history").glob("*/metadata.json")) == before, "cancel/failure persisted media"
            if mode in ("cancel", "failure", "lock"):
                # The next request must succeed in the same process after every
                # terminal non-success, not just display an enabled-looking button.
                saver.locked = False
                stop(fixture)
                wait(lambda: not bus.name_has_owner(DESKTOP), "cancelled portal fixture released its name")
                fixture = subprocess.Popen([sys.executable, str(Path(__file__).with_name("wayland_screenshot_smoke.py")),
                                            "--fixture", "success", "--uri", source.as_uri(), "--log", str(fixture_log)],
                                           env=env, stdout=subprocess.PIPE)
                ready(fixture)
                history = focus_history()
                x1, y1, x2, y2 = wait(button, "enabled recovered screenshot")
                rect = history["rect"]
                click(int(rect["x"] + (x1 + x2) / 2), int(rect["y"] + (y1 + y2) / 2))
                entries = wait(lambda: set((profile / "history").glob("*/metadata.json")) - before,
                               "screenshot after recovery")
                assert len(entries) == 1
                assert rgba(next(iter(entries)).parent / "capture.png") == bytes((129, 66, 107, 255)) * (37 * 23)
                wait(shown, "all three windows restored after recovery")
                focus_history()
                wait(button, "recovered capture released the busy gate")
            assert source.read_bytes() == original
            shot("history")
            print(json.dumps({"native_host": appearance, "case": mode,
                              "excluded_windows": ["Capture History", "Preferences", "Screenshot Editor"],
                              "exact_pixels": WIDTH * HEIGHT * 2 if mode == "real" else 0,
                              "restored": mode != "quit", "initial_failure_artifacts": 0 if mode != "real" else None,
                              "new_artifacts": 2 if mode == "real" else 0 if mode == "quit" else 1}), flush=True)
        except Exception:
            log.flush()
            print((profile / "host.log").read_text(errors="replace"), file=sys.stderr)
            print(json.dumps(windows(env)), file=sys.stderr)
            if screenshots:
                subprocess.run(["grim", str(screenshots / f"native-wayland-{appearance}-failed.png")], env=env, check=True)
            raise
        finally:
            stop(app)
            log.close()
            if fixture:
                stop(fixture)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--injector", type=Path, required=True)
    parser.add_argument("--screenshots", type=Path)
    parser.add_argument("--isolated", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    binary, injector = str(args.binary.resolve()), str(args.injector.resolve())
    screenshots = args.screenshots.resolve() if args.screenshots else None
    if screenshots:
        screenshots.mkdir(parents=True, exist_ok=True)
    if not args.isolated:
        # Old wlr backends write /tmp/out.png: isolate it, then drop privileges.
        command = ["sudo", "unshare", "--mount", "--propagation", "private", "sh", "-eu", "-c",
                   'mount -t tmpfs tmpfs /tmp; chmod 1777 /tmp; exec setpriv --reuid="$1" --regid="$2" --init-groups env HOME="$3" "$4" "$5" --isolated --binary "$6" --injector "$7" ${8:+--screenshots} ${8:+"$8"}',
                   "sh", str(os.getuid()), str(os.getgid()), str(Path.home()), sys.executable,
                   str(Path(__file__).resolve()), binary, injector, str(screenshots) if screenshots else ""]
        subprocess.run(command, check=True)
        return
    with tempfile.TemporaryDirectory(prefix="captures-native-wayland-") as temporary:
        root = Path(temporary)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        env = os.environ.copy()
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "SWAYSOCK", "DBUS_SESSION_BUS_ADDRESS", "XDG_DESKTOP_PORTAL_DIR"):
            env.pop(key, None)
        env.update(XDG_RUNTIME_DIR=str(runtime), XDG_DATA_HOME=str(root / "data"), XDG_CONFIG_HOME=str(root / "config"),
                   XDG_CACHE_HOME=str(root / "cache"), XDG_CURRENT_DESKTOP="sway", XDG_SESSION_TYPE="wayland",
                   GDK_BACKEND="wayland", NO_AT_BRIDGE="1", WLR_BACKENDS="headless", WLR_HEADLESS_OUTPUTS="1",
                   WLR_LIBINPUT_NO_DEVICES="1", WLR_RENDERER="pixman", WGPU_BACKEND="gl",
                   CAPTURES_NATIVE_LAYOUT_PROBE="1",
                   CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER="1")
        services = []
        log = (root / "services.log").open("w")
        daemon = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1"], env=env, stdout=subprocess.PIPE)
        services.append(daemon)
        env["DBUS_SESSION_BUS_ADDRESS"] = daemon.stdout.readline().decode().strip()
        env["DBUS_SYSTEM_BUS_ADDRESS"] = env["DBUS_SESSION_BUS_ADDRESS"]
        DBusGMainLoop(set_as_default=True)
        bus = dbus.bus.BusConnection(env["DBUS_SESSION_BUS_ADDRESS"])
        saver_name = dbus.service.BusName("org.freedesktop.ScreenSaver", bus=bus)
        saver = ScreenSaver(saver_name, "/org/freedesktop/ScreenSaver")
        loop = GLib.MainLoop()
        thread = threading.Thread(target=loop.run, daemon=True)
        thread.start()
        config = root / "sway.conf"
        config.write_text(f'output HEADLESS-1 resolution {WIDTH}x{HEIGHT}\noutput * bg #234567 solid_color\n'
                          'seat seat0 fallback true\ndefault_border none\ndefault_floating_border none\n'
                          # The Screenshot portal controls cursor inclusion. Hide
                          # this fixture's idle pointer before comparing desktop pixels.
                          'seat seat0 hide_cursor 100\n'
                          'for_window [title="Capture History"] floating enable, resize set 880 640, move position 20 20\n'
                          'for_window [title="Preferences"] floating enable, resize set 600 560, move position 650 300\n'
                          'for_window [title="^Captures$"] floating enable\n')
        sway = subprocess.Popen(["sway", "--unsupported-gpu", "--config", str(config)], env=env, stdout=log, stderr=log)
        services.append(sway)
        try:
            sockets = wait(lambda: [path for path in runtime.glob("wayland-*") if path.is_socket()], "Wayland socket")
            env["WAYLAND_DISPLAY"] = sockets[0].name
            env["SWAYSOCK"] = str(wait(lambda: list(runtime.glob("sway-ipc.*.sock")), "Sway IPC")[0])
            pipewire = subprocess.Popen(["pipewire"], env=env, stdout=log, stderr=log)
            services.append(pipewire)
            wait(lambda: (runtime / "pipewire-0").is_socket(), "PipeWire socket")
            for executable, name in (("xdg-permission-store", "org.freedesktop.impl.portal.PermissionStore"),
                                     ("xdg-desktop-portal-gtk", "org.freedesktop.impl.portal.desktop.gtk"),
                                     ("xdg-desktop-portal-wlr", "org.freedesktop.impl.portal.desktop.wlr"),
                                     ("xdg-desktop-portal", DESKTOP)):
                service = subprocess.Popen([f"/usr/libexec/{executable}"], env=env, stdout=log, stderr=log)
                services.append(service)
                wait_owner(bus, name, service)
            store = dbus.Interface(bus.get_object("org.freedesktop.impl.portal.PermissionStore",
                                                  "/org/freedesktop/impl/portal/PermissionStore"),
                                   "org.freedesktop.impl.portal.PermissionStore")
            front = services[-1]
            # Keep seat pointer capability stable before any native client starts.
            # wlroots 0.15.1 has an inert-relative-pointer crash on device removal.
            pointer = subprocess.Popen([injector, "pointer"], env=env,
                                       stdin=subprocess.PIPE, stdout=subprocess.PIPE)
            services.append(pointer)
            ready(pointer)
            host_cases(binary, pointer, root, env, bus, front, store, saver, screenshots)
        except Exception:
            log.flush()
            print((root / "services.log").read_text(errors="replace"), file=sys.stderr)
            raise
        finally:
            loop.quit()
            thread.join(timeout=3)
            for service in reversed(services):
                stop(service)
            # A D-Bus-activated document portal may outlive the front end.
            # Detach its disposable FUSE mount before traversing the temp tree.
            subprocess.run(["fusermount3", "-uz", str(runtime / "doc")],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            log.close()


if __name__ == "__main__":
    main()
