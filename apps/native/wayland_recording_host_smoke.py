#!/usr/bin/python3
"""Acceptance fixture for the native Wayland display-recording host.

Runs the live application on a private 1280x900 Sway desktop and exercises its
real History button and recording HUD with real Wayland pointer input.  Portal
consent in this fixture is the pinned, non-interactive wlr test backend; it is
not evidence of physical compositor consent UX.
"""
import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import threading
import time

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

from wayland_screenshot_smoke import DESKTOP, ready, stop, wait_owner
from x11_capture_smoke import ScreenSaver


WIDTH, HEIGHT = 1280, 900
BACKGROUND = (35, 69, 103)


def wait(predicate, description, timeout=25):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.05)
    raise AssertionError(f"Timed out: {description}")


def windows(env):
    tree = json.loads(subprocess.check_output(["swaymsg", "-t", "get_tree"], env=env))

    def leaves(node):
        for child in node.get("nodes", []) + node.get("floating_nodes", []):
            yield from leaves(child)
        if node.get("pid") is not None:
            yield node

    return list(leaves(tree))


def events(path, name=None):
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        return []
    parsed = []
    for line in path.read_text(errors="replace").splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict) and (name is None or event.get("event") == name):
            parsed.append(event)
    return parsed


def run_appearance(binary, pointer, root, appearance, env, portal_mode, interrupt_source, window_only=False):
    profile = root / (appearance + "-window" if window_only else appearance)
    profile.mkdir()
    history_root = profile / "history"
    settings = profile / "settings.json"
    settings.write_text(json.dumps({
        "onboarding_completed": True,
        "settings_schema_version": 5,
        "appearance": appearance,
        "theme": "mustard",
        "output_directory": str(profile / "exports"),
        "launch_at_login": False,
        "auto_copy_to_clipboard": False,
        "new_capture_shortcut": "Ctrl+Shift+F10",
        "region_shortcut": "Ctrl+Shift+F7",
        "window_shortcut": "Ctrl+Shift+F8",
        "display_shortcut": "Ctrl+Shift+F9",
        "recording": {
            "video_format": "mp4",
            "video_fps": 15,
            "countdown_seconds": 3,
            "show_cursor": False,
            "capture_system_audio": False,
            "microphone_device_id": None,
            "highlight_clicks": False,
            "show_keystrokes": False,
            "open_editor_after_recording": False,
        },
    }))
    host_log_path = profile / "host.log"
    host_log = host_log_path.open("w")
    app = subprocess.Popen([
        binary, "--live", "--open-history", "--open-preferences",
        "--settings-file", str(settings), "--history-root", str(history_root),
        "--quit-after", "40",
    ], env=env, stdout=host_log, stderr=host_log)

    def click(x, y):
        pointer.stdin.write(f"{round(x)} {round(y)} {WIDTH} {HEIGHT}\n".encode())
        pointer.stdin.flush()
        assert select.select([pointer.stdout], [], [], 5)[0], "input injector did not answer"
        assert pointer.stdout.readline().strip() == b"CLICKED"

    def window(title):
        return next((item for item in windows(env) if item["name"] == title), None)

    def shot(name):
        path = profile / f"{appearance}-{name}.png"
        subprocess.run(["grim", str(path)], env=env, check=True, timeout=5)
        return path

    def arrange(title, geometry):
        item = wait(lambda: window(title), title)
        subprocess.run([
            "swaymsg", f'[con_id={item["id"]}] floating enable, resize set {geometry[2]} '
            f'{geometry[3]}, move position {geometry[0]} {geometry[1]}, focus'
        ], env=env, check=True, stdout=subprocess.DEVNULL)

    def settled_layout(event_name, title, predicate=lambda _detail: True, control=None):
        settled = None

        def inspect():
            nonlocal settled
            item = window(title)
            found = events(host_log_path, event_name)
            if not item or not found:
                settled = None
                return None
            candidates = [event["detail"] for event in found
                          if control is None or event["detail"].get("control") == control]
            if not candidates:
                settled = None
                return None
            detail = candidates[-1]
            if not predicate(detail):
                settled = None
                return None
            viewport = [item["rect"]["width"], item["rect"]["height"]]
            if detail.get("viewport_size") != viewport:
                settled = None
                return None
            # elapsed_ms changes while recording; compositor geometry, viewport
            # and the actionable rectangle are the layout stability contract.
            signature = (item["id"], item["rect"], detail.get("viewport_size"),
                         detail.get("rect", detail.get("button")), detail.get("control"),
                         detail.get("state"), detail.get("enabled"))
            if settled is None or settled[0] != signature:
                settled = (signature, time.monotonic())
                return None
            return (detail, item["rect"]) if time.monotonic() - settled[1] >= .35 else None

        return wait(inspect, f"settled {event_name} matching compositor rectangle")

    def hud_control(control, state=None, enabled=None):
        def valid(detail):
            return (detail["control"] == control and
                    (state is None or detail["state"] == state) and
                    (enabled is None or detail["enabled"] is enabled))
        return settled_layout("recording-hud-control-layout", "Captures Recording Controls",
                              valid, control=control)

    def click_layout(layout):
        detail, outer = layout
        x1, y1, x2, y2 = detail.get("rect", detail.get("button"))
        click(outer["x"] + (x1 + x2) / 2, outer["y"] + (y1 + y2) / 2)

    try:
        wait(lambda: app.poll() is None and window("Capture History") and
             window("Captures Preferences"), "History and Preferences mapped")
        arrange("Capture History", (24, 28, 880, 640))
        arrange("Captures Preferences", (650, 300, 600, 560))
        if window_only:
            window_layout = settled_layout("portal-window-recording-layout", "Capture History")
            assert window_layout[0]["enabled"], "Record window must be reachable without a tray"
            shot("window-action")
            arrange("Capture History", (24, 28, 560, 480))
            minimum = settled_layout("portal-window-recording-layout", "Capture History")
            left, top, right, bottom = minimum[0]["button"]
            width, height = minimum[0]["viewport_size"]
            assert 0 <= left < right <= width and 0 <= top < bottom <= height, minimum
            settled_layout("portal-limitation-layout", "Capture History", lambda detail:
                           detail["clip"][0] <= detail["rect"][0] < detail["rect"][2] <= detail["clip"][2]
                           and detail["clip"][1] <= detail["rect"][1] < detail["rect"][3] <= detail["clip"][3])
            shot("window-action-minimum")
            click_layout(minimum)
            retry = hud_control("restart", "failed", True)
            assert retry[0]["label"] == "Retry recording"
            assert hud_control("delete", "failed", True)[0]["enabled"]
            draft_path = wait(lambda: next(iter((profile / "recording-recovery").glob("*/manifest.json")), None),
                              "failed window take retained for retry/delete")
            draft = json.loads(draft_path.read_text())
            assert draft["state"] == "failed" and draft["options"]["target"] == {"type": "portal_window"}, draft
            assert "cannot share a window" in draft["last_error"], draft
            assert not draft["segments"] and not list(profile.rglob("*.mp4"))
            assert not list(history_root.glob("*/metadata.json")), "window request must not fall back to a display"
            shot("window-unavailable")
            report = {"appearance": appearance, "target": draft["options"]["target"],
                      "window_only_request": True, "unsupported_backend_error": draft["last_error"],
                      "retry_delete_available": True, "fallback": False, "published_media": False}
            (profile / "result.json").write_text(json.dumps(report, indent=2))
            print(json.dumps(report), flush=True)
            return
        history_layout = settled_layout("portal-recording-layout", "Capture History")
        assert history_layout[0]["enabled"], \
            "Record display button is disabled (recording toolchain did not initialize)"
        shot("history")
        click_layout(history_layout)

        # The countdown is compositor-placed and must visibly precede the HUD.
        countdown = wait(lambda: window("Captures Recording Countdown"),
                         "visible recording countdown", timeout=4)
        assert not window("Captures Recording Controls"), "HUD appeared before countdown completed"
        time.sleep(.35)  # inspect the settled entrance, not its partial-opacity frame
        shot("countdown")
        submit = wait(lambda: events(host_log_path, "portal-recording-submit"),
                      "recording submit after countdown", timeout=12)[-1]["detail"]
        submit_wall = time.monotonic()
        assert submit["root_visible"] is False and submit["children_hidden"] is True, submit
        assert not window("Capture History") and not window("Captures Preferences"), \
            "workspace remained mapped during portal submission"
        assert countdown["rect"]["width"] > 0 and countdown["rect"]["height"] > 0

        running = hud_control("pause_resume", "recording", True)
        running_wall = time.monotonic()
        assert not window("Captures Recording Countdown"), "countdown and HUD overlapped"
        screenshot = hud_control("screenshot", "recording", False)
        stop_control = hud_control("stop", "recording", True)
        assert screenshot[0]["label"] == "Take a region screenshot"
        assert stop_control[0]["enabled"]
        shot("running-hud")

        before_pause = running[0]["elapsed_ms"]
        click_layout(running)
        pause_wall_start = time.monotonic()
        paused = hud_control("pause_resume", "paused", True)
        paused_wall = time.monotonic()
        paused_at = paused[0]["elapsed_ms"]
        assert paused_at >= before_pause
        shot("paused-hud")
        # The hold must dominate two acquisitions and observation latency, so
        # the independent duration bounds still reject a pause-inclusive clock.
        time.sleep(3.13)  # deliberately not aligned with the HUD's 100 ms timer
        paused_later = hud_control("pause_resume", "paused", True)
        assert paused_later[0]["elapsed_ms"] == paused_at, (paused_at, paused_later[0])
        assert hud_control("screenshot", "paused", False)[0]["enabled"] is False
        assert hud_control("stop", "paused", True)[0]["enabled"] is True

        # Resume opens a fresh portal grant/segment; real input must remain usable.
        resume_wall = time.monotonic()
        paused_wall_seconds = resume_wall - paused_wall
        click_layout(paused_later)
        resumed = hud_control("pause_resume", "recording", True)
        resumed_wall = time.monotonic()
        assert resumed[0]["elapsed_ms"] >= paused_at
        time.sleep(.9)
        stop_layout = hud_control("stop", "recording", True)
        hud_active_seconds = stop_layout[0]["elapsed_ms"] / 1000
        click_layout(stop_layout)
        stop_clicked_wall = time.monotonic()
        stopped = wait(lambda: events(host_log_path, "recording-stop-submit"), "accepted Stop command")[-1]["detail"]
        stop_submitted_wall = time.monotonic()
        assert stopped["generation"] == submit["generation"], stopped
        lower_duration = pause_wall_start - running_wall + stop_clicked_wall - resumed_wall
        upper_duration = paused_wall - submit_wall + stop_submitted_wall - resume_wall

        metadata = wait(lambda: next(iter(history_root.glob("*/metadata.json")), None),
                        "published recording metadata", timeout=25)
        entry = json.loads(metadata.read_text())
        assert entry["kind"] == "video" and entry["target"] == {"type": "portal_display"}, entry
        assert "rect" not in entry["target"] and (entry["width"], entry["height"]) == (WIDTH, HEIGHT), entry
        media = metadata.parent / "media.mp4"
        wait(media.is_file, "published MP4")
        probe = json.loads(subprocess.check_output([
            "ffprobe", "-v", "error", "-show_format", "-show_streams", "-of", "json", str(media)
        ], timeout=10))
        videos = [stream for stream in probe["streams"] if stream["codec_type"] == "video"]
        assert len(videos) == 1 and videos[0]["codec_name"] == "h264", probe
        assert (int(videos[0]["width"]), int(videos[0]["height"])) == (WIDTH, HEIGHT), probe
        duration = float(probe["format"]["duration"])
        # Bound accepted media with independently observed lifecycle times, not
        # the HUD counter or encoded duration. Grant/first-frame latency belongs
        # only in the upper bound. Running-state intervals give the lower bound.
        # Two frame periods plus dispatch rounding fit inside 150 ms at 15 FPS.
        tolerance = .15
        assert lower_duration > 1 and upper_duration >= lower_duration
        # This discriminates a clock that accidentally includes the entire pause
        # even if valid output lands at the lowest allowed recording duration.
        assert paused_wall_seconds > upper_duration - lower_duration + 2 * tolerance, \
            (paused_wall_seconds, [lower_duration, upper_duration], duration)
        assert lower_duration - tolerance <= duration <= upper_duration + tolerance, \
            (duration, [lower_duration, upper_duration], entry)

        # Sample the first decoded frame beneath both pre-capture windows.
        frame = subprocess.check_output([
            "ffmpeg", "-v", "error", "-i", str(media), "-frames:v", "1",
            "-f", "rawvideo", "-pix_fmt", "rgb24", "-"
        ], timeout=15)
        assert len(frame) == WIDTH * HEIGHT * 3, len(frame)
        samples = {}
        for name, (x, y) in {"history": (100, 100), "preferences": (1100, 700)}.items():
            actual = tuple(frame[(y * WIDTH + x) * 3:(y * WIDTH + x) * 3 + 3])
            samples[name] = actual
            assert all(abs(a - b) <= 15 for a, b in zip(actual, BACKGROUND)), (name, actual)

        wait(lambda: window("Capture History") and window("Captures Preferences"),
             "workspace restored after publication")
        def record_button():
            # Wayland remapping lets the compositor choose fresh placement and
            # stacking. Focus/rearrange before input, never click through a child.
            arrange("Captures Preferences", (650, 300, 600, 560))
            arrange("Capture History", (24, 28, 880, 640))
            return settled_layout("portal-recording-layout", "Capture History",
                                  lambda detail: detail["enabled"])

        # Close the actual countdown surface: no consent request, empty draft
        # or stale mapping may survive cancellation.
        submissions = len(events(host_log_path, "portal-recording-submit"))
        click_layout(record_button())
        wait(lambda: window("Captures Recording Countdown"), "second countdown")
        subprocess.run(["swaymsg", '[title="Captures Recording Countdown"] kill'],
                       env=env, check=True, stdout=subprocess.DEVNULL)
        record_button()
        assert len(events(host_log_path, "portal-recording-submit")) == submissions
        assert not list((profile / "recording-recovery").glob("*/manifest.json"))

        # A real public-portal protocol cancellation must not become a failed
        # take/HUD. The fixture does not substitute a video source or use X11.
        portal_mode("cancel")
        click_layout(record_button())
        wait(lambda: len(events(host_log_path, "portal-recording-submit")) > submissions,
             "cancelled consent request")
        record_button()
        assert not window("Captures Recording Controls")
        assert not list((profile / "recording-recovery").glob("*/manifest.json"))
        assert len(list(history_root.glob("*/metadata.json"))) == 1
        shot("consent-cancelled")
        portal_mode("real")

        # Source loss ends the HUD without Stop and leaves a playable, stable
        # recovery segment. Quitting must not delete that accepted partial take.
        click_layout(record_button())
        hud_control("pause_resume", "recording", True)
        time.sleep(.55)
        interrupt_source()
        wait(lambda: window("Capture History") and not window("Captures Recording Controls"),
             "source loss returned to History")
        draft_path = wait(lambda: next(iter((profile / "recording-recovery").glob("*/manifest.json")), None),
                          "retained failed draft")
        draft = json.loads(draft_path.read_text())
        assert draft["state"] == "failed" and draft["last_error"], draft
        partial = draft_path.parent / "segment-000.mp4"
        partial_probe = json.loads(subprocess.check_output([
            "ffprobe", "-v", "error", "-show_format", "-of", "json", str(partial)
        ], timeout=10))
        assert float(partial_probe["format"]["duration"]) > .3, partial_probe
        retained = partial.read_bytes()
        time.sleep(.9)
        assert partial.read_bytes() == retained, "failed recording continued encoding"
        assert len(list(history_root.glob("*/metadata.json"))) == 1
        arrange("Captures Preferences", (650, 300, 600, 560))
        arrange("Capture History", (24, 28, 1020, 720))
        settled_layout("portal-recording-layout", "Capture History")
        shot("source-lost-recovery")
        wait(lambda: app.poll() is not None, "clean timed app exit", timeout=45)
        assert app.returncode == 0, app.returncode
        assert partial.read_bytes() == retained, "Quit deleted accepted recovery media"
        report = {
            "appearance": appearance,
            "target": entry["target"],
            "dimensions": [WIDTH, HEIGHT],
            "duration_seconds": duration,
            "expected_active_bounds_seconds": [lower_duration, upper_duration],
            "hud_active_seconds": hud_active_seconds,
            "paused_wall_seconds": paused_wall_seconds,
            "pause_elapsed_ms": [paused_at, paused_later[0]["elapsed_ms"]],
            "first_frame_samples": samples,
            "screenshots": [str(path.name) for path in sorted(profile.glob("*.png"))],
            "mp4": str(media.relative_to(profile)),
            "countdown_cancelled": True,
            "consent_cancelled": True,
            "source_loss_retained_seconds": float(partial_probe["format"]["duration"]),
            "source_loss_stopped_and_survived_quit": True,
        }
        (profile / "result.json").write_text(json.dumps(report, indent=2))
        print(json.dumps(report), flush=True)
    except Exception:
        host_log.flush()
        try:
            shot("failure")
        except Exception:
            pass
        print(host_log_path.read_text(errors="replace"), file=sys.stderr)
        print(json.dumps(windows(env)), file=sys.stderr)
        raise
    finally:
        stop(app)
        host_log.close()


def isolated(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    runtime = output / "runtime"
    runtime.mkdir(mode=0o700)
    env = os.environ.copy()
    for key in ("DISPLAY", "WAYLAND_DISPLAY", "SWAYSOCK", "DBUS_SESSION_BUS_ADDRESS",
                "DBUS_SYSTEM_BUS_ADDRESS", "XDG_DESKTOP_PORTAL_DIR"):
        env.pop(key, None)
    env.update(
        XDG_RUNTIME_DIR=str(runtime), XDG_DATA_HOME=str(output / "data"),
        XDG_CONFIG_HOME=str(output / "config"), XDG_CACHE_HOME=str(output / "cache"),
        XDG_CURRENT_DESKTOP="sway", XDG_SESSION_TYPE="wayland", GDK_BACKEND="wayland",
        NO_AT_BRIDGE="1", WLR_BACKENDS="headless", WLR_HEADLESS_OUTPUTS="1",
        WLR_LIBINPUT_NO_DEVICES="1", WLR_RENDERER="pixman", WGPU_BACKEND="gl",
        CAPTURES_NATIVE_LAYOUT_PROBE="1", CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER="1",
    )
    services, logs = [], []

    def spawn(name, command, announce=False):
        stdout = subprocess.PIPE if announce else (output / f"{name}.stdout.log").open("w")
        stderr = (output / f"{name}.stderr.log").open("w")
        logs.append(stderr)
        if not announce:
            logs.append(stdout)
        process = subprocess.Popen(command, env=env, stdin=subprocess.PIPE,
                                   stdout=stdout, stderr=stderr)
        services.append(process)
        return process

    loop = None
    thread = None
    bus = None
    try:
        daemon = spawn("dbus", ["dbus-daemon", "--session", "--nofork", "--print-address=1"], True)
        env["DBUS_SESSION_BUS_ADDRESS"] = daemon.stdout.readline().decode().strip()
        env["DBUS_SYSTEM_BUS_ADDRESS"] = env["DBUS_SESSION_BUS_ADDRESS"]
        DBusGMainLoop(set_as_default=True)
        bus = dbus.bus.BusConnection(env["DBUS_SESSION_BUS_ADDRESS"])
        saver_name = dbus.service.BusName("org.freedesktop.ScreenSaver", bus=bus, do_not_queue=True)
        saver = ScreenSaver(saver_name, "/org/freedesktop/ScreenSaver")
        saver.locked = False
        loop = GLib.MainLoop()
        thread = threading.Thread(target=loop.run, daemon=True)
        thread.start()

        sway_config = output / "sway.conf"
        sway_config.write_text(
            f"output HEADLESS-1 resolution {WIDTH}x{HEIGHT}\n"
            "output * bg #234567 solid_color\nseat seat0 fallback true\n"
            "default_border none\ndefault_floating_border none\n"
            'for_window [title="Capture History"] floating enable\n'
            'for_window [title="Captures Preferences"] floating enable\n'
            'for_window [title="Captures Recording Controls"] floating enable\n'
            'for_window [title="Captures Recording Countdown"] floating enable\n')
        sway = spawn("sway", ["sway", "--unsupported-gpu", "--config", str(sway_config)])
        sockets = wait(lambda: [path for path in runtime.glob("wayland-*") if path.is_socket()],
                       "private Wayland socket")
        env["WAYLAND_DISPLAY"] = sockets[0].name
        env["SWAYSOCK"] = str(wait(lambda: list(runtime.glob("sway-ipc.*.sock")), "Sway IPC")[0])

        pipewire = spawn("pipewire", ["pipewire"])
        wait(lambda: (runtime / "pipewire-0").is_socket(), "PipeWire socket")
        wireplumber = spawn("wireplumber", ["wireplumber"])
        wait(lambda: wireplumber.poll() is None and any(
            item.get("info", {}).get("props", {}).get("application.name") == "WirePlumber"
            for item in json.loads(subprocess.check_output(["pw-dump"], env=env, timeout=3))),
            "WirePlumber policy")

        backend_config = output / "wlr.ini"
        backend_config.write_text(
            "[screencast]\nchooser_type=none\noutput_name=HEADLESS-1\nmax_fps=30\n")
        for name, executable, owner, extra in (
            ("permission-store", "/usr/libexec/xdg-permission-store",
             "org.freedesktop.impl.portal.PermissionStore", []),
            ("portal-gtk", "/usr/libexec/xdg-desktop-portal-gtk",
             "org.freedesktop.impl.portal.desktop.gtk", []),
            ("portal-wlr", str(args.wlr_backend.resolve()),
             "org.freedesktop.impl.portal.desktop.wlr", ["-c", str(backend_config), "-l", "DEBUG"]),
            ("portal", "/usr/libexec/xdg-desktop-portal", DESKTOP, []),
        ):
            process = spawn(name, [executable, *extra])
            wait_owner(bus, owner, process)
            if name == "portal-wlr":
                backend = process
            elif name == "portal":
                public_portal = process

        def portal_mode(mode):
            nonlocal backend, public_portal
            stop(public_portal)
            wait(lambda: not bus.name_has_owner(DESKTOP), "public portal stopped")
            if mode == "real":
                if backend.poll() is not None:
                    backend = spawn("portal-wlr-restarted", [str(args.wlr_backend.resolve()),
                                    "-c", str(backend_config), "-l", "DEBUG"])
                    wait_owner(bus, "org.freedesktop.impl.portal.desktop.wlr", backend)
                public_portal = spawn("portal-restored", ["/usr/libexec/xdg-desktop-portal"])
                wait_owner(bus, DESKTOP, public_portal)
            else:
                public_portal = spawn("portal-cancel", [sys.executable,
                    str(Path(__file__).with_name("wayland_video_smoke.py")), "--fixture", mode,
                    "--log", str(output / "consent-cancel.jsonl")], True)
                ready(public_portal)

        pointer = spawn("injector", [str(args.injector.resolve()), "pointer"], True)
        ready(pointer)
        for appearance in ("dark", "light"):
            portal_mode("real")
            run_appearance(str(args.binary.resolve()), pointer, output, appearance, env,
                           portal_mode, lambda: stop(backend), window_only=True)
            run_appearance(str(args.binary.resolve()), pointer, output, appearance, env,
                           portal_mode, lambda: stop(backend))
        (output / "PASS").write_text("dark and light native Wayland recording host acceptance passed\n")
    except Exception:
        (output / "FAIL").write_text("inspect retained logs and screenshots\n")
        raise
    finally:
        if loop:
            loop.quit()
        if thread:
            thread.join(timeout=3)
        for service in reversed(services):
            stop(service)
        subprocess.run(["fusermount3", "-uz", str(runtime / "doc")],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if bus:
            bus.close()
        for log in logs:
            log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--injector", type=Path, required=True)
    parser.add_argument("--wlr-backend", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True,
                        help="new disposable directory; retained with logs, media, and screenshots")
    parser.add_argument("--isolated", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    for path, label in ((args.binary, "binary"), (args.injector, "injector"),
                        (args.wlr_backend, "wlr backend")):
        if not path.is_file():
            parser.error(f"{label} does not exist: {path}")
    if not args.isolated:
        command = [
            "sudo", "unshare", "--mount", "--propagation", "private", "sh", "-eu", "-c",
            'mount -t tmpfs tmpfs /tmp; chmod 1777 /tmp; exec setpriv --reuid="$1" '
            '--regid="$2" --init-groups env HOME="$3" "$4" "$5" --isolated '
            '--binary "$6" --injector "$7" --wlr-backend "$8" --output "$9"',
            "sh", str(os.getuid()), str(os.getgid()), str(Path.home()), sys.executable,
            str(Path(__file__).resolve()), str(args.binary.resolve()), str(args.injector.resolve()),
            str(args.wlr_backend.resolve()), str(args.output.resolve()),
        ]
        subprocess.run(command, check=True)
        return
    isolated(args)


if __name__ == "__main__":
    main()
