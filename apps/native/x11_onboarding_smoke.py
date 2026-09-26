#!/usr/bin/env python3
"""Real native first-run input on private X11; no permission bypass or real user data."""
import argparse
import json
import os
import shutil
from pathlib import Path
import subprocess
import time

from instance_smoke import png


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = {**os.environ, "WGPU_BACKEND": "gl", "WINIT_X11_SCALE_FACTOR": "1", "XDG_SESSION_TYPE": "x11"}
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

        try:
            server = spawn(["Xvfb", "-displayfd", "1", "-screen", "0", "1280x900x24", "-nolisten", "tcp"], True)
            env["DISPLAY"] = ":" + server.stdout.readline().decode().strip()
            run("xdpyinfo")
            spawn(["openbox", "--sm-disable"])
            wait(lambda: b"window id" in run("xprop", "-root", "_NET_SUPPORTING_WM_CHECK"), "window manager ready")
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
                assert not list(history.glob("*/metadata.json")), "cold media imported before setup"
                secondary = subprocess.run(common + ["--", str(forwarded)], env=env, capture_output=True, timeout=15)
                assert secondary.returncode == 0, secondary.stderr
                run("xdotool", "key", "super+shift+s")
                time.sleep(.4)
                assert not list(history.glob("*/metadata.json")), "capture/forwarding bypassed setup"
                assert len(windows(app.pid, ".*")) == 1, "capture selector/editor opened before setup"
                click(window, 732, 476)  # Start capturing, right-aligned under the cards.
                wait(lambda: settings.exists() and json.loads(settings.read_text()).get("onboarding_completed"),
                     "setup completion persisted")
                wait(lambda: len(list(history.glob("*/metadata.json"))) == 2, "queued cold and forwarded media imported")
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
                again = spawn(common)
                window = wait(lambda: windows(again.pid), "completed profile workspace")[0]
                time.sleep(1)
                assert not windows(again.pid, "^Captures is running$"), "visible relaunch showed the launch notice"
                run("import", "-window", window, str(output / f"completed-{appearance}.png"))
                accepted_settings = settings.read_bytes()
                recovery_media = root / "during permission recovery.png"
                recovery_media.write_bytes(png(19, 9))
                click(window, 835, 126)  # Capture permissions, without an OS prompt.
                time.sleep(.5)
                secondary = subprocess.run(common + ["--", str(recovery_media)], env=env,
                                           capture_output=True, timeout=15)
                assert secondary.returncode == 0, secondary.stderr
                click(window, 196, 18)  # Navigation behind the dialog stays disabled.
                run("xdotool", "key", "super+shift+s")
                time.sleep(.5)
                assert len(list(history.glob("*/metadata.json"))) == 2, "recovery imported queued media"
                assert len(windows(again.pid, ".*")) == 1, "recovery launched capture/editor"
                run("import", "-window", window, str(output / f"permission-recovery-{appearance}.png"))
                click(window, 546, 449)  # Refresh status (secondary) is prompt-free and does not complete setup.
                time.sleep(.4)
                assert settings.read_bytes() == accepted_settings, "recovery changed setup/settings"
                click(window, 681, 449)  # Done (primary card action), including when no upfront permission is required.
                wait(lambda: len(list(history.glob("*/metadata.json"))) == 3, "recovery releases queued media")
                assert recovery_media.read_bytes() == png(19, 9), "recovery changed the input"
                time.sleep(1)
                click(window, 196, 18)  # Real Preferences navigation, absent on setup.
                time.sleep(.4)
                run("import", "-window", window, str(output / f"preferences-{appearance}.png"))
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
            click(window, 604, 500)  # Retry setup, beside the disabled primary.
            time.sleep(.5)
            run("import", "-window", window, str(output / "onboarding-retry.png"))
            assert not broken.exists(), "retry silently completed setup"
            click(window, 732, 476)
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


def quiet_launch_notice(binary, output, env, spawn, wait, windows, run):
    """A hidden, tray-resident launch of a completed profile shows the notice for 5 s."""
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
    profile = output / "dark"
    app = spawn([str(binary), "--live", "--scene", "idle",
                 "--history-root", str(profile / "history"),
                 "--settings-file", str(profile / "fresh settings %.json"), "--appearance", "dark"])
    notice = wait(lambda: windows(app.pid, "^Captures is running$"), "quiet launch notice")[0]
    shown = time.monotonic()
    # eframe maps the root for its first paint before hiding it.
    wait(lambda: not windows(app.pid), "quiet launch root hidden")
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
    bus.close()
    print(f"PASS quiet tray launch: hidden root, nonactivating notice, expired after {elapsed:.1f}s", flush=True)


if __name__ == "__main__":
    main()
