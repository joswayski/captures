#!/usr/bin/python3
"""Real signed development updates on private Xvfb/Openbox/software GL.

Shares the Wayland acquisition/supervision matrix, not the stub notice source.
X11 window geometry and xdotool input, including the real XFCE tray popup,
exercise disposable profiles/copied packages. No installed or physical acceptance.
"""
import argparse
import json
import os
from pathlib import Path
import re
import select
import shutil
import subprocess
import tarfile
import time

from wayland_update_smoke import (
    HEIGHT, PREFERENCES, VERSION, WIDTH, Host as WaylandHost, Loopback,
    acquisition, digest, fingerprint, supervision,
)
from x11_onboarding_smoke import start_tray


def run(env, *command):
    return subprocess.check_output(list(map(str, command)), env=env, stderr=subprocess.PIPE, timeout=15)


def geometry(env, window):
    # xdotool double-counts Openbox's reparented frame offset. Query the
    # translated client origin so absolute clicks use real root coordinates.
    info = run(env, "xwininfo", "-id", window).decode()
    return {name: int(re.search(rf"^\s*{key}:\s*(-?\d+)", info, re.MULTILINE)[1])
            for name, key in (("x", "Absolute upper-left X"), ("y", "Absolute upper-left Y"),
                              ("width", "Width"), ("height", "Height"))}


class Host(WaylandHost):
    @staticmethod
    def windows(env):
        found = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", ".*"],
                               env=env, capture_output=True, text=True, timeout=5)
        assert found.returncode in (0, 1), found.stderr
        focus = int(run(env, "xdotool", "getwindowfocus").strip())
        nodes = []
        for identifier in found.stdout.split():
            # Managed and nonactivating notice windows both carry the real PID.
            # Ignore the root/panel's unnamed children, not arbitrary app titles.
            try:
                pid = run(env, "xprop", "-id", identifier, "_NET_WM_PID").decode()
                if " = " not in pid:
                    continue
                nodes.append({"id": int(identifier), "pid": int(pid.split(" = ", 1)[1]),
                              "name": run(env, "xdotool", "getwindowname", identifier).decode().strip(),
                              "rect": geometry(env, identifier), "focused": int(identifier) == focus})
            except subprocess.CalledProcessError as error:
                vanished = (b"BadWindow" in error.stderr or
                            (error.cmd[0] == "xwininfo" and b"xwininfo: error: No such window with id " in error.stderr))
                if not vanished:
                    raise
                # A GTK popup/app window can disappear after search during Quit.
        return nodes

    @staticmethod
    def capture(env, path):
        run(env, "import", "-window", "root", path)

    def focus(self, node):
        run(self.env, "xdotool", "windowactivate", "--sync", node["id"],
            "windowfocus", "--sync", node["id"])

    def move(self, node, x, y):
        run(self.env, "xdotool", "windowmove", node["id"], x, y)

    def close_preferences(self):
        node = self.window()
        self.focus(node)
        run(self.env, "xdotool", "key", "--clearmodifiers", "alt+F4")

    def arrange(self, width=740, height=660):
        from wayland_native_capture_smoke import wait
        node = wait(self.window, PREFERENCES)
        run(self.env, "xdotool", "windowsize", node["id"], width, height)
        self.move(node, 20, 100)
        wait(lambda: self.window()["rect"]["width"] == width
             and self.window()["rect"]["height"] == height, "Preferences arrangement")
        time.sleep(.5)
        self.focus(node)
        wait(lambda: self.window()["focused"], "Preferences focus after arrangement")
        time.sleep(.5)

    def input_at(self, x, y, scroll=None):
        run(self.env, "xdotool", "mousemove", "--sync", round(x) - 1, round(y),
            "mousemove_relative", "--sync", "1", "0")
        if scroll is None:
            run(self.env, "xdotool", "sleep", ".15", "mousedown", "1", "sleep", ".1", "mouseup", "1")
        else:
            # Native X11 wheel input is discrete; only the delivery changes.
            # The shared loop still requires actual page/control clipping.
            run(self.env, "xdotool", "click", "--repeat", max(1, round(abs(scroll) / 40)),
                "--delay", "80", "5" if scroll > 0 else "4")

    def quit(self):
        from wayland_native_capture_smoke import wait
        panels = run(self.env, "xdotool", "search", "--onlyvisible", "--class", "xfce4-panel").decode().split()
        panel = next(window for window in panels if geometry(self.env, window)["width"] >= 24)
        rect = geometry(self.env, panel)
        run(self.env, "xdotool", "mousemove", "--window", panel, rect["width"] // 2,
            rect["height"] // 2, "click", "3")

        def popup():
            windows = run(self.env, "xdotool", "search", "--onlyvisible", "--class", ".*").decode().split()
            return next((window for window in windows
                         if b"_MENU" in run(self.env, "xprop", "-id", window, "_NET_WM_WINDOW_TYPE")), None)

        menu = wait(popup, "real X11 tray popup for Quit")
        evidence = Path(self.env["XDG_RUNTIME_DIR"]).parent / f"{self.pid}-quit-menu.png"
        self.capture(self.env, evidence)
        evidence.with_suffix(".json").write_text(json.dumps({"window": int(menu),
            "rect": geometry(self.env, menu), "input": "End Return"}, indent=2))
        # Quit is the final shipping row. End avoids counting Check for Updates,
        # whose enabled state varies across pending I/O and restarted hosts.
        run(self.env, "xdotool", "key", "--delay", "60", "End", "Return")


def main():
    from wayland_native_capture_smoke import wait
    from wayland_screenshot_smoke import stop

    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("binary", "signing-test", "importer", "package", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--helper", type=Path, help="Also exercise copied-package supervision and startup health")
    args = parser.parse_args()
    for name in ("binary", "signing_test", "importer", "package", "helper"):
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
                "version": VERSION, "notes": "Disposable X11 updater fixture.",
                "artifacts": {"x86_64-unknown-linux-gnu": {"url": fixture.url + "/full",
                    "size": archive.stat().st_size, "sha256": digest(archive)}}}
    (root / "manifest.json").write_text(json.dumps(manifest))
    env = os.environ.copy()
    for key in ("DISPLAY", "WAYLAND_DISPLAY", "SWAYSOCK", "DBUS_SESSION_BUS_ADDRESS", "DBUS_SYSTEM_BUS_ADDRESS"):
        env.pop(key, None)
    for key, directory in (("XDG_DATA_HOME", "data"), ("XDG_CONFIG_HOME", "config"),
                           ("XDG_CACHE_HOME", "cache"), ("XDG_RUNTIME_DIR", "runtime")):
        path = root / directory
        path.mkdir(mode=0o700)
        env[key] = str(path)
    env.update(XDG_SESSION_TYPE="x11", XDG_CURRENT_DESKTOP="XFCE", GDK_BACKEND="x11",
               WINIT_X11_SCALE_FACTOR="1", WGPU_BACKEND="gl", NO_AT_BRIDGE="1",
               NO_PROXY="127.0.0.1,localhost", CAPTURES_NATIVE_LAYOUT_PROBE="1",
               CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER="1",
               CAPTURES_FFMPEG=str(args.package / "binaries/ffmpeg-x86_64-unknown-linux-gnu"),
               CAPTURES_FFPROBE=str(args.package / "binaries/ffprobe-x86_64-unknown-linux-gnu"))
    children, evidence = [], []
    try:
        subprocess.run([str(args.signing_test), "tests::sign_development_fixture", "--exact", "--ignored"],
                       env={**env, "CAPTURES_NATIVE_DELTA_FIXTURE": str(root)}, check=True, timeout=30)
        with (root / "services.log").open("w") as log:
            def spawn(command, announce=False):
                child = subprocess.Popen(command, env=env, stdout=subprocess.PIPE if announce else log, stderr=log)
                children.append(child)
                if announce:
                    assert select.select([child.stdout], [], [], 30)[0], "process did not announce readiness"
                return child

            server = spawn(["Xvfb", "-displayfd", "1", "-screen", "0", f"{WIDTH}x{HEIGHT}x24", "-nolisten", "tcp"], True)
            endpoint = server.stdout.readline().decode().strip()
            assert endpoint, "X server failed before readiness"
            env["DISPLAY"] = ":" + endpoint
            run(env, "xdpyinfo")
            spawn(["openbox", "--sm-disable"])
            wait(lambda: b"window id" in run(env, "xprop", "-root", "_NET_SUPPORTING_WM_CHECK"), "window manager ready")
            bus = start_tray(root, env, spawn, wait)
            env["DBUS_SYSTEM_BUS_ADDRESS"] = env["DBUS_SESSION_BUS_ADDRESS"]
            acquisition(args, root, env, bus, None, fixture, evidence, host_class=Host)
            if args.helper:
                supervision(args, root, env, bus, None, fixture, evidence, host_class=Host)
        assert not fixture.errors, fixture.errors
        assert fingerprint(args.package) == original_package and fingerprint(target) == target_before
        summary = {"passed": True, "display_backend": "Xvfb/Openbox/software GL", "wayland_display_unset": True,
                   "host_sha256": digest(args.binary), "package_files": original_package,
                   "package_build_info": json.loads((args.package / "BUILD_INFO.json").read_text()),
                   "archive_bytes": archive.stat().st_size, "archive_sha256": digest(archive), "cases": evidence,
                   "input_package_unchanged": True, "test_target_unchanged": True,
                   "scope": "Private X11, real signed loopback acquisition and copied-package supervision; "
                            "no installed, physical, screen-reader or production signing/channel acceptance."}
        (root / "result.json").write_text(json.dumps(summary, indent=2))
        print(json.dumps(summary, indent=2))
    finally:
        fixture.close()
        for child in reversed(children):
            stop(child)


if __name__ == "__main__":
    main()
