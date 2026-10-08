#!/usr/bin/python3
"""Signed helper restarts of enrolled development profiles on private software X11.

Copies explicit built packages; all profile/config/export data is disposable.
No installed profile, GUI installer, OS permission or release-channel acceptance.
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
    args = parser.parse_args()
    assert sys.platform == "linux", "This real-host fixture is private X11 only"
    # Adopt the GUI after its helper exits so real Ctrl+Q can assert exit status,
    # not merely PID disappearance. This affects only this disposable test process.
    libc = ctypes.CDLL(None, use_errno=True)
    pr_set_child_subreaper = 36
    assert libc.prctl(pr_set_child_subreaper, 1, 0, 0, 0) == 0, os.strerror(ctypes.get_errno())
    helper, importer, signer = (path.resolve(strict=True) for path in
        (args.helper, args.importer, args.signing_test))
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
            for appearance, visible in (("dark", True), ("light", True), ("dark", False)):
                label = f"{appearance}-{'visible' if visible else 'closed'}"
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
                saved = {str(path.relative_to(profile)): path.read_bytes()
                    for name in ("settings.json", "history", "editor-drafts", "recording-recovery",
                                 ".captures-native-development-profile.json")
                    for path in ([profile / name] if (profile / name).is_file() else (profile / name).rglob("*"))
                    if path.is_file()}
                before_requests = len(requests)
                command = [helper, "--manifest-url", url + "/manifest.json",
                    "--public-key-file", output / "public.key", "--current-version", "2026.9.99",
                    "--renderer", "wgpu", "--stopped-development-package", package,
                    "--existing-development-profile", profile, "--all-app-processes-stopped",
                    "--health-timeout-seconds", "60", "--restore-preferences", str(visible).lower()]
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
                notice = wait(ready_notice, "ready notice", timeout=120)[0]
                pid = int(run("xdotool", "getwindowpid", notice))
                handle = os.pidfd_open(pid)
                app_handles.append(handle)
                assert not windows(pid, "^Captures$"), "completed profile reopened setup"
                wait(lambda: not windows(pid, "^Capture History$"), "restart keeps History hidden")
                preferences = None
                if visible:
                    preferences = wait(lambda: windows(pid, "^Captures Preferences$"), "restored Preferences")[0]
                    # Keep the whole native frame within this small private desktop.
                    run("xdotool", "windowmove", "--sync", preferences, "180", "150")
                else:
                    assert not windows(pid, "^Captures Preferences$"), "closed intent restored Preferences"
                time.sleep(.6)
                run("import", "-window", "root" if visible else notice, directory / "restart.png")
                stdout, _ = handoff.communicate(timeout=120)
                assert handoff.returncode == 0, "helper failed; inspect processes.log"
                reply = json.loads(stdout)
                assert reply["state"] == "confirmed" and reply["process_id"] == pid, reply
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
                    "checks": ["signed real-host health", "byte-exact data snapshot", "settings preserved",
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
