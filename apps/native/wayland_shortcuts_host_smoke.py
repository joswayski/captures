#!/usr/bin/python3
"""Native shortcut Preferences/routing on private Sway with scripted grants.

Uses real windows, tray and pointer input, not physical compositor key binding
or consent acceptance. Never opens X11 or touches an installed profile.
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

from instance_smoke import png
from wayland_lifecycle_smoke import menu_action, watcher
from wayland_native_capture_smoke import wait, windows
from wayland_recording_host_smoke import events
from wayland_screenshot_smoke import ready, rgba, stop
from wayland_shortcuts_smoke import CONTROL, DESKTOP, ROOT, launch_fixture, read_log
from x11_capture_smoke import ScreenSaver

WIDTH, HEIGHT = 1280, 900
PREFERENCES = "Captures Preferences"
LONG_TRIGGER = "Desktop-selected Control + Alt + Shift + the external keyboard's special screenshot key; not the requested chord"


def cases(binary, pointer, root, env, bus):
    script = str(Path(__file__).with_name("wayland_shortcuts_smoke.py"))
    for appearance in ("dark", "light"):
        for mode in ("ui", "empty", "v1", "configure-error", "deny", "pending-create", "pending-bind"):
            profile = root / f"{appearance}-{mode}"
            profile.mkdir()
            service, portal_log = launch_fixture(script, mode, profile, env)
            settings = profile / "settings.json"
            settings.write_text(json.dumps({
                "settings_schema_version": 5, "onboarding_completed": True,
                "appearance": appearance, "launch_at_login": False,
                "auto_copy_to_clipboard": False, "screenshot_countdown_seconds": 0,
                "output_directory": str(profile / "exports"),
                "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
                "display_shortcut": "Ctrl+Shift+F9",
                "recording": {"highlight_clicks": False, "show_keystrokes": False,
                              "capture_system_audio": False, "microphone_device_id": None},
            }))
            original = settings.read_bytes()
            host_log = profile / "host.log"
            with host_log.open("w") as log:
                # Exercise restoration with History hidden and shown, rather
                # than letting asynchronous mapping decide which path runs.
                history_shown = appearance == "light" and mode == "ui"
                app = subprocess.Popen([binary, "--live", "--open-preferences",
                                        "--settings-file", str(settings),
                                        "--history-root", str(profile / "history"),
                                        "--quit-after", "90"], env=env, stdout=log, stderr=log)
                try:
                    def window(title=PREFERENCES):
                        assert app.poll() is None, host_log.read_text()
                        return next((node for node in windows(env) if node["pid"] == app.pid
                                     and node["name"] == title), None)

                    def layout():
                        found = events(host_log, "preferences-shortcuts-layout")
                        return found[-1]["detail"] if found else {}

                    def input_at(x, y, scroll=None):
                        suffix = f" {scroll}" if scroll is not None else ""
                        pointer.stdin.write(f"{round(x)} {round(y)} {WIDTH} {HEIGHT}{suffix}\n".encode())
                        pointer.stdin.flush()
                        assert select.select([pointer.stdout], [], [], 5)[0], "input injector stalled"
                        assert pointer.stdout.readline().strip() == (b"SCROLLED" if scroll is not None else b"CLICKED")

                    def arrange(width, height):
                        node = wait(window, PREFERENCES)
                        subprocess.run(["swaymsg", f'[con_id={node["id"]}] floating enable, '
                                        f'resize set {width} {height}, move position 80 80, focus'],
                                       env=env, check=True, stdout=subprocess.DEVNULL)
                        wait(lambda: window()["rect"]["width"] == width
                             and window()["rect"]["height"] == height
                             and window()["focused"], "Preferences resize and focus")

                    def state(expected):
                        def found():
                            detail = layout()
                            return detail if (detail.get("desktop_shortcuts") or {}).get("state") == expected else None
                        return wait(found, f"desktop shortcut state {expected}")

                    def control_visible():
                        detail = layout()
                        node = window()
                        rect = detail.get("controls", {}).get("Desktop shortcuts")
                        page = detail.get("page", [])
                        return (detail if rect and node and node["focused"] and len(page) == 4
                                and page[2:] == [node["rect"]["width"], node["rect"]["height"]]
                                and page[1] <= rect[1] < rect[3] <= page[3] else None)

                    def show_shortcuts():
                        # At normal width use the shipping sidebar; compact
                        # windows hide it, so scroll with real Wayland input.
                        if window()["rect"]["width"] > 720:
                            rect = window()["rect"]
                            input_at(rect["x"] + 98, rect["y"] + 187)
                        # Let sidebar navigation and the native resize finish
                        # before correcting scroll from layout probe geometry.
                        time.sleep(.5)
                        for _ in range(60):
                            if not window()["focused"]:
                                subprocess.run(["swaymsg", f'[con_id={window()["id"]}] focus'],
                                               env=env, check=True, stdout=subprocess.DEVNULL)
                                wait(lambda: window()["focused"], "Preferences focus before scrolling")
                            detail = wait(lambda: layout().get("page") and layout(), "Preferences page layout")
                            offset = detail["card_top"] - detail["page"][1] - 16
                            if control_visible() and abs(offset) <= 2:
                                # Input can finish before the app processes the
                                # last axis event. Do not accept a transient match
                                # while navigation/scroll animation is in flight.
                                time.sleep(.5)
                                if control_visible() and abs(layout()["card_top"] - layout()["page"][1] - 16) <= 2:
                                    break
                                continue
                            rect = window()["rect"]
                            input_at(rect["x"] + rect["width"] - 40, rect["y"] + 200,
                                     max(-200, min(200, offset)))
                            time.sleep(.2)
                        wait(control_visible, "visible desktop shortcut control")
                        time.sleep(.5)
                        assert abs(layout()["card_top"] - layout()["page"][1] - 16) <= 2, "shortcut card scrolled out of view"
                        assert layout()["recording"] is None
                        assert not ({"New Capture", "Region", "Window", "Full Screen",
                                     "Record Region", "Record Window", "Record Full Screen"}
                                    & layout()["controls"].keys()), "portal exposes local recorders"

                    def click_control():
                        detail = wait(control_visible, "desktop control ready for input")
                        x1, y1, x2, y2 = detail["controls"]["Desktop shortcuts"]
                        rect = window()["rect"]
                        input_at(rect["x"] + (x1 + x2) / 2, rect["y"] + (y1 + y2) / 2)

                    def shot(name):
                        rect = window()["rect"]
                        subprocess.run(["grim", "-g", f'{rect["x"]},{rect["y"]} '
                                        f'{rect["width"]}x{rect["height"]}', str(profile / f"{name}.png")],
                                       env=env, check=True, timeout=5)

                    wait(lambda: watcher(bus) and watcher(bus).Get(
                        "org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems"), "real tray registration")
                    expected = "pending" if mode.startswith("pending-") else "unavailable" if mode == "deny" else "bound"
                    state(expected)
                    ctl = dbus.Interface(bus.get_object(DESKTOP, ROOT), CONTROL)
                    if history_shown:
                        menu_action(bus, "Capture History…")
                        wait(lambda: window("Capture History"), "explicitly shown History")
                    for width, height in ((880, 660), (560, 440)):
                        arrange(width, height)
                        show_shortcuts()
                        assert state(expected)["desktop_shortcuts"]["enabled"] == (mode not in ("v1", "pending-create", "pending-bind"))
                        shot(f"{expected}-{width}")
                    click_control()
                    if expected == "pending" or mode == "v1":
                        time.sleep(.3)
                        assert not any(item["event"] == "configure" for item in read_log(portal_log))
                        assert len([item for item in read_log(portal_log) if item["event"] == "create"]) == 1
                    elif mode == "deny":
                        wait(lambda: len([item for item in read_log(portal_log) if item["event"] == "create"]) == 2,
                             "explicit Retry starts a fresh consent request")
                        state("unavailable")
                    else:
                        wait(lambda: any(item["event"] == "configure" for item in read_log(portal_log)),
                             "native Configure invokes the desktop")
                        if mode == "configure-error":
                            wait(lambda: (layout().get("desktop_shortcuts") or {}).get("configuration_error"),
                                 "native configuration error")
                            time.sleep(.5)
                            shot("configuration-error")
                    if mode == "ui":
                        ctl.ReplaceSnapshot(["display", "record_window", "new_capture"],
                                            [LONG_TRIGGER, "Alt+9 (desktop)", "Desktop launch"])
                        ctl.EmitChanged(["display"], ["partial must not replace membership"])
                        wait(lambda: any(item["event"] == "list" for item in read_log(portal_log)), "full snapshot refresh")
                        time.sleep(.5)
                        rect = window()["rect"]
                        input_at(rect["x"] + rect["width"] - 40, rect["y"] + 200, 260)
                        time.sleep(.5)
                        shot("long-trigger-560")
                        arrange(880, 660)
                        show_shortcuts()
                        shot("long-trigger-880")
                        ctl.EmitClosed()
                        state("unavailable")
                        show_shortcuts()
                        shot("closed-retry")
                        click_control()
                        state("bound")
                        source = profile / "portal-owned.png"
                        source.write_bytes(png(41, 19))
                        original_pixels = rgba(source)
                        ctl.SetScreenshot(source.as_uri())
                        before = set((profile / "history").glob("*/metadata.json"))
                        ctl.EmitActivated("display"); ctl.EmitDeactivated("display")
                        added = wait(lambda: set((profile / "history").glob("*/metadata.json")) - before,
                                     "granted shortcut publishes a screenshot while Preferences is focused")
                        assert len(added) == 1
                        assert rgba(next(iter(added)).parent / "capture.png") == original_pixels
                        assert source.read_bytes() == png(41, 19), "modified portal-owned source"
                        wait(window, "Preferences restored after capture")
                        # Portal captures intentionally show History. Wait for
                        # that completed UI transition before starting a take;
                        # file publication precedes the host's History request.
                        history = wait(lambda: window("Capture History"), "captured screenshot shown in History")
                        if not history_shown:
                            subprocess.run(["swaymsg", f'[con_id={history["id"]}] kill'],
                                           env=env, check=True, stdout=subprocess.DEVNULL)
                        wait(lambda: (window("Capture History") is not None) == history_shown,
                             "chosen History visibility before recording")
                        arrange(880, 660)
                        show_shortcuts()
                        ctl.EmitActivated("record_window"); ctl.EmitDeactivated("record_window")
                        draft = wait(lambda: next(iter((profile / "recording-recovery").glob("*/manifest.json")), None),
                                     "recording shortcut creates a window take")
                        wait(lambda: json.loads(draft.read_text())["state"] == "failed",
                             "fixture without ScreenCast reports a failed take")
                        take = json.loads(draft.read_text())
                        assert take["options"]["target"] == {"type": "portal_window"} and take["last_error"]
                        assert not take["segments"] and not list(profile.rglob("*.mp4"))
                        assert set((profile / "history").glob("*/metadata.json")) == before | added
                        def delete_control():
                            hud = window("Captures Recording Controls")
                            controls = events(host_log, "recording-hud-control-layout")
                            matches = [item["detail"] for item in controls if item["detail"].get("control") == "delete"
                                       and item["detail"].get("state") == "failed" and item["detail"].get("enabled")]
                            if hud and matches and matches[-1]["viewport_size"] == [hud["rect"]["width"], hud["rect"]["height"]]:
                                return matches[-1]["rect"], hud["rect"]
                        (x1, y1, x2, y2), rect = wait(delete_control, "failed take Delete control")
                        input_at(rect["x"] + (x1 + x2) / 2, rect["y"] + (y1 + y2) / 2)
                        confirmation = wait(lambda: window("Delete recording?"), "shipping Delete confirmation")
                        # Shipping confirmation layout, also exercised by the
                        # existing X11 failed-take regression.
                        input_at(confirmation["rect"]["x"] + 104, confirmation["rect"]["y"] + 115)
                        wait(lambda: not window("Captures Recording Controls") and not draft.exists(),
                             "Delete removes only the empty failed window take")
                        wait(window, "Preferences restored after recording failure")
                        wait(lambda: (window("Capture History") is not None) == history_shown,
                             "recording completion restores prior History visibility")
                        arrange(880, 660)
                        show_shortcuts()
                        assert (window("Capture History") is not None) == history_shown
                        (profile / "restored-windows.json").write_text(json.dumps(windows(env), indent=2) + "\n")
                        shot("restored")
                        ctl.BeginFlood()
                    started = time.monotonic()
                    menu_action(bus, "Quit Captures")
                    assert app.wait(timeout=10) == 0
                    assert time.monotonic() - started < 5, "native Quit did not drain promptly"
                    portal_events = read_log(portal_log)
                    assert any(item["event"] == "session-close" for item in portal_events)
                    if expected == "pending":
                        assert any(item["event"] == "request-close" for item in portal_events)
                    assert settings.read_bytes() == original, "desktop binding UI edited requested keys"
                    print(json.dumps({"appearance": appearance, "mode": mode, "passed": True}), flush=True)
                except Exception:
                    (profile / "failed-windows.json").write_text(json.dumps(windows(env), indent=2) + "\n")
                    subprocess.run(["grim", str(profile / "failed-desktop.png")],
                                   env=env, check=True, timeout=5)
                    if app.poll() is None and window():
                        shot("failed")
                    print(host_log.read_text())
                    raise
                finally:
                    stop(app)
                    stop(service)
    return {"host_cases": 14}


def main(run_cases=cases, description=__doc__):
    parser = argparse.ArgumentParser(description=description)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--injector", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    runtime = root / "runtime"
    runtime.mkdir(mode=0o700)
    env = os.environ.copy()
    for key in ("DISPLAY", "WAYLAND_DISPLAY", "SWAYSOCK", "DBUS_SESSION_BUS_ADDRESS"):
        env.pop(key, None)
    env.update(XDG_RUNTIME_DIR=str(runtime), XDG_DATA_HOME=str(root / "data"),
               XDG_CONFIG_HOME=str(root / "config"), XDG_CACHE_HOME=str(root / "cache"),
               XDG_SESSION_TYPE="wayland", XDG_CURRENT_DESKTOP="sway", GDK_BACKEND="wayland",
               WLR_BACKENDS="headless", WLR_HEADLESS_OUTPUTS="1", WLR_LIBINPUT_NO_DEVICES="1",
               WLR_RENDERER="pixman", WGPU_BACKEND="gl", NO_AT_BRIDGE="1",
               CAPTURES_NATIVE_LAYOUT_PROBE="1", CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER="1")
    daemon = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1"],
                              env=env, stdout=subprocess.PIPE)
    env["DBUS_SESSION_BUS_ADDRESS"] = daemon.stdout.readline().decode().strip()
    env["DBUS_SYSTEM_BUS_ADDRESS"] = env["DBUS_SESSION_BUS_ADDRESS"]
    DBusGMainLoop(set_as_default=True)
    bus = dbus.bus.BusConnection(env["DBUS_SESSION_BUS_ADDRESS"])
    name = dbus.service.BusName("org.freedesktop.ScreenSaver", bus=bus, do_not_queue=True)
    saver = ScreenSaver(name, "/org/freedesktop/ScreenSaver")
    loop = GLib.MainLoop()
    thread = threading.Thread(target=loop.run, daemon=True)
    thread.start()
    config = root / "sway.conf"
    config.write_text('output HEADLESS-1 resolution 1280x900\noutput * bg #234567 solid_color\n'
                      'seat seat0 fallback true\ndefault_border none\ndefault_floating_border none\n'
                      'for_window [title=".*"] floating enable\n'
                      'bar {\n id captures-test\n position top\n workspace_buttons no\n}\n')
    pointer = sway = None
    try:
        with (root / "services.log").open("w") as log:
            sway = subprocess.Popen(["sway", "--unsupported-gpu", "-c", str(config)], env=env, stdout=log, stderr=log)
            socket = wait(lambda: next((p for p in runtime.glob("wayland-*") if p.is_socket()), None), "Wayland socket")
            env["WAYLAND_DISPLAY"] = socket.name
            env["SWAYSOCK"] = str(wait(lambda: next(iter(runtime.glob("sway-ipc.*.sock")), None), "Sway IPC"))
            pointer = subprocess.Popen([str(args.injector.resolve(strict=True)), "pointer"], env=env,
                                       stdin=subprocess.PIPE, stdout=subprocess.PIPE)
            ready(pointer)
            result = run_cases(str(args.binary.resolve(strict=True)), pointer, root, env, bus)
            print(json.dumps({**result, "display_unset": True,
                              "physical_compositor_acceptance": False}), flush=True)
    finally:
        if pointer:
            stop(pointer)
        if sway:
            stop(sway)
        loop.quit()
        thread.join(timeout=2)
        saver.remove_from_connection()
        del saver, name
        bus.close()
        stop(daemon)
        subprocess.run(["fusermount3", "-uz", str(runtime / "doc")],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
