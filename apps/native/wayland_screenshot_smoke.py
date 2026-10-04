#!/usr/bin/python3
"""Portal protocol regressions and real headless-Sway still acquisition.

Never uses the caller's display, bus, permissions or data. A private /tmp mount
also contains old wlr backends' fixed /tmp/out.png. System Python needs dbus/gi.
The native UI visibility/placement and recording gates are not exercised here.
"""

import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import time

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib


DESKTOP = "org.freedesktop.portal.Desktop"
DESKTOP_PATH = "/org/freedesktop/portal/desktop"
REQUEST = "org.freedesktop.portal.Request"
SCREENSHOT = "org.freedesktop.portal.Screenshot"
PIXELS = bytes([1, 2, 3, 4, 250, 17, 99, 255, 0, 127, 255, 63,
                19, 211, 7, 128, 88, 44, 222, 200, 5, 6, 7, 8])


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)


def ready(process):
    if not select.select([process.stdout], [], [], 10)[0]:
        raise RuntimeError("service did not become ready")
    assert process.stdout.readline().strip() == b"READY"


def fixture(mode, uri, log):
    DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    name = dbus.service.BusName(DESKTOP, bus)

    def record(event):
        with open(log, "a") as file:
            file.write(json.dumps(event) + "\n")

    class Request(dbus.service.Object):
        @dbus.service.method(REQUEST, in_signature="", out_signature="")
        def Close(self):
            record({"event": "close"})

        @dbus.service.signal(REQUEST, signature="ua{sv}")
        def Response(self, status, results):
            pass

    class Portal(dbus.service.Object):
        @dbus.service.method(SCREENSHOT, in_signature="sa{sv}", out_signature="o", sender_keyword="sender")
        def Screenshot(self, parent, options, sender):
            assert not parent and not options["modal"] and not options["interactive"]
            token = str(options["handle_token"])
            assert token.startswith("captures_") and token.removeprefix("captures_").isalnum()
            record({"event": "request", "token": token})
            path = f"{DESKTOP_PATH}/request/{sender[1:].replace('.', '_')}/{token}"
            if mode == "legacy":
                path += "_legacy"
            request = Request(name, path)
            self.requests.append(request)
            if mode == "method-error":
                raise dbus.exceptions.DBusException("Reply lost", name="org.freedesktop.portal.Error.Failed")
            if mode in ("late-success", "method-timeout"):
                time.sleep(.3)
            if mode == "method-timeout":
                return dbus.ObjectPath(path)
            if mode == "wait":
                return dbus.ObjectPath(path)
            # A peer's directed, same-path signal must not win over the portal.
            peer = dbus.bus.BusConnection(os.environ["DBUS_SESSION_BUS_ADDRESS"])
            spoof = dbus.lowlevel.SignalMessage(path, REQUEST, "Response")
            spoof.set_destination(sender)
            spoof.append(dbus.UInt32(0), dbus.Dictionary({"uri": "file:///spoof.png"}, signature="sv"), signature="ua{sv}")
            peer.send_message(spoof)
            peer.flush()
            peer.close()
            # A real-owner response for a different request must also be ignored.
            unrelated = Request(name, path + "_unrelated")
            self.requests.append(unrelated)
            unrelated.Response(0, {"uri": "file:///unrelated.png"})
            status = 1 if mode == "cancel" else 2 if mode == "failure" else 0
            results = {} if mode == "missing-uri" else {"uri": uri}
            # Emit before the method reply to expose subscribe-after-call races.
            request.Response(status, results)
            return dbus.ObjectPath(path)

    portal = Portal(name, DESKTOP_PATH)
    portal.requests = []
    print("READY", flush=True)
    GLib.MainLoop().run()


def rgba(path):
    return subprocess.check_output(["convert", str(path), "-depth", "8", "rgba:-"], timeout=5)


def protocols(binary, root, env):
    source = root / "still é #1.png"
    subprocess.run(["convert", "-size", "3x2", "-depth", "8", "rgba:-", str(source)], input=PIXELS, check=True)
    original = source.read_bytes()
    tokens = []
    outcomes = []
    for mode in ("success", "legacy", "cancel", "failure", "missing-uri", "remote-uri", "wait-cancel", "wait-timeout", "method-error", "method-timeout", "late-success"):
        log = root / f"{mode}.jsonl"
        output = root / f"{mode}.png"
        uri = "https://example.com/still.png" if mode == "remote-uri" else source.as_uri()
        service = subprocess.Popen([sys.executable, __file__, "--fixture", "wait" if mode.startswith("wait-") else mode,
                                    "--uri", uri, "--log", str(log)], env=env, stdout=subprocess.PIPE)
        try:
            ready(service)
            arguments = [binary, "--output", str(output), "--timeout-ms", "2000"]
            if mode in ("wait-cancel", "late-success"):
                arguments += ["--cancel-after-ms", "150"]
            elif mode in ("wait-timeout", "method-timeout"):
                arguments[-1] = "150"
            started = time.monotonic()
            result = subprocess.run(arguments, env=env, capture_output=True, text=True, timeout=4)
            elapsed = time.monotonic() - started
            events = [json.loads(line) for line in log.read_text().splitlines()]
            assert events[0]["event"] == "request", events
            tokens.append(events[0]["token"])
            if mode in ("success", "legacy"):
                assert result.returncode == 0, result.stderr
                assert json.loads(result.stdout) == {"event": "screenshot", "width": 3, "height": 2, "display_unset": True}
                assert rgba(output) == PIXELS
            elif mode in ("cancel", "wait-cancel", "late-success"):
                assert result.returncode == 0, result.stderr
                assert json.loads(result.stdout)["event"] == "cancelled"
                assert not output.exists()
            else:
                assert result.returncode == 1, (mode, result.stdout, result.stderr)
                assert not output.exists()
                if mode in ("wait-timeout", "method-timeout"):
                    assert "Timed out" in result.stderr
            if mode.startswith("wait-") or mode in ("method-error", "method-timeout", "late-success"):
                assert events[-1] == {"event": "close"}, events
                assert elapsed < 2, elapsed
            else:
                assert len(events) == 1, events
            assert source.read_bytes() == original, "portal-owned source was changed/removed"
            outcomes.append(mode)
        finally:
            stop(service)
    assert len(set(tokens)) == len(tokens), "request tokens were reused"
    # Pre-cancellation cannot contact or activate the portal.
    bus = dbus.bus.BusConnection(env["DBUS_SESSION_BUS_ADDRESS"])
    assert not bus.name_has_owner(DESKTOP)
    result = subprocess.run([binary, "--output", str(root / "pre-cancel.png"), "--cancel-after-ms", "0"], env=env,
                            capture_output=True, text=True, check=True, timeout=3)
    assert json.loads(result.stdout)["event"] == "cancelled"
    assert not bus.name_has_owner(DESKTOP)
    bus.close()
    print(json.dumps({"protocol_cases": outcomes + ["pre-cancel"], "exact_rgba": True, "source_retained": True}))


def wait_owner(bus, name, process):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if bus.name_has_owner(name):
            return
        assert process.poll() is None, f"{name} exited"
        time.sleep(.05)
    raise RuntimeError(f"{name} did not own its bus name")


def compositor_capture(binary, root, env):
    services = []
    log = (root / "real-portal.log").open("w")
    try:
        # An asymmetric native pixel fixture, independently encoded for swaybg.
        width, height = 310, 170
        colors = ((36, 52, 68), (173, 114, 39), (29, 99, 144), (90, 38, 118))
        expected = b"".join(bytes((*colors[(y >= 63) * 2 + (x >= 117)], 255))
                            for y in range(height) for x in range(width))
        background = root / "desktop.png"
        subprocess.run(["convert", "-size", f"{width}x{height}", "-depth", "8", "rgba:-", str(background)], input=expected, check=True)
        config = root / "sway.conf"
        config.write_text(f'output HEADLESS-1 resolution {width}x{height}\noutput * bg "{background}" stretch\nseat seat0 fallback true\n')
        sway = subprocess.Popen(["sway", "--unsupported-gpu", "--config", str(config)], env=env, stdout=log, stderr=log)
        services.append(sway)
        runtime = Path(env["XDG_RUNTIME_DIR"])
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            sockets = [path for path in runtime.glob("wayland-*") if path.is_socket()]
            if sockets:
                env["WAYLAND_DISPLAY"] = sockets[0].name
                break
            assert sway.poll() is None, "Sway exited"
            time.sleep(.05)
        else:
            raise RuntimeError("Sway did not create a socket")
        # wlr initializes its ScreenCast interface even for still-only requests.
        pipewire = subprocess.Popen(["pipewire"], env=env, stdout=log, stderr=log)
        services.append(pipewire)
        deadline = time.monotonic() + 5
        while not (runtime / "pipewire-0").is_socket():
            assert pipewire.poll() is None, "PipeWire exited"
            if time.monotonic() >= deadline:
                raise RuntimeError("PipeWire did not create a socket")
            time.sleep(.05)
        DBusGMainLoop(set_as_default=True)
        bus = dbus.bus.BusConnection(env["DBUS_SESSION_BUS_ADDRESS"])
        for executable, name in (("xdg-permission-store", "org.freedesktop.impl.portal.PermissionStore"),
                                 ("xdg-desktop-portal-gtk", "org.freedesktop.impl.portal.desktop.gtk"),
                                 ("xdg-desktop-portal-wlr", "org.freedesktop.impl.portal.desktop.wlr")):
            service = subprocess.Popen([f"/usr/libexec/{executable}"], env=env, stdout=log, stderr=log)
            services.append(service)
            wait_owner(bus, name, service)
        # Consent is granted only on the disposable bus/data profile, never the host.
        store = dbus.Interface(bus.get_object("org.freedesktop.impl.portal.PermissionStore",
                                             "/org/freedesktop/impl/portal/PermissionStore"),
                               "org.freedesktop.impl.portal.PermissionStore")
        store.SetPermission("screenshot", True, "screenshot", "", ["yes"])
        front = subprocess.Popen(["/usr/libexec/xdg-desktop-portal", "--verbose"], env=env, stdout=log, stderr=log)
        services.append(front)
        wait_owner(bus, DESKTOP, front)
        # Wait for the independently composed background, not just a socket.
        reference = root / "reference.png"
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            subprocess.run(["grim", str(reference)], env=env, check=True, timeout=3)
            if rgba(reference) == expected:
                break
            time.sleep(.1)
        else:
            raise AssertionError("compositor never displayed the expected desktop")
        output = root / "portal.png"
        result = subprocess.run([binary, "--output", str(output), "--timeout-ms", "5000"], env=env,
                                capture_output=True, text=True, timeout=8)
        assert result.returncode == 0, result.stderr + "\n" + (root / "real-portal.log").read_text()
        assert json.loads(result.stdout) == {"event": "screenshot", "width": width, "height": height, "display_unset": True}
        assert rgba(output) == expected, "portal capture differs from independently specified desktop pixels"
        backend = dbus.Interface(bus.get_object("org.freedesktop.impl.portal.desktop.wlr", DESKTOP_PATH),
                                 "org.freedesktop.DBus.Properties")
        version = int(backend.Get("org.freedesktop.impl.portal.Screenshot", "version"))
        # xdg-desktop-portal's screenshot.c checks PermissionStore only for
        # backend version >= 2. A v1 backend's success is NOT a client fallback
        # or proof of consent enforcement; record that limitation explicitly.
        store.SetPermission("screenshot", True, "screenshot", "", ["no"])
        denied = root / "denied.png"
        result = subprocess.run([binary, "--output", str(denied), "--timeout-ms", "5000"], env=env,
                                capture_output=True, text=True, timeout=8)
        if version >= 2:
            assert result.returncode == 1 and not denied.exists(), (result.stdout, result.stderr)
            assert "the request did not succeed" in result.stderr, result.stderr
        else:
            assert result.returncode == 0, result.stderr
            assert rgba(denied) == expected
        subprocess.run(["grim", str(reference)], env=env, check=True, timeout=3)
        assert rgba(reference) == expected
        print(json.dumps({"real_portal": "xdg-desktop-portal + wlr", "compositor": "headless Sway", "exact_pixels": width * height,
                          "display_unset": True, "backend_version": version, "permission_store_denial_enforced": version >= 2}))
    except Exception:
        print((root / "real-portal.log").read_text(errors="replace"), file=sys.stderr)
        raise
    finally:
        for service in reversed(services):
            stop(service)
        log.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--isolated", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--fixture", help=argparse.SUPPRESS)
    parser.add_argument("--uri", help=argparse.SUPPRESS)
    parser.add_argument("--log", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.fixture:
        fixture(args.fixture, args.uri, args.log)
        return
    binary = str(args.binary.resolve())
    if not args.isolated:
        # Debian wlr 0.7 writes a fixed /tmp/out.png. Isolate the entire test,
        # then drop root before launching graphical clients (Sway rejects root).
        subprocess.run(["sudo", "unshare", "--mount", "--propagation", "private", "sh", "-eu", "-c",
                        'mount -t tmpfs tmpfs /tmp; chmod 1777 /tmp; exec setpriv --reuid="$1" --regid="$2" --init-groups env HOME="$3" "$4" "$5" --isolated --binary "$6"',
                        "sh", str(os.getuid()), str(os.getgid()), str(Path.home()), sys.executable, str(Path(__file__).resolve()), binary], check=True)
        return
    with tempfile.TemporaryDirectory(prefix="captures-wayland-screenshot-") as temporary:
        root = Path(temporary)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        env = os.environ.copy()
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "SWAYSOCK", "DBUS_SESSION_BUS_ADDRESS", "XDG_DESKTOP_PORTAL_DIR"):
            env.pop(key, None)
        env.update(XDG_RUNTIME_DIR=str(runtime), XDG_DATA_HOME=str(root / "data"), XDG_CONFIG_HOME=str(root / "config"),
                   XDG_CACHE_HOME=str(root / "cache"), XDG_CURRENT_DESKTOP="sway", XDG_SESSION_TYPE="wayland",
                   WAYLAND_DISPLAY="fixture-only", GDK_BACKEND="wayland", NO_AT_BRIDGE="1", WLR_BACKENDS="headless",
                   WLR_HEADLESS_OUTPUTS="1", WLR_LIBINPUT_NO_DEVICES="1", WLR_RENDERER="pixman")
        daemon = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1"], env=env, stdout=subprocess.PIPE)
        try:
            env["DBUS_SESSION_BUS_ADDRESS"] = daemon.stdout.readline().decode().strip()
            protocols(binary, root, env)
            compositor_capture(binary, root, env)
        finally:
            stop(daemon)


if __name__ == "__main__":
    main()
