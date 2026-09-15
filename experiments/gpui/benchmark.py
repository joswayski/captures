#!/usr/bin/env python3
"""Reproducible Linux/X11 comparison of release Tauri, GPUI and full GTK builds.

This measures whole process trees for matched, production-reachable states. Feature and
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


WINDOW_SIZES = {
    "preferences": (980, 720),
    "image": (1280, 760),
    "video-editor": (1280, 760),
    "screenshot-selection": (1600, 1000),
    "recording-selection": (1600, 1000),
}
TITLES = {
    # Keep the production matching used by native_comparison.py: the image
    # editor's full title is currently "Captures Screenshot Editor".
    ("tauri", "preferences"): "Preferences",
    ("tauri", "image"): "Screenshot",
    ("tauri", "video-editor"): "^Captures Editor$",
    ("tauri", "screenshot-selection"): "^Captures$",
    ("tauri", "recording-selection"): "^Captures$",
    ("tauri", "previews-collapsed"): "^Captures$",
    ("tauri", "previews-expanded"): "^Captures$",
    ("tauri", "history"): "^Capture History$",
    ("tauri", "recording-hud"): "^Captures Recording Controls$",
    ("gpui", "preferences"): "^Captures GPUI Preferences$",
    ("gpui", "image"): "^Captures GPUI Image$",
    ("gpui", "video-editor"): "^Captures GPUI Recording$",
    ("gpui", "screenshot-selection"): "^Captures GPUI Select target$",
    ("gpui", "recording-selection"): "^Captures GPUI Select target$",
    ("gpui", "previews-collapsed"): "^Captures GPUI Previews$",
    ("gpui", "previews-expanded"): "^Captures GPUI Previews$",
    ("gpui", "history"): "^Captures GPUI Capture History$",
    ("gpui", "recording-hud"): "^Captures GPUI Recording controls$",
    ("native", "preferences"): "^Captures Preferences$",
    ("native", "image"): "^Captures — Image editor$",
    ("native", "video-editor"): "^Edit recording — Captures$",
    ("native", "screenshot-selection"): "^Captures — Select target$",
    ("native", "recording-selection"): "^Captures — Select target$",
    ("native", "previews-collapsed"): "^Captures — Mini previews$",
    ("native", "previews-expanded"): "^Captures — Mini previews$",
    ("native", "recording-hud"): "^Captures recording controls$",
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
        return ([str(binary), str(fixture)] if state in ("image", "video-editor")
                else [str(binary)])
    if implementation == "native":
        return ([str(binary), "--open", str(fixture)] if state in ("image", "video-editor")
                else [str(binary), "--preferences"])
    args = [str(binary), "--appearance", appearance]
    if state in ("image", "video-editor"):
        return args + ["--open", str(fixture)]
    # Selection is entered through the same real global shortcut as production
    # Tauri below, rather than GPUI's convenient direct CLI switches.
    return args + ["--preferences"]


def activate_reference():
    reference = subprocess.check_output([
        "xdotool", "search", "--onlyvisible", "--name",
        "^Reference content — Captures comparison$"], text=True).splitlines()[0]
    subprocess.run(["xdotool", "windowactivate", "--sync", reference], check=True)
    subprocess.run(["xdotool", "windowfocus", "--sync", reference], check=True)
    time.sleep(.5)


def wait_window(title, pid, timeout=30, minimum_width=0, maximum_width=None):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        window = find_window(title, pid, minimum_width, maximum_width)
        if window:
            return window
        time.sleep(.01)
    raise RuntimeError(f"window did not appear: {title}")


def complete_region_selection(process, implementation, shortcut):
    activate_reference()
    subprocess.run(["xdotool", "keydown", shortcut, "sleep", ".1", "keyup", shortcut], check=True)
    selector = wait_window(TITLES[(implementation, "recording-selection" if shortcut == "ctrl+shift+alt+r" else "screenshot-selection")], process.pid, minimum_width=1000)
    time.sleep(1)
    subprocess.run(["xdotool", "mousemove", "--window", selector, "320", "180"], check=True)
    subprocess.run(["xdotool", "mousedown", "1"], check=True)
    time.sleep(.1)
    subprocess.run(["xdotool", "mousemove", "--sync", "1050", "630"], check=True)
    time.sleep(.2)
    subprocess.run(["xdotool", "mouseup", "1"], check=True)
    # Let the asynchronous frontend commit the completed drag before confirming.
    time.sleep(.5)
    if shortcut == "Print" and implementation != "native":
        # Tauri and GPUI put Capture here in the 1600×1000 lab.
        subprocess.run(["xdotool", "mousemove", "1160", "915", "click", "1"], check=True)
    else:
        subprocess.run(["xdotool", "windowfocus", "--sync", selector], check=True)
        subprocess.run(["xdotool", "keydown", "Return", "sleep", ".1", "keyup", "Return"], check=True)
    deadline = time.monotonic() + 30
    while find_window(TITLES[(implementation, "screenshot-selection")], process.pid, minimum_width=1000):
        if time.monotonic() > deadline:
            raise RuntimeError("selection did not complete")
        time.sleep(.1)


def prepare_state(process, implementation, state, profile):
    if state in ("previews-collapsed", "previews-expanded"):
        for _ in range(3):
            complete_region_selection(process, implementation, "Print")
            wait_window(TITLES[(implementation, state)], process.pid, maximum_width=400)
            time.sleep(2)
        if implementation == "tauri":
            count = len(list((profile / "data/captures/capture-history").glob("*/metadata.json")))
        else:
            count = len(json.loads((profile / implementation / "history.json").read_text()))
        if count != 3:
            raise RuntimeError(f"expected three completed captures, got {count}")
        window = wait_window(TITLES[(implementation, state)], process.pid, maximum_width=400)
        geometry = subprocess.check_output(["xdotool", "getwindowgeometry", "--shell", window], text=True)
        height = int(next(line[7:] for line in geometry.splitlines() if line.startswith("HEIGHT=")))
        # GPUI/GTK start collapsed in a fixed 340×760 transparent frame. Tauri
        # starts expanded; neither frame's height reliably identifies collapse.
        if implementation in ("gpui", "native") and state == "previews-expanded":
            subprocess.run(["xdotool", "mousemove", "--window", window, "170", "620", "click", "1", "key", "Escape"], check=True)
            time.sleep(1)
        elif implementation == "tauri" and state == "previews-collapsed":
            # Tauri's native cursor polling must re-enable hit testing before
            # the click; moving and clicking immediately can pass through.
            subprocess.run(["xdotool", "mousemove", "--window", window, "72", str(height - 30), "sleep", "1", "click", "1"], check=True)
            time.sleep(3)
        subprocess.run(["xdotool", "mousemove", "1100", "100"], check=True)
    elif state == "recording-hud":
        complete_region_selection(process, implementation, "ctrl+shift+alt+r")
        # Mapping the GPUI HUD can precede its countdown. Wait for the shared
        # durable recording state before applying the common settling interval.
        root = profile / {"gpui": "gpui/recording-drafts",
                          "native": "captures/.captures-recording-drafts",
                          "tauri": "data/captures/recording-recovery"}[implementation]
        deadline = time.monotonic() + 30
        while not any(json.loads(path.read_text()).get("state") == "recording"
                      for path in root.glob("*/manifest.json")):
            if time.monotonic() > deadline:
                raise RuntimeError("recording did not start")
            time.sleep(.1)


def enter_state(state):
    shortcut = {"screenshot-selection": "Print",
                "recording-selection": "ctrl+shift+alt+r"}.get(state)
    if shortcut:
        subprocess.run(["xdotool", "keydown", shortcut, "sleep", ".1", "keyup", shortcut], check=True)


def profile_environment(profile, appearance):
    config = profile / "config/captures"
    config.mkdir(parents=True)
    runtime = profile / "runtime"
    runtime.mkdir(mode=0o700)
    native = profile / "native"
    native.mkdir()
    (native / "settings.json").write_text(json.dumps({
        "appearance": appearance,
        "output_directory": str(profile / "captures"),
        "launch_at_login": False,
    }))
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
                CAPTURES_NATIVE_DATA=str(native),
                LIBGL_ALWAYS_SOFTWARE="1",
                HTTP_PROXY="http://127.0.0.1:9", HTTPS_PROXY="http://127.0.0.1:9",
                ALL_PROXY="http://127.0.0.1:9", NO_PROXY="localhost,127.0.0.1")


def find_window(title, pid, minimum_width=0, maximum_width=None):
    found = subprocess.run(["xdotool", "search", "--onlyvisible", "--all", "--pid", str(pid), "--name", title],
                           capture_output=True, text=True)
    if found.returncode != 0:
        return None
    for window in reversed(found.stdout.strip().splitlines()):
        if minimum_width or maximum_width is not None:
            geometry = subprocess.check_output(["xdotool", "getwindowgeometry", "--shell", window], text=True)
            width = int(next(line[6:] for line in geometry.splitlines() if line.startswith("WIDTH=")))
            if width < minimum_width or (maximum_width is not None and width > maximum_width):
                continue
        return window
    return None


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
                if state.endswith("-selection"):
                    # Tauri deliberately ignores capture shortcuts while its
                    # Preferences has focus. Activate the same external fixture
                    # for every app before timing the real shortcut.
                    deadline = time.monotonic() + 30
                    while not find_window(TITLES[(implementation, "preferences")], process.pid):
                        if process.poll() is not None or time.monotonic() > deadline:
                            raise RuntimeError("Preferences failed before shortcut setup")
                        time.sleep(.01)
                    activate_reference()
                    started = time.perf_counter()
                    enter_state(state)
                elif state in ("previews-collapsed", "previews-expanded", "recording-hud"):
                    wait_window(TITLES[(implementation, "preferences")], process.pid)
                    started = time.perf_counter()
                    prepare_state(process, implementation, state, profile)
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
                size = WINDOW_SIZES.get(state)
                if size is not None:
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
                    if size is not None and actual != size:
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
    parser.add_argument("--tauri", help="absolute release-source Tauri binary")
    parser.add_argument("--gpui", help="absolute release-source GPUI binary")
    parser.add_argument("--native", help="absolute full captures-linux-native GTK binary (not minimal probe)")
    parser.add_argument("--video-fixture", help="absolute MP4/WebM for matched production open-file editors")
    parser.add_argument("--states", nargs="+", choices=list(dict.fromkeys(
        state for _, state in TITLES if state != "history")),
        help="limit a rerun to named states; by default run every available state")
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
    image_fixture = args.lab / "fixture.png"
    if not environment_file.is_file() or not image_fixture.is_file():
        parser.error("--lab must contain native_desktop.py's environment.json and fixture.png")
    video_fixture = (absolute_file(parser, args.video_fixture, "--video-fixture")
                     if args.video_fixture else None)
    os.environ.update(json.loads(environment_file.read_text()))
    binaries = {name: absolute_file(parser, value, f"--{name}")
                for name in ("tauri", "gpui", "native")
                if (value := getattr(args, name))}
    if not binaries:
        parser.error("provide at least one of --tauri, --gpui, --native")
    args.artifacts.mkdir(parents=True, exist_ok=True)
    appearance = args.appearance if args.inspect else "dark"
    states = [state for state in WINDOW_SIZES if state != "video-editor" or video_fixture]
    states += ["previews-collapsed", "previews-expanded", "recording-hud"]
    if args.states:
        if "video-editor" in args.states and video_fixture is None:
            parser.error("video-editor requires --video-fixture")
        states = [state for state in states if state in args.states]
    samples = {state: {name: [] for name in binaries} for state in states}

    for state, by_name in samples.items():
        fixture = video_fixture if state == "video-editor" else image_fixture
        for name, binary in binaries.items():
            screenshot = args.artifacts / f"{name}-{state}-{appearance}.png"
            print(f"Visual warmup: {name} {state}", file=sys.stderr, flush=True)
            trial(binary, name, state, fixture, args.settle, args.idle, appearance, screenshot)
        if args.inspect:
            continue
        for index in range(args.runs):
            names = list(binaries)
            offset = index % len(names)
            order = names[offset:] + names[:offset]
            for name in order:
                result = trial(binaries[name], name, state, fixture, args.settle, args.idle, appearance,
                               args.artifacts / f"{name}-{state}-trial-{index + 1}.png")
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
        "scope": ("Linux X11 release-source whole-process-tree comparison of the supplied Tauri, GPUI and/or full GTK implementations. "
                  "Preferences and media editors use their production entry paths; screenshot and recording "
                  "selectors use each app's real configured global shortcut. Matched client size and appearance; "
                  "feature/interaction parity is assessed separately."),
        "caveats": ("First mapped window is not readiness. No FPS or energy measurement, and no "
                    "extrapolation to macOS, Windows, or other Linux environments. Overlay resource samples "
                    "retain the Preferences used to enter them. Native means the full GTK/Cairo experiment, "
                    "not a minimal native window or AppKit/Win32. Native captures publish files while the "
                    "other implementations keep private copies; these are settled-state costs, not capture throughput. "
                    "Preview/HUD timing includes "
                    "multi-step fixture setup and deliberate waits; do not compare it as startup latency. "
                    "The idle_cpu field measures active capture CPU for recording-hud."),
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
                          "implementations": list(binaries),
                          "client_viewports": {state: WINDOW_SIZES.get(state, "native") for state in states}},
        "source_base": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
        "binaries": {name: {"path": str(path), "bytes": path.stat().st_size,
                             "sha256": sha256(path)} for name, path in binaries.items()},
        "inputs": {"image": {"path": str(image_fixture), "bytes": image_fixture.stat().st_size,
                               "sha256": sha256(image_fixture)},
                   "video": ({"path": str(video_fixture), "bytes": video_fixture.stat().st_size,
                              "sha256": sha256(video_fixture)} if video_fixture else None)},
        "coverage": {
            "matched": states,
            "unmatched": {
                "history": "Symmetric production tray activation was not verified in this X11 fixture; no history benchmark is claimed.",
                "export-timings": "Exports require editor IPC/UI interaction and completion signaling; the apps do not expose a symmetric production CLI workflow.",
                **({} if video_fixture else {"video-editor": "Not run: supply --video-fixture only after verifying actual playback in every supplied app."}),
            },
        },
        "samples": samples,
        "summary": {state: {name: summarize(rows) for name, rows in by_name.items()}
                    for state, by_name in samples.items()},
    }
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
