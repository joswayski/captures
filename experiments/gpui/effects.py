#!/usr/bin/python3
"""Exercise actual GTK/GPUI preview effects on disposable files in the X11 lab.

Resource trials have no screen recorder. Video passes are separate, visual
evidence only: recorder frame rate is not application frame rate. The existing
effects differ in duration/rendering, so this compares implementations, not
identical renderer workloads. No production files or capture-session bypass.
"""
import argparse
from datetime import datetime, timezone
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import time


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("gpui_benchmark", HERE / "benchmark.py")
bench = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bench)
SPEC = importlib.util.spec_from_file_location("native_preview", HERE.parent / "native-ui/preview_check.py")
native = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(native)


def run(*args):
    return subprocess.check_output(list(map(str, args)), text=True).strip()


def point(window, x, y, click=False):
    run("xdotool", "mousemove", "--window", window, x, y)
    time.sleep(.3)  # Native input-shape polling must enable the hovered controls.
    if click:
        run("xdotool", "click", 1, "key", "Escape")


def expand(implementation, window):
    if implementation == "native":
        native.pointer_click(native.wait(lambda: native.find(prefix="Expand ", role="push button")))
    else:
        point(window, 170, 620, click=True)


def arm_delete(implementation, window, victim):
    if implementation == "native":
        native.pointer_hover(native.wait(lambda: native.find(f"Preview {victim.name}")))
        native.pointer_click(native.wait(lambda: native.find(f"Delete {victim.name}", role="push button")))
        button = native.wait(lambda: native.find("Delete", role="push button"))
        rect = native.bounds(button)
        return lambda: run("xdotool", "mousemove", rect.x + rect.width // 2,
                           rect.y + rect.height // 2, "click", 1)
    # GPUI expanded cards start at y28 with 184px slots, unlike GTK's bottom anchor.
    run("xdotool", "windowfocus", "--sync", window)
    point(window, 170, 460)
    point(window, 91, 418)
    run("xdotool", "click", 1)
    time.sleep(.3)
    return lambda: run("xdotool", "click", 1)


def sample_cpu(pid, action, seconds=4):
    before = bench.process_tree(pid)
    started = time.perf_counter()
    action()
    snapshots = []
    for index in range(1, 9):
        time.sleep(max(0, started + seconds * index / 8 - time.perf_counter()))
        snapshots.append(bench.resources(pid))
    elapsed = time.perf_counter() - started
    after = bench.process_tree(pid)
    if before.keys() != after.keys():
        raise RuntimeError("process tree changed during effect sampling")
    cpu = (sum(after.values()) - sum(before.values())) / os.sysconf("SC_CLK_TCK")
    return {"window_seconds": elapsed, "cpu_seconds": cpu,
            "cpu_percent_one_core": 100 * cpu / elapsed,
            "max_sampled_pss_mib": max(row["pss_mib"] for row in snapshots)}


def trial(binary, implementation, effect, fixture, artifacts=None):
    with tempfile.TemporaryDirectory(prefix=f"captures-effects-{implementation}-") as directory:
        profile = Path(directory)
        env = bench.profile_environment(profile, "dark")
        config = profile / implementation / "settings.json"
        config.parent.mkdir(exist_ok=True)
        settings = json.loads(config.read_text()) if config.exists() else {}
        settings.update(appearance="dark", mini_preview_placement=1)
        config.write_text(json.dumps(settings))
        images = []
        for index in range(3):
            image = profile / f"fixture-{index}.png"
            shutil.copyfile(fixture, image)
            images.append(image)
        with tempfile.TemporaryFile() as log:
            process = subprocess.Popen([str(binary), "--previews", *map(str, images)],
                                       env=env, stdout=log, stderr=log, start_new_session=True)
            try:
                window = bench.wait_window(bench.TITLES[(implementation, "previews-collapsed")], process.pid)
                time.sleep(3)
                run("xdotool", "mousemove", 1100, 100)
                time.sleep(1)
                baseline = sample_cpu(process.pid, lambda: None) if not artifacts else None
                geometry = dict(line.split("=", 1) for line in
                                run("xdotool", "getwindowgeometry", "--shell", window).splitlines())
                # Observe the compositor, not the window's potentially stale backing pixmap.
                crop = f"80x40+{int(geometry['X']) + 120}+{int(geometry['Y']) + 80}"
                upper_slot = ["import", "-window", "root", "-crop", crop, "RGB:-"]
                before_slot = (subprocess.check_output(upper_slot)
                               if effect == "expand" and implementation == "gpui" else None)
                if effect == "delete":
                    expand(implementation, window)
                    time.sleep(1)
                    action = arm_delete(implementation, window, images[-1])
                else:
                    action = lambda: expand(implementation, window)
                if artifacts:
                    run("import", "-window", "root", artifacts / f"{implementation}-{effect}-before.png")
                    video = subprocess.Popen([
                        "ffmpeg", "-v", "error", "-y", "-f", "x11grab", "-video_size", "420x1000",
                        "-framerate", "60", "-i", os.environ["DISPLAY"] + "+0,0",
                        "-t", "6", "-c:v", "libx264", "-preset", "ultrafast",
                        "-pix_fmt", "yuv420p", "-progress", "pipe:1", "-stats_period", "0.1",
                        str(artifacts / f"{implementation}-{effect}.mp4")], stdout=subprocess.PIPE, text=True)
                    try:
                        for line in video.stdout:
                            if line.startswith("frame=") and int(line.split("=")[1]) > 0:
                                break
                        else:
                            raise RuntimeError("recorder produced no initial frame")
                        time.sleep(1)  # Visible lead-in; short animations need reviewable context.
                        action()
                        video.wait(timeout=15)
                        if video.returncode:
                            raise RuntimeError("recording failed")
                    finally:
                        if video.poll() is None:
                            video.terminate()
                            video.wait(timeout=5)
                    result = {"mode": "visual-only; recorder overhead is not benchmarked"}
                    run("import", "-window", "root", artifacts / f"{implementation}-{effect}-after.png")
                else:
                    result = sample_cpu(process.pid, action)
                    result["collapsed_idle_baseline"] = baseline
                if effect == "delete":
                    removed = [image.name for image in images if not image.exists()]
                    # GPUI decodes concurrently, so the last visible identical fixture
                    # need not be the last CLI argument. Exactly one source must go.
                    if len(removed) != 1:
                        log.seek(0)
                        raise RuntimeError(f"expected one deleted disposable source, got {removed}: " + log.read().decode(errors="replace"))
                    if implementation == "native" and removed != [images[-1].name]:
                        raise RuntimeError("native delete affected a different source")
                    result["deleted_source"] = removed[0]
                elif implementation == "native":
                    native.wait(lambda: native.find("Clear all", role="push button"))
                elif subprocess.check_output(upper_slot) == before_slot:
                    raise RuntimeError("GPUI expansion did not populate the previously empty upper card slot")
                if effect == "expand" and not all(image.exists() for image in images):
                    raise RuntimeError("expansion modified the disposable sources")
                return result
            finally:
                bench.stop(process)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lab", type=Path, required=True)
    parser.add_argument("--native", type=Path, required=True)
    parser.add_argument("--gpui", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--inspect", action="store_true")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--implementation", choices=("native", "gpui"))
    parser.add_argument("--effect", choices=("expand", "delete"))
    args = parser.parse_args()
    if args.runs < 2:
        parser.error("use at least two resource trials")
    os.environ.update(json.loads((args.lab / "environment.json").read_text()))
    args.artifacts.mkdir(parents=True, exist_ok=True)
    binaries = {name: getattr(args, name).resolve() for name in ("native", "gpui")
                if not args.implementation or args.implementation == name}
    effects = [args.effect] if args.effect else ["expand", "delete"]
    results = {effect: {name: [] for name in binaries} for effect in effects}
    for effect in effects:
        for index in range(1 if args.inspect else args.runs):
            for name in list(binaries)[::1 if index % 2 == 0 else -1]:
                print(f"{effect} {name} {index + 1}", flush=True)
                value = trial(binaries[name], name, effect, args.lab / "fixture.png",
                              args.artifacts if args.inspect else None)
                results[effect][name].append(value)
                print(json.dumps(value), flush=True)
    report = {"recorded_at": datetime.now(timezone.utc).isoformat(),
              "scope": __doc__, "results": results,
              "environment": {"platform": platform.platform(), "cpu_count": os.cpu_count(),
                              "display_geometry": run("xdotool", "getdisplaygeometry"),
                              "cpu_clock_tick_hz": os.sysconf("SC_CLK_TCK")},
              "fixture_sha256": bench.sha256(args.lab / "fixture.png"),
              "binaries": {name: {"sha256": bench.sha256(path), "path": str(path)}
                           for name, path in binaries.items()},
              "caveats": "Linux Mesa25 software X11. Native delete 2200ms, GPUI 2900ms; not identical effects. "
                         "CPU is application process-tree cost across a fixed 4s window including settled tail. "
                         "PSS maximum is sampled at 0.5s intervals, not a true peak. No FPS measurement."}
    (args.artifacts / ("inspection.json" if args.inspect else "resources.json")).write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
