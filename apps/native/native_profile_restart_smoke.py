#!/usr/bin/python3
"""Signed helper restarts of enrolled development profiles on private software X11.

Copies explicit built packages; all profile/config/export data is disposable.
No installed profile, OS permission or release-channel acceptance.
The signing-test executable is native_update_patch's Cargo test binary.
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
import signal
import subprocess
import sys
import tarfile
import threading
import time

from history_fixture import write_completed_settings, write_history
from x11_onboarding_smoke import start_tray


def digest(path):
    hasher = hashlib.sha256()
    with path.open("rb") as source:
        while block := source.read(1024 * 1024):
            hasher.update(block)
    return hasher.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("helper", "importer", "signing-test", "base-package", "target-package", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--gui", type=Path,
        help="Capture real post-verification GUI shutdown visibility instead of operator intent")
    parser.add_argument("--supervised", action="store_true",
        help="Launch the base package through the real GUI supervisor; exercise installation and ordinary Quit")
    args = parser.parse_args()
    assert not (args.gui and args.supervised), "Choose manual GUI intent or supervised installation"
    assert sys.platform == "linux", "This real-host fixture is private X11 only"
    # Adopt the GUI after its helper exits so real Ctrl+Q can assert exit status,
    # not merely PID disappearance. This affects only this disposable test process.
    libc = ctypes.CDLL(None, use_errno=True)
    pr_set_child_subreaper = 36
    assert libc.prctl(pr_set_child_subreaper, 1, 0, 0, 0) == 0, os.strerror(ctypes.get_errno())
    helper, importer, signer = (path.resolve(strict=True) for path in
        (args.helper, args.importer, args.signing_test))
    gui = args.gui.resolve(strict=True) if args.gui else None
    base, target = (path.resolve(strict=True) for path in (args.base_package, args.target_package))
    output = args.output.resolve()
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    archive = output / "target.tar.gz"
    with tarfile.open(archive, "w:gz", format=tarfile.PAX_FORMAT) as file:
        file.add(target, arcname="native-linux")
    original_packages = {str(root): {str(path.relative_to(root)): digest(path)
        for path in root.rglob("*") if path.is_file()} for root in (base, target)}
    env = {**os.environ, "WGPU_BACKEND": "gl", "WINIT_X11_SCALE_FACTOR": "1",
           "XDG_SESSION_TYPE": "x11", "CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER": "1"}
    if gui or args.supervised:
        env["CAPTURES_NATIVE_LAYOUT_PROBE"] = "1"
    env.pop("WAYLAND_DISPLAY", None)
    for key, directory in (("XDG_DATA_HOME", "data"), ("XDG_CONFIG_HOME", "config"),
                           ("XDG_CACHE_HOME", "cache"), ("XDG_RUNTIME_DIR", "runtime")):
        path = output / directory
        path.mkdir(mode=0o700)
        env[key] = str(path)
    children, app_handles, requests = [], [], []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            requests.append(self.path)
            path = {"/manifest.json": output / "manifest.json",
                    "/manifest.json.minisig": output / "manifest.json.minisig", "/full": archive}.get(self.path)
            if path is None:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Length", str(path.stat().st_size))
            self.end_headers()
            try:
                with path.open("rb") as source:
                    shutil.copyfileobj(source, self.wfile)
            except (BrokenPipeError, ConnectionResetError):
                pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}"
    manifest = {"schema": 1, "identity": "es.captur.native-development", "renderer": "wgpu",
        "version": "2026.10.50", "notes": "Disposable existing-profile restart fixture.",
        "artifacts": {"x86_64-unknown-linux-gnu": {"url": url + "/full",
            "size": archive.stat().st_size, "sha256": digest(archive)}}}
    (output / "manifest.json").write_text(json.dumps(manifest))
    results = []
    with (output / "processes.log").open("w") as log:
        def run(*command):
            return subprocess.check_output(list(map(str, command)), env=env, stderr=log, timeout=120)

        def spawn(command, announce=False):
            process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE if announce else log, stderr=log)
            children.append(process)
            if announce:
                assert select.select([process.stdout], [], [], 60)[0], "process did not announce readiness"
            return process

        def wait(predicate, description, timeout=25):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if value := predicate():
                    return value
                time.sleep(.05)
            run("import", "-window", "root", output / "timeout.png")
            raise AssertionError(description)

        def windows(pid, name):
            result = subprocess.run(["xdotool", "search", "--all", "--onlyvisible", "--pid", str(pid),
                                     "--name", name], env=env, capture_output=True, text=True, timeout=5)
            assert result.returncode in (0, 1), result.stderr
            return result.stdout.splitlines()

        try:
            signed = subprocess.run([str(signer), "tests::sign_development_fixture", "--exact", "--ignored"],
                env={**env, "CAPTURES_NATIVE_DELTA_FIXTURE": str(output)}, stdout=log, stderr=log, timeout=30)
            assert signed.returncode == 0, "fixture signing failed"
            xvfb = spawn(["Xvfb", "-displayfd", "1", "-screen", "0", "1280x900x24", "-nolisten", "tcp"], True)
            endpoint = xvfb.stdout.readline().decode().strip()
            assert endpoint, "X server failed before readiness"
            env["DISPLAY"] = ":" + endpoint
            spawn(["openbox", "--sm-disable"])
            wait(lambda: b"window id" in run("xprop", "-root", "_NET_SUPPORTING_WM_CHECK"), "window manager ready")
            start_tray(output, env, spawn, wait)
            cases = [("dark", True, True), ("light", True, True), ("dark", False, True)]
            if args.supervised:
                cases.append(("dark", True, False))
            for appearance, visible, install in cases:
                label = f"{appearance}-{'visible' if visible else 'closed'}"
                if not install:
                    label += "-ordinary-quit"
                directory = output / label
                directory.mkdir()
                package = directory / "package"
                shutil.copytree(base, package, symlinks=True)
                source = directory / "shipping-data"
                source.mkdir()
                write_history(source / "capture-history")
                source_settings = directory / "source-settings.json"
                write_completed_settings(source_settings)
                profile = directory / "development profile % é"
                run(importer, "--source-settings-file", source_settings, "--source-data-directory", source,
                    "--new-development-profile", profile, "--all-app-processes-stopped")
                settings = profile / "settings.json"
                value = json.loads(settings.read_text())
                value.update(onboarding_completed=True, appearance=appearance, theme="mustard",
                    output_directory=str(directory / "external exports"), unknown_future_field={"keep": [3, 17, 91]})
                settings.write_text(json.dumps(value, indent=2))
                old_log = b"Original startup diagnostics must not be truncated.\n"
                (profile / "startup.log").write_bytes(old_log)
                for root in ("editor-drafts", "recording-recovery"):
                    retained = profile / root / ".retained"
                    retained.mkdir(parents=True)
                    (retained / "opaque-bytes.bin").write_bytes(b"retain\0\xff" + root.encode())
                intent = directory / "shutdown-intent.json"
                command = [helper, "--manifest-url", url + "/manifest.json",
                    "--public-key-file", output / "public.key", "--current-version", "2026.9.99",
                    "--renderer", "wgpu", "--stopped-development-package", package,
                    "--existing-development-profile", profile, "--all-app-processes-stopped",
                    "--health-timeout-seconds", "60"]

                def saved_data():
                    # Normal Quit removes this live-session marker; it is not
                    # retained user data, and the replacement owns a new marker.
                    return {str(path.relative_to(profile)): path.read_bytes()
                        for name in ("settings.json", "history", "editor-drafts", "recording-recovery",
                                     ".captures-native-development-profile.json")
                        for path in ([profile / name] if (profile / name).is_file() else (profile / name).rglob("*"))
                        if path.is_file() and path != profile / "history/.crash-diagnostics/current-session"}

                if gui or args.supervised:
                    if args.supervised:
                        handoff = spawn(list(map(str, command + ["--supervise-gui"])), True)
                        announced = json.loads(handoff.stdout.readline())
                        assert announced["state"] == "gui_running", announced
                        source_pid = announced["process_id"]
                        session_directory = Path(announced["supervision_directory"])
                        event_path = Path(announced["gui_log"])
                        scratch, intent = (session_directory / name for name in ("gui-scratch", "shutdown-intent.json"))
                        source_handle = os.pidfd_open(source_pid)
                        app_handles.append(source_handle)
                    else:
                        scratch = directory / "gui-scratch"
                        scratch.mkdir()
                        event_path = directory / "gui-events.jsonl"
                        with event_path.open("w") as events:
                            app = subprocess.Popen(list(map(str, [gui, "--live", "--open-preferences", "--open-history",
                                "--history-root", profile / "history", "--settings-file", settings,
                                "--native-update-manifest-url", url + "/manifest.json",
                                "--native-update-public-key-file", output / "public.key",
                                "--native-update-current-version", "2026.9.99",
                                "--native-update-staging-directory", scratch,
                                "--native-update-shutdown-intent-file", intent])), env=env, stdout=events, stderr=log)
                        children.append(app)
                        source_pid = app.pid
                    preferences = wait(lambda: windows(source_pid, "^Captures Preferences$"), "source Preferences")[0]
                    history = None if args.supervised else wait(lambda: windows(source_pid, "^Capture History$"), "source History")[0]

                    def reports():
                        values = {}
                        for line in event_path.read_text().splitlines():
                            try:
                                value = json.loads(line)
                                values[value["event"]] = value["detail"]
                            except (ValueError, KeyError):
                                pass
                        return values

                    def click(x, y):
                        run("xdotool", "windowactivate", "--sync", preferences,
                            "mousemove", "--window", preferences, str(x - 1), str(y),
                            "mousemove_relative", "1", "0", "sleep", ".15", "click", "1")

                    def activate(name):
                        previous = {"rect": None, "since": time.monotonic()}
                        def stable():
                            detail = reports().get("preferences-shortcuts-layout", {})
                            rect = detail.get("controls", {}).get(name)
                            page = detail.get("page")
                            if rect != previous["rect"]:
                                previous.update(rect=rect, since=time.monotonic())
                            return rect if (rect and page and page[1] <= rect[1] and rect[3] <= page[3]
                                and time.monotonic() - previous["since"] >= .6) else None
                        rect = wait(stable, f"{name} visible and settled")
                        click(round((rect[0] + rect[2]) / 2), round((rect[1] + rect[3]) / 2))

                    click(90, 289)  # Established Updates sidebar row; actions use the real layout probe.
                    activate("Check for updates")
                    wait(lambda: (reports().get("update-notice") or {}).get("visualState") == "available", "signed available")
                    activate("Package verification")
                    wait(lambda: "verified" in (reports().get("preferences-shortcuts-layout", {}).get("update_checks") or {}).get("status", "").lower(),
                         "real GUI package verified", timeout=90)
                    assert not intent.exists(), "intent published before an accepted shutdown"
                    if args.supervised:
                        wait(lambda: ((reports().get("update-notice") or {}).get("footer") or {}).get("primary") == "Restart and install",
                            "real supervised install action")
                    if not visible:
                        # Normal WM close, not windowclose's forced XDestroyWindow.
                        run("xdotool", "windowactivate", "--sync", preferences,
                            "key", "--clearmodifiers", "alt+F4")
                        wait(lambda: not windows(source_pid, "^Captures Preferences$"), "source Preferences closed")
                    # Layout probes emit during a draw, before native presentation.
                    time.sleep(.6)
                    run("import", "-window", "root", directory / "before-shutdown.png")
                    if args.supervised:
                        saved = saved_data()
                        before_requests = len(requests)
                        if not install:
                            run("xdotool", "windowactivate", "--sync", preferences,
                                "key", "--clearmodifiers", "ctrl+q")
                            reply = json.loads(handoff.communicate(timeout=30)[0])
                            assert handoff.returncode == 0 and reply == {"state": "gui_exited", "replaced": False}, reply
                            assert requests[before_requests:] == [], "ordinary Quit started update HTTP"
                            assert not session_directory.exists(), "ordinary Quit retained owned scratch"
                            assert not list(directory.glob(".captures-native-pre-update-*")), "ordinary Quit created a snapshot"
                            assert {str(path.relative_to(package)): digest(path) for path in package.rglob("*") if path.is_file()} == original_packages[str(base)]
                            assert saved_data() == saved, "ordinary Quit changed accepted profile data"
                            results.append({"case": label, "passed": True, "checks": ["verified ordinary Quit never installs"]})
                            continue
                        if visible:
                            activate("Package verification")
                        else:
                            notice = wait(lambda: windows(source_pid, "^Captures Update$"), "install notice")[0]
                            geometry = dict(line.split("=", 1) for line in run("xdotool", "getwindowgeometry", "--shell", notice).decode().split())
                            # Shipping footer placement, as exercised by the existing notice smoke.
                            run("xdotool", "windowactivate", "--sync", notice, "mousemove", "--window", notice,
                                str(int(geometry["WIDTH"]) - 118), str(int(geometry["HEIGHT"]) - 48), "sleep", ".2", "click", "1")
                        wait(lambda: select.select([source_handle], [], [], 0)[0], "supervised source GUI drained")
                        assert not scratch.exists() or not list(scratch.iterdir()), "GUI scratch survived installation drain"
                    else:
                        # Quit from History, not the update action or Preferences focus.
                        run("xdotool", "windowactivate", "--sync", history, "windowfocus", "--sync", history, "sleep", ".4")
                        assert run("xdotool", "getwindowfocus").decode().strip() != preferences
                        run("xdotool", "key", "--clearmodifiers", "ctrl+q")
                        assert app.wait(timeout=30) == 0, "source GUI Quit failed"
                        assert not list(scratch.iterdir()), "GUI scratch survived shutdown"
                        record = json.loads(intent.read_text())
                        assert record["preferences_visible"] == visible, record
                        assert record["sha256"] == digest(archive) and record["profile"] == str(profile), record
                    for root in ("editor-drafts", "recording-recovery"):
                        assert (profile / root / ".retained/opaque-bytes.bin").read_bytes() == b"retain\0\xff" + root.encode()
                if not args.supervised:
                    saved = saved_data()
                    before_requests = len(requests)
                    command += ["--shutdown-intent-file", intent] if gui else ["--restore-preferences", str(visible).lower()]
                if gui:
                    original_intent = intent.read_bytes()
                    altered = json.loads(original_intent)
                    altered["sha256"] = "01" * 32
                    intent.write_text(json.dumps(altered))
                    rejected = subprocess.run(list(map(str, command)), env=env,
                        stdout=subprocess.PIPE, stderr=log, timeout=30)
                    assert rejected.returncode != 0 and intent.exists(), "mismatched target accepted/consumed"
                    assert requests[before_requests:] == ["/manifest.json", "/manifest.json.minisig"], "mismatch downloaded a package"
                    assert not list(directory.glob(".captures-native-pre-update-*")), "mismatch created a profile snapshot"
                    for name, expected in saved.items():
                        assert (profile / name).read_bytes() == expected, "mismatch changed profile data"
                    intent.write_bytes(original_intent)
                    before_requests = len(requests)
                if not args.supervised:
                    handoff = subprocess.Popen(list(map(str, command)), env=env, stdout=subprocess.PIPE, stderr=log)
                    children.append(handoff)
                # Inspect presentation while confirmation rehashes the whole
                # package; the five-second notice may expire before CLI reply.
                def ready_notice():
                    assert handoff.poll() in (None, 0), "helper failed; inspect processes.log"
                    result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name",
                        "^Captures is running$"], env=env, capture_output=True, text=True, timeout=5)
                    assert result.returncode in (0, 1), result.stderr
                    return result.stdout.splitlines()
                # Debug helpers rehash/re-extract the real ~200 MiB package more
                # than once before activation. This is not the post-spawn health
                # deadline; preserve that separate bounded startup assertion.
                notice = wait(ready_notice, "ready notice", timeout=300)[0]
                pid = int(run("xdotool", "getwindowpid", notice))
                handle = os.pidfd_open(pid)
                app_handles.append(handle)
                assert not windows(pid, "^Captures$"), "completed profile reopened setup"
                wait(lambda: not windows(pid, "^Capture History$"), "restart keeps History hidden")
                preferences = None
                if visible:
                    preferences = wait(lambda: windows(pid, "^Captures Preferences$"), "restored Preferences")[0]
                    # Keep the whole native frame within this small private desktop.
                    # The notice expires; do not block its review capture on a
                    # window-manager placement acknowledgement.
                    run("xdotool", "windowmove", preferences, "180", "150")
                else:
                    assert not windows(pid, "^Captures Preferences$"), "closed intent restored Preferences"
                time.sleep(.6)
                assert windows(pid, "^Captures is running$") == [notice], "notice expired before review capture"
                run("import", "-window", "root" if visible else notice, directory / "restart.png")
                assert windows(pid, "^Captures is running$") == [notice], "notice expired during review capture"
                stdout, _ = handoff.communicate(timeout=120)
                assert handoff.returncode == 0, "helper failed; inspect processes.log"
                reply = json.loads(stdout)
                assert reply["state"] == "confirmed" and reply["process_id"] == pid, reply
                assert {str(path.relative_to(package)): digest(path) for path in package.rglob("*")
                    if path.is_file()} == original_packages[str(target)], "confirmed replacement differs from the signed full target"
                if gui:
                    assert not intent.exists(), "helper did not consume intent once"
                snapshot = Path(reply["profile_snapshot"])
                assert snapshot.is_dir() and snapshot.parent == profile.parent
                for name, expected in saved.items():
                    assert (snapshot / name).read_bytes() == expected, f"snapshot changed {name}"
                    assert (profile / name).read_bytes() == expected, f"working data changed {name}"
                assert (profile / "startup.log").read_bytes() == old_log
                assert len(list(profile.glob("startup-*.log"))) == 1
                assert not (snapshot / "startup.log").exists() and not (snapshot / "source-snapshot").exists()
                assert requests[before_requests:] == ["/manifest.json", "/manifest.json.minisig", "/full"]
                if visible:
                    wait(lambda: not windows(pid, "^Captures is running$"), "notice expiry")
                    assert windows(pid, "^Captures Preferences$") == [preferences]
                    run("xdotool", "windowactivate", "--sync", preferences, "windowfocus", "--sync", preferences,
                        "sleep", ".4", "key", "ctrl+q")
                    wait(lambda: select.select([handle], [], [], 0)[0], "normal GUI Quit")
                else:
                    assert not windows(pid, "^Captures Preferences$"), "closed intent restored Preferences"
                    signal.pidfd_send_signal(handle, signal.SIGTERM)
                    wait(lambda: select.select([handle], [], [], 0)[0], "fixture termination")
                waited, status = os.waitpid(pid, 0)
                assert waited == pid and os.waitstatus_to_exitcode(status) == (0 if visible else -signal.SIGTERM), status
                for name, expected in saved.items():
                    assert (snapshot / name).read_bytes() == expected, f"snapshot lost after exit: {name}"
                results.append({"case": label, "passed": True, "profile_snapshot": str(snapshot),
                    "checks": ["signed real-host health", "byte-exact target package", "byte-exact data snapshot", "settings preserved",
                               "old log preserved", "completed-profile window routing", "snapshot retained after exit"]})
            for root in (base, target):
                assert original_packages[str(root)] == {str(path.relative_to(root)): digest(path)
                    for path in root.rglob("*") if path.is_file()}, "input package changed"
            summary = {"passed": True, "cases": results,
                "scope": "Real helper and explicit built Linux packages on private X11/software GL; "
                         "opaque draft/recovery byte retention here, real editor-draft semantics in Rust tests; "
                         "no installed update, permission identity or physical acceptance."}
            (output / "result.json").write_text(json.dumps(summary, indent=2))
            print(json.dumps(summary, indent=2))
        finally:
            for handle in app_handles:
                try:
                    if not select.select([handle], [], [], 0)[0]:
                        signal.pidfd_send_signal(handle, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                finally:
                    os.close(handle)
            for child in reversed(children):
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()
            server.shutdown()
            server.server_close()
            thread.join()


if __name__ == "__main__":
    main()
