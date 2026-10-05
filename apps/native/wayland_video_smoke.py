#!/usr/bin/python3
"""ScreenCast protocol and real CPU-mapped PipeWire video on private Sway.

This is a source diagnostic, not native recording UI or physical consent acceptance.
The private backend chooser is configured for the one disposable headless output.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

from wayland_screenshot_smoke import DESKTOP, DESKTOP_PATH, REQUEST, ready, rgba, stop, wait_owner

SCREENCAST = "org.freedesktop.portal.ScreenCast"
SESSION = "org.freedesktop.portal.Session"


def desktop_fixture():
    import gi
    gi.require_version("Gtk", "3.0")
    from gi.repository import Gtk

    window = Gtk.Window()
    window.set_decorated(False)
    window.fullscreen()
    canvas = Gtk.DrawingArea()
    window.add(canvas)
    phase = 0

    def draw(widget, context):
        colors = ((35, 69, 103), (211, 37, 81), (51, 173, 29), (73, 41, 197)) if phase == 0 else (
            (181, 23, 57), (31, 211, 63), (93, 17, 191), (207, 127, 41))
        for (x, y, width, height), color in zip(((0, 0, 117, 73), (117, 0, 203, 73),
                                                (0, 73, 117, 127), (117, 73, 203, 127)), colors):
            context.set_source_rgb(*(channel / 255 for channel in color))
            context.rectangle(x, y, width, height)
            context.fill()

    def advance():
        nonlocal phase
        phase = 1 - phase
        canvas.queue_draw()
        return True

    canvas.connect("draw", draw)
    window.connect("map-event", lambda *_: print("READY", flush=True))
    window.show_all()
    GLib.timeout_add(120, advance)
    Gtk.main()


def fixture(mode, log):
    DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    name = dbus.service.BusName(DESKTOP, bus)

    def record(event):
        with open(log, "a") as file:
            file.write(json.dumps({"event": event}) + "\n")

    class Session(dbus.service.Object):
        @dbus.service.method(SESSION, in_signature="", out_signature="")
        def Close(self):
            record("Session.Close")

    class Request(dbus.service.Object):
        @dbus.service.method(REQUEST, in_signature="", out_signature="")
        def Close(self):
            record("Request.Close")

        @dbus.service.signal(REQUEST, signature="ua{sv}")
        def Response(self, status, results):
            pass

    class Portal(dbus.service.Object):
        def response(self, method, sender, options, results):
            record(method)
            token = str(options["handle_token"])
            path = f"{DESKTOP_PATH}/request/{sender[1:].replace('.', '_')}/{token}"
            request = Request(name, path)
            self.requests.append(request)
            # Directed signals bypass D-Bus match rules: pinning the owner must
            # still reject a same-path success sent by a different bus peer.
            peer = dbus.bus.BusConnection(os.environ["DBUS_SESSION_BUS_ADDRESS"])
            spoof = dbus.lowlevel.SignalMessage(path, REQUEST, "Response")
            spoof.set_destination(sender)
            spoof.append(dbus.UInt32(0), dbus.Dictionary({}, signature="sv"), signature="ua{sv}")
            peer.send_message(spoof)
            peer.flush()
            peer.close()
            unrelated = Request(name, path + "_other")
            self.requests.append(unrelated)
            unrelated.Response(0, {})
            if mode == "method-error" and method == "CreateSession":
                raise dbus.exceptions.DBusException("Reply lost", name="org.freedesktop.portal.Error.Failed")
            if mode == "wait" and method == "Start":
                return dbus.ObjectPath(path)
            status = 1 if mode == "cancel" and method == "Start" else 2 if mode == "denied" and method == "SelectSources" else 0
            # Deliver before the method reply, as real portal backends may do.
            request.Response(status, results)
            return dbus.ObjectPath(path)

        @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="ss", out_signature="v")
        def Get(self, interface, property):
            assert interface == SCREENCAST
            return dbus.UInt32({"version": 1 if mode == "legacy" else 6,
                                "AvailableSourceTypes": 2 if mode == "no-display" else 1,
                                "AvailableCursorModes": 1 if mode == "no-cursor" else 3}[property])

        @dbus.service.method(SCREENCAST, in_signature="a{sv}", out_signature="o", sender_keyword="sender")
        def CreateSession(self, options, sender):
            path = f"{DESKTOP_PATH}/session/{sender[1:].replace('.', '_')}/{options['session_handle_token']}"
            self.session = Session(name, path)
            self.path = path
            return self.response("CreateSession", sender, options, {"session_handle": dbus.String(path)})

        @dbus.service.method(SCREENCAST, in_signature="oa{sv}", out_signature="o", sender_keyword="sender")
        def SelectSources(self, session, options, sender):
            assert str(session) == self.path
            assert int(options["types"]) == 1 and not options["multiple"]
            if mode == "legacy":
                assert "cursor_mode" not in options
            else:
                assert int(options["cursor_mode"]) == 1
            assert "restore_token" not in options and "persist_mode" not in options
            return self.response("SelectSources", sender, options, {})

        @dbus.service.method(SCREENCAST, in_signature="osa{sv}", out_signature="o", sender_keyword="sender")
        def Start(self, session, parent, options, sender):
            assert str(session) == self.path and not parent
            streams = [(dbus.UInt32(37), dbus.Dictionary({"source_type": dbus.UInt32(1), "pipewire-serial": dbus.UInt64(987654321)}, signature="sv"))]
            if mode == "legacy":
                streams = [(dbus.UInt32(37), dbus.Dictionary({}, signature="sv"))]
            elif mode == "invalid-serial":
                streams[0][1]["pipewire-serial"] = dbus.UInt64(0)
            elif mode == "window":
                streams[0][1]["source_type"] = dbus.UInt32(2)
            if mode == "multiple":
                streams.append((dbus.UInt32(53), dbus.Dictionary({}, signature="sv")))
            if mode == "missing-streams":
                return self.response("Start", sender, options, {})
            return self.response("Start", sender, options, {"streams": dbus.Array(streams, signature="(ua{sv})")})

        @dbus.service.method(SCREENCAST, in_signature="oa{sv}", out_signature="h")
        def OpenPipeWireRemote(self, session, options):
            assert str(session) == self.path and not options
            record("OpenPipeWireRemote")
            raise dbus.exceptions.DBusException("No remote in this protocol fixture", name="org.freedesktop.portal.Error.Failed")

    portal = Portal(name, DESKTOP_PATH)
    portal.requests = []
    print("READY", flush=True)
    GLib.MainLoop().run()


def protocols(binary, root, env, recording=False):
    modes = ("fd-error", "legacy", "cancel", "denied", "multiple", "missing-streams", "invalid-serial",
             "window", "no-display", "no-cursor", "wait", "method-error")
    for mode in modes:
        suffix = "recording" if recording else "source"
        log = root / f"{mode}-{suffix}.jsonl"
        output = root / f"{mode}-{suffix}"
        service = subprocess.Popen([sys.executable, __file__, "--fixture", mode, "--log", str(log)], env=env, stdout=subprocess.PIPE)
        try:
            ready(service)
            arguments = [binary, "--output", str(output)]
            if mode == "wait":
                arguments += ["--cancel-after-ms", "150"]
            elif mode == "no-cursor":
                arguments += ["--show-cursor", "true"]
            started = time.monotonic()
            result = subprocess.run(arguments, env=env, capture_output=True, text=True, timeout=8)
            assert result.returncode == 1, (mode, result.stdout, result.stderr)
            if recording:
                assert not list(output.rglob("*.mp4")) and not (output / "history").exists(), (mode, result.stderr)
            else:
                assert not output.exists(), (mode, result.stderr)
            events = [json.loads(line)["event"] for line in log.read_text().splitlines()]
            assert events[-1] == "Session.Close", (mode, events, result.stderr)
            calls = [event for event in events if not event.endswith(".Close")]
            expected = ["CreateSession"]
            if mode not in ("method-error", "no-display", "no-cursor"):
                expected += ["SelectSources"]
            if mode not in ("method-error", "denied", "no-display", "no-cursor"):
                expected += ["Start"]
            if mode in ("fd-error", "legacy"):
                expected += ["OpenPipeWireRemote"]
            assert calls == expected, (mode, calls, result.stderr)
            if mode in ("wait", "method-error"):
                assert "Request.Close" in events and time.monotonic() - started < 2
        finally:
            stop(service)
    bus = dbus.bus.BusConnection(env["DBUS_SESSION_BUS_ADDRESS"])
    assert not bus.name_has_owner(DESKTOP)
    result = subprocess.run([binary, "--output", str(root / f"pre-cancel-{recording}"), "--cancel-after-ms", "0"], env=env, capture_output=True, text=True, timeout=3)
    assert result.returncode == 1 and "cancelled" in result.stderr and not bus.name_has_owner(DESKTOP)
    bus.close()
    print(json.dumps({"protocols": len(modes) + 1, "recording": recording, "peer_spoofs_ignored": True, "failed_sessions_closed": True, "fallback": False}), flush=True)


def check_recording(output, events, phases, duration_range, kind="video/mp4"):
    event = events[-1]
    assert event["event"] in ("ready", "recovered") and event["warning"] is None, events
    entry = event["entry"]
    assert entry["target"] == {"type": "portal_display"}, entry
    assert (entry["width"], entry["height"], entry["mime_type"]) == (320, 200, kind), entry
    artifact = Path(event["path"])
    assert artifact.is_relative_to(output / "history") and artifact.is_file(), event
    metadata = json.loads(subprocess.check_output(["ffprobe", "-v", "error", "-show_format", "-show_streams", "-of", "json", str(artifact)], timeout=5))
    streams = metadata["streams"]
    assert len(streams) == 1 and streams[0]["codec_name"] == ("h264" if kind == "video/mp4" else "gif"), metadata
    duration = float(metadata["format"]["duration"])
    assert duration_range[0] <= duration <= duration_range[1], (duration, duration_range, events)
    decoded = subprocess.check_output(["ffmpeg", "-v", "error", "-i", str(artifact), "-f", "rawvideo", "-pix_fmt", "rgb24", "-threads", "1", "-"], timeout=8)
    frame_size = 320 * 200 * 3
    assert len(decoded) >= frame_size * 3 and len(decoded) % frame_size == 0, len(decoded)
    points = ((18, 19), (240, 21), (21, 150), (263, 177))
    seen = set()
    for start in range(0, len(decoded), frame_size):
        samples = [decoded[start + (y * 320 + x) * 3:start + (y * 320 + x) * 3 + 3] for x, y in points]
        matches = [phase for phase, colors in enumerate(phases)
                   if all(all(abs(actual - expected) <= 15 for actual, expected in zip(sample, color))
                          for sample, color in zip(samples, colors))]
        # H264/GIF are lossy: compare independently specified interior colors,
        # away from block/region boundaries, not exact source image bytes.
        assert matches, (start // frame_size, [list(sample) for sample in samples])
        seen.update(matches)
    assert seen == {0, 1}, seen
    manifests = list((output / "recording-recovery").glob("*/manifest.json"))
    assert len(manifests) == (1 if kind == "image/gif" else 0), manifests
    if manifests:
        draft = json.loads(manifests[0].read_text())
        assert draft["state"] == "ready" and draft["options"]["target"] == {"type": "portal_display"}, draft
    return len(decoded) // frame_size


def recording_sessions(binary, root, env):
    phases = (((35, 69, 103), (211, 37, 81), (51, 173, 29), (73, 41, 197)),
              ((181, 23, 57), (31, 211, 63), (93, 17, 191), (207, 127, 41)))
    frames = 0
    for scenario in ("video", "gif", "pause", "restart", "recover", "discard", "cancel"):
        output = root / f"session-{scenario}"
        arguments = [binary, "--output", str(output), "--scenario", "video" if scenario == "cancel" else scenario]
        if scenario == "cancel":
            arguments += ["--duration-ms", "3000", "--cancel-after-ms", "700"]
        result = subprocess.run(arguments, env=env, capture_output=True, text=True, timeout=15)
        if scenario == "cancel":
            assert result.returncode == 1 and "cancelled" in result.stderr.lower(), result.stderr
        else:
            assert result.returncode == 0, (scenario, result.stdout, result.stderr)
        events = [json.loads(line) for line in result.stdout.splitlines()]
        if scenario in ("discard", "cancel"):
            assert not list(output.rglob("*.mp4")) and not (output / "history").exists(), events
            assert not list((output / "recording-recovery").glob("*/manifest.json")), events
        else:
            duration_range = (.9, 1.6) if scenario == "pause" else (.6, 1.1) if scenario == "restart" else (.3, .8)
            frames += check_recording(output, events, phases, duration_range, "image/gif" if scenario == "gif" else "video/mp4")
            if scenario in ("pause", "restart"):
                assert events[-1]["segments"] == (2 if scenario == "pause" else 1), events
        # Same output directory is rejected without changing saved bytes.
        before = {str(path.relative_to(output)): path.read_bytes() for path in output.rglob("*") if path.is_file()}
        retry = subprocess.run(arguments, env=env, capture_output=True, text=True, timeout=3)
        assert retry.returncode == 1, retry.stdout
        assert before == {str(path.relative_to(output)): path.read_bytes() for path in output.rglob("*") if path.is_file()}
    print(json.dumps({"recording_sessions": 7, "decoded_frames": frames, "mp4_gif": True,
                      "pause_resume": True, "restart": True, "recovery": True,
                      "discard_cancel": True, "target_has_no_geometry": True}), flush=True)
    return phases


def real_video(binary, backend, root, env, recording_binary=None):
    width, height = 320, 200
    pixels = bytes(channel for y in range(height) for x in range(width)
                   for channel in ((35, 69, 103, 255) if x < 117 and y < 73 else
                                   (211, 37, 81, 255) if x >= 117 and y < 73 else
                                   (51, 173, 29, 255) if x < 117 else (73, 41, 197, 255)))
    alternate_pixels = bytes(channel for y in range(height) for x in range(width)
                             for channel in ((181, 23, 57, 255) if x < 117 and y < 73 else
                                             (31, 211, 63, 255) if x >= 117 and y < 73 else
                                             (93, 17, 191, 255) if x < 117 else (207, 127, 41, 255)))
    config = root / "sway.conf"
    config.write_text(f'output HEADLESS-1 resolution {width}x{height}\noutput * bg #3f3f3f solid_color\nseat seat0 fallback true\n')
    backend_config = root / "wlr.ini"
    backend_config.write_text("[screencast]\nchooser_type=none\noutput_name=HEADLESS-1\nmax_fps=30\n")
    log = (root / "video-services.log").open("w")
    services = []
    bus = dbus.bus.BusConnection(env["DBUS_SESSION_BUS_ADDRESS"])
    try:
        sway = subprocess.Popen(["sway", "--unsupported-gpu", "--config", str(config)], env=env, stdout=log, stderr=log)
        services.append(sway)
        runtime = Path(env["XDG_RUNTIME_DIR"])
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            sockets = [path for path in runtime.glob("wayland-*") if path.is_socket()]
            if sockets:
                break
            assert sway.poll() is None, "Sway exited"
            time.sleep(.1)
        else:
            raise AssertionError("No private Wayland socket")
        env["WAYLAND_DISPLAY"] = sockets[0].name
        env["SWAYSOCK"] = str(next(runtime.glob("sway-ipc.*.sock")))
        desktop = subprocess.Popen([sys.executable, __file__, "--desktop-fixture"], env=env, stdout=subprocess.PIPE, stderr=log)
        services.append(desktop)
        ready(desktop)
        pipewire = subprocess.Popen(["pipewire"], env=env, stdout=log, stderr=log)
        services.append(pipewire)
        deadline = time.monotonic() + 8
        while not (runtime / "pipewire-0").is_socket():
            assert pipewire.poll() is None and time.monotonic() < deadline, "No private PipeWire socket"
            time.sleep(.1)
        policy = subprocess.Popen(["wireplumber"], env=env, stdout=log, stderr=log)
        services.append(policy)
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            assert policy.poll() is None, "Private WirePlumber exited"
            objects = json.loads(subprocess.check_output(["pw-dump"], env=env, timeout=3))
            if any(item.get("info", {}).get("props", {}).get("application.name") == "WirePlumber" for item in objects):
                break
            time.sleep(.1)
        else:
            raise AssertionError("No private PipeWire session policy")
        for executable, name, extra in (("/usr/libexec/xdg-desktop-portal-gtk", "org.freedesktop.impl.portal.desktop.gtk", []),
                                       (backend, "org.freedesktop.impl.portal.desktop.wlr", ["-c", str(backend_config), "-l", "DEBUG"]),
                                       ("/usr/libexec/xdg-desktop-portal", DESKTOP, [])):
            service = subprocess.Popen([executable, *extra], env=env, stdout=log, stderr=log)
            services.append(service)
            wait_owner(bus, name, service)
            if executable == backend:
                wlr = service
        reference = root / "reference.png"
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            subprocess.run(["grim", str(reference)], env=env, check=True, timeout=3)
            if rgba(reference) == pixels:
                break
            time.sleep(.1)
        else:
            raise AssertionError("The independent desktop fixture was not presented")

        def collect(output):
            result = subprocess.run([binary, "--output", str(output), "--frames", "12"], env=env,
                                    capture_output=True, text=True, timeout=12)
            assert result.returncode == 0, result.stderr
            event = json.loads(result.stdout)
            assert (event["frames"], event["width"], event["height"]) == (12, width, height), event
            assert sorted(event["corner_colors"]) == [[35, 69, 103, 255], [181, 23, 57, 255]], event
            assert rgba(output) in (pixels, alternate_pixels), "Portal pixels differ from both independently specified video frames"

        collect(root / "video.png")
        cancelled = root / "cancelled.png"
        result = subprocess.run([binary, "--output", str(cancelled), "--frames", "120", "--cancel-after-ms", "700"],
                                env=env, capture_output=True, text=True, timeout=4)
        assert result.returncode == 1 and "Video portal diagnostic cancelled" in result.stderr and not cancelled.exists(), result.stderr
        collect(root / "again.png")
        if recording_binary:
            phases = recording_sessions(recording_binary, root, env)
        revoked = root / "revoked.png"
        probe = subprocess.Popen([binary, "--output", str(revoked), "--frames", "120"], env=env,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                assert probe.poll() is None, "Revocation probe ended before the stream became active"
                objects = json.loads(subprocess.check_output(["pw-dump"], env=env, timeout=3))
                if any(item.get("info", {}).get("state") == "running" and
                       item["info"].get("props", {}).get("media.name") == "Captures portal video" for item in objects):
                    break
                time.sleep(.05)
            else:
                raise AssertionError("No active portal video node to revoke")
            stop(wlr)
            stdout, stderr = probe.communicate(timeout=3)
            assert probe.returncode == 1 and not revoked.exists(), (stdout, stderr)
            assert "Timed out" not in stderr and ("disconnected" in stderr or "ended" in stderr or "error" in stderr.lower()), stderr
        finally:
            stop(probe)
        if recording_binary:
            wlr = subprocess.Popen([backend, "-c", str(backend_config), "-l", "DEBUG"], env=env, stdout=log, stderr=log)
            services.append(wlr)
            wait_owner(bus, "org.freedesktop.impl.portal.desktop.wlr", wlr)
            output = root / "lost-recording"
            probe = subprocess.Popen([recording_binary, "--output", str(output), "--scenario", "stream-loss", "--duration-ms", "10000"],
                                     env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    assert probe.poll() is None, "Recording ended before stream loss"
                    objects = json.loads(subprocess.check_output(["pw-dump"], env=env, timeout=3))
                    if any(item.get("info", {}).get("state") == "running" and
                           item["info"].get("props", {}).get("media.name") == "Captures portal video" for item in objects):
                        break
                    time.sleep(.05)
                else:
                    raise AssertionError("No active recording stream")
                time.sleep(.4)  # Retain independently decodable media before transport loss.
                lost_at = time.monotonic()
                stop(wlr)
                stdout, stderr = probe.communicate(timeout=8)
                assert probe.returncode == 0, (stdout, stderr)
                events = [json.loads(line) for line in stdout.splitlines()]
                assert any(event["event"] == "stream-lost" for event in events), events
                frames = check_recording(output, events, phases, (.2, 1.5))
                print(json.dumps({"stream_loss_failed_then_recovered": True, "decoded_frames": frames,
                                  "elapsed_after_loss_seconds": round(time.monotonic() - lost_at, 2)}), flush=True)
            finally:
                stop(probe)
        print(json.dumps({"real_video": "ScreenCast + portal-granted PipeWire remote", "frames": 24, "exact_pixels": width * height * 2,
                          "changing_frames": True, "stream_cancelled": True, "grant_revoked": True,
                          "repeat_session": True, "display_unset": True}), flush=True)
    except Exception:
        log.flush()
        print((root / "video-services.log").read_text(errors="replace"), file=sys.stderr)
        raise
    finally:
        for service in reversed(services):
            stop(service)
        bus.close()
        log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--recording-binary", type=Path, help="Also exercise real MP4/GIF recording sessions")
    parser.add_argument("--wlr-backend", type=Path, default=Path("/usr/libexec/xdg-desktop-portal-wlr"),
                        help="Private ScreenCast backend; use build_wayland_portal_fixture.sh for SHM-only desktops")
    parser.add_argument("--fixture", help=argparse.SUPPRESS)
    parser.add_argument("--desktop-fixture", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--log", help=argparse.SUPPRESS)
    parser.add_argument("--isolated", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.desktop_fixture:
        desktop_fixture()
        return
    if args.fixture:
        fixture(args.fixture, args.log)
        return
    if not args.binary:
        parser.error("--binary is required")
    binary = str(args.binary.resolve())
    recording_binary = str(args.recording_binary.resolve()) if args.recording_binary else None
    backend = str(args.wlr_backend.resolve())
    if not args.isolated:
        subprocess.run(["sudo", "unshare", "--mount", "--propagation", "private", "sh", "-eu", "-c",
                        'mount -t tmpfs tmpfs /tmp; chmod 1777 /tmp; exec setpriv --reuid="$1" --regid="$2" --init-groups env HOME="$3" "$4" "$5" --isolated --binary "$6" --wlr-backend "$7" ${8:+--recording-binary "$8"}',
                        "sh", str(os.getuid()), str(os.getgid()), str(Path.home()), sys.executable, str(Path(__file__).resolve()), binary, backend, recording_binary or ""], check=True)
        return
    with tempfile.TemporaryDirectory(prefix="captures-wayland-video-") as temporary:
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
            if recording_binary:
                protocols(recording_binary, root, env, recording=True)
            real_video(binary, backend, root, env, recording_binary)
        finally:
            stop(daemon)
            subprocess.run(["fusermount3", "-uz", str(runtime / "doc")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
