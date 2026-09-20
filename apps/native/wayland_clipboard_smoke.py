#!/usr/bin/env python3
"""Verify native Wayland image clipboard transport on disposable headless Sway.

Requires Sway/wlroots with wlr-data-control, wl-paste, and ImageMagick. DISPLAY
is removed, so arboard cannot turn a failed Wayland connection into X11 success.
"""

import argparse
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import tempfile
import time


WIDTH = 3
HEIGHT = 2
PIXELS = bytes([
    1, 2, 3, 4,
    250, 17, 99, 255,
    0, 127, 255, 63,
    19, 211, 7, 128,
    88, 44, 222, 200,
    5, 6, 7, 8,
])


def wait_for_socket(runtime: Path, compositor: subprocess.Popen, log: Path) -> Path:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        sockets = [path for path in runtime.glob("wayland-*") if path.is_socket()]
        if len(sockets) == 1:
            return sockets[0]
        if len(sockets) > 1:
            raise RuntimeError(f"headless Sway created multiple Wayland sockets: {sockets}")
        if compositor.poll() is not None:
            raise RuntimeError(f"headless Sway exited early:\n{log.read_text(errors='replace')}")
        time.sleep(0.05)
    raise RuntimeError(f"headless Sway did not create a socket:\n{log.read_text(errors='replace')}")


def read_ready_line(probe: subprocess.Popen) -> dict:
    readable, _, _ = select.select([probe.stdout], [], [], 10)
    if not readable:
        raise RuntimeError("clipboard probe did not report readiness")
    line = probe.stdout.readline()
    if not line:
        error = probe.stderr.read().decode(errors="replace")
        raise RuntimeError(f"clipboard probe exited before readiness: {error}")
    return json.loads(line)


def pasted_rgba(env: dict) -> bytes:
    png = subprocess.check_output(["wl-paste", "--type", "image/png"], env=env)
    return subprocess.check_output(
        ["convert", "png:-", "-alpha", "on", "-depth", "8", "rgba:-"],
        input=png,
        env=env,
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    args = parser.parse_args()
    for command in ["sway", "wl-paste", "convert"]:
        if shutil.which(command) is None:
            raise RuntimeError(f"required command is unavailable: {command}")
    binary = args.binary.resolve()
    if not binary.is_file():
        raise RuntimeError(f"clipboard probe binary is unavailable: {binary}")

    with tempfile.TemporaryDirectory(prefix="captures-wayland-clipboard-") as temporary:
        root = Path(temporary)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        config = root / "sway.conf"
        config.write_text("output HEADLESS-1 resolution 800x600\nseat seat0 fallback true\n")
        log = root / "sway.log"
        env = os.environ.copy()
        env.pop("DISPLAY", None)
        env.pop("SWAYSOCK", None)
        env.update({
            "XDG_RUNTIME_DIR": str(runtime),
            "XDG_SESSION_TYPE": "wayland",
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_LIBINPUT_NO_DEVICES": "1",
            "WLR_RENDERER": "pixman",
        })

        with log.open("wb") as compositor_log:
            compositor = subprocess.Popen(
                ["sway", "--unsupported-gpu", "--config", str(config)],
                env=env,
                stdout=compositor_log,
                stderr=subprocess.STDOUT,
            )
        probe = None
        try:
            socket = wait_for_socket(runtime, compositor, log)
            env["WAYLAND_DISPLAY"] = socket.name
            probe = subprocess.Popen(
                [str(binary), "--serve-image"],
                env=env,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            ready = read_ready_line(probe)
            assert ready == {
                "event": "clipboard_ready",
                "width": WIDTH,
                "height": HEIGHT,
                "rgba_bytes": len(PIXELS),
                "display_unset": True,
            }, ready

            first = pasted_rgba(env)
            second = pasted_rgba(env)
            assert first == PIXELS, (first, PIXELS)
            assert second == PIXELS, (second, PIXELS)
            assert probe.poll() is None, "selection owner exited before repeated paste"
            print(json.dumps({
                "wayland_display": env["WAYLAND_DISPLAY"],
                "display_unset": "DISPLAY" not in env,
                "protocol": "wlr-data-control",
                "image": {"width": WIDTH, "height": HEIGHT, "rgba_bytes": len(PIXELS)},
                "exact_rgba_pastes": 2,
                "owner_alive_during_reads": True,
                "scope": "Disposable headless Sway transport; not physical Wayland UI, input, capture, or accessibility acceptance.",
            }, indent=2))
        finally:
            if probe is not None and probe.poll() is None:
                probe.stdin.write(b"\n")
                probe.stdin.flush()
                probe.wait(timeout=5)
            compositor.terminate()
            try:
                compositor.wait(timeout=5)
            except subprocess.TimeoutExpired:
                compositor.kill()
                compositor.wait(timeout=5)


if __name__ == "__main__":
    main()
