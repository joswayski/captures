#!/usr/bin/python3
"""Stock GNOME window-only portal acquisition and native Window → History.

Disposable HOME/XDG directories, session bus, software Mutter and PipeWire.
No installed profile, patched portal, display fallback or production policy edit.
Mutter's private RemoteDesktop API injects input only; Captures uses the public
portal and its granted remote. See gnome-window-verification.md for limits.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
import gi

from wayland_screenshot_smoke import DESKTOP, DESKTOP_PATH, ready, rgba, stop, wait_owner
from wayland_video_smoke import SCREENCAST

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib

TITLE = "Captures asymmetric window"
PHASES = (((35, 69, 103), (211, 37, 81), (51, 173, 29), (73, 41, 197)),
          ((181, 23, 57), (31, 211, 63), (93, 17, 191), (207, 127, 41)))
WIDTH, HEIGHT = 320, 200


def wait(predicate, description, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.05)
    raise AssertionError(f"Timed out: {description}")


def window_fixture():
    gi.require_version("Gtk", "3.0")
    from gi.repository import Gtk
    windows = []
    phase = 0
    for title in (TITLE, "Captures decoy window"):
        window = Gtk.Window(title=title)
        window.set_decorated(False)
        window.set_resizable(False)
        window.set_default_size(WIDTH, HEIGHT)
        area = Gtk.DrawingArea()
        window.add(area)
        def draw(widget, context, decoy=title != TITLE):
            colors = ((13, 231, 199),) * 4 if decoy else PHASES[phase]
            for rectangle, color in zip(((0, 0, 117, 73), (117, 0, 203, 73),
                                         (0, 73, 117, 127), (117, 73, 203, 127)), colors):
                context.set_source_rgb(*(value / 255 for value in color))
                context.rectangle(*rectangle)
                context.fill()
        area.connect("draw", draw)
        window.show_all()
        windows.append(window)
    def advance():
        nonlocal phase
        phase = 1 - phase
        for window in windows:
            window.get_child().queue_draw()
        return True
    GLib.timeout_add(120, advance)
    GLib.idle_add(lambda: print("READY", flush=True))
    Gtk.main()


def observer(path):
    """Read-only public bus traffic, including directed portal responses."""
    DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    log = path.open("w", buffering=1)
    def received(connection, message):
        if message.get_interface() in (SCREENCAST, "org.freedesktop.portal.Request", "org.freedesktop.portal.Session"):
            log.write(json.dumps({"interface": message.get_interface(), "member": message.get_member(),
                                  "sender": message.get_sender(), "path": message.get_path(),
                                  "args": message.get_args_list()}) + "\n")
    bus.add_message_filter(received)
    bus.get_object("org.freedesktop.DBus", "/org/freedesktop/DBus").BecomeMonitor(
        [f"interface='{interface}'" for interface in (SCREENCAST, "org.freedesktop.portal.Request", "org.freedesktop.portal.Session")],
        dbus.UInt32(0), dbus_interface="org.freedesktop.DBus.Monitoring")
    print("READY", flush=True)
    GLib.MainLoop().run()


def tray_fixture():
    """Invisible SNI host fixture; only the tray, never capture, is simulated."""
    DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    name = dbus.service.BusName("org.kde.StatusNotifierWatcher", bus)
    class Watcher(dbus.service.Object):
        items = []
        @dbus.service.method("org.kde.StatusNotifierWatcher", in_signature="s", sender_keyword="sender")
        def RegisterStatusNotifierItem(self, service, sender):
            item = sender + service if service.startswith("/") else service
            self.items.append(item)
            self.StatusNotifierItemRegistered(item)
        @dbus.service.signal("org.kde.StatusNotifierWatcher", signature="s")
        def StatusNotifierItemRegistered(self, item):
            pass
        @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="ss", out_signature="v")
        def Get(self, interface, property):
            return {"IsStatusNotifierHostRegistered": dbus.Boolean(True),
                    "RegisteredStatusNotifierItems": dbus.Array(self.items, signature="s"),
                    "ProtocolVersion": dbus.Int32(0)}[property]
        @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="s", out_signature="a{sv}")
        def GetAll(self, interface):
            return {key: self.Get(interface, key) for key in
                    ("IsStatusNotifierHostRegistered", "RegisteredStatusNotifierItems", "ProtocolVersion")}
    watcher = Watcher(name, "/StatusNotifierWatcher")
    print("READY", flush=True)
    GLib.MainLoop().run()
    del watcher


def descendants(item):
    yield item
    for index in range(item.get_child_count()):
        yield from descendants(item.get_child_at_index(index))


def chooser():
    desktop = Atspi.get_desktop(0)
    for index in range(desktop.get_child_count()):
        app = desktop.get_child_at_index(index)
        if app.get_name() == "xdg-desktop-portal-gnome":
            return next((item for item in descendants(app) if item.get_name() == "Screen Share"
                         and item.get_role_name() == "frame"), None)
    return None


class Input:
    def __init__(self, bus):
        remote = bus.get_object("org.gnome.Mutter.RemoteDesktop", "/org/gnome/Mutter/RemoteDesktop")
        path = remote.CreateSession(dbus_interface="org.gnome.Mutter.RemoteDesktop")
        proxy = bus.get_object("org.gnome.Mutter.RemoteDesktop", path)
        self.session = dbus.Interface(proxy, "org.gnome.Mutter.RemoteDesktop.Session")
        identifier = proxy.Get("org.gnome.Mutter.RemoteDesktop.Session", "SessionId",
                               dbus_interface="org.freedesktop.DBus.Properties")
        path = bus.get_object("org.gnome.Mutter.ScreenCast", "/org/gnome/Mutter/ScreenCast").CreateSession(
            {"remote-desktop-session-id": identifier}, dbus_interface="org.gnome.Mutter.ScreenCast")
        self.stream = bus.get_object("org.gnome.Mutter.ScreenCast", path).RecordMonitor(
            "", {}, dbus_interface="org.gnome.Mutter.ScreenCast.Session")
        self.session.Start()
    def key(self, key):
        self.session.NotifyKeyboardKeysym(key, True)
        time.sleep(.05)
        self.session.NotifyKeyboardKeysym(key, False)
    def click(self, x, y):
        self.session.NotifyPointerMotionAbsolute(str(self.stream), float(x), float(y))
        time.sleep(.1)
        self.session.NotifyPointerButton(272, True)
        time.sleep(.05)
        self.session.NotifyPointerButton(272, False)
    def choose(self, share, title=TITLE, screenshot=None):
        frame = wait(chooser, "stock GNOME Screen Share chooser")
        time.sleep(.3)
        nodes = list(descendants(frame))
        if share:
            rows = [item for item in nodes if item.get_role_name() == "list item"]
            if title is None:
                assert len(rows) == 1, "display control must have exactly one real monitor"
                row = rows[0]
            else:
                row = next(item for item in rows if item.get_name() == title)
            # GTK4 4.8 exposes window-local SCREEN extents on Wayland. These
            # coordinates drive only this pinned, centered 1280x900 test chooser;
            # they are never a source/window geometry oracle. Output pixels and
            # actual public source_type establish what was captured instead.
            bounds = frame.get_component_iface().get_extents(Atspi.CoordType.SCREEN)
            rect = row.get_component_iface().get_extents(Atspi.CoordType.SCREEN)
            x, y = (1280 - bounds.width) / 2, (900 + 32 - bounds.height) / 2
            self.click(x + rect.x + rect.width / 2, y + rect.y + rect.height / 2)
            button = next(item for item in nodes if item.get_name() == "_Share Share")
            wait(lambda: button.get_state_set().contains(Atspi.StateType.SENSITIVE), "Share enabled after row click")
        else:
            bounds = frame.get_component_iface().get_extents(Atspi.CoordType.SCREEN)
            x, y = (1280 - bounds.width) / 2, (900 + 32 - bounds.height) / 2
            button = next(item for item in nodes if item.get_name() == "_Cancel Cancel")
        if screenshot:
            screenshot()
        rect = button.get_component_iface().get_extents(Atspi.CoordType.SCREEN)
        self.click(x + rect.x + rect.width / 2, y + rect.y + rect.height / 2)
        wait(lambda: not chooser(), "stock chooser dismissed")


def expected_pixels(phase):
    colors = PHASES[phase]
    return bytes(value for y in range(HEIGHT) for x in range(WIDTH)
                 for value in (*colors[(2 if y >= 73 else 0) + (1 if x >= 117 else 0)], 255))


def check_pixels(path):
    width, height = map(int, subprocess.check_output(["identify", "-format", "%w %h", str(path)], text=True).split())
    pixels = rgba(path)
    assert width >= WIDTH and height >= HEIGHT, (width, height)
    # Continue collecting native metadata/cleanup evidence after a size defect,
    # but the final contract verdict still fails. Never trim the published file.
    interior = b"".join(pixels[y * width * 4:(y * width + WIDTH) * 4] for y in range(HEIGHT))
    matches = [phase for phase in range(2) if interior == expected_pixels(phase)]
    assert len(matches) == 1, f"{path}: selected-window pixels differ"
    return {"phase": matches[0], "observed_size": [width, height], "expected_size": [WIDTH, HEIGHT],
            "exact_window_pixels": WIDTH * HEIGHT, "window_contract": (width, height) == (WIDTH, HEIGHT),
            "outside_window_colors": sorted({tuple(pixels[offset:offset + 4])
                                              for offset in range(0, len(pixels), 4)
                                              if offset // 4 // width >= HEIGHT or offset // 4 % width >= WIDTH})}


def traffic(path):
    return [json.loads(line) for line in path.read_text().splitlines()]


def check_traffic(events, successful, source_type=2):
    selects = [event for event in events if event["member"] == "SelectSources"]
    assert len(selects) == 1 and selects[0]["args"][1]["types"] == source_type, selects
    assert selects[0]["args"][1]["multiple"] == 0, selects
    streams = [event["args"][1]["streams"] for event in events
               if event["member"] == "Response" and "streams" in event["args"][1]]
    if successful:
        assert len(streams) == 1 and len(streams[0]) == 1, streams
        assert streams[0][0][1]["source_type"] == source_type, streams
    assert any(event["member"] == "Close" and event["interface"] == "org.freedesktop.portal.Session"
               for event in events), "client did not close its real portal session"
    return streams


def run(args, output, root, env, bus, spawn, backend, results):
    observer_log = output / "public-portal.jsonl"
    monitor = spawn([sys.executable, __file__, "--observe", str(observer_log)], announce=True)
    ready(monitor)
    source = spawn([sys.executable, __file__, "--window-fixture"], announce=True)
    ready(source)
    pointer = Input(bus)
    pointer.key(0xff1b)  # GNOME's initial overview, not consent.
    time.sleep(.5)
    bus.request_name("org.gnome.Screenshot")
    def shot(name):
        success, path = bus.get_object("org.gnome.Shell.Screenshot", "/org/gnome/Shell/Screenshot").Screenshot(
            False, False, str(output / (name + ".png")), dbus_interface="org.gnome.Shell.Screenshot")
        assert success and Path(path).is_file(), path
    def idle():
        objects = json.loads(subprocess.check_output(["pw-dump"], env=env, timeout=3))
        return not any(item.get("info", {}).get("props", {}).get("media.name") == "Captures portal video" for item in objects)
    def probe(name, binary, extra=(), consent=True, success=True, revoke=False):
        start = len(traffic(observer_log))
        path = output / (name + ".png")
        process = spawn([str(binary), "--output", str(path), "--target", "window", *extra], announce=True)
        if consent is not None:
            pointer.choose(consent, screenshot=lambda: shot(name + "-chooser"))
        else:
            wait(chooser, "real chooser pending before Request.Close")
            shot(name + "-chooser")
        lost_at = None
        if revoke:
            wait(lambda: not idle(), "active granted source before removing its selected window")
            time.sleep(.3)
            lost_at = time.monotonic()
            stop(source)
        stdout, stderr = process.communicate(timeout=10)
        result = {"case": name, "returncode": process.returncode, "stdout": stdout.decode(), "stderr": stderr.decode()}
        if lost_at:
            result["seconds_after_window_loss"] = round(time.monotonic() - lost_at, 3)
        assert process.returncode == (0 if success else 1), result
        wait(idle, "Captures PipeWire nodes removed")
        time.sleep(.1)
        result["streams"] = check_traffic(traffic(observer_log)[start:], consent is True)
        if path.exists():
            assert consent is True and success, "cancel/failure published an image"
            result.update(check_pixels(path))
        else:
            assert consent is not True or not success, result
        print(json.dumps(result), flush=True)
        return result
    results.append(probe("video", args.video_binary, ("--frames", "12")))
    video = json.loads(results[-1]["stdout"])
    assert video["corner_colors"] == [[35, 69, 103, 255], [181, 23, 57, 255]], video
    results.append(probe("still", args.screenshot_binary))
    results.append(probe("chooser-cancel", args.screenshot_binary, consent=False))
    assert json.loads(results[-1]["stdout"])["event"] == "cancelled"
    results.append(probe("stream-cancel", args.video_binary, ("--frames", "120", "--cancel-after-ms", "2000"), success=False))
    assert "cancelled" in results[-1]["stderr"].lower()
    results.append(probe("window-loss", args.video_binary, ("--frames", "120"), success=False, revoke=True))
    source = spawn([sys.executable, __file__, "--window-fixture"], announce=True)
    ready(source)
    results.append(probe("repeat", args.screenshot_binary))
    if args.recording_binary:
        start = len(traffic(observer_log))
        recording = output / "recording"
        process = spawn([str(args.recording_binary), "--output", str(recording), "--target", "window",
                         "--duration-ms", "900"], announce=True)
        pointer.choose(True, screenshot=lambda: shot("recording-chooser"))
        stdout, stderr = process.communicate(timeout=20)
        assert process.returncode == 0, (stdout, stderr)
        event = next(json.loads(line) for line in stdout.splitlines() if json.loads(line)["event"] == "ready")
        metadata_paths = list(recording.glob("history/*/metadata.json"))
        assert len(metadata_paths) == 1, metadata_paths
        metadata = json.loads(metadata_paths[0].read_text())
        assert metadata == event["entry"] and metadata["target"] == {"type": "portal_window"}, metadata
        assert metadata["kind"] == "video" and metadata["mode"] is None, metadata
        assert not any(key in metadata["target"] for key in ("window_id", "display_id", "bounds", "rect")), metadata
        subprocess.run(["ffmpeg", "-v", "error", "-i", event["path"], "-fps_mode", "passthrough", "-frames:v", "100",
                        str(recording / "decoded-%02d.png")], check=True, timeout=10)
        phases = set()
        for path in sorted(recording.glob("decoded-*.png")):
            pixels = rgba(path)
            colors = [tuple(pixels[(y * metadata["width"] + x) * 4: (y * metadata["width"] + x) * 4 + 3])
                      for x, y in ((20, 20), (200, 20), (20, 150), (200, 150))]
            matches = [phase for phase in range(2) if all(abs(actual - expected) <= 6
                       for color, reference in zip(colors, PHASES[phase]) for actual, expected in zip(color, reference))]
            assert len(matches) == 1, colors
            phases.add(matches[0])
        assert phases == {0, 1}, phases
        wait(idle, "recording source released")
        results.append({"case": "recording-history", "metadata": metadata,
                        "streams": check_traffic(traffic(observer_log)[start:], True),
                        "decoded_phases": sorted(phases), "color_tolerance": 6,
                        "observed_size": [metadata["width"], metadata["height"]],
                        "window_contract": (metadata["width"], metadata["height"]) == (WIDTH, HEIGHT),
                        "not_native_screenshot_history": True})
    start = len(traffic(observer_log))
    control = output / "display-control.png"
    process = spawn([str(args.video_binary), "--output", str(control), "--frames", "3", "--target", "display"], announce=True)
    pointer.choose(True, title=None, screenshot=lambda: shot("display-control-chooser"))
    stdout, stderr = process.communicate(timeout=10)
    assert process.returncode == 0, stderr
    event = json.loads(stdout)
    assert [event["width"], event["height"]] == [1280, 900], event
    assert bytes((13, 231, 199, 255)) in rgba(control), "display control must contain the independent decoy"
    wait(idle, "display control source released")
    results.append({"case": "display-control", "streams": check_traffic(traffic(observer_log)[start:], True, 1), "event": event})
    if args.binary:
        from wayland_lifecycle_smoke import menu_action, watcher
        for appearance in ("dark", "light"):
            tray = spawn([sys.executable, __file__, "--tray-fixture"], announce=True)
            ready(tray)
            profile = root / appearance
            profile.mkdir()
            history = profile / "history"
            settings = profile / "settings.json"
            settings.write_text(json.dumps({"onboarding_completed": True, "settings_schema_version": 5,
                                           "appearance": appearance, "launch_at_login": False,
                                           "auto_copy_to_clipboard": False, "screenshot_countdown_seconds": 1,
                                           "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
                                           "display_shortcut": "Ctrl+Shift+F9",
                                           "show_cursor_in_screenshots": False,
                                           "output_directory": str(profile / "exports")}))
            with (output / f"native-{appearance}.log").open("w") as log:
                app = spawn([str(args.binary), "--live", "--open-history", "--open-preferences",
                             "--settings-file", str(settings), "--history-root", str(history), "--quit-after", "60"], log=log)
                try:
                    wait(lambda: watcher(bus) and watcher(bus).Get("org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems"), "native real tray registration")
                    wait(lambda: '"action":"StartupNoticeAndPreferences"' in (output / f"native-{appearance}.log").read_text()
                         or '"action":"Preferences"' in (output / f"native-{appearance}.log").read_text(), "completed setup and native launch")
                    before = set(history.rglob("*"))
                    start = len(traffic(observer_log))
                    menu_action(bus, "Screenshot Window")
                    # GNOME 43 disables ScreenShield without GDM on the
                    # private system bus. No login1 session exists there,
                    # so Captures must fail closed; do not fake unlock state.
                    closed = True
                    try:
                        wait(lambda: any(event["member"] == "Close" for event in traffic(observer_log)[start:]), "native session-unavailable cancellation")
                    except AssertionError:
                        closed = False  # Bound this setup limit; still run the other controls.
                    time.sleep(.5)
                    assert set(history.rglob("*")) == before, "unavailable session published History files"
                    assert not any(event["member"] == "OpenPipeWireRemote" for event in traffic(observer_log)[start:]), "unknown session acquired pixels"
                    results.append({"case": f"native-{appearance}-window", "published_images": 0,
                                    "successful_native_history_verified": False,
                                    "session_close_before_shutdown": closed,
                                    "setup_limit": "No real login1 session or GNOME ScreenShield on the private bus; no simulated unlock"})
                    wait(idle, "native portal video source released")
                    shot(f"native-{appearance}-after-window")
                finally:
                    stop(app)
                    stop(tray)
    # This stock GNOME 43 backend may crash while handling Request.Close;
    # keep it last so that a backend defect does not erase earlier evidence.
    results.append(probe("pending-cancel", args.screenshot_binary, ("--cancel-after-ms", "3000"), consent=None))
    assert json.loads(results[-1]["stdout"])["event"] == "cancelled"
    time.sleep(.5)
    results[-1]["backend_exit_after_cancel"] = backend.poll()
    results[-1]["backend_survived_cancel"] = backend.poll() is None
    start = len(traffic(observer_log))
    followup = output / "after-pending-cancel.png"
    process = spawn([str(args.screenshot_binary), "--output", str(followup), "--target", "window",
                     "--cancel-after-ms", "2000"], announce=True)
    stdout, stderr = process.communicate(timeout=8)
    assert process.returncode in (0, 1) and not followup.exists(), (stdout, stderr)
    if process.returncode == 0:
        assert json.loads(stdout)["event"] == "cancelled", stdout
    results.append({"case": "after-pending-cancel", "returncode": process.returncode,
                    "stdout": stdout.decode(), "stderr": stderr.decode(),
                    "traffic": traffic(observer_log)[start:], "published_images": 0})
    pointer.session.Stop()
    bus.release_name("org.gnome.Screenshot")
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="New evidence directory; never a profile")
    parser.add_argument("--video-binary", type=Path)
    parser.add_argument("--screenshot-binary", type=Path)
    parser.add_argument("--recording-binary", type=Path, help="Also inspect real MP4/History metadata, not the native screenshot gate")
    parser.add_argument("--binary", type=Path, help="Also exercise the live native Window action")
    parser.add_argument("--window-fixture", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--tray-fixture", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--observe", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.window_fixture:
        window_fixture()
        return
    if args.tray_fixture:
        tray_fixture()
        return
    if args.observe:
        observer(args.observe)
        return
    if not (args.output and args.video_binary and args.screenshot_binary):
        parser.error("--output, --video-binary and --screenshot-binary are required")
    for name in ("output", "video_binary", "screenshot_binary", "recording_binary", "binary"):
        if getattr(args, name):
            setattr(args, name, getattr(args, name).resolve())
    args.output.mkdir(parents=True)  # Refuse existing output; never overwrite evidence.
    services = []
    report = {"stock_backend": True, "display_unset": True, "passed": False, "results": [],
              "checkout_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
              "binary_sha256": {str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in
                                (args.video_binary, args.screenshot_binary, args.recording_binary, args.binary,
                                 Path("/usr/bin/gnome-shell"), Path("/usr/libexec/xdg-desktop-portal-gnome")) if path},
              "package_versions": subprocess.check_output(["dpkg-query", "-W", "-f=${Package} ${Version}\n",
                                     "gnome-shell", "mutter", "xdg-desktop-portal-gnome", "xdg-desktop-portal",
                                     "pipewire", "wireplumber", "gjs"], text=True).splitlines()}
    with tempfile.TemporaryDirectory(prefix="captures-stock-gnome-") as temporary:
        root = Path(temporary)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        env = os.environ.copy()
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "SWAYSOCK", "XDG_DESKTOP_PORTAL_DIR", "NO_AT_BRIDGE"):
            env.pop(key, None)
        env.update(HOME=str(root), XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(root / "config"),
                   XDG_DATA_HOME=str(root / "data"), XDG_CACHE_HOME=str(root / "cache"),
                   XDG_STATE_HOME=str(root / "state"), XDG_CURRENT_DESKTOP="GNOME", XDG_SESSION_TYPE="wayland",
                   LIBGL_ALWAYS_SOFTWARE="1", LP_NUM_THREADS="2", GDK_BACKEND="wayland", GTK_A11Y="atspi",
                   GSETTINGS_BACKEND="keyfile", WGPU_BACKEND="gl", CAPTURES_NATIVE_LAYOUT_PROBE="1",
                   CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER="1")
        with (args.output / "services.log").open("w") as log:
            def spawn(command, announce=False, log=log):
                process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE if announce else log,
                                           stderr=subprocess.PIPE if announce else log, start_new_session=True)
                services.append(process)
                return process
            daemon = spawn(["dbus-daemon", "--session", "--nofork", "--print-address=1"], announce=True)
            env["DBUS_SESSION_BUS_ADDRESS"] = daemon.stdout.readline().decode().strip()
            env["DBUS_SYSTEM_BUS_ADDRESS"] = env["DBUS_SESSION_BUS_ADDRESS"]
            bus = dbus.bus.BusConnection(env["DBUS_SESSION_BUS_ADDRESS"])
            try:
                spawn(["pipewire"])
                wait(lambda: (runtime / "pipewire-0").is_socket(), "private PipeWire socket")
                spawn(["wireplumber"])
                shell = spawn(["gnome-shell", "--wayland", "--headless", "--virtual-monitor", "1280x900",
                               "--no-x11", "--wayland-display", "captures-gnome"])
                wait_owner(bus, "org.gnome.Mutter.ScreenCast", shell)
                wait(lambda: (runtime / "captures-gnome").is_socket(), "private Mutter Wayland socket")
                env["WAYLAND_DISPLAY"] = "captures-gnome"
                os.environ.update(env)  # AT-SPI must connect to this private bus, not the caller's.
                bus.get_object("org.freedesktop.DBus", "/org/freedesktop/DBus").UpdateActivationEnvironment(
                    env, dbus_interface="org.freedesktop.DBus")
                wait_owner(bus, "org.gnome.Shell.Introspect", shell)
                report["real_gnome_screenshield_available"] = bus.name_has_owner("org.gnome.Shell.ScreenShield")
                report["real_login1_available"] = bus.name_has_owner("org.freedesktop.login1")
                report["native_screenshot_requested"] = bool(args.binary)
                assert not report["real_gnome_screenshield_available"], "fixture session changed: add native success assertions"
                backend = spawn(["/usr/libexec/xdg-desktop-portal-gnome", "--verbose"])
                wait_owner(bus, "org.freedesktop.impl.portal.desktop.gnome", backend)
                front = spawn(["/usr/libexec/xdg-desktop-portal", "--verbose"])
                wait_owner(bus, DESKTOP, front)
                portal = bus.get_object(DESKTOP, DESKTOP_PATH)
                capability = portal.GetAll(SCREENCAST, dbus_interface="org.freedesktop.DBus.Properties")
                report["public_capability"] = dict(capability)
                print(json.dumps(report), flush=True)
                assert capability["AvailableSourceTypes"] & 2 and capability["version"] >= 3, capability
                time.sleep(1)
                run(args, args.output, root, env, bus, spawn, backend, report["results"])
                report["passed"] = all(result.get("window_contract", True) and result.get("backend_survived_cancel", True)
                                       and result.get("successful_native_history_verified", True)
                                       for result in report["results"])
            except Exception as error:
                report["failure"] = str(error)
            finally:
                bus.close()
                for process in reversed(services):
                    stop(process)
                    # Also stop D-Bus-activated children on this disposable bus.
                    try:
                        os.killpg(process.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                subprocess.run(["fusermount3", "-uz", str(runtime / "doc")], capture_output=True)
                report["owned_processes_stopped"] = all(process.poll() is not None for process in services)
                (args.output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    report["private_profile_removed"] = not root.exists()
    (args.output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    hashes = {str(path.relative_to(args.output)): hashlib.sha256(path.read_bytes()).hexdigest()
              for path in sorted(args.output.rglob("*")) if path.is_file()}
    (args.output / "SHA256SUMS").write_text("".join(f"{digest}  {name}\n" for name, digest in hashes.items()))
    print(json.dumps(report), flush=True)
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
