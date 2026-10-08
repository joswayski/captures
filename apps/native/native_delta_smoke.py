#!/usr/bin/env python3
"""Real signed delta/full acquisition against explicit built development archives.

Disposable loopback keys and scratch only; never installs, registers or launches
an update. The signing-test executable is native_update_patch's Cargo test binary.
"""
import argparse
import copy
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import os
from pathlib import Path
from platform import machine
import shutil
import subprocess
import sys
import threading
import time

from history_fixture import write_completed_settings


def digest(path):
    hasher = hashlib.sha256()
    with path.open("rb") as source:
        while block := source.read(1024 * 1024):
            hasher.update(block)
    return hasher.hexdigest()


def gui_check(binary, output, url, base, state, renderer, log):
    """Hold each real HTTP body so native progress can be asserted and inspected."""
    assert sys.platform == "linux" and renderer == "wgpu", "GUI fixture is private X11 only"
    directory = output / "gui"
    directory.mkdir()
    settings = directory / "settings.json"
    write_completed_settings(settings)
    scratch = directory / "scratch"
    scratch.mkdir()
    env = {**os.environ, "WGPU_BACKEND": "gl", "WINIT_X11_SCALE_FACTOR": "1",
           "XDG_SESSION_TYPE": "x11", "CAPTURES_NATIVE_LAYOUT_PROBE": "1",
           "CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER": "1"}
    env.pop("WAYLAND_DISPLAY", None)
    children = []
    state.update(mode="tampered-patch", paths=[], held={}, gates={"/patch": threading.Event(), "/full": threading.Event()})

    def spawn(command, announce=False):
        child = subprocess.Popen(list(map(str, command)), env=env,
            stdout=subprocess.PIPE if announce else log, stderr=log)
        children.append(child)
        return child

    def run(*command):
        return subprocess.check_output(command, env=env, stderr=log, timeout=10)

    def wait(predicate, description, timeout=45):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if result := predicate():
                return result
            time.sleep(.05)
        run("import", "-window", "root", str(directory / "timeout.png"))
        raise AssertionError(description)

    try:
        xvfb = spawn(["Xvfb", "-displayfd", "1", "-screen", "0", "1280x900x24", "-nolisten", "tcp"], True)
        env["DISPLAY"] = ":" + xvfb.stdout.readline().decode().strip()
        spawn(["openbox", "--sm-disable"])
        wait(lambda: b"window id" in run("xprop", "-root", "_NET_SUPPORTING_WM_CHECK"), "window manager")
        event_path = directory / "events.jsonl"
        with event_path.open("w") as events:
            app = subprocess.Popen([str(binary.resolve(strict=True)), "--live", "--open-preferences",
                "--history-root", str(directory / "history"), "--settings-file", str(settings),
                "--native-update-manifest-url", url + "/manifest.json",
                "--native-update-public-key-file", str(output / "public.key"),
                "--native-update-current-version", "2026.9.99",
                "--native-update-staging-directory", str(scratch),
                "--native-update-base-archive", str(base)], env=env, stdout=events, stderr=log)
        children.append(app)
        last = {}
        offset = 0

        def reports():
            nonlocal offset
            with event_path.open() as events:
                events.seek(offset)
                while (line := events.readline()).endswith("\n"):
                    offset += len(line.encode())
                    try:
                        value = json.loads(line)
                        last[value["event"]] = value["detail"]
                    except (ValueError, KeyError):
                        pass
            return last

        def window(name):
            result = subprocess.run(["xdotool", "search", "--all", "--onlyvisible", "--pid", str(app.pid),
                                     "--name", name], env=env, capture_output=True, text=True, timeout=5)
            assert result.returncode in (0, 1)
            return result.stdout.splitlines()

        def click(window, x, y):
            run("xdotool", "windowactivate", "--sync", window, "mousemove", "--window", window,
                str(x - 1), str(y), "mousemove_relative", "1", "0", "sleep", ".15", "click", "1")

        preferences = wait(lambda: window("^Captures Preferences$"), "Preferences visible")[0]
        # Same established sidebar geometry as x11_feedback_smoke.py; control
        # clicks below follow the runtime layout probe, never guessed positions.
        click(preferences, 90, 289)

        def control(name):
            detail = reports().get("preferences-shortcuts-layout", {})
            rect = detail.get("controls", {}).get(name)
            page = detail.get("page")
            return rect if rect and page and page[1] <= rect[1] and rect[3] <= page[3] else None

        def activate(name):
            previous = {"rect": None, "since": time.monotonic()}
            def stable():
                rect = control(name)
                if rect != previous["rect"]:
                    previous.update(rect=rect, since=time.monotonic())
                return rect if rect and time.monotonic()-previous["since"] >= .6 else None
            rect = wait(stable, f"{name} visible and layout settled")
            click(preferences, round((rect[0]+rect[2])/2), round((rect[1]+rect[3])/2))

        activate("Check for updates")
        wait(lambda: (reports().get("update-notice") or {}).get("visualState") == "available", "signed update available")
        activate("Package verification")
        for path, total, name in [("/patch", (output / "update.bsdiff").stat().st_size, "patch-progress"),
                                  ("/full", state["full_size"], "full-fallback-progress")]:
            received = wait(lambda: state["held"].get(path), f"{path} held HTTP progress")
            percent = math.floor(received * 100 / total + .5)
            expected = f"Downloading {received} of {total} bytes"
            def shown():
                values = reports()
                detail = values.get("update-notice") or {}
                download = detail.get("download")
                copy = values.get("preferences-shortcuts-layout", {}).get("update_checks") or {}
                return download and download[1] == percent and copy.get("status") == expected
            wait(shown, f"actual {path} byte total and percentage published")
            notice = wait(lambda: window("^Captures Update$"), "progress notice visible")[0]
            time.sleep(.4)
            run("import", "-window", notice, str(directory / f"{name}.png"))
            state["gates"][path].set()
        wait(lambda: "verified" in (reports().get("preferences-shortcuts-layout", {}).get("update_checks") or {}).get("status", "").lower(),
             "fallback package verified", timeout=90)
        run("xdotool", "windowactivate", "--sync", preferences, "key", "--clearmodifiers", "ctrl+q")
        assert app.wait(timeout=20) == 0, "real Quit failed"
        assert not list(scratch.iterdir()), "owned acquisition/staging files survived Quit"
        assert state["paths"] == ["/manifest.json", "/manifest.json.minisig", "/patch", "/full"]
        return {"passed": True, "scope": "Private-X11 signed patch and full-fallback progress, staging, real Quit and cleanup; no installation."}
    finally:
        for gate in state["gates"].values():
            gate.set()
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("patch-tool", "signing-test", "probe", "base-archive", "target-archive", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--renderer", choices=("wgpu", "appkit"), default="wgpu")
    parser.add_argument("--gui", type=Path, help="Also exercise real progress on private X11 with this native host")
    parser.add_argument("--existing-patch", type=Path, help="Reuse an already generated patch; acquisition still verifies exact reconstruction")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    base, target = args.base_archive.resolve(strict=True), args.target_archive.resolve(strict=True)
    patch = output / "update.bsdiff"
    original = (digest(base), digest(target))
    state = {"paths": [], "mode": "valid"}
    state["full_size"] = target.stat().st_size

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            state["paths"].append(self.path)
            path = {"/manifest.json": output / "manifest.json",
                    "/manifest.json.minisig": output / "manifest.json.minisig",
                    "/patch": patch, "/full": target}.get(self.path)
            if path is None:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Length", str(path.stat().st_size))
            self.end_headers()
            position = 0
            try:
                with path.open("rb") as source:
                    while block := source.read(64 * 1024):
                        if (self.path == "/patch" and state["mode"] == "tampered-patch"
                                or self.path == "/full" and state["mode"] == "tampered-full") and position == 0:
                            altered = bytearray(block)
                            altered[31] ^= 17
                            block = altered
                        if self.path == "/manifest.json" and state["mode"] == "tampered-manifest" and position == 0:
                            block = b" " + block[1:]
                        self.wfile.write(block)
                        position += len(block)
                        gate = state.get("gates", {}).get(self.path)
                        if gate is not None and self.path not in state["held"] and position >= path.stat().st_size // 3:
                            self.wfile.flush()
                            state["held"][self.path] = position
                            assert gate.wait(120), "GUI did not release held response"
            except (BrokenPipeError, ConnectionResetError):
                pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    url = f"http://127.0.0.1:{server.server_port}"
    thread = threading.Thread(target=server.serve_forever)
    thread.start()
    results = []
    with (output / "processes.log").open("w") as log:
        def run(command, name, env=None):
            command = list(map(str, command))
            timing = output / f"{name}-cost.txt"
            if sys.platform == "linux" and Path("/usr/bin/time").is_file():
                command = ["/usr/bin/time", "-f", "elapsed_seconds=%e\npeak_rss_kib=%M", "-o", str(timing), *command]
            started = time.monotonic()
            result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=600)
            log.write(f"{name}: exit {result.returncode}\n{result.stdout}\n{result.stderr}\n")
            log.flush()
            return result, time.monotonic() - started

        try:
            if args.existing_patch:
                shutil.copyfile(args.existing_patch.resolve(strict=True), patch)
                elapsed = None
                metadata = {"delta": {"format": "bsdiff40", "base_version": "2026.9.99",
                    "base_size": base.stat().st_size, "base_sha256": original[0], "url": url + "/patch",
                    "size": patch.stat().st_size, "sha256": digest(patch)},
                    "target": {"size": target.stat().st_size, "sha256": original[1]},
                    "smaller": patch.stat().st_size < target.stat().st_size}
            else:
                generated, elapsed = run([args.patch_tool.resolve(strict=True), "--base-archive", base,
                    "--target-archive", target, "--base-version", "2026.9.99",
                    "--patch-url", url + "/patch", "--output", patch], "generation")
                assert generated.returncode == 0, generated.stderr
                metadata = json.loads(generated.stdout)
            assert metadata["delta"]["base_sha256"] == original[0]
            assert metadata["target"]["sha256"] == original[1]
            assert metadata["delta"]["sha256"] == digest(patch)
            assert metadata["delta"]["size"] == patch.stat().st_size
            assert metadata["target"]["size"] == target.stat().st_size
            platform = {"linux": "x86_64-unknown-linux-gnu", "win32": "x86_64-pc-windows-msvc",
                        "darwin": "aarch64-apple-darwin" if machine() == "arm64" else "x86_64-apple-darwin"}[sys.platform]
            artifact = {**metadata["target"], "url": url + "/full", "delta": metadata["delta"]}
            manifest = {"schema": 2, "identity": "es.captur.native-development", "renderer": args.renderer,
                        "version": "2026.10.50", "notes": "Disposable acquisition fixture.", "artifacts": {platform: artifact}}
            (output / "metadata.json").write_text(json.dumps(metadata, indent=2))
            wrong = output / "wrong-base"
            shutil.copyfile(base, wrong)
            with wrong.open("r+b") as file:
                byte = file.read(1)
                file.seek(0)
                file.write(bytes([byte[0] ^ 31]))
            scratch = output / "scratch"
            scratch.mkdir()
            sentinel = scratch / "operator-data"
            sentinel.write_bytes(b"not owned by updater")
            patch_saves = metadata["smaller"]
            for mode in ("valid", "wrong-base", "absent-base", "tampered-patch", "not-smaller",
                         "legacy", "tampered-manifest", "tampered-full"):
                value = copy.deepcopy(manifest)
                if mode == "not-smaller":
                    value["artifacts"][platform]["delta"]["size"] = target.stat().st_size
                if mode == "legacy":
                    value["schema"] = 1
                    del value["artifacts"][platform]["delta"]
                (output / "manifest.json").write_text(json.dumps(value))
                signed, _ = run([args.signing_test.resolve(strict=True), "tests::sign_development_fixture",
                    "--exact", "--ignored"], f"sign-{mode}",
                    {**os.environ, "CAPTURES_NATIVE_DELTA_FIXTURE": str(output)})
                assert signed.returncode == 0, signed.stderr
                state.update(mode=mode, paths=[])
                command = [args.probe.resolve(strict=True), "--manifest-url", url + "/manifest.json",
                    "--public-key-file", output / "public.key", "--current-version", "2026.9.99",
                    "--renderer", args.renderer, "--stage-directory", scratch]
                if mode != "absent-base":
                    command += ["--base-archive", wrong if mode in ("wrong-base", "tampered-full") else base]
                result, seconds = run(command, mode)
                expected = ["/manifest.json", "/manifest.json.minisig"]
                if mode != "tampered-manifest":
                    if patch_saves and mode in ("valid", "tampered-patch"):
                        expected += ["/patch"]
                    if not patch_saves or mode != "valid":
                        expected += ["/full"]
                assert state["paths"] == expected, (mode, state["paths"], expected)
                if mode in ("tampered-manifest", "tampered-full"):
                    assert result.returncode != 0, mode
                    assert "signature verification failed" in result.stderr if mode == "tampered-manifest" else "hash does not match" in result.stderr
                else:
                    assert result.returncode == 0, (mode, result.stderr)
                    reply = json.loads(result.stdout)
                    assert reply["state"] == "staged" and not reply["installed"]
                    assert reply["release"]["size"] == target.stat().st_size
                assert list(scratch.iterdir()) == [sentinel], mode
                assert sentinel.read_bytes() == b"not owned by updater"
                results.append({"case": mode, "paths": expected, "seconds": round(seconds, 3), "passed": True})
            assert original == (digest(base), digest(target)), "source archives changed"
            summary = {"passed": True, "target": platform, "base_bytes": base.stat().st_size,
                       "full_bytes": target.stat().st_size, "patch_bytes": patch.stat().st_size,
                       "patch_smaller": patch_saves, "generation_seconds": round(elapsed, 3) if elapsed is not None else None, "cases": results,
                       "scope": "Actual explicitly supplied built development archives; no installation or physical-platform acceptance."}
            if args.gui:
                assert patch_saves, "GUI patch-progress fixture needs a smaller patch"
                summary["gui"] = gui_check(args.gui, output, url, base, state, args.renderer, log)
            (output / "result.json").write_text(json.dumps(summary, indent=2))
            print(json.dumps(summary, indent=2))
        finally:
            server.shutdown()
            server.server_close()
            thread.join()


if __name__ == "__main__":
    main()
