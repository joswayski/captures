#!/usr/bin/env python3
"""Reproducible Linux/X11 comparison of release-source Tauri and GPUI builds.

This measures whole process trees for two matched window states. Feature and
interaction parity are assessed separately; equal window sizes alone are not
parity. First mapped-window timing is not readiness or first presented content.
No FPS, energy use, or extrapolation to other operating systems is reported.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import tempfile
import time

# Reuse the established process accounting rather than subtly forking it.
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "native-ui"))
from benchmark import process_tree, resources, stop  # noqa: E402


WINDOW_SIZES = {"preferences": (980, 720), "image": (1280, 760)}
TITLES = {
    # Keep the production matching used by native_comparison.py: the image
    # editor's full title is currently "Captures Screenshot Editor".
    ("tauri", "preferences"): "Preferences",
    ("tauri", "image"): "Screenshot",
    ("gpui", "preferences"): "^Captures GPUI Preferences$",
    ("gpui", "image"): "^Captures GPUI Image$",
}
METRICS = ("rss_mib", "pss_mib", "private_mib", "processes",
           "first_window_mapped_ms", "idle_cpu_percent_one_core")


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def summarize(rows):
    """Return explicit median and range data, rejecting incomplete samples."""
    if not rows:
        raise ValueError("cannot summarize zero samples")
    if any(set(row) != set(METRICS) for row in rows):
        raise ValueError("resource samples have missing or unexpected metrics")
    return {
        key: {
            "median": statistics.median(row[key] for row in rows),
            "min": min(row[key] for row in rows),
            "max": max(row[key] for row in rows),
        }
        for key in METRICS
    }


def png_dimensions(path):
    header = path.read_bytes()[:24]
    if len(header) != 24 or header[:8] != b"\x89PNG\r\n\x1a\n" or header[12:16] != b"IHDR":
        raise RuntimeError(f"Screenshot is not a valid PNG: {path}")
    return tuple(int.from_bytes(header[offset:offset + 4], "big") for offset in (16, 20))


def command(binary, implementation, state, fixture, appearance):
    if implementation == "tauri":
        return [str(binary)] if state == "preferences" else [str(binary), str(fixture)]
    args = [str(binary), "--appearance", appearance]
    return args + (["--preferences"] if state == "preferences" else ["--open", str(fixture)])


def profile_environment(profile, appearance):
    config = profile / "config/captures"
    config.mkdir(parents=True)
    runtime = profile / "runtime"
    runtime.mkdir(mode=0o700)
    (config / "settings.json").write_text(json.dumps({
        "settings_schema_version": 5,
        "onboarding_completed": True,
        "appearance": appearance,
        "output_directory": str(profile / "captures"),
        "launch_at_login": False,
        "region_shortcut": "Super+Shift+S",
        "window_shortcut": "Alt+PrintScreen",
        "display_shortcut": "Shift+PrintScreen",
    }))
    return dict(os.environ, HOME=str(profile), XDG_CONFIG_HOME=str(profile / "config"),
                XDG_DATA_HOME=str(profile / "data"), XDG_CACHE_HOME=str(profile / "cache"),
                CAPTURES_GPUI_DATA=str(profile / "gpui"), XDG_RUNTIME_DIR=str(runtime),
                LIBGL_ALWAYS_SOFTWARE="1",
                HTTP_PROXY="http://127.0.0.1:9", HTTPS_PROXY="http://127.0.0.1:9",
                ALL_PROXY="http://127.0.0.1:9", NO_PROXY="localhost,127.0.0.1")


def find_window(title, pid):
    found = subprocess.run(["xdotool", "search", "--onlyvisible", "--all", "--pid", str(pid), "--name", title],
                           capture_output=True, text=True)
    return found.stdout.strip().splitlines()[-1] if found.returncode == 0 else None


def trial(binary, implementation, state, fixture, settle, idle, appearance,
          screenshot=None):
    with tempfile.TemporaryDirectory(prefix=f"captures-{implementation}-{state}-") as directory:
        profile = Path(directory)
        env = profile_environment(profile, appearance)
        with tempfile.TemporaryFile() as log:
            started = time.perf_counter()
            process = subprocess.Popen(command(binary, implementation, state, fixture, appearance),
                                       env=env, stdout=log, stderr=log, start_new_session=True)
            try:
                window = None
                while window is None:
                    if process.poll() is not None or time.perf_counter() - started > 30:
                        log.seek(0)
                        raise RuntimeError(f"{implementation} {state} failed before mapping: "
                                           f"{log.read().decode(errors='replace')}")
                    window = find_window(TITLES[(implementation, state)], process.pid)
                    if window is None:
                        time.sleep(0.005)
                mapped_ms = (time.perf_counter() - started) * 1000
                size = WINDOW_SIZES[state]
                subprocess.run(["xdotool", "windowsize", "--sync", window, *map(str, size)], check=True)
                subprocess.run(["xdotool", "set_window", "--overrideredirect", "1", window], check=True)
                subprocess.run(["xdotool", "windowmove", window, "0", "0"], check=True)
                time.sleep(settle)
                memory = resources(process.pid)
                if set(memory) != {"rss_mib", "pss_mib", "private_mib", "processes"}:
                    raise RuntimeError(f"Incomplete memory sample: {memory}")
                before = process_tree(process.pid)
                if len(before) != memory["processes"]:
                    raise RuntimeError("Process tree changed while collecting memory")
                idle_started = time.perf_counter()
                time.sleep(idle)
                elapsed = time.perf_counter() - idle_started
                after = process_tree(process.pid)
                if before.keys() != after.keys() or process.poll() is not None:
                    raise RuntimeError("Process exited or process tree changed during idle sample")
                if screenshot is not None:
                    subprocess.run(["import", "-window", window, str(screenshot)], check=True)
                    actual = png_dimensions(screenshot)
                    if actual != size:
                        raise RuntimeError(f"{implementation} {state} screenshot is {actual}, expected {size}")
                ticks = sum(after.values()) - sum(before.values())
                return dict(memory, first_window_mapped_ms=mapped_ms,
                            idle_cpu_percent_one_core=(100 * ticks / os.sysconf("SC_CLK_TCK") / elapsed))
            finally:
                stop(process)


def absolute_file(parser, value, label):
    path = Path(value)
    if not path.is_absolute():
        parser.error(f"{label} must be an absolute path")
    if not path.is_file():
        parser.error(f"{label} does not exist or is not a file: {path}")
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lab", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--tauri", required=True, help="absolute release-source Tauri binary")
    parser.add_argument("--gpui", required=True, help="absolute release-source GPUI binary")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--settle", type=float, default=5)
    parser.add_argument("--idle", type=float, default=2)
    parser.add_argument("--inspect", action="store_true", help="visual-only warmup; do not benchmark")
    parser.add_argument("--appearance", choices=("light", "dark"), default="dark",
                        help="inspection appearance; benchmarks always use dark")
    args = parser.parse_args()
    if args.runs < 2 or args.settle < 0 or args.idle <= 0:
        parser.error("use at least two runs, nonnegative settle, and positive idle")
    args.lab = args.lab.resolve()
    environment_file = args.lab / "environment.json"
    fixture = args.lab / "fixture.png"
    if not environment_file.is_file() or not fixture.is_file():
        parser.error("--lab must contain native_desktop.py's environment.json and fixture.png")
    os.environ.update(json.loads(environment_file.read_text()))
    binaries = {"tauri": absolute_file(parser, args.tauri, "--tauri"),
                "gpui": absolute_file(parser, args.gpui, "--gpui")}
    args.artifacts.mkdir(parents=True, exist_ok=True)
    appearance = args.appearance if args.inspect else "dark"
    samples = {state: {name: [] for name in binaries} for state in WINDOW_SIZES}

    for state, by_name in samples.items():
        for name, binary in binaries.items():
            screenshot = args.artifacts / f"{name}-{state}-{appearance}.png"
            print(f"Visual warmup: {name} {state}", file=sys.stderr, flush=True)
            trial(binary, name, state, fixture, args.settle, args.idle, appearance, screenshot)
        if args.inspect:
            continue
        for index in range(args.runs):
            order = ("tauri", "gpui") if index % 2 == 0 else ("gpui", "tauri")
            for name in order:
                result = trial(binaries[name], name, state, fixture, args.settle, args.idle, appearance)
                by_name[name].append(result)
                print(f"{state} {index + 1}/{args.runs} {name}: {result}",
                      file=sys.stderr, flush=True)
    if args.inspect:
        print(json.dumps({"mode": "visual-only warmup", "appearance": appearance,
                          "screenshots": [str(p) for p in sorted(args.artifacts.glob(f"*-{appearance}.png"))]}, indent=2))
        return

    root = Path(__file__).resolve().parents[2]
    report = {
        "recorded_at": datetime.now(timezone.utc).isoformat(),
        "scope": ("Linux X11 release-source whole-process-tree comparison of Tauri and GPUI. "
                  "Matched client size, appearance and image input; feature/interaction parity is assessed separately."),
        "caveats": ("First mapped window is not readiness. No FPS or energy measurement, and no "
                    "extrapolation to macOS, Windows, or other Linux environments."),
        "environment": {
            "lab": "native_desktop.py disposable Xvfb/Openbox/xcompmgr/DBus fixture",
            "platform": platform.platform(), "machine": platform.machine(),
            "cpu_count": os.cpu_count(), "display": os.environ.get("DISPLAY"),
            "software_gl": True,
            "vulkan": subprocess.check_output(["vulkaninfo", "--summary"], text=True, stderr=subprocess.DEVNULL),
            "mesa_packages": subprocess.check_output(["dpkg-query", "-W", "mesa-vulkan-drivers", "libgl1-mesa-dri", "libvulkan1"], text=True),
            "display_geometry": subprocess.check_output(["xdotool", "getdisplaygeometry"], text=True).strip(),
        },
        "configuration": {"runs": args.runs, "settle_seconds": args.settle,
                          "idle_seconds": args.idle, "appearance": appearance,
                          "client_viewports": WINDOW_SIZES},
        "source_base": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
        "binaries": {name: {"path": str(path), "bytes": path.stat().st_size,
                             "sha256": sha256(path)} for name, path in binaries.items()},
        "input": {"path": str(fixture), "bytes": fixture.stat().st_size, "sha256": sha256(fixture)},
        "samples": samples,
        "summary": {state: {name: summarize(rows) for name, rows in by_name.items()}
                    for state, by_name in samples.items()},
    }
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
