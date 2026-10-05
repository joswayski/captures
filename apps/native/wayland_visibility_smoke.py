#!/usr/bin/env python3
"""Live NULL-buffer visibility regression test on a private headless Sway."""

import os
from pathlib import Path
import select
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "apps/native/wayland_drag_probe/Cargo.toml"
PROBE = ROOT / "apps/native/wayland_drag_probe/target/debug/captures-wayland-drag-probe"
BG = bytes((16, 32, 48, 255))
PRIMARY = bytes((204, 51, 17, 255))
SECONDARY = bytes((34, 170, 85, 255))


def line(process, prefix, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if select.select([process.stdout], [], [], deadline - time.monotonic())[0]:
            value = process.stdout.readline().strip()
            if value.startswith(prefix):
                return value
        if process.poll() is not None:
            raise RuntimeError(f"probe exited ({process.returncode})")
    raise RuntimeError(f"timed out waiting for {prefix!r}")


def command(process, value):
    process.stdin.write(value + "\n")
    process.stdin.flush()


def status(process, index, expected):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        command(process, f"status {index}")
        if line(process, f"STATUS {index} ") == f"STATUS {index} Some({str(expected).lower()})":
            return
        time.sleep(.02)
    raise AssertionError(f"window {index} did not become visible={expected}")


def pixels(path):
    return subprocess.check_output(["convert", str(path), "-depth", "8", "rgba:-"])


def capture(env, path):
    subprocess.run(["grim", str(path)], env=env, check=True, timeout=5)
    return pixels(path)


def settled_capture(env, path, predicate):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        image = capture(env, path)
        if predicate(image):
            return image
        time.sleep(.05)
    raise AssertionError("compositor did not reach the independently expected pixels")


def main():
    subprocess.run(["cargo", "build", "--manifest-path", str(MANIFEST)], cwd=ROOT, check=True)
    with tempfile.TemporaryDirectory(prefix="captures-visibility-") as temporary:
        root = Path(temporary)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        config = root / "sway.conf"
        config.write_text("""output HEADLESS-1 resolution 320x240
output * bg #102030 solid_color
default_border none
default_floating_border none
for_window [title="visibility-primary"] floating enable, resize set 80 60, move absolute position 20 20
for_window [title="visibility-initially-hidden"] floating enable, resize set 80 60, move absolute position 140 20
""")
        env = os.environ.copy()
        env.update(XDG_RUNTIME_DIR=str(runtime), WLR_BACKENDS="headless", WLR_LIBINPUT_NO_DEVICES="1")
        log = (root / "sway.log").open("w")
        sway = subprocess.Popen(["sway", "--unsupported-gpu", "-c", str(config)], env=env,
                                stdout=log, stderr=log)
        probe = None
        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                sockets = [path for path in runtime.glob("wayland-*") if path.is_socket()]
                if sockets:
                    env["WAYLAND_DISPLAY"] = sockets[0].name
                    break
                assert sway.poll() is None, "Sway exited"
                time.sleep(.05)
            else:
                raise RuntimeError("Sway socket timeout")
            probe = subprocess.Popen([str(PROBE), "visibility"], env=env, text=True,
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log)
            line(probe, "READY visibility-primary")
            line(probe, "READY visibility-initially-hidden")
            status(probe, 0, True)
            status(probe, 1, False)
            visible = capture(env, root / "visible.png")
            assert visible.count(PRIMARY) == 80 * 60
            assert SECONDARY not in visible

            command(probe, "hide 0")
            command(probe, "redraw 0")
            status(probe, 0, False)  # wl_display.sync proves compositor processed NULL commit.
            hidden = capture(env, root / "hidden.png")
            assert hidden == BG * (320 * 240), "hidden surface left compositor pixels"

            command(probe, "hide 0")  # idempotent
            command(probe, "show 0")
            command(probe, "show 0")  # idempotent
            status(probe, 0, True)
            restored = settled_capture(env, root / "restored.png", lambda image: image.count(PRIMARY) == 80 * 60)
            assert restored.count(PRIMARY) == 80 * 60, restored.count(PRIMARY)
            assert restored.count(BG) == 320 * 240 - 80 * 60

            command(probe, "show 1")
            status(probe, 1, True)
            both = settled_capture(env, root / "both.png", lambda image: image.count(PRIMARY) == 80 * 60 and image.count(SECONDARY) == 80 * 60)
            assert both.count(PRIMARY) == 80 * 60 and both.count(SECONDARY) == 80 * 60
            for action in ("hide", "show", "hide", "show"):
                command(probe, f"{action} 0")
            status(probe, 0, True)
            settled_capture(env, root / "rapid.png", lambda image: image.count(PRIMARY) == 80 * 60 and image.count(SECONDARY) == 80 * 60)
            print("visibility smoke: repeated hide/show, hidden redraw, initial hidden, exact pixels PASS")
        except Exception:
            log.flush()
            print((root / "sway.log").read_text())
            raise
        finally:
            if probe and probe.poll() is None:
                command(probe, "quit")
                probe.wait(timeout=3)
            sway.terminate()
            sway.wait(timeout=3)


if __name__ == "__main__":
    main()
