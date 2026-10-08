#!/usr/bin/env python3
"""Real native first-run input on private X11; no permission bypass or real user data."""
import argparse
import json
import os
import shutil
from pathlib import Path
import subprocess
import time
import uuid

from history_fixture import write_completed_settings
from instance_smoke import png


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--notices-only", action="store_true", help="Focused quiet launch/long-shortcut notice checks")
    mode.add_argument("--update-restart-only", action="store_true", help="Injected development restart intent; binary needs packaged media tools")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = {**os.environ, "WGPU_BACKEND": "gl", "WINIT_X11_SCALE_FACTOR": "1", "XDG_SESSION_TYPE": "x11"}
    # Live hosts must never unbind the developer's real OS screenshot keys.
    env["CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER"] = "1"
    env.pop("WAYLAND_DISPLAY", None)
    children = []
    with (output / "processes.log").open("w") as log:
        def spawn(command, announce=False):
            process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE if announce else log, stderr=log)
            children.append(process)
            return process

        def run(*command):
            return subprocess.check_output(command, env=env, stderr=log, timeout=15)

        def wait(predicate, description):
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                if result := predicate():
                    return result
                time.sleep(.05)
            run("import", "-window", "root", str(output / "timeout.png"))
            raise AssertionError(description)

        def windows(pid, name="^Captures$"):
            result = subprocess.run(["xdotool", "search", "--all", "--onlyvisible", "--pid", str(pid), "--name", name],
                                    env=env, capture_output=True, text=True, timeout=5)
            assert result.returncode in (0, 1)
            return result.stdout.splitlines()

        def click(window, x, y):
            run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
                "mousemove", "--sync", "--window", window, str(x - 1), str(y),
                "mousemove_relative", "--sync", "1", "0", "sleep", ".15", "mousedown", "1",
                "sleep", ".15", "mouseup", "1")

        def notice_click(window, x, y):
            # The launch notice never activates; click without WM activation.
            run("xdotool", "mousemove", "--sync", "--window", window, str(x - 1), str(y),
                "mousemove_relative", "--sync", "1", "0",
                "sleep", ".15", "mousedown", "1", "sleep", ".15", "mouseup", "1")

        def published(history):
            # A History publish renames a hidden staging directory into place;
            # count only published entries.
            return [path for path in history.glob("*/metadata.json")
                    if not path.parent.name.startswith(".")]

        try:
            server = spawn(["Xvfb", "-displayfd", "1", "-screen", "0", "1280x900x24", "-nolisten", "tcp"], True)
            env["DISPLAY"] = ":" + server.stdout.readline().decode().strip()
            run("xdpyinfo")
            spawn(["openbox", "--sm-disable"])
            wait(lambda: b"window id" in run("xprop", "-root", "_NET_SUPPORTING_WM_CHECK"), "window manager ready")
            if args.update_restart_only:
                update_restart_launch(binary, output, env, spawn, wait, windows, run)
                (output / "result.json").write_text(json.dumps({"passed": True,
                    "scope": "Injected development intent on private X11; not an installed update."}, indent=2))
                return
            if args.notices_only:
                profile = output / "dark"
                profile.mkdir()
                write_completed_settings(profile / "fresh settings %.json")
                quiet_launch_notice(binary, output, env, spawn, wait, windows, run)
                (output / "result.json").write_text(json.dumps({"passed": True,
                    "scope": "Quiet X11 notices only; not full onboarding or physical platform acceptance."}, indent=2))
                return
            for appearance, hidden in (("dark", False), ("light", True)):
                root = output / appearance
                root.mkdir()
                settings = root / "fresh settings %.json"
                history = root / "history"
                source = root / "cold source.png"
                forwarded = root / "forwarded source.png"
                source.write_bytes(png(13, 7))
                forwarded.write_bytes(png(17, 11))
                common = [str(binary), "--live", "--history-root", str(history), "--settings-file", str(settings),
                          "--appearance", appearance]
                app = spawn(common + (["--scene", "idle"] if hidden else []) + ["--", str(source)])
                window = wait(lambda: windows(app.pid), "first-run setup must be visible even for idle launch")[0]
                time.sleep(1)
                run("import", "-window", window, str(output / f"onboarding-{appearance}.png"))
                assert not settings.exists(), "checking first run completed/wrote settings"
                assert not published(history), "cold media imported before setup"
                secondary = subprocess.run(common + ["--", str(forwarded)], env=env, capture_output=True, timeout=15)
                assert secondary.returncode == 0, secondary.stderr
                run("xdotool", "key", "super+shift+s")
                time.sleep(.4)
                assert not published(history), "capture/forwarding bypassed setup"
                assert len(windows(app.pid, ".*")) == 1, "capture selector/editor opened before setup"
                click(window, 514, 384)  # Start capturing, right-aligned under the cards.
                wait(lambda: settings.exists() and json.loads(settings.read_text()).get("onboarding_completed"),
                     "setup completion persisted")
                # Without a tray host, completing setup keeps Captures reachable:
                # the setup window becomes the Capture History window.
                wait(lambda: windows(app.pid, "^Capture History$") == [window], "History after setup")
                wait(lambda: len(published(history)) == 2, "queued cold and forwarded media imported")
                # Shipping launch notice: nonactivating, titled like the Tauri
                # window, anchored top-right without an X11 tray rect, dismissible.
                notice = wait(lambda: windows(app.pid, "^Captures is running$"), "launch notice after setup")[0]
                time.sleep(.6)
                assert run("xdotool", "getwindowfocus").decode().strip() != notice, "launch notice took focus"
                geometry = run("xdotool", "getwindowgeometry", "--shell", notice).decode()
                assert "WIDTH=352" in geometry and "HEIGHT=110" in geometry, geometry
                run("import", "-window", notice, str(output / f"startup-notice-{appearance}.png"))
                notice_click(notice, 304, 55)  # Close, right-aligned in the pill.
                wait(lambda: not windows(app.pid, "^Captures is running$"), "launch notice dismissed")
                assert source.read_bytes() == png(13, 7) and forwarded.read_bytes() == png(17, 11)
                # Publication precedes the editor's asynchronous presentation.
                # Let that focus transfer settle before quitting from the root.
                time.sleep(1)
                run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
                    "sleep", ".4", "key", "ctrl+q")
                assert app.wait(timeout=20) == 0
                # Shipping `interactive_launch_action`: a visible launch of a
                # completed profile opens Preferences alone.
                again = spawn(common)
                preferences = wait(lambda: windows(again.pid, "^Captures Preferences$"),
                                   "visible launch opens Preferences")[0]
                time.sleep(1)
                assert not windows(again.pid, "^Capture History$"), "visible launch showed History"
                assert not windows(again.pid, "^Captures is running$"), "visible launch showed the launch notice"
                run("xdotool", "windowactivate", "--sync", preferences, "windowfocus", "--sync", preferences,
                    "sleep", ".4", "key", "ctrl+q")
                assert again.wait(timeout=20) == 0
                # Open Preferences beside History, as its own window.
                again = spawn(common + ["--open-history", "--open-preferences"])
                window = wait(lambda: windows(again.pid, "^Capture History$"), "completed profile History")[0]
                preferences = wait(lambda: windows(again.pid, "^Captures Preferences$"), "Preferences window")[0]
                time.sleep(1)
                assert not windows(again.pid, "^Captures is running$"), "visible relaunch showed the launch notice"
                run("import", "-window", window, str(output / f"completed-{appearance}.png"))
                run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
                    "sleep", ".4", "key", "ctrl+q")
                assert again.wait(timeout=20) == 0
                # Shipping History has no permissions button: a denied capture
                # opens recovery. The workbench hook opens it the same way,
                # without an OS prompt.
                again = spawn(common + ["--open-history", "--open-preferences", "--permission-dialog", "ready"])
                window = wait(lambda: windows(again.pid, "^Capture History$"), "History under recovery")[0]
                preferences = wait(lambda: windows(again.pid, "^Captures Preferences$"), "Preferences window")[0]
                time.sleep(1)
                accepted_settings = settings.read_bytes()
                recovery_media = root / "during permission recovery.png"
                recovery_media.write_bytes(png(19, 9))
                secondary = subprocess.run(common + ["--", str(recovery_media)], env=env,
                                           capture_output=True, timeout=15)
                assert secondary.returncode == 0, secondary.stderr
                run("xdotool", "key", "Print")  # The New Capture shortcut stays blocked behind the dialog.
                run("xdotool", "key", "super+shift+s")
                time.sleep(.5)
                assert len(published(history)) == 2, "recovery imported queued media"
                assert sorted(windows(again.pid, ".*")) == sorted([window, preferences]), "recovery launched capture/editor"
                run("import", "-window", window, str(output / f"permission-recovery-{appearance}.png"))
                click(window, 556, 449)  # Refresh status (secondary) is prompt-free and does not complete setup.
                time.sleep(.4)
                assert settings.read_bytes() == accepted_settings, "recovery changed setup/settings"
                click(window, 691, 449)  # Done (primary card action), including when no upfront permission is required.
                wait(lambda: len(published(history)) == 3, "recovery releases queued media")
                assert recovery_media.read_bytes() == png(19, 9), "recovery changed the input"
                time.sleep(1)
                run("xdotool", "windowactivate", "--sync", preferences)
                time.sleep(.4)
                run("import", "-window", preferences, str(output / f"preferences-{appearance}.png"))
                run("xdotool", "key", "ctrl+q")
                assert again.wait(timeout=20) == 0
                print(f"PASS {appearance}: first run, hidden={hidden}, capture gate, queued media, launch notice, persistence, relaunch and permission recovery", flush=True)
            broken = output / "malformed.json"
            broken.write_text("invalid-json")
            app = spawn([str(binary), "--live", "--history-root", str(output / "error-history"),
                         "--settings-file", str(broken), "--appearance", "dark"])
            window = wait(lambda: windows(app.pid), "settings error visible")[0]
            time.sleep(1)
            run("import", "-window", window, str(output / "onboarding-error.png"))
            assert broken.read_text() == "invalid-json", "failed setup replaced malformed settings"
            # Another app may retain focus while setup is open (no repaint-driven activation).
            spawn(["xmessage", "-title", "Setup focus probe", "Other application"])
            probe = run("xdotool", "search", "--sync", "--onlyvisible", "--name", "^Setup focus probe$").decode().splitlines()[0]
            run("xdotool", "windowactivate", "--sync", probe, "windowfocus", "--sync", probe)
            time.sleep(.5)
            assert run("xdotool", "getwindowfocus").decode().strip() == probe, "setup stole focus"
            broken.unlink()  # User fixes the file; retry must reload and recheck.
            click(window, 382, 401)  # Retry setup, beside the disabled primary.
            time.sleep(.5)
            run("import", "-window", window, str(output / "onboarding-retry.png"))
            assert not broken.exists(), "retry silently completed setup"
            click(window, 514, 384)
            wait(lambda: broken.exists() and json.loads(broken.read_text()).get("onboarding_completed"), "completion after retry")
            run("xdotool", "key", "ctrl+q")
            assert app.wait(timeout=20) == 0
            print("PASS malformed settings preserved, focus retained, explicit retry and completion", flush=True)
            quiet_launch = bool(shutil.which("xfce4-panel") and shutil.which("dbus-daemon"))
            if quiet_launch:
                quiet_launch_notice(binary, output, env, spawn, wait, windows, run)
            else:
                print("SKIP quiet launch notice: xfce4-panel/dbus-daemon unavailable", flush=True)
            (output / "result.json").write_text(json.dumps({"passed": True, "quietLaunchNotice": quiet_launch,
                "scope": "Private X11/software GL; not macOS TCC, Windows or physical acceptance."}, indent=2))
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()


PANEL = """<?xml version="1.0" encoding="UTF-8"?>
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
  <property name="plugin-1" type="string" value="systray"/>
 </property>
</channel>"""


def start_tray(output, env, spawn, wait):
    """Private SNI host lets quiet launches keep their root window hidden."""
    import dbus

    config = output / "config/xfce4/xfconf/xfce-perchannel-xml/xfce4-panel.xml"
    config.parent.mkdir(parents=True)
    config.write_text(PANEL)
    env["XDG_CONFIG_HOME"] = str(output / "config")
    daemon = spawn(["dbus-daemon", "--session", "--nofork", "--print-address=1"], True)
    address = daemon.stdout.readline().decode().strip()
    env["DBUS_SESSION_BUS_ADDRESS"] = address
    bus = dbus.bus.BusConnection(address)
    spawn(["xfce4-panel", "--disable-wm-check", "--sm-client-disable"])

    def host_registered():
        if not bus.name_has_owner("org.kde.StatusNotifierWatcher"):
            return False
        watcher = bus.get_object("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher")
        return bool(watcher.Get("org.kde.StatusNotifierWatcher", "IsStatusNotifierHostRegistered",
                                dbus_interface="org.freedesktop.DBus.Properties"))

    wait(host_registered, "real SNI tray host")
    return bus


def update_restart_launch(binary, output, env, spawn, wait, windows, run):
    """Exercise real primary routing with private one-shot health markers."""
    bus = start_tray(output, env, spawn, wait)
    for label, appearance, intent, setup in (
        ("visible-dark", "dark", True, False),
        ("visible-light", "light", True, False),
        ("closed", "dark", False, False),
        ("legacy", "dark", None, False),
        ("setup", "dark", True, True),
    ):
        profile = output / label
        profile.mkdir()
        settings = profile / "settings.json"
        if not setup:
            write_completed_settings(settings)
        ready = profile / "ready"
        ready.write_bytes(b"")
        marker = ready.with_suffix(".restart.json")
        marker.write_bytes(b"" if intent is None else json.dumps({"restore_preferences": intent}).encode())
        token = str(uuid.uuid4())
        common = [str(binary), "--live", "--history-root", str(profile / "history"),
                  "--settings-file", str(settings), "--appearance", appearance]
        app = spawn(common + ["--native-update-ready-file", str(ready), "--native-update-ready-token", token])
        wait(lambda: ready.read_bytes() == f"{token}\n".encode(), f"{label}: exact live readiness")
        assert not marker.exists(), "primary did not consume intent"
        if setup:
            wait(lambda: windows(app.pid), "setup wins over Preferences restoration")
            assert not windows(app.pid, "^Captures Preferences$"), "restart bypassed setup"
            assert not settings.exists(), "restart completed setup"
        else:
            notice = wait(lambda: windows(app.pid, "^Captures is running$"), f"{label}: ready notice")[0]
            wait(lambda: not windows(app.pid, "^Capture History$"), f"{label}: hidden History")
            if intent:
                preferences = wait(lambda: windows(app.pid, "^Captures Preferences$"), "restored Preferences")[0]
                wait(lambda: run("xdotool", "getwindowfocus").decode().strip() == preferences,
                     "restored Preferences focused; notice stays nonactivating")
                time.sleep(.6)
                run("import", "-window", "root", str(output / f"restart-{label}.png"))
                wait(lambda: not windows(app.pid, "^Captures is running$"), "ready notice expires")
                assert windows(app.pid, "^Captures Preferences$") == [preferences], "expiry closed Preferences"
            else:
                time.sleep(.6)
                assert not windows(app.pid, "^Captures Preferences$"), "closed/legacy intent restored Preferences"
                run("import", "-window", notice, str(output / f"restart-{label}.png"))
        if intent and not setup:
            run("xdotool", "windowactivate", "--sync", preferences, "windowfocus", "--sync", preferences,
                "sleep", ".4", "key", "ctrl+q")
            assert app.wait(timeout=20) == 0
        else:
            app.terminate()
            app.wait(timeout=20)
        if not setup:
            again = spawn(common + ["--scene", "idle"])
            wait(lambda: windows(again.pid, "^Captures is running$"), "ordinary quiet relaunch")
            time.sleep(.6)
            assert not windows(again.pid, "^Captures Preferences$"), "restore repeated after consumption"
            again.terminate()
            again.wait(timeout=20)
        print(f"PASS {label}: primary consumption, launch priority, readiness and no repeated restore", flush=True)

    profile = output / "secondary"
    profile.mkdir()
    settings = profile / "settings.json"
    write_completed_settings(settings)
    common = [str(binary), "--live", "--history-root", str(profile / "history"), "--settings-file", str(settings)]
    primary = spawn(common)
    wait(lambda: windows(primary.pid, "^Captures Preferences$"), "primary before secondary")
    ready = profile / "ready"
    ready.write_bytes(b"")
    marker = ready.with_suffix(".restart.json")
    contents = b'{"restore_preferences":true}'
    marker.write_bytes(contents)
    secondary = subprocess.run(common + ["--native-update-ready-file", str(ready),
                               "--native-update-ready-token", str(uuid.uuid4())], env=env, capture_output=True, timeout=20)
    assert secondary.returncode == 0, secondary.stderr
    assert marker.read_bytes() == contents, "secondary consumed primary intent"
    assert ready.read_bytes() == b"", "secondary acknowledged readiness"
    preferences = windows(primary.pid, "^Captures Preferences$")[0]
    run("xdotool", "windowactivate", "--sync", preferences, "windowfocus", "--sync", preferences,
        "sleep", ".4", "key", "ctrl+q")
    assert primary.wait(timeout=20) == 0
    bus.close()
    print("PASS forwarded secondary: intent and readiness remain untouched", flush=True)


def quiet_launch_notice(binary, output, env, spawn, wait, windows, run):
    """A hidden, tray-resident launch of a completed profile shows the notice for 5 s."""
    bus = start_tray(output, env, spawn, wait)
    profile = output / "dark"
    app = spawn([str(binary), "--live", "--scene", "idle",
                 "--history-root", str(profile / "history"),
                 "--settings-file", str(profile / "fresh settings %.json"), "--appearance", "dark"])
    notice = wait(lambda: windows(app.pid, "^Captures is running$"), "quiet launch notice")[0]
    shown = time.monotonic()
    # eframe maps the root for its first paint before hiding it.
    wait(lambda: not windows(app.pid), "quiet launch root hidden")
    time.sleep(.6)
    geometry = run("xdotool", "getwindowgeometry", "--shell", notice).decode()
    assert "WIDTH=352" in geometry and "HEIGHT=110" in geometry, geometry
    assert run("xdotool", "getwindowfocus").decode().strip() != notice, "launch notice took focus"
    run("import", "-window", notice, str(output / "startup-notice-quiet.png"))
    time.sleep(max(0, 3.5 - (time.monotonic() - shown)))
    assert windows(app.pid, "^Captures is running$"), "quiet notice expired too early"
    wait(lambda: not windows(app.pid, "^Captures is running$"), "5-second quiet notice expiry")
    elapsed = time.monotonic() - shown
    assert 4 <= elapsed <= 9, elapsed
    assert not windows(app.pid), "notice expiry showed the root window"
    app.terminate()
    app.wait(timeout=20)
    for appearance, long in (("light", False), ("dark", True), ("light", True)):
        label = f"{'long' if long else 'normal'}-{appearance}"
        settings = output / f"shortcut-{label}.json"
        value = json.loads((profile / "fresh settings %.json").read_text())
        if long:
            value["new_capture_shortcut"] = "Control+Alt+Shift+Super+F12"
        settings.write_text(json.dumps(value))
        app = spawn([str(binary), "--live", "--scene", "idle", "--settings-file", str(settings),
                     "--history-root", str(output / f"history-{label}"), "--appearance", appearance])
        notice = wait(lambda: windows(app.pid, "^Captures is running$"), f"{label} notice")[0]
        wait(lambda: not windows(app.pid), f"{label} quiet root hidden")
        time.sleep(.6)
        geometry = dict(line.split("=", 1) for line in
                        run("xdotool", "getwindowgeometry", "--shell", notice).decode().splitlines())
        assert int(geometry["WIDTH"]) == 352, geometry
        assert int(geometry["HEIGHT"]) > 110 if long else int(geometry["HEIGHT"]) == 110, geometry
        assert run("xdotool", "getwindowfocus").decode().strip() != notice, "wrapped notice took focus"
        run("import", "-window", notice, str(output / f"startup-notice-{label}.png"))
        run("xdotool", "mousemove", "--sync", "--window", notice, "303", str(int(geometry["HEIGHT"]) // 2),
            "mousemove_relative", "--sync", "1", "0",
            "sleep", ".15", "mousedown", "1", "sleep", ".15", "mouseup", "1")
        wait(lambda: not windows(app.pid, "^Captures is running$"), "wrapped notice Close")
        assert not windows(app.pid), "Close activated the hidden root"
        app.terminate()
        app.wait(timeout=20)
        print(f"PASS {label}: measured shortcut layout, nonactivating Close", flush=True)
    bus.close()
    print(f"PASS quiet tray launch: hidden root, nonactivating notice, expired after {elapsed:.1f}s", flush=True)


if __name__ == "__main__":
    main()
