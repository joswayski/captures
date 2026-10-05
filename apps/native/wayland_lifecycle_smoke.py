#!/usr/bin/python3
"""Native resident lifecycle on private headless Sway and its real SNI tray.

No capture permissions, installed data or real login registration are touched.
"""
import argparse
import json
import os
from pathlib import Path
import select
import shlex
import signal
import socket
import struct
import subprocess
import tempfile
import threading
import time

import dbus

from instance_smoke import png
from wayland_native_capture_smoke import wait, windows
from wayland_screenshot_smoke import ready, stop


WATCHER = "org.kde.StatusNotifierWatcher"
HISTORY = "Capture History"
PREFERENCES = "Captures Preferences"
EDITOR = "Captures Screenshot Editor"
RECORDING_EDITOR = "Captures Editor"


def watcher(bus):
    if not bus.name_has_owner(WATCHER):
        return None
    try:
        proxy = dbus.Interface(bus.get_object(WATCHER, "/StatusNotifierWatcher"),
                               "org.freedesktop.DBus.Properties")
        return proxy if proxy.Get(WATCHER, "IsStatusNotifierHostRegistered") else None
    except dbus.exceptions.DBusException as error:
        if error.get_dbus_name() in ("org.freedesktop.DBus.Error.ServiceUnknown",
                                    "org.freedesktop.DBus.Error.NameHasNoOwner"):
            return None  # The real tray can exit between the owner/property calls.
        raise


def menu_action(bus, label):
    items = watcher(bus).Get(WATCHER, "RegisteredStatusNotifierItems")
    assert len(items) == 1, items
    item = str(items[0])
    destination, separator, path = item.partition("/")
    proxy = bus.get_object(destination, "/" + path if separator else "/StatusNotifierItem")
    menu_path = proxy.Get("org.kde.StatusNotifierItem", "Menu",
                          dbus_interface="org.freedesktop.DBus.Properties")
    menu = dbus.Interface(bus.get_object(destination, menu_path), "com.canonical.dbusmenu")
    _, layout = menu.GetLayout(dbus.Int32(0), dbus.Int32(-1), ["label", "enabled"])
    def find(node):
        identifier, properties, children = node
        if properties.get("label") == label:
            assert properties.get("enabled", True), label
            return identifier
        for child in children:
            found = find(child)
            if found is not None:
                return found
        return None
    identifier = find(layout)
    assert identifier is not None, f"No real tray menu action: {label}"
    menu.Event(dbus.Int32(identifier), "clicked", dbus.Int32(0),
               dbus.UInt32(int(time.monotonic() * 1000) % 2**32))


def cases(binary, root, env, bus, screenshots, pointer):
    recording = root / "Recording é.mp4"
    subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", "color=c=0x81426b:s=80x46:r=4",
                    "-t", "1", "-c:v", "libx264", "-pix_fmt", "yuv420p", str(recording)], check=True)
    recording_bytes = recording.read_bytes()
    events = []
    subscription = socket.socket(socket.AF_UNIX)
    subscription.settimeout(5)
    subscription.connect(env["SWAYSOCK"])
    stream = subscription.makefile("rb")
    def message():
        header = stream.read(14)
        if not header:
            return None
        assert header[:6] == b"i3-ipc"
        length, kind = struct.unpack("<II", header[6:])
        return kind, json.loads(stream.read(length))
    payload = b'["window","tick"]'
    subscription.sendall(b"i3-ipc" + struct.pack("<II", len(payload), 2) + payload)
    assert message() == (2, {"success": True}), "Sway subscription handshake failed"
    subscription.settimeout(None)
    barrier = threading.Event()
    observation_errors = []
    def observe():
        try:
            while (received := message()) is not None:
                kind, event = received
                if kind == 0x80000003:
                    events.append(event)
                elif kind == 0x80000007 and event.get("payload") == "captures-test-barrier":
                    barrier.set()
        except Exception as error:
            observation_errors.append(error)
    thread = threading.Thread(target=observe, daemon=True)
    thread.start()
    try:
        for mode in ("quiet", "interactive", "media", "recording", "tray-loss", "missing-tray"):
            profile = root / mode
            history = profile / "history"
            history.mkdir(parents=True)
            settings = profile / "settings.json"
            settings.write_text(json.dumps({"settings_schema_version": 5, "onboarding_completed": True,
                                           "appearance": "dark" if mode == "quiet" else "light",
                                           "launch_at_login": mode == "quiet", "auto_copy_to_clipboard": False,
                                           "output_directory": str(profile / "exports"),
                                           "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
                                           "display_shortcut": "Ctrl+Shift+F9"}))
            source = recording if mode == "recording" else profile / "Capture é.png"
            if mode != "recording":
                source.write_bytes(png(41, 19))
            original = source.read_bytes()
            common = [binary, "--live", "--settings-file", str(settings), "--history-root", str(history)]
            args = [] if mode in ("interactive", "media", "recording") else ["--scene", "idle"]
            if mode in ("media", "recording"):
                args += ["--open-media", str(source)]
            first_event = len(events)
            log_path = profile / "host.log"
            with log_path.open("w") as log:
                app = subprocess.Popen(common + args + ["--quit-after", "40"], env=env, stdout=log, stderr=log)
                try:
                    def visible(title):
                        assert app.poll() is None, log_path.read_text()
                        return next((node for node in windows(env) if node["pid"] == app.pid
                                     and node["name"] == title), None)
                    def hidden(title):
                        return not visible(title)
                    def close(title):
                        node = visible(title)
                        assert node, title
                        subprocess.run(["swaymsg", f'[con_id={node["id"]}] kill'], env=env,
                                       check=True, stdout=subprocess.DEVNULL)
                    def shot(name):
                        if screenshots:
                            time.sleep(.5)
                            subprocess.run(["grim", str(screenshots / f"wayland-{mode}-{name}.png")],
                                           env=env, check=True)
                    def click_login():
                        def control():
                            latest = None
                            for line in log_path.read_text().splitlines():
                                if line.startswith("{") and line.endswith("}"):
                                    event = json.loads(line)
                                    if event.get("event") == "preferences-shortcuts-layout":
                                        latest = event["detail"]
                            if latest and latest["section"] == "general":
                                rect = latest["controls"].get("Start Captures on login")
                                page = latest["page"]
                                size = visible(PREFERENCES)["rect"]
                                if (rect and page[1] <= rect[1] and rect[3] <= page[3]
                                        and page[2:] == [size["width"], size["height"]]):
                                    return rect
                            return None
                        x1, y1, x2, y2 = wait(control, "live General login control")
                        rect = visible(PREFERENCES)["rect"]
                        pointer.stdin.write(f'{int(rect["x"] + (x1 + x2) / 2)} '
                                            f'{int(rect["y"] + (y1 + y2) / 2)} 1280 900\n'.encode())
                        pointer.stdin.flush()
                        assert select.select([pointer.stdout], [], [], 5)[0], "input probe did not finish"
                        assert pointer.stdout.readline().strip() == b"CLICKED"
                    def forwarded(*paths):
                        unused = profile / "secondary-must-not-write.json"
                        command = [binary, "--live", "--history-root", str(history), "--settings-file", str(unused)]
                        for path in paths:
                            command += ["--open-media", str(path)]
                        result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=10)
                        assert result.returncode == 0, result.stderr
                        assert not unused.exists(), "secondary changed settings"
                        assert '"event":"ready"' not in result.stdout, "secondary created a renderer"
                    def no_history_map():
                        assert not visible(HISTORY)
                        barrier.clear()
                        subprocess.run(["swaymsg", "-t", "send_tick", "captures-test-barrier"],
                                       env=env, check=True, stdout=subprocess.DEVNULL)
                        assert barrier.wait(5), "window-event stream did not reach the check barrier"
                        assert not observation_errors, observation_errors
                        assert thread.is_alive(), "window-event subscription stopped"
                        assert not any(event.get("change") in ("new", "title")
                                       and event.get("container", {}).get("pid") == app.pid
                                       and event["container"].get("name") == HISTORY
                                       for event in events[first_event:]), "History flashed during hidden child bootstrap"

                    if mode == "missing-tray":
                        wait(lambda: visible(HISTORY), "no-tray quiet launch exposes History")
                        shot("recovery")
                        close(HISTORY)
                        assert app.wait(timeout=10) == 0, "no-tray close stranded the process"
                    else:
                        wait(lambda: watcher(bus).Get(WATCHER, "RegisteredStatusNotifierItems"), "real SNI registration")
                        if mode in ("quiet", "tray-loss"):
                            wait(lambda: visible("Captures is running"), "quiet launch notice")
                            no_history_map()
                            shot("notice")
                            wait(lambda: hidden("Captures is running"), "quiet launch notice expires", timeout=10)
                            no_history_map()
                        if mode == "quiet":
                            menu_action(bus, "Preferences")
                            wait(lambda: visible(PREFERENCES), "tray opens Preferences without History")
                            no_history_map()
                            shot("preferences")
                            autostart = root / "config/autostart"
                            assert not list(autostart.glob("*.desktop")), "saved setting registered automatically"
                            click_login()
                            entry = wait(lambda: next(autostart.glob("*.desktop"), None), "explicit Wayland login enable")
                            owned = entry.read_bytes()
                            command = next(line[5:] for line in owned.decode().splitlines() if line.startswith("Exec="))
                            assert shlex.split(command) == [binary, "--live", "--scene", "idle",
                                                           "--history-root", str(history), "--settings-file", str(settings)]
                            time.sleep(.5)
                            shot("login-on")
                            close(PREFERENCES)
                            wait(lambda: hidden(PREFERENCES), "Preferences closes into residency")
                            assert app.poll() is None
                            # GIO exercises the entry's actual executable/argv;
                            # the resident process handles it like an empty relaunch.
                            subprocess.run(["gio", "launch", str(entry)], env=env, check=True,
                                           stdout=log, stderr=log, timeout=10)
                            wait(lambda: visible(PREFERENCES), "registered entry relaunch reaches Preferences")
                            no_history_map()
                            assert entry.read_bytes() == owned
                            shot("login-restored")
                            click_login()
                            wait(lambda: not entry.exists(), "explicit Wayland login disable")
                            close(PREFERENCES)
                            wait(lambda: hidden(PREFERENCES), "login Preferences closes")
                            forwarded()
                            wait(lambda: visible(PREFERENCES), "empty relaunch opens Preferences")
                            no_history_map()
                            close(PREFERENCES)
                            wait(lambda: hidden(PREFERENCES), "relaunch Preferences closes")
                            forwarded(source)
                            wait(lambda: visible(EDITOR), "forwarded media editor bootstraps from hidden root")
                            no_history_map()
                            entries = list(history.glob("*/metadata.json"))
                            assert len(entries) == 1
                            entry = json.loads(entries[0].read_text())
                            assert (entry["width"], entry["height"]) == (41, 19)
                            assert Path(entry["saved_path"]).samefile(source)
                            shot("editor")
                            close(EDITOR)
                            wait(lambda: hidden(EDITOR), "editor closes into residency")
                            forwarded(recording)
                            wait(lambda: visible(RECORDING_EDITOR), "forwarded recording opens only its editor")
                            no_history_map()
                            entries = [json.loads(path.read_text()) for path in history.glob("*/metadata.json")]
                            assert len(entries) == 2
                            video = next(item for item in entries if item["kind"] == "video")
                            assert (video["width"], video["height"]) == (80, 46)
                            assert Path(video["saved_path"]).samefile(recording)
                            shot("recording-editor")
                            close(RECORDING_EDITOR)
                            wait(lambda: hidden(RECORDING_EDITOR), "recording editor closes into residency")
                            menu_action(bus, "Send Feedback…")
                            wait(lambda: visible("Send Feedback"), "feedback bootstraps from hidden root")
                            no_history_map()
                            shot("feedback")
                            close("Send Feedback")
                            wait(lambda: hidden("Send Feedback"), "feedback closes into residency")
                        elif mode == "interactive":
                            wait(lambda: visible(PREFERENCES), "interactive launch opens only Preferences")
                            no_history_map()
                        elif mode == "media":
                            wait(lambda: visible(EDITOR), "initial media launch opens only its editor")
                            no_history_map()
                        elif mode == "recording":
                            wait(lambda: visible(RECORDING_EDITOR), "initial recording launch opens only its editor")
                            no_history_map()
                        elif mode == "tray-loss":
                            # Stop only this private compositor's real tray client.
                            candidates = subprocess.check_output(["pgrep", "-x", "swaybar"], text=True).split()
                            tray_pids = [int(pid) for pid in candidates
                                         if f'SWAYSOCK={env["SWAYSOCK"]}'.encode()
                                         in Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")]
                            assert len(tray_pids) == 1, "private Swaybar not identified"
                            os.kill(tray_pids[0], signal.SIGTERM)
                            wait(lambda: not watcher(bus), "real tray host removed")
                            wait(lambda: visible(HISTORY), "tray loss restores reachable History")
                            shot("recovery")
                            close(HISTORY)
                            assert app.wait(timeout=10) == 0, "tray loss retained close-to-hide"
                        if mode != "tray-loss":
                            menu_action(bus, "Quit Captures")
                            assert app.wait(timeout=10) == 0, "real tray Quit did not drain normally"
                        if watcher(bus):
                            wait(lambda: not watcher(bus).Get(WATCHER, "RegisteredStatusNotifierItems"), "normal quit unregisters tray")
                    assert source.read_bytes() == original
                    assert recording.read_bytes() == recording_bytes
                    print(json.dumps({"case": mode, "passed": True, "history_never_mapped": mode not in ("tray-loss", "missing-tray")}), flush=True)
                except Exception:
                    log.flush()
                    shot("failed")
                    print(log_path.read_text())
                    raise
                finally:
                    stop(app)
    finally:
        subscription.shutdown(socket.SHUT_RDWR)
        thread.join(timeout=2)
        stream.close()
        subscription.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--injector", type=Path, required=True)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    binary = str(args.binary.resolve(strict=True))
    injector = str(args.injector.resolve(strict=True))
    screenshots = args.screenshots.resolve() if args.screenshots else None
    if screenshots:
        screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="captures-wayland-lifecycle-") as temporary:
        root = Path(temporary)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        env = os.environ.copy()
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "SWAYSOCK", "DBUS_SESSION_BUS_ADDRESS"):
            env.pop(key, None)
        env.update(XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(root / "config"), XDG_DATA_HOME=str(root / "data"),
                   XDG_CACHE_HOME=str(root / "cache"), XDG_CURRENT_DESKTOP="sway", GDK_BACKEND="wayland",
                   XDG_SESSION_TYPE="wayland", NO_AT_BRIDGE="1", CAPTURES_NATIVE_LAYOUT_PROBE="1",
                   WLR_BACKENDS="headless", WLR_HEADLESS_OUTPUTS="1", WLR_LIBINPUT_NO_DEVICES="1", WLR_RENDERER="pixman",
                   WGPU_BACKEND="gl", CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER="1")
        daemon = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1"], env=env, stdout=subprocess.PIPE)
        env["DBUS_SESSION_BUS_ADDRESS"] = daemon.stdout.readline().decode().strip()
        env["DBUS_SYSTEM_BUS_ADDRESS"] = env["DBUS_SESSION_BUS_ADDRESS"]
        bus = dbus.bus.BusConnection(env["DBUS_SESSION_BUS_ADDRESS"])
        config = root / "sway.conf"
        config.write_text('output HEADLESS-1 resolution 1280x900\noutput * bg #234567 solid_color\n'
                          'seat seat0 fallback true\n'
                          'default_border none\ndefault_floating_border none\n'
                          'for_window [title=".*"] floating enable\n'
                          'bar {\n id captures-test\n position top\n workspace_buttons no\n}\n')
        with (root / "services.log").open("w") as log:
            sway = subprocess.Popen(["sway", "--unsupported-gpu", "-c", str(config)], env=env, stdout=log, stderr=log)
            pointer = None
            try:
                socket = wait(lambda: next((path for path in runtime.glob("wayland-*") if path.is_socket()), None), "Wayland socket")
                env["WAYLAND_DISPLAY"] = socket.name
                env["SWAYSOCK"] = str(wait(lambda: next(iter(runtime.glob("sway-ipc.*.sock")), None), "Sway IPC"))
                subprocess.run(["dbus-update-activation-environment", "WAYLAND_DISPLAY", "SWAYSOCK"],
                               env=env, check=True)
                wait(lambda: watcher(bus), "real Swaybar SNI host")
                pointer = subprocess.Popen([injector, "pointer"], env=env,
                                           stdin=subprocess.PIPE, stdout=subprocess.PIPE)
                ready(pointer)
                cases(binary, root, env, bus, screenshots, pointer)
            except Exception:
                log.flush()
                print((root / "services.log").read_text())
                raise
            finally:
                if pointer:
                    stop(pointer)
                stop(sway)
                stop(daemon)
                subprocess.run(["fusermount3", "-uz", str(runtime / "doc")],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
