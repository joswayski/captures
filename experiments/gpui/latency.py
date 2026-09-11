#!/usr/bin/env python3
"""External X11 input-to-visible-selection latency sampler for Captures.

This deliberately observes compositor output in the root window.  It does not
instrument either application and it does not treat acceptance of an X request
as presentation.
"""
import argparse
import ctypes
from datetime import datetime, timezone
import importlib.util
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import tempfile
import time


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("gpui_benchmark", HERE / "benchmark.py")
gpui_benchmark = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gpui_benchmark)

PATCH = (494, 176, 13, 9)  # Surrounds (500, 180), on the expected top edge.
START = (320, 180)
END = (1050, 630)
EXPECTED = (0xFF, 0xCA, 0x28)


class X11:
    """Minimal libX11/libXtst binding; intentionally avoids Python packages."""

    def __init__(self):
        self.x = ctypes.CDLL("libX11.so.6")
        self.xtst = ctypes.CDLL("libXtst.so.6")
        self.x.XOpenDisplay.argtypes = [ctypes.c_char_p]
        self.x.XOpenDisplay.restype = ctypes.c_void_p
        self.display = self.x.XOpenDisplay(None)
        if not self.display:
            raise RuntimeError("cannot open DISPLAY")
        self.x.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
        self.x.XDefaultRootWindow.restype = ctypes.c_ulong
        self.root = self.x.XDefaultRootWindow(self.display)
        self.x.XGetImage.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int,
                                     ctypes.c_int, ctypes.c_uint, ctypes.c_uint,
                                     ctypes.c_ulong, ctypes.c_int]
        self.x.XGetImage.restype = ctypes.c_void_p
        self.x.XGetPixel.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int]
        self.x.XGetPixel.restype = ctypes.c_ulong
        self.x.XDestroyImage.argtypes = [ctypes.c_void_p]
        self.xtst.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int,
                                                   ctypes.c_int, ctypes.c_int,
                                                   ctypes.c_ulong]
        self.xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint,
                                                   ctypes.c_int, ctypes.c_ulong]
        self.x.XFlush.argtypes = [ctypes.c_void_p]
        self.x.XCloseDisplay.argtypes = [ctypes.c_void_p]

    def close(self):
        if self.display:
            self.x.XCloseDisplay(self.display)
            self.display = None

    def patch(self, box=PATCH):
        x, y, width, height = box
        image = self.x.XGetImage(self.display, self.root, x, y, width, height,
                                 ctypes.c_ulong(-1).value, 2)  # ZPixmap
        if not image:
            raise RuntimeError("XGetImage failed")
        try:
            return tuple(self.x.XGetPixel(image, px, py) & 0xFFFFFF
                         for py in range(height) for px in range(width))
        finally:
            self.x.XDestroyImage(image)

    def motion(self, x, y):
        if not self.xtst.XTestFakeMotionEvent(self.display, -1, x, y, 0):
            raise RuntimeError("XTestFakeMotionEvent failed")
        self.x.XFlush(self.display)

    def button(self, pressed):
        if not self.xtst.XTestFakeButtonEvent(self.display, 1, pressed, 0):
            raise RuntimeError("XTestFakeButtonEvent failed")
        self.x.XFlush(self.display)


def pixel_is_yellow(pixel, expected=EXPECTED, tolerance=12):
    rgb = ((pixel >> 16) & 255, (pixel >> 8) & 255, pixel & 255)
    return all(abs(actual - wanted) <= tolerance for actual, wanted in zip(rgb, expected))


def patch_has_yellow(pixels):
    return any(pixel_is_yellow(pixel) for pixel in pixels)


def percentile(values, percent):
    """Nearest-rank percentile (also well-defined for short benchmark runs)."""
    ordered = sorted(values)
    if not ordered:
        raise ValueError("cannot percentile an empty sample")
    rank = max(1, int((percent * len(ordered) + 99) // 100))
    return ordered[rank - 1]


def summarize(rows):
    successes = [row["latency_ms"] for row in rows if row["status"] == "success"]
    result = {"trials": len(rows), "successful_trials": len(successes),
              "status_counts": {status: sum(row["status"] == status for row in rows)
                                for status in ("success", "no_change", "timeout")}}
    if successes:
        result.update(median_ms=statistics.median(successes), p95_ms=percentile(successes, 95),
                      min_ms=min(successes), max_ms=max(successes))
    return result


def exact_window(title, pid):
    found = subprocess.run(["xdotool", "search", "--onlyvisible", "--all", "--pid",
                            str(pid), "--name", f"^{title}$"], capture_output=True, text=True)
    return found.stdout.strip().splitlines()[-1] if found.returncode == 0 else None


def open_fresh_selector(x11, process, implementation, log, inject_shortcut=True, timeout=20):
    title = "Captures" if implementation == "tauri" else "Captures GPUI Select target"
    x11.motion(100, 100)
    if inject_shortcut:
        # Shipping Captures intentionally ignores capture shortcuts while its
        # Preferences window has focus, so activate the same external fixture
        # a user would be capturing before pressing PrintScreen.
        reference = subprocess.check_output(["xdotool", "search", "--onlyvisible", "--name",
                                             "^Reference content — Captures comparison$"], text=True).splitlines()[0]
        subprocess.run(["xdotool", "windowactivate", "--sync", reference], check=True)
        # X11 focus delivery and Tauri's focus observer are asynchronous.
        time.sleep(0.5)
        subprocess.run(["xdotool", "keydown", "Print"], check=True)
        time.sleep(0.1)
        subprocess.run(["xdotool", "keyup", "Print"], check=True)
    deadline = time.perf_counter() + timeout
    window = None
    stable_since = None
    previous = None
    while time.perf_counter() < deadline:
        if process.poll() is not None:
            log.seek(0)
            raise RuntimeError(f"application exited opening selector: {log.read().decode(errors='replace')}")
        window = exact_window(title, process.pid)
        if window:
            current = x11.patch()
            if patch_has_yellow(current):
                raise RuntimeError("selector baseline patch is already yellow")
            if current == previous:
                stable_since = stable_since or time.perf_counter()
                if time.perf_counter() - stable_since >= 0.10:
                    return window, current
            else:
                stable_since = None
                previous = current
        time.sleep(0.005)
    raise RuntimeError(f"timed out waiting for stable visible selector {title!r} owned by PID {process.pid}")


def input_trial(x11, timeout=5.0):
    x11.motion(*START)
    x11.button(True)
    # Timestamp the first drag motion 80 ms after pointer-down for both apps.
    # This delay does not prove either application's input queue has drained.
    time.sleep(0.08)
    baseline = x11.patch()
    if patch_has_yellow(baseline):
        x11.button(False)
        raise RuntimeError("pre-input patch is yellow; stale selection would invalidate trial")
    changed = False
    started_ns = time.perf_counter_ns()  # Must precede injection.
    x11.motion(*END)
    deadline_ns = started_ns + int(timeout * 1e9)
    polls = 0
    try:
        while time.perf_counter_ns() < deadline_ns:
            current = x11.patch()
            polls += 1
            changed |= current != baseline
            if patch_has_yellow(current):
                return {"status": "success",
                        "latency_ms": (time.perf_counter_ns() - started_ns) / 1e6,
                        "polls": polls}
        return {"status": "timeout" if changed else "no_change", "latency_ms": None,
                "polls": polls}
    finally:
        x11.button(False)


def polling_overhead(x11, count=50):
    durations = []
    for _ in range(count):
        started = time.perf_counter_ns()
        x11.patch()
        durations.append((time.perf_counter_ns() - started) / 1e6)
    return {"calls": count, "median_ms": statistics.median(durations),
            "min_ms": min(durations), "max_ms": max(durations)}


def run(binary, implementation, samples, warmups, artifacts=None):
    rows = []
    with tempfile.TemporaryDirectory(prefix=f"captures-latency-{implementation}-") as directory:
        env = gpui_benchmark.profile_environment(Path(directory), "dark")
        x11 = X11()
        try:
            overhead = polling_overhead(x11)
            for index in range(warmups + samples):
                # A new process as well as a new selector prevents stale pixels and
                # lets benchmark.stop provide isolated process-tree cleanup.
                command = ([str(binary), "--capture"] if implementation == "gpui"
                           else [str(binary)])
                with tempfile.TemporaryFile() as log:
                    process = subprocess.Popen(command, env=env, stdout=log, stderr=log,
                                               start_new_session=True)
                    try:
                        if implementation == "tauri":
                            # Tauri starts its tray keeper/possibly Preferences first;
                            # title matching below excludes that Preferences window.
                            deadline = time.monotonic() + 20
                            while not exact_window("Captures Preferences", process.pid):
                                if process.poll() is not None or time.monotonic() >= deadline:
                                    raise RuntimeError("Tauri Preferences did not initialize")
                                time.sleep(.05)
                            time.sleep(.5)
                        window, _ = open_fresh_selector(x11, process, implementation, log,
                                                       inject_shortcut=implementation == "tauri")
                        subprocess.run(["xdotool", "windowactivate", "--sync", window], check=True)
                        time.sleep(3)
                        row = input_trial(x11)
                        if artifacts is not None and index == warmups:
                            artifacts.mkdir(parents=True, exist_ok=True)
                            subprocess.run(["import", "-window", "root", str(artifacts / f"{implementation}-latency-selection.png")], check=True)
                        subprocess.run(["xdotool", "key", "Escape"], check=True)
                        time.sleep(0.1)
                    finally:
                        gpui_benchmark.stop(process)
                if index >= warmups:
                    row["sample"] = index - warmups + 1
                    rows.append(row)
                    print(f"sample {row['sample']}/{samples}: {row}", file=sys.stderr, flush=True)
            return rows, overhead
        finally:
            x11.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lab", type=Path, required=True)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--implementation", choices=("tauri", "gpui"), required=True)
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--warmups", type=int, default=2)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path)
    args = parser.parse_args()
    if args.samples < 1 or args.warmups < 0:
        parser.error("--samples must be positive and --warmups nonnegative")
    environment = args.lab.resolve() / "environment.json"
    if not environment.is_file():
        parser.error("--lab must contain native_desktop.py's environment.json")
    binary = Path(args.binary).resolve()
    if not binary.is_file():
        parser.error("--binary must name an existing release binary")
    os.environ.update(json.loads(environment.read_text()))
    rows, overhead = run(binary, args.implementation, args.samples, args.warmups, args.artifacts)
    report = {
        "recorded_at": datetime.now(timezone.utc).isoformat(),
        "implementation": args.implementation,
        "binary": {"path": str(binary), "bytes": binary.stat().st_size,
                   "sha256": gpui_benchmark.sha256(binary)},
        "fixture": {"display": os.environ.get("DISPLAY"), "root_size": [1600, 1000],
                    "lab": str(args.lab.resolve()), "software_gl": True,
                    "patch": {"x": PATCH[0], "y": PATCH[1], "width": PATCH[2], "height": PATCH[3]},
                    "drag": {"start": list(START), "end": list(END), "selection_size": [730, 450]},
                    "expected_border_rgb": list(EXPECTED)},
        "configuration": {"samples": args.samples, "warmups": args.warmups},
        "samples": rows, "summary": summarize(rows), "polling_overhead_estimate": overhead,
        "methodology": ("Fresh real selector per trial; timestamp immediately before XTestFakeMotionEvent+XFlush, "
                        "then poll the compositor's ROOT-window pixels until the mustard selection border appears."),
        "caveats": ("Linux X11 software Mesa 25/Xvfb/Openbox/xcompmgr event-to-observed-compositor-pixel only; "
                    "not physical display latency and must not be extrapolated to hardware or other platforms. "
                    "Runs made while builds or integration work are active are not final benchmark results."),
        "host": {"platform": platform.platform(), "machine": platform.machine()},
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    if any(row["status"] != "success" for row in rows):
        raise SystemExit("Incomplete latency run: failed trials are recorded, not treated as fast frames.")


if __name__ == "__main__":
    main()
