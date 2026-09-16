#!/usr/bin/env python3
"""Matched on-screen macOS component trials. Never launches the installed Captures app."""
import argparse
import hashlib
import io
import json
import math
import platform
import statistics
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

from PIL import Image, ImageChops, ImageCms, ImageDraw, ImageFilter, ImageStat

LAB = Path(__file__).resolve().parent
REPO = LAB.parent.parent
SCENARIOS = ("dust-bottom-left", "dust-top-right", "settle-bottom", "settle-top")
BINARIES = {"tauri": LAB / "tauri/target/release/captures-parity-tauri",
            "appkit": LAB / ".build/captures-parity-native"}
BG = (32, 36, 43)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def sha(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def manifest():
    files = [p for p in LAB.rglob("*") if p.is_file()
             and not any(x in p.relative_to(LAB).parts for x in (".build", "target", "gen", "results", "__pycache__"))]
    files += [REPO / "apps/desktop/ui/src/lib/thumbnailExit.ts",
              REPO / "apps/desktop/ui/src/styles/mini-preview.css",
              LAB / ".build/fixture.png", LAB / ".build/parity-observer",
              LAB / ".build/parity-resources", *BINARIES.values()]
    files += list((LAB / ".build/public").glob("*.json"))
    files += [p for p in (LAB / ".build/web").rglob("*") if p.is_file()]
    return {str(p.relative_to(REPO)): sha(p) for p in sorted(files)}


def cpu_seconds(value):
    days, rest = value.split("-", 1) if "-" in value else ("0", value)
    result = 0.0
    for part in rest.split(":"):
        result = result * 60 + float(part)
    return int(days) * 86400 + result


def snapshot():
    output = subprocess.check_output(["ps", "-axo", "pid=,ppid=,rss=,time=,comm="], text=True)
    rows = {}
    for line in output.splitlines():
        pid, parent, rss, cpu, command = line.strip().split(None, 4)
        rows[int(pid)] = {"pid": int(pid), "parent": int(parent), "rssKiB": int(rss),
                          "cpuSeconds": cpu_seconds(cpu), "name": Path(command).name}
    return rows


def descendants(rows, pid):
    selected = {pid} if pid in rows else set()
    while True:
        expanded = selected | {key for key, row in rows.items() if row["parent"] in selected}
        if expanded == selected:
            return [rows[key] for key in sorted(selected)]
        selected = expanded


def order(labels, trial):
    # Alternates two candidates; rotates three when a GPUI adapter is supplied.
    offset = trial % len(labels)
    return labels[offset:] + labels[:offset]


def percentile(values, quantile):
    values = sorted(values)
    return values[max(0, math.ceil(len(values) * quantile) - 1)]


def srgb(path):
    with Image.open(path) as source:
        if source.info.get("icc_profile"):
            return ImageCms.profileToProfile(source.convert("RGB"),
                ImageCms.ImageCmsProfile(io.BytesIO(source.info["icc_profile"])),
                ImageCms.createProfile("sRGB"), outputMode="RGB")
        return source.convert("RGB")


def max_channel(image):
    r, g, b = image.split()
    return ImageChops.lighter(ImageChops.lighter(r, g), b)


def compare(reference, candidate, destination):
    a, b = srgb(reference), srgb(candidate)
    if a.size != (1280, 1440) or b.size != a.size:
        raise ValueError(f"Wrong physical content size: {a.size} / {b.size}; expected 1280x1440")
    background = Image.new("RGB", a.size, BG)
    active_a = max_channel(ImageChops.difference(a, background)).point(lambda x: 255 if x > 12 else 0)
    active_b = max_channel(ImageChops.difference(b, background)).point(lambda x: 255 if x > 12 else 0)
    # Never let a shared blank window pass, or dilute the error with empty stage.
    if active_a.histogram()[255] < 40000 or active_b.histogram()[255] < 40000:
        raise ValueError("Missing visible fixture content; blank/error windows cannot pass")
    mask = ImageChops.lighter(active_a, active_b).filter(ImageFilter.MaxFilter(5))
    difference = ImageChops.difference(a, b)
    mean = statistics.mean(ImageStat.Stat(difference, mask).mean)
    bad = max_channel(difference).point(lambda x: 255 if x > 16 else 0)
    bad_fraction = ImageChops.multiply(bad, mask).histogram()[255] / mask.histogram()[255]
    heat = difference.point(lambda x: min(255, x * 6))
    heat.save(destination.with_suffix(".diff.png"))
    contact = Image.new("RGB", (1920, 760), BG)
    draw = ImageDraw.Draw(contact)
    for index, (label, image) in enumerate((("Tauri reference", a), ("Candidate", b), ("Difference x6", heat))):
        draw.text((index * 640 + 12, 12), label, fill="white")
        contact.paste(image.resize((640, 720)), (index * 640, 40))
    contact.save(destination.with_suffix(".comparison.png"))
    return {"meanAbsoluteRGBError": mean, "fractionOver16": bad_fraction,
            "pixelGatePassed": mean <= 2 and bad_fraction <= 0.01,
            "thresholds": {"meanAbsoluteRGBError": 2, "fractionOver16": 0.01},
            "note": "Pixel gate is diagnostic, not a replacement for reviewing motion and imagery."}


def wait_file(path, process, timeout, sample=None):
    deadline = time.monotonic() + timeout
    while not path.exists():
        error = Path(str(path).replace(".ready.json", "") + ".error.json")
        if error.exists():
            raise RuntimeError(error.read_text())
        if process.poll() is not None:
            raise RuntimeError(f"Renderer exited {process.returncode}; inspect its log")
        if time.monotonic() > deadline:
            raise TimeoutError(f"Timed out waiting for {path.name}")
        if sample:
            sample()
        time.sleep(0.25)
    return json.loads(path.read_text())


def trial(executable, config, folder):
    folder.mkdir()
    config_path, output = folder / "config.json", folder / "renderer.json"
    write_json(config_path, config)
    rows_before = snapshot()
    samples = []
    with (folder / "process.log").open("w") as log:
        process = subprocess.Popen([str(executable), str(config_path), str(output)], stdout=log, stderr=log)
        ready = None
        try:
            ready = wait_file(Path(str(output) + ".ready.json"), process, 90)
            for key in ("schema", "scenario", "mode", "scale", "checkpointMs"):
                if ready[key] != config[key]:
                    raise ValueError(f"Ready marker mismatch: {key}")
            if ready["pid"] != process.pid or ready["window_id"] <= 0:
                raise ValueError("Ready marker does not identify the launched window")

            def capture(name):
                subprocess.run(["screencapture", "-x", "-o", "-l", str(ready["window_id"]), str(folder / name)], check=True, timeout=15)
                with Image.open(folder / name) as image:
                    if image.size != (config["width"] * config["scale"], config["height"] * config["scale"]):
                        raise ValueError("Window capture is not the matched physical viewport")

            if config["mode"] == "checkpoint":
                time.sleep(.1)  # Application marker is not proof of presentation.
                capture("window.png")
                return {"ready": ready, "screenshot": str((folder / "window.png").relative_to(folder.parent))}

            def sample():
                rows = snapshot()
                tree = descendants(rows, process.pid)
                if not tree:
                    raise RuntimeError("Renderer exited during process sampling")
                # WebKit's XPC children may be reparented to launchd. Preserve
                # candidates separately, never silently credit them to this app.
                tree_ids = {p["pid"] for p in tree}
                unrelated = [p for pid, p in rows.items() if pid not in tree_ids and "WebKit" in p["name"]]
                samples.append({"monotonicSeconds": time.monotonic(), "tree": tree,
                    "treeRSSMiB": sum(p["rssKiB"] for p in tree) / 1024,
                    "unattributedWebKit": [{**p, "newSinceLaunch": p["pid"] not in rows_before} for p in unrelated]})

            sample()
            result = wait_file(output, process, config["durationMs"] / 1000 + 90, sample)
            sample()
            if not result.get("complete") or result["elapsedMs"] < config["durationMs"] or len(result["callbackIntervalsMs"]) < 2:
                raise ValueError("Incomplete live workload")
            intervals = result["callbackIntervalsMs"]
            if not all(math.isfinite(x) and x > 0 for x in intervals):
                raise ValueError("Invalid animation callback intervals")
            cpu = []
            for first, last in zip(samples, samples[1:]):
                before = {p["pid"]: p for p in first["tree"]}
                after = {p["pid"]: p for p in last["tree"]}
                if before.keys() == after.keys():
                    delta = sum(after[pid]["cpuSeconds"] - before[pid]["cpuSeconds"] for pid in after)
                    if delta >= 0:
                        cpu.append(100 * delta / (last["monotonicSeconds"] - first["monotonicSeconds"]))
            result["processSamples"] = samples
            result["summary"] = {"callbackP50Ms": statistics.median(intervals), "callbackP95Ms": percentile(intervals, .95),
                "callbackMaxMs": max(intervals), "callbacksOver25Ms": sum(x > 25 for x in intervals),
                "setupP50Ms": statistics.median(result["setupMs"]),
                "treeRSSMedianMiB": statistics.median(s["treeRSSMiB"] for s in samples),
                "treeCPUMedianPercentOneCore": statistics.median(cpu) if cpu else None}
            write_json(folder / "measured.json", result)
            return result["summary"]
        except BaseException:
            if ready:
                try:
                    subprocess.run(["screencapture", "-x", "-o", "-l", str(ready["window_id"]), str(folder / "failure.png")], timeout=15, check=False)
                except (OSError, subprocess.TimeoutExpired):
                    pass  # Preserve the original trial failure.
            raise
        finally:
            # Stop only the process this trial created, never Captures or other apps.
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def main():
    if sys.argv[1:] == ["manifest"]:
        write_json(LAB / ".build/manifest.json", manifest())
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=LAB / "results" / datetime.now().strftime("%Y%m%d-%H%M%S"))
    parser.add_argument("--trials", type=int, default=3)
    parser.add_argument("--duration", type=float, help="Seconds per trial (default 9.6; profiling 12.8)")
    parser.add_argument("--profile", action="store_true", help="Separate full-coalition resource and WindowServer frame passes")
    parser.add_argument("--capture-hz", type=int, default=120, help="Requested observer ceiling, not a measured FPS value")
    parser.add_argument("--visual-only", action="store_true")
    parser.add_argument("--diagnostic-performance", action="store_true", help="Collect timings despite pixel-gate failures; mark them non-comparable")
    parser.add_argument("--gpui", type=Path, help="Optional optimized GPUI adapter implementing this exact contract")
    args = parser.parse_args()
    if args.duration is None:
        args.duration = 12.8 if args.profile else 9.6
    if platform.system() != "Darwin":
        parser.error("Run on the Mac being compared, with Terminal Screen Recording permission")
    if args.trials < 2 or not math.isfinite(args.duration) or args.duration < 6.4:
        parser.error("Use at least two trials and 6.4 seconds")
    if args.profile and (args.visual_only or args.duration > 60 or not 60 <= args.capture_hz <= 240):
        parser.error("Profiling requires live trials <=60s and a capture ceiling between60 and240Hz")
    expected = json.loads((LAB / ".build/manifest.json").read_text())
    if expected != manifest():
        parser.error("Source, fixture, or binary changed since build. Run bash experiments/macos-parity/build.sh")
    binaries = dict(BINARIES)
    if args.gpui:
        binaries["gpui"] = args.gpui.resolve(strict=True)
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"schema": 1, "createdUTC": datetime.now(timezone.utc).isoformat(), "environment": {
        "os": platform.platform(), "machine": platform.machine(),
        "hardware": subprocess.check_output(["sysctl", "-n", "hw.model"], text=True).strip(),
        "displays": json.loads(subprocess.check_output(["system_profiler", "SPDisplaysDataType", "-json"], text=True)),
        "power": subprocess.check_output(["pmset", "-g", "batt"], text=True).strip()},
        "buildManifest": expected, "executables": {label: sha(path) for label, path in binaries.items()},
        "scope": "Matched media-effect components, not full apps. Callback cadence is not GPU presentation FPS. RSS double-counts shared pages; root+descendants omits unattributed WebKit XPC/WindowServer memory and CPU.",
        "completeAppMemory": False, "visual": [], "trials": [], "failures": []}
    report_path = args.output / "report.json"
    if args.profile:
        report["schema"] = 2
        report["scope"] = "Matched component windows, not full apps. Separate WindowServer frame and isolated app-coalition resource passes. No physical-panel scanout or unassigned WindowServer/kernel/GPU accounting."
    try:
        for scenario in SCENARIOS:
            config = json.loads((LAB / f".build/public/{scenario}.json").read_text())
            times = [0, 204, 420, 900, 1300, 1800, 2090, 2380] if scenario.startswith("dust") else [0, 145, 290, 580]
            for timestamp in times:
                config["checkpointMs"] = timestamp
                prefix = f"{scenario}-{timestamp}"
                for label, executable in binaries.items():
                    print(f"Visual {prefix}: {label}", flush=True)
                    trial(executable, config, args.output / f"{prefix}-{label}")
                for label in list(binaries)[1:]:
                    score = compare(args.output / f"{prefix}-tauri/window.png", args.output / f"{prefix}-{label}/window.png", args.output / f"{prefix}-{label}")
                    report["visual"].append({"scenario": scenario, "checkpointMs": timestamp, "candidate": label, **score})
                write_json(report_path, report)
        report["pixelGatePassed"] = all(row["pixelGatePassed"] for row in report["visual"])
        if not report["pixelGatePassed"] and not args.diagnostic_performance:
            raise RuntimeError("Pixel gate failed. Review comparison PNGs; use --diagnostic-performance only for explicitly non-comparable timings")
        if args.profile:
            from profiling import run_profiles
            configs = {scenario: {**json.loads((LAB / f".build/public/{scenario}.json").read_text()),
                       "mode": "run", "checkpointMs": 0, "durationMs": args.duration * 1000}
                       for scenario in SCENARIOS}
            run_profiles(binaries, configs, args.output, args.trials, args.capture_hz, report, report_path)
        elif not args.visual_only:
            for scenario in SCENARIOS:
                config = json.loads((LAB / f".build/public/{scenario}.json").read_text())
                config.update(mode="run", checkpointMs=0, durationMs=args.duration * 1000)
                for repetition in range(args.trials + 1):
                    warmup = repetition == 0
                    for label in order(list(binaries), repetition):
                        name = f"{scenario}-{'warmup' if warmup else repetition}-{label}"
                        print(f"Live {name}", flush=True)
                        metrics = trial(binaries[label], config, args.output / name)
                        report["trials"].append({"scenario": scenario, "candidate": label, "warmup": warmup, **metrics})
                        write_json(report_path, report)
            report["medians"] = []
            for scenario in SCENARIOS:
                for label in binaries:
                    rows = [r for r in report["trials"] if r["scenario"] == scenario and r["candidate"] == label and not r["warmup"]]
                    if len(rows) != args.trials:
                        raise RuntimeError("Incomplete measured trial group")
                    report["medians"].append({"scenario": scenario, "candidate": label, "measuredSuccesses": len(rows),
                        **{key: statistics.median(r[key] for r in rows if r[key] is not None) if any(r[key] is not None for r in rows) else None
                           for key in rows[0] if key not in ("scenario", "candidate", "warmup")}})
        report["complete"] = True
    except (Exception, KeyboardInterrupt) as error:
        report["complete"] = False
        report["failures"].append(str(error))
        print(str(error), file=sys.stderr)
    finally:
        report["performanceComparable"] = bool(report.get("complete") and report.get("pixelGatePassed")
            and not args.visual_only and not report.get("frameCaptureBackpressureDetected"))
        write_json(report_path, report)
        print(f"Results: {args.output}")
    if report.get("frameCaptureBackpressureDetected"):
        print("Frame observer backlog detected; raw results retained, performance is not comparable", file=sys.stderr)
    if not report["complete"] or not report.get("pixelGatePassed") or report.get("frameCaptureBackpressureDetected"):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
