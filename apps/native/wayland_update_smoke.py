#!/usr/bin/python3
"""Real native development updates on private Sway/software GL, never installed data.

Requires system Python (D-Bus), the pinned compositor launcher PATH, an explicit
current host/package with pinned target-suffixed media tools, and the Cargo signing
test executable. Fresh keys, loopback HTTP, enrolled profiles and replacement
packages are disposable. No physical compositor or installed-update acceptance.
"""
import argparse
import ctypes
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import tarfile
import tempfile
import threading
import time

from history_fixture import write_completed_settings, write_history

WIDTH, HEIGHT = 1280, 900
PREFERENCES, NOTICE = "Captures Preferences", "Captures Update"
VERSION = "2026.10.50"
CHUNK = 64 * 1024


def digest(path):
    hasher = hashlib.sha256()
    with path.open("rb") as source:
        while block := source.read(1024 * 1024):
            hasher.update(block)
    return hasher.hexdigest()


def fingerprint(root):
    return {str(path.relative_to(root)): ("link:" + os.readlink(path) if path.is_symlink()
            else digest(path) if path.is_file() else "directory")
            for path in sorted(root.rglob("*"))}


def retained(profile):
    # Exclude only the transient live-session marker, not arbitrary dotfiles.
    return {str(path.relative_to(profile)): digest(path)
            for name in ("settings.json", "history", "editor-drafts", "recording-recovery",
                         ".captures-native-development-profile.json")
            for path in ([profile / name] if (profile / name).is_file() else (profile / name).rglob("*"))
            if path.is_file() and path != profile / "history/.crash-diagnostics/current-session"}


class Loopback:
    """Hold real response bodies at known byte boundaries; never fake UI progress."""
    def __init__(self, root, archive):
        self.root, self.archive = root, archive
        self.requests, self.holds, self.held = [], [], []
        self.all_requests = []
        self.mode = "valid"
        self.errors = []
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                record = {"path": self.path, "mode": fixture.mode, "sent": 0, "complete": False}
                fixture.requests.append(record)
                fixture.all_requests.append(record)
                path = {"/manifest.json": root / "manifest.json",
                        "/manifest.json.minisig": root / "manifest.json.minisig",
                        "/full": archive}.get(self.path)
                if path is None:
                    self.send_error(404)
                    return
                holds = list(fixture.holds)
                self.send_response(200)
                self.send_header("Content-Length", str(path.stat().st_size))
                self.end_headers()
                try:
                    with path.open("rb") as source:
                        while True:
                            remaining = [boundary - record["sent"] for route, boundary, _ in holds
                                         if route == self.path]
                            block = source.read(min([CHUNK, *remaining]))
                            if not block:
                                break
                            if record["sent"] == 0 and (
                                    self.path == "/manifest.json" and record["mode"] == "signature-error"
                                    or self.path == "/full" and record["mode"] == "hash-error"):
                                altered = bytearray(block)
                                altered[0] ^= 1
                                block = altered
                            self.wfile.write(block)
                            record["sent"] += len(block)
                            for route, boundary, gate in holds:
                                if route == self.path and record["sent"] >= boundary:
                                    self.wfile.flush()
                                    fixture.held.append({"path": route, "bytes": record["sent"]})
                                    if not gate.wait(90):
                                        fixture.errors.append("held HTTP body timed out")
                                        return
                                    holds.remove((route, boundary, gate))
                    record["complete"] = True
                except (BrokenPipeError, ConnectionResetError):
                    record["disconnected"] = True

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()

    def plan(self, mode="valid", route=None, boundaries=()):
        self.release()
        self.mode, self.requests, self.held = mode, [], []
        self.holds = [(route, boundary, threading.Event()) for boundary in boundaries]

    def release(self):
        for _, _, gate in self.holds:
            gate.set()

    def paths(self):
        return [request["path"] for request in self.requests]

    def close(self):
        self.release()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


class Host:
    def __init__(self, pid, log, pointer, env, bus):
        self.pid, self.log, self.pointer, self.env, self.bus = pid, log, pointer, env, bus

    def reports(self):
        values = {}
        for line in self.log.read_text().splitlines(keepends=True):
            if not line.endswith("\n"):
                break
            if line.startswith("{"):
                value = json.loads(line)
                values[value["event"]] = value["detail"]
        return values

    def layout(self):
        return self.reports().get("preferences-shortcuts-layout", {})

    def copy(self):
        return self.layout().get("update_checks") or {}

    def notice(self):
        return self.reports().get("update-notice")

    def window(self, title=PREFERENCES):
        from wayland_native_capture_smoke import windows
        return next((node for node in windows(self.env)
                     if node["pid"] == self.pid and node["name"] == title), None)

    def arrange(self, width=740, height=660):
        from wayland_native_capture_smoke import wait
        node = wait(self.window, PREFERENCES)
        subprocess.run(["swaymsg", f'[con_id={node["id"]}] floating enable, '
                        f'resize set {width} {height}, move position 20 100'],
                       env=self.env, check=True, stdout=subprocess.DEVNULL)
        wait(lambda: self.window()["rect"]["width"] == width
             and self.window()["rect"]["height"] == height, "Preferences arrangement")
        # A newly mapped error notice can take focus after resize acknowledgment.
        # Let presentation settle, then choose the actual input destination.
        time.sleep(.5)
        subprocess.run(["swaymsg", f'[con_id={node["id"]}] focus'], env=self.env,
                       check=True, stdout=subprocess.DEVNULL)
        wait(lambda: self.window()["focused"], "Preferences focus after arrangement")
        time.sleep(.5)

    def input_at(self, x, y, scroll=None):
        suffix = f" {scroll}" if scroll is not None else ""
        self.pointer.stdin.write(f"{round(x)} {round(y)} {WIDTH} {HEIGHT}{suffix}\n".encode())
        self.pointer.stdin.flush()
        assert select.select([self.pointer.stdout], [], [], 5)[0], "pointer stalled"
        assert self.pointer.stdout.readline().strip() == (b"SCROLLED" if scroll is not None else b"CLICKED")

    def updates(self):
        from wayland_native_capture_smoke import wait
        node = wait(self.window, PREFERENCES)
        if notice := self.window(NOTICE):
            subprocess.run(["swaymsg", f'[con_id={notice["id"]}] move position 770 100; '
                            f'[con_id={node["id"]}] focus'], env=self.env,
                           check=True, stdout=subprocess.DEVNULL)
            wait(lambda: self.window()["focused"], "Preferences focus before scrolling")
            time.sleep(.5)
        if node["rect"]["width"] > 720:
            self.input_at(node["rect"]["x"] + 90, node["rect"]["y"] + 289)
            time.sleep(.6)
        # Compact Preferences hides the sidebar; scroll using the actual control
        # rectangle and page clip rather than trusting the active-section label.
        for _ in range(60):
            detail = wait(lambda: self.layout().get("page") and self.layout(), "Updates layout")
            rect = detail["controls"].get("Check for updates")
            page = detail["page"]
            if rect and page[1] <= rect[1] < rect[3] <= page[3]:
                time.sleep(.6)
                rect = self.layout()["controls"]["Check for updates"]
                if page[1] <= rect[1] < rect[3] <= page[3]:
                    return
            node = self.window()["rect"]
            offset = rect[1] - page[1] - 20 if rect else 200
            self.input_at(node["x"] + node["width"] - 35, node["y"] + 210,
                          max(-200, min(200, offset)))
            time.sleep(.2)
        raise AssertionError("Updates control never became visible")

    def activate(self, name):
        from wayland_native_capture_smoke import wait
        # Sway controls remapped placement and can recenter the floating notice
        # over this control. Dismiss it normally before using Preferences; the
        # explicit action starts a new reveal generation.
        if self.notice() is not None or self.window(NOTICE):
            self.notice_action()
            wait(self.hidden, "notice dismissed before Preferences input")
        node = self.window()
        subprocess.run(["swaymsg", f'[con_id={node["id"]}] focus'], env=self.env,
                       check=True, stdout=subprocess.DEVNULL)
        time.sleep(.5)

        previous = {"rect": None, "since": time.monotonic()}

        def visible():
            detail = self.layout()
            rect = detail.get("controls", {}).get(name)
            page = detail.get("page", [])
            size = self.window()["rect"]
            if rect != previous["rect"]:
                previous.update(rect=rect, since=time.monotonic())
            return rect if (rect and len(page) == 4 and page[2:] == [size["width"], size["height"]]
                            and page[1] <= rect[1] < rect[3] <= page[3]
                            and time.monotonic() - previous["since"] >= .6) else None

        x1, y1, x2, y2 = wait(visible, f"visible and settled {name}")
        node = self.window()["rect"]
        self.input_at(node["x"] + (x1 + x2) / 2, node["y"] + (y1 + y2) / 2)

    def notice_action(self, primary=False):
        from wayland_native_capture_smoke import wait
        def settled():
            node, detail = self.window(NOTICE), self.notice()
            return node if (node and detail and [node["rect"]["width"], node["rect"]["height"]]
                            == detail["window"][2:]) else None
        node = wait(settled, "notice mapped at its current state size")
        subprocess.run(["swaymsg", f'[con_id={node["id"]}] focus'], env=self.env,
                       check=True, stdout=subprocess.DEVNULL)
        wait(lambda: self.window(NOTICE)["focused"], "notice raised for real input")
        time.sleep(.5)
        rect = self.window(NOTICE)["rect"]
        # Shipping footer placement, reused by native_profile_restart_smoke.py.
        self.input_at(rect["x"] + (rect["width"] - 118 if primary else 80),
                      rect["y"] + rect["height"] - 48)

    def hidden(self):
        return self.notice() is None and self.window(NOTICE) is None

    def shot(self, path):
        from wayland_native_capture_smoke import wait
        notice = wait(lambda: self.window(NOTICE), "notice for capture")
        # On Wayland placement is compositor-owned; arrange both real windows
        # without changing the app's requested notice dimensions.
        y = 480 if self.window()["rect"]["width"] > 720 else 100
        subprocess.run(["swaymsg", f'[con_id={notice["id"]}] move position 770 {y}, focus'],
                       env=self.env, check=True, stdout=subprocess.DEVNULL)
        wait(lambda: self.window(NOTICE)["focused"], "notice raised for capture")
        time.sleep(.6)
        subprocess.run(["grim", str(path)], env=self.env, check=True, timeout=10)
        path.with_suffix(".json").write_text(json.dumps({"preferences": self.layout(),
            "notice": self.notice(), "windows": [self.window(), self.window(NOTICE)]}, indent=2))

    def quit(self):
        from wayland_lifecycle_smoke import menu_action
        menu_action(self.bus, "Quit Captures")


def enroll(root, importer, appearance, env):
    source = root / "operator-source"
    source.mkdir()
    write_history(source / "capture-history")
    settings = source / "settings.json"
    write_completed_settings(settings)
    (source / "exports").mkdir()
    (source / "exports/do-not-replace.bin").write_bytes(b"operator\0\xff")
    original = fingerprint(source)
    profile = root / "development profile % é"
    subprocess.run([str(importer), "--source-settings-file", str(settings),
                    "--source-data-directory", str(source), "--new-development-profile", str(profile),
                    "--all-app-processes-stopped"], env=env, check=True, capture_output=True, timeout=30)
    settings = profile / "settings.json"
    value = json.loads(settings.read_text())
    value.update(onboarding_completed=True, appearance=appearance, launch_at_login=False,
                 output_directory=str(root / "development-exports"), unknown_future_field={"keep": [3, 17, 91]})
    settings.write_text(json.dumps(value, indent=2))
    for name in ("editor-drafts", "recording-recovery"):
        directory = profile / name / ".retained"
        directory.mkdir(parents=True)
        (directory / "opaque.bin").write_bytes(b"retain\0\xff" + name.encode())
    # The recovery worker normally creates this persistent empty election file.
    # Seed bookkeeping before taking the byte-exact retained-data baseline.
    (profile / "recording-recovery/.recording-recovery.lock").touch()
    return profile, source, original


def acquisition(args, root, env, bus, pointer, fixture, evidence):
    from wayland_lifecycle_smoke import watcher
    from wayland_native_capture_smoke import wait, windows
    from wayland_screenshot_smoke import stop

    total = fixture.archive.stat().st_size
    boundaries = [total // (3 * CHUNK) * CHUNK, total // (3 * CHUNK) * 2 * CHUNK]
    assert 0 < boundaries[0] < boundaries[1] < total, "use a real built package"
    for appearance in ("dark", "light"):
        for pending in ("download", "check"):
            case = root / f"{appearance}-{pending}"
            case.mkdir()
            first_request = len(fixture.all_requests)
            profile, source, original = enroll(case, args.importer, appearance, env)
            saved = retained(profile)
            scratch = case / "scratch"
            scratch.mkdir()
            sentinel = scratch / "operator-owned.bin"
            sentinel.write_bytes(b"never updater-owned\0\xff")
            scratch_before = fingerprint(scratch)
            fixture.plan()
            log_path = case / "host.jsonl"
            with log_path.open("w") as log:
                app = subprocess.Popen(list(map(str, [args.binary, "--live", "--open-preferences",
                    "--settings-file", profile / "settings.json", "--history-root", profile / "history",
                    "--native-update-manifest-url", fixture.url + "/manifest.json",
                    "--native-update-public-key-file", root / "public.key",
                    "--native-update-current-version", "2026.9.99",
                    "--native-update-staging-directory", scratch])), env=env, stdout=log, stderr=log)
            host = Host(app.pid, log_path, pointer, env, bus)
            facts = {"case": case.name, "progress": []}

            def copy_is(status):
                return host.copy().get("status") == status

            def available():
                return copy_is(f"Development update {VERSION} available")

            def shown_available():
                return available() and (host.notice() or {}).get("visualState") == "available" and host.window(NOTICE)

            def verified():
                return copy_is(f"Development package {VERSION} verified")

            def clean():
                assert fingerprint(scratch) == scratch_before, "owned scratch survived cleanup or sentinel changed"

            def progress(index):
                expected = boundaries[index]
                wait(lambda: len(fixture.held) > index, "held package body")
                assert fixture.held[index] == {"path": "/full", "bytes": expected}
                percent = (expected * 100 + total // 2) // total
                wait(lambda: copy_is(f"Downloading {expected} of {total} bytes")
                     and (host.notice() or {}).get("download", [None, None])[1] == percent,
                     "exact real received bytes and signed total")
                assert not host.copy()["enabled"], "check enabled during download"
                facts["progress"].append({"received": expected, "total": total, "percent": percent})
                assert fingerprint(scratch) != scratch_before, "partial download has no owned file"

            try:
                wait(lambda: watcher(bus) and watcher(bus).Get(
                    "org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems"), "real SNI registration")
                host.arrange()
                host.updates()
                wait(lambda: copy_is("Not checked"), "idle updater")
                time.sleep(.7)
                assert fixture.paths() == [] and host.hidden(), "idle made HTTP or opened a notice"
                clean()
                assert retained(profile) == saved, "idle profile changed"
                facts["idle_requests"] = []

                fixture.plan(route="/manifest.json", boundaries=[1])
                host.activate("Check for updates")
                wait(lambda: fixture.held and copy_is("Checking signed metadata…") and host.notice(), "held signed check")
                assert fixture.paths() == ["/manifest.json"]
                assert not host.copy()["enabled"]
                host.notice_action()
                wait(host.hidden, "dismissed held check")
                fixture.release()
                wait(available, "signed completion in Preferences")
                time.sleep(.5)
                assert host.hidden(), "check completion revived dismissed notice"
                assert fixture.paths() == ["/manifest.json", "/manifest.json.minisig"]
                facts["dismissed_check_completion"] = host.copy()

                if pending == "download":
                    fixture.plan()
                    host.activate("Check for updates")
                    wait(shown_available, "explicit recheck reveals signed available")
                    host.shot(case / "available.png")
                    fixture.plan(route="/full", boundaries=boundaries)
                    host.activate("Package verification")
                    progress(0)
                    host.shot(case / "progress.png")
                    fixture.holds[0][2].set()
                    progress(1)
                    host.notice_action()
                    wait(host.hidden, "dismissed real download")
                    fixture.release()
                    wait(verified, "hidden signed package validation", timeout=90)
                    assert host.hidden() and fixture.paths() == ["/full"]
                    assert fingerprint(scratch) != scratch_before, "verified stage was not retained"
                    facts["dismissed_download_completion"] = host.copy()
                    facts["staged_entries"] = sorted(path.name for path in scratch.iterdir())

                    fixture.plan()
                    host.activate("Check for updates")
                    wait(shown_available, "recheck from staged")
                    clean()
                    fixture.plan(route="/full", boundaries=[boundaries[0]])
                    host.activate("Package verification")
                    progress(0)
                    host.notice_action(primary=True)
                    wait(lambda: copy_is("Cancelling; waiting for I/O…"), "cancel stays busy during held I/O")
                    assert not host.copy()["enabled"] and not host.copy()["acquisition"]["enabled"]
                    facts["held_cancel"] = host.copy()
                    host.notice_action()
                    wait(host.hidden, "dismissed cancellation")
                    fixture.release()
                    wait(available, "cancel cleanup acknowledged")
                    clean()
                    assert host.hidden() and fixture.paths() == ["/full"]

                    fixture.plan(mode="signature-error")
                    host.activate("Check for updates")
                    wait(lambda: host.copy().get("failed") and "signature verification failed" in host.copy()["status"],
                         "real signature rejection")
                    assert fixture.paths() == ["/manifest.json", "/manifest.json.minisig"]
                    facts["signature_error"] = host.copy()
                    clean()
                    host.arrange(560, 440)
                    host.updates()
                    host.shot(case / "signature-error-minimum.png")
                    fixture.plan()
                    host.notice_action(primary=True)
                    wait(shown_available, "notice explicit check retry")
                    assert fixture.paths() == ["/manifest.json", "/manifest.json.minisig"]

                    fixture.plan(mode="hash-error")
                    host.activate("Package verification")
                    wait(lambda: host.copy().get("failed") and "hash does not match" in host.copy()["status"],
                         "real hash rejection", timeout=90)
                    assert fixture.paths() == ["/full"]
                    facts["hash_error"] = host.copy()
                    clean()
                    host.shot(case / "hash-error-minimum.png")
                    fixture.plan()
                    host.notice_action(primary=True)
                    wait(verified, "notice authenticated download retry", timeout=90)
                    assert fixture.paths() == ["/full"], "download retry refetched metadata"
                    facts["retry_verified"] = host.copy()
                    host.shot(case / "verified-minimum.png")

                    fixture.plan()
                    host.activate("Check for updates")
                    wait(shown_available, "recheck removes verified retry stage")
                    clean()
                    fixture.plan(route="/full", boundaries=[CHUNK])
                    host.activate("Package verification")
                    wait(lambda: fixture.held and copy_is(f"Downloading {CHUNK} of {total} bytes"), "held download before Quit")
                else:
                    fixture.plan(route="/manifest.json", boundaries=[1])
                    host.activate("Check for updates")
                    wait(lambda: fixture.held and copy_is("Checking signed metadata…"), "held metadata before Quit")

                marker = profile / "history/.crash-diagnostics/current-session"
                assert marker.is_file(), "live profile marker missing"
                host.quit()
                wait(lambda: not [node for node in windows(env) if node["pid"] == app.pid], "accepted Quit hides all windows")
                time.sleep(.4)
                assert app.poll() is None and marker.is_file(), "Quit released profile before pending I/O drain"
                assert retained(profile) == saved
                import fcntl
                ipc = Path(f"/tmp/captures-native-{os.getuid()}")
                key = hashlib.sha256(os.fsencode(ipc) + b"\0" + os.fsencode((profile / "history").resolve())).hexdigest()[:32]
                with (ipc / f"{key}.lock").open("rb") as lock:
                    try:
                        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    except BlockingIOError:
                        pass
                    else:
                        raise AssertionError("Quit released election before cleanup")
                facts["held_quit"] = {"alive": True, "windows": [], "marker_retained": True,
                                      "election_retained": True,
                                      "scratch_entries": sorted(path.name for path in scratch.iterdir())}
                fixture.release()
                assert app.wait(timeout=20) == 0, "accepted Quit failed"
                assert not marker.exists(), "clean Quit retained crash marker"
                with (ipc / f"{key}.lock").open("rb") as lock:
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                clean()
                assert fixture.paths() == (["/full"] if pending == "download" else ["/manifest.json"]), "late HTTP after Quit"
                assert retained(profile) == saved and fingerprint(source) == original
                facts.update(passed=True, requests=fixture.all_requests[first_request:], cleanup=fingerprint(scratch),
                             retained_profile_unchanged=True, operator_source_unchanged=True,
                             retained_profile=saved, operator_source=original)
                evidence.append(facts)
                (case / "result.json").write_text(json.dumps(facts, indent=2))
                print(json.dumps(facts), flush=True)
            except Exception:
                subprocess.run(["grim", str(case / "failed.png")], env=env, check=True, timeout=10)
                (case / "failed-windows.json").write_text(json.dumps(windows(env), indent=2))
                print(log_path.read_text()[-10000:])
                raise
            finally:
                fixture.release()
                stop(app)


def supervision(args, root, env, bus, pointer, fixture, evidence):
    from wayland_lifecycle_smoke import watcher
    from wayland_native_capture_smoke import wait, windows
    from wayland_screenshot_smoke import stop

    # Supervised hosts must verify/use their own copied sibling tools, not the
    # acquisition-only checkout host's explicit tool overrides.
    env = {key: value for key, value in env.items() if key not in ("CAPTURES_FFMPEG", "CAPTURES_FFPROBE")}
    # Adopt the replacement only in this disposable test process, so normal Quit
    # asserts its actual exit status instead of merely observing PID disappearance.
    libc = ctypes.CDLL(None, use_errno=True)
    assert libc.prctl(36, 1, 0, 0, 0) == 0, os.strerror(ctypes.get_errno())
    for appearance, visible, install in (("dark", True, True), ("light", True, True),
                                         ("dark", False, True), ("dark", True, False)):
        label = f"supervised-{appearance}-{'visible' if visible else 'closed'}-{'install' if install else 'quit'}"
        case = root / label
        case.mkdir()
        first_request = len(fixture.all_requests)
        package = case / "package"
        shutil.copytree(args.package, package, symlinks=True)
        package_before = fingerprint(package)
        profile, source, original = enroll(case, args.importer, appearance, env)
        (profile / "startup.log").write_bytes(b"Original diagnostics must survive.\n")
        saved = retained(profile)
        fixture.plan()
        with (case / "helper.log").open("w") as log:
            helper = subprocess.Popen(list(map(str, [args.helper, "--manifest-url", fixture.url + "/manifest.json",
                "--public-key-file", root / "public.key", "--current-version", "2026.9.99", "--renderer", "wgpu",
                "--stopped-development-package", package, "--existing-development-profile", profile,
                "--all-app-processes-stopped", "--health-timeout-seconds", "60", "--supervise-gui"])),
                env=env, stdout=subprocess.PIPE, stderr=log)
        handle = replacement_handle = None
        host = None
        try:
            assert select.select([helper.stdout], [], [], 60)[0], "supervisor did not announce"
            announced = json.loads(helper.stdout.readline())
            assert announced["state"] == "gui_running", announced
            pid = announced["process_id"]
            handle = os.pidfd_open(pid)
            session = Path(announced["supervision_directory"])
            host = Host(pid, Path(announced["gui_log"]), pointer, env, bus)
            wait(lambda: watcher(bus) and watcher(bus).Get(
                "org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems"), "supervised tray")
            host.arrange()
            host.updates()
            assert fixture.paths() == [], "supervisor launch made HTTP"
            host.activate("Check for updates")
            wait(lambda: (host.notice() or {}).get("visualState") == "available" and host.window(NOTICE),
                 "supervised signed available mapped")
            host.activate("Package verification")
            wait(lambda: (host.copy().get("acquisition") or {}).get("label") == "Restart and install",
                 "supervised verified install action", timeout=90)
            assert retained(profile) == saved
            before_requests = len(fixture.requests)
            if not visible:
                node = host.window()
                subprocess.run(["swaymsg", f'[con_id={node["id"]}] kill'], env=env,
                               check=True, stdout=subprocess.DEVNULL)
                wait(lambda: host.window() is None, "Preferences closed into tray residency")
            if not install:
                host.quit()
                stdout, _ = helper.communicate(timeout=30)
                reply = json.loads(stdout)
                assert helper.returncode == 0 and reply == {"state": "gui_exited", "replaced": False}
                assert fixture.requests[before_requests:] == [] and not session.exists()
                assert fingerprint(package) == package_before and retained(profile) == saved
                assert not list(case.glob(".captures-native-pre-update-*"))
            else:
                if visible:
                    host.activate("Package verification")
                else:
                    host.notice_action(primary=True)
                wait(lambda: select.select([handle], [], [], 0)[0], "source GUI drained")
                assert not (session / "gui-scratch").exists() or not list((session / "gui-scratch").iterdir())

                def restarted():
                    assert helper.poll() in (None, 0), (case / "helper.log").read_text()
                    return next((node for node in windows(env) if node["name"] == "Captures is running"
                                 and node["pid"] != pid), None)

                notice = wait(restarted, "replacement real ready notice", timeout=300)
                new_pid = notice["pid"]
                replacement_handle = os.pidfd_open(new_pid)
                wait(lambda: any(node["pid"] == new_pid and node["name"] == PREFERENCES for node in windows(env)) == visible,
                     "actual Preferences visibility restored")
                assert not any(node["pid"] == new_pid and node["name"] in ("Captures", "Capture History")
                               for node in windows(env)), "replacement reopened setup or History"
                time.sleep(.5)
                subprocess.run(["grim", str(case / "restart.png")], env=env, check=True, timeout=10)
                (case / "restart-windows.json").write_text(json.dumps(windows(env), indent=2))
                stdout, _ = helper.communicate(timeout=120)
                reply = json.loads(stdout)
                assert helper.returncode == 0 and reply["state"] == "confirmed" and reply["process_id"] == new_pid, reply
                assert fingerprint(package) == fingerprint(root / "target-package"), "replacement not byte-exact target"
                snapshot = Path(reply["profile_snapshot"])
                assert retained(snapshot) == saved and retained(profile) == saved
                assert (profile / "startup.log").read_bytes() == b"Original diagnostics must survive.\n"
                assert len(list(profile.glob("startup-*.log"))) == 1
                assert fixture.paths()[before_requests:] == ["/manifest.json", "/manifest.json.minisig", "/full"]
                replacement = Host(new_pid, Path(announced["gui_log"]), pointer, env, bus)
                replacement.quit()
                wait(lambda: select.select([replacement_handle], [], [], 0)[0], "replacement normal tray Quit")
                waited, status = os.waitpid(new_pid, 0)
                assert waited == new_pid and os.waitstatus_to_exitcode(status) == 0
                assert retained(snapshot) == saved and retained(profile) == saved
                assert not session.exists(), "supervisor retained owned session scratch"
            assert fingerprint(source) == original
            evidence.append({"case": label, "passed": True, "installed": install,
                "requests": fixture.all_requests[first_request:], "retained_profile_unchanged": True,
                "operator_source_unchanged": True, "supervision_scratch_removed": True,
                "helper_reply": reply, "retained_profile": saved, "operator_source": original,
                "preferences_restored": visible if install else None,
                "replacement_exit_code": 0 if install else None})
            (case / "result.json").write_text(json.dumps(evidence[-1], indent=2))
            print(json.dumps(evidence[-1]), flush=True)
        except Exception:
            subprocess.run(["grim", str(case / "failed.png")], env=env, check=True, timeout=10)
            (case / "failed-windows.json").write_text(json.dumps(windows(env), indent=2))
            if host:
                print(host.log.read_text()[-10000:])
            raise
        finally:
            fixture.release()
            for descriptor in (replacement_handle, handle):
                if descriptor is not None:
                    if not select.select([descriptor], [], [], 0)[0]:
                        import signal
                        signal.pidfd_send_signal(descriptor, signal.SIGTERM)
                    os.close(descriptor)
            stop(helper)


def main():
    import dbus
    from wayland_lifecycle_smoke import watcher
    from wayland_native_capture_smoke import wait
    from wayland_screenshot_smoke import ready, stop

    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("binary", "injector", "signing-test", "importer", "package", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--helper", type=Path, help="Also exercise copied-package supervision and startup health")
    args = parser.parse_args()
    for name in ("binary", "injector", "signing_test", "importer", "package", "helper"):
        if value := getattr(args, name):
            setattr(args, name, value.resolve(strict=True))
    assert digest(args.binary) == digest(args.package / "captures-native"), "host and package binary differ"
    for tool in ("ffmpeg", "ffprobe"):
        assert (args.package / "binaries" / f"{tool}-x86_64-unknown-linux-gnu").is_file(), "package needs pinned media pair"
    original_package = fingerprint(args.package)
    root = args.output.resolve()
    root.mkdir(parents=True, mode=0o700, exist_ok=False)
    target = root / "target-package"
    shutil.copytree(args.package, target, symlinks=True)
    (target / "WAYLAND-UPDATER-FIXTURE.txt").write_text("Disposable signed target; not a production version.\n")
    target_before = fingerprint(target)
    archive = root / "target.tar.gz"
    with tarfile.open(archive, "w:gz", format=tarfile.PAX_FORMAT) as file:
        file.add(target, arcname="native-linux")
    fixture = Loopback(root, archive)
    manifest = {"schema": 1, "identity": "es.captur.native-development", "renderer": "wgpu",
                "version": VERSION, "notes": "Disposable Wayland updater fixture.",
                "artifacts": {"x86_64-unknown-linux-gnu": {"url": fixture.url + "/full",
                    "size": archive.stat().st_size, "sha256": digest(archive)}}}
    (root / "manifest.json").write_text(json.dumps(manifest))
    # Keep Sway's Unix socket below sun_path's limit even for long evidence paths.
    runtime_directory = tempfile.TemporaryDirectory(prefix="captures-wayland-updater-")
    runtime = Path(runtime_directory.name)
    env = os.environ.copy()
    for key in ("DISPLAY", "WAYLAND_DISPLAY", "SWAYSOCK", "DBUS_SESSION_BUS_ADDRESS"):
        env.pop(key, None)
    env.update(XDG_RUNTIME_DIR=str(runtime), XDG_DATA_HOME=str(root / "data"),
               XDG_CONFIG_HOME=str(root / "config"), XDG_CACHE_HOME=str(root / "cache"),
               XDG_SESSION_TYPE="wayland", XDG_CURRENT_DESKTOP="sway", GDK_BACKEND="wayland",
               WLR_BACKENDS="headless", WLR_HEADLESS_OUTPUTS="1", WLR_LIBINPUT_NO_DEVICES="1",
               WLR_RENDERER="pixman", WGPU_BACKEND="gl", NO_AT_BRIDGE="1",
               NO_PROXY="127.0.0.1,localhost",
               CAPTURES_FFMPEG=str(args.package / "binaries/ffmpeg-x86_64-unknown-linux-gnu"),
               CAPTURES_FFPROBE=str(args.package / "binaries/ffprobe-x86_64-unknown-linux-gnu"),
               CAPTURES_NATIVE_LAYOUT_PROBE="1", CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER="1")
    children = []
    evidence = []
    try:
        compositor = subprocess.check_output(["sway", "--version"], env=env, text=True).strip()
        assert compositor.startswith("sway version 1.9"), "select the pinned private Sway launcher PATH"
        subprocess.run([str(args.signing_test), "tests::sign_development_fixture", "--exact", "--ignored"],
                       env={**env, "CAPTURES_NATIVE_DELTA_FIXTURE": str(root)}, check=True, timeout=30)
        daemon = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1"],
                                  env=env, stdout=subprocess.PIPE)
        children.append(daemon)
        env["DBUS_SESSION_BUS_ADDRESS"] = daemon.stdout.readline().decode().strip()
        env["DBUS_SYSTEM_BUS_ADDRESS"] = env["DBUS_SESSION_BUS_ADDRESS"]
        bus = dbus.bus.BusConnection(env["DBUS_SESSION_BUS_ADDRESS"])
        config = root / "sway.conf"
        config.write_text('output HEADLESS-1 resolution 1280x900\noutput * bg #234567 solid_color\n'
                          'seat seat0 fallback true\ndefault_border none\ndefault_floating_border none\n'
                          'for_window [title=".*"] floating enable\n'
                          'bar {\n id captures-test\n position top\n workspace_buttons no\n}\n')
        with (root / "services.log").open("w") as log:
            sway = subprocess.Popen(["sway", "--unsupported-gpu", "-c", str(config)], env=env, stdout=log, stderr=log)
            children.append(sway)
            socket = wait(lambda: next((p for p in runtime.glob("wayland-*") if p.is_socket()), None), "Wayland socket")
            env["WAYLAND_DISPLAY"] = socket.name
            env["SWAYSOCK"] = str(wait(lambda: next(iter(runtime.glob("sway-ipc.*.sock")), None), "Sway IPC"))
            wait(lambda: watcher(bus), "real Swaybar SNI host")
            pointer = subprocess.Popen([str(args.injector), "pointer"], env=env,
                                       stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log)
            children.append(pointer)
            ready(pointer)
            acquisition(args, root, env, bus, pointer, fixture, evidence)
            if args.helper:
                supervision(args, root, env, bus, pointer, fixture, evidence)
        assert not fixture.errors, fixture.errors
        assert fingerprint(args.package) == original_package and fingerprint(target) == target_before
        summary = {"passed": True, "compositor": compositor, "host_sha256": digest(args.binary), "archive_bytes": archive.stat().st_size,
                   "archive_sha256": digest(archive), "cases": evidence, "display_unset": True,
                   "input_package_unchanged": True, "test_target_unchanged": True,
                   "scope": "Private Sway 1.9/software GL, fresh test key and loopback package; "
                            "no installed update, production signing/channel or physical acceptance."}
        (root / "result.json").write_text(json.dumps(summary, indent=2))
        print(json.dumps(summary, indent=2))
    finally:
        fixture.close()
        for child in reversed(children):
            stop(child)
        subprocess.run(["fusermount3", "-uz", str(runtime / "doc")],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        runtime_directory.cleanup()


if __name__ == "__main__":
    main()
