"""Isolated LaunchServices resource accounting and WindowServer frame observation."""
import json
import math
import os
import plistlib
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path

LAB = Path(__file__).resolve().parent
OBSERVER = LAB / ".build/parity-observer"
RESOURCES = LAB / ".build/parity-resources"
MIB = 1024 ** 2


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def probe(pid):
    return json.loads(subprocess.check_output([str(RESOURCES), str(pid)], text=True, timeout=10))


def validate_resources(sample, ready, baseline=None):
    if sample["rootPid"] != ready["pid"] or not sample["rootStartMach"]:
        raise ValueError("Resource sample does not identify the launched application")
    if not sample["resourceCoalitionId"] or sample["resourceCoalitionId"] == sample["samplerCoalitionId"]:
        raise ValueError("App lacks an isolated resource coalition; refusing Terminal-wide totals")
    identities = {p["pid"]: p["startMach"] for p in sample["processes"]}
    if len(identities) != len(sample["processes"]) or identities.get(ready["pid"]) != sample["rootStartMach"]:
        raise ValueError("Duplicate or missing root process identity")
    required = {ready["pid"]}
    if ready["renderer"].startswith("tauri"):
        helpers = ready.get("webkitProcesses", [])
        if len(helpers) != 3 or {p["role"] for p in helpers} != {"webContent", "gpu", "network"}:
            raise ValueError("Missing explicit WebKit process ownership; refusing partial totals")
        if any(p["pid"] < 0 or (p["role"] == "webContent" and p["pid"] == 0) for p in helpers):
            raise ValueError("Invalid explicit WebKit process identity")
        # Non-launching private getters may report no current GPU/Network PID.
        # All coalition members still count; new members invalidate the baseline.
        required.update(p["pid"] for p in helpers if p["pid"] > 0)
    if not required <= identities.keys():
        raise ValueError("A required WebKit/app process is outside the isolated coalition or unreadable")
    allowed_unreadable = (baseline or sample).get("unreadableBeforeLaunch", [])
    if set(sample["unreadableSameUidPids"]) - set(allowed_unreadable):
        raise ValueError("New unreadable same-user process; full attribution cannot be established")
    if baseline:
        if (sample["rootStartMach"], sample["resourceCoalitionId"]) != (
                baseline["rootStartMach"], baseline["resourceCoalitionId"]):
            raise ValueError("Application identity/coalition changed during measurement")
        if identities != {p["pid"]: p["startMach"] for p in baseline["processes"]}:
            raise ValueError("Process membership changed; refusing CPU totals with missing lifetimes")


def quantile(values, fraction):
    if not values:
        return None
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)]


def summarize_resources(samples):
    if len(samples) < 3:
        raise ValueError("Not enough resource samples")
    cpu = [sum(p["userCpuNs"] + p["systemCpuNs"] for p in s["processes"]) for s in samples]
    intervals = []
    for first, last, a, b in zip(samples, samples[1:], cpu, cpu[1:]):
        dt = last["hostTimeNs"] - first["hostTimeNs"]
        if dt <= 0 or b < a:
            raise ValueError("Resource clock/counter went backwards")
        intervals.append(100 * (b - a) / dt)
    elapsed = (samples[-1]["hostTimeNs"] - samples[0]["hostTimeNs"]) / 1e9
    footprints = [sum(p["physicalFootprintBytes"] for p in s["processes"]) / MIB for s in samples]
    resident = [sum(p["residentBytes"] for p in s["processes"]) / MIB for s in samples]
    average_cpu = 100 * (cpu[-1] - cpu[0]) / (elapsed * 1e9)
    middle = len(samples) // 2
    tail_seconds = (samples[-1]["hostTimeNs"] - samples[middle]["hostTimeNs"]) / 1e9
    return {
        "sampledSeconds": elapsed, "cpuSeconds": (cpu[-1] - cpu[0]) / 1e9,
        "cpuAveragePercentOneCore": average_cpu,
        "cpuAveragePercentMachine": average_cpu / (os.cpu_count() or 1),
        "cpuIntervalP95PercentOneCore": quantile(intervals, .95),
        "physicalFootprintMedianMiB": statistics.median(footprints),
        "physicalFootprintPeakSampledMiB": max(footprints),
        "physicalFootprintFirstMiB": footprints[0], "physicalFootprintLastMiB": footprints[-1],
        "physicalFootprintTailGrowthMiBPerSecond": (footprints[-1] - footprints[middle]) / tail_seconds,
        "summedRSSMedianMiB": statistics.median(resident), "summedRSSPeakSampledMiB": max(resident),
        "processCount": len(samples[0]["processes"]),
        "probeP95Ms": quantile([s["elapsedProbeNs"] / 1e6 for s in samples], .95),
    }


def summarize_frames(capture, started_ns, config):
    if not capture["complete"]:
        raise ValueError("Frame observation did not complete")
    end_ns = started_ns + config["durationMs"] * 1e6
    frames, previous_time, previous_hash = [], None, None
    status_counts = {}
    for frame in capture["frames"]:
        status = str(frame["status"])
        status_counts[status] = status_counts.get(status, 0) + 1
        if frame["status"] != 0:  # Only complete frames carry documented image data.
            continue
        timestamp = frame.get("displayTimeNs")
        if not timestamp or not frame.get("pixelHash"):
            raise ValueError("Populated capture frame lacks a display timestamp or pixel digest")
        if abs(timestamp - frame["arrivalHostTimeNs"]) > 5e9:
            raise ValueError("WindowServer and host clocks disagree")
        if previous_time is not None and timestamp < previous_time:
            raise ValueError("WindowServer display timestamps went backwards")
        if timestamp == previous_time:
            if frame["pixelHash"] != previous_hash:
                raise ValueError("Conflicting pixel digests for the same display timestamp")
            continue
        if timestamp >= started_ns and previous_time is None:
            raise ValueError("No complete pre-start frame establishes the pixel-change baseline")
        changed = previous_hash is not None and frame["pixelHash"] != previous_hash
        if started_ns <= timestamp <= end_ns:
            frames.append({**frame, "changed": changed})
        previous_time, previous_hash = timestamp, frame["pixelHash"]
    if len(frames) < 2:
        raise ValueError("Too few populated WindowServer frames to measure this run")
    # Intentional stationary gaps must not lower the motion phase's cadence.
    phases = [("settle", 0, 580)]
    if config["scenario"].startswith("dust"):
        phases = [("dust", 0, 1300), ("settle", 1800, 2380)]
    windows = []
    for cycle in range(math.ceil(config["durationMs"] / config["cycleMs"])):
        for phase, begin, end in phases:
            begin += cycle * config["cycleMs"]
            end = min(end + cycle * config["cycleMs"], config["durationMs"])
            if begin >= end:
                continue
            start, stop = started_ns + begin * 1e6, started_ns + end * 1e6
            observed = [f for f in frames if start <= f["displayTimeNs"] < stop]
            changed = [f["displayTimeNs"] for f in observed if f["changed"]]
            gaps = [(b - a) / 1e6 for a, b in zip(changed, changed[1:])]
            boundaries = [start, *changed, stop]
            windows.append({"cycle": cycle, "phase": phase, "beginMs": begin, "endMs": end,
                "populatedFrames": len(observed), "changedFrames": len(changed),
                "observedChangedFramesPerSecond": len(changed) * 1000 / (end - begin),
                "changedIntervalP50Ms": quantile(gaps, .5), "changedIntervalP95Ms": quantile(gaps, .95),
                "maxUnchangedSpanMs": max((b - a) / 1e6 for a, b in zip(boundaries, boundaries[1:]))})
    processing = [f["processingNs"] / 1e6 for f in frames]
    lag = [max(0, f["arrivalHostTimeNs"] - f["displayTimeNs"]) / 1e6 for f in frames]
    budget = 1000 / capture["requestedHz"]
    summary = {"observedPopulatedFrames": len(frames),
        "observedChangedFrames": sum(f["changed"] for f in frames),
        "observerProcessingP95Ms": quantile(processing, .95),
        "observerArrivalLagP95Ms": quantile(lag, .95)}
    for phase, _, _ in phases:
        rows = [w for w in windows if w["phase"] == phase]
        summary[f"{phase}ObservedChangedHz"] = sum(w["changedFrames"] for w in rows) * 1000 / sum(w["endMs"] - w["beginMs"] for w in rows)
        summary[f"{phase}MaxUnchangedSpanMs"] = max(w["maxUnchangedSpanMs"] for w in rows)
    return {"summary": summary, "motionWindows": windows, "rawStatusCounts": status_counts,
        "captureBackpressureDetected": quantile(processing, .95) >= budget or max(lag) > 8 * budget,
        "exhaustivePresentationMeasurement": False,
        "scope": "Observed WindowServer frames and pixel changes, not panel scanout. Capture may miss frames; easing can legitimately repeat pixels. No automatic dropped-frame or 120-FPS certification."}


def package_app(executable, folder, label):
    app = folder / f"{label}.app"
    contents = app / "Contents"
    binary = contents / "MacOS/renderer"
    binary.parent.mkdir(parents=True)
    shutil.copy2(executable, binary)
    binary.chmod(0o755)
    identifier = f"io.github.joswayski.captures.parity.{label}.{uuid.uuid4().hex}"
    with (contents / "Info.plist").open("wb") as stream:
        plistlib.dump({"CFBundleIdentifier": identifier, "CFBundleExecutable": "renderer",
            "CFBundleName": f"Captures parity {label}", "CFBundlePackageType": "APPL",
            "CFBundleVersion": "1", "LSUIElement": True, "NSHighResolutionCapable": True}, stream)
    return app, identifier


def wait_json(path, pid, timeout, sample=None):
    deadline = time.monotonic() + timeout
    while not path.exists():
        error = Path(str(path).replace(".ready.json", "").replace(".started.json", "") + ".error.json")
        if error.exists():
            raise RuntimeError(error.read_text())
        if pid <= 0:
            raise RuntimeError("LaunchServices returned no live process; inspect renderer.json.error.json")
        os.kill(pid, 0)  # Liveness only; never signal an unverified PID here.
        if time.monotonic() > deadline:
            raise TimeoutError(f"Timed out waiting for {path}")
        if sample:
            sample()
        time.sleep(.1)
    return json.loads(path.read_text())


def profile_trial(executable, config, folder, label, measurement, capture_hz):
    folder.mkdir()
    output = folder / "renderer.json"
    config = {**config, "startGatePath": str(folder / "start.gate"), "profileResources": True}
    save(folder / "config.json", config)
    samples, observer, app_pid = [], None, None
    with tempfile.TemporaryDirectory(prefix="captures-parity-") as temporary:
        app, identifier = package_app(executable, Path(temporary), label)
        before_launch = probe(os.getpid())
        try:
            launch = json.loads(subprocess.check_output([
                str(OBSERVER), "launch", str(app), str(folder / "config.json"), str(output)], text=True, timeout=40))
            app_pid = launch["pid"]
            ready = wait_json(Path(str(output) + ".ready.json"), app_pid, 90)
            if ready["pid"] != app_pid or ready.get("measurementProtocol") != 2 or ready["window_id"] <= 0:
                raise ValueError("Renderer lacks profiling protocol2 or returned the wrong identity")
            for key in ("schema", "scenario", "mode", "scale", "checkpointMs"):
                if ready[key] != config[key]:
                    raise ValueError(f"Ready marker mismatch: {key}")
            baseline = probe(app_pid)
            baseline["unreadableBeforeLaunch"] = before_launch["unreadableSameUidPids"]
            save(folder / "ownership.json", {"ready": ready, "baseline": baseline})
            validate_resources(baseline, ready)
            capture = folder / "frames.json"
            if measurement == "frames":
                with (folder / "observer.log").open("w") as log:
                    observer = subprocess.Popen([str(OBSERVER), "capture", str(ready["window_id"]),
                        str(app_pid), str(int(config["width"] * config["scale"])),
                        str(int(config["height"] * config["scale"])), str(capture_hz), str(capture)], stdout=log, stderr=log)
                wait_json(Path(str(capture) + ".ready.json"), observer.pid, 90)

            def sample():
                current = probe(app_pid)
                samples.append(current)
                validate_resources(current, ready, baseline)

            sample()
            Path(config["startGatePath"]).touch()
            started = wait_json(Path(str(output) + ".started.json"), app_pid, 10)
            if started["schema"] != 1 or started["pid"] != app_pid or not samples[0]["hostTimeNs"] <= started["startHostTimeNs"] <= samples[0]["hostTimeNs"] + 10e9:
                raise ValueError("Renderer start marker is not on the expected host clock")
            result = wait_json(output, app_pid, config["durationMs"] / 1000 + 20,
                sample if measurement == "resources" else None)
            sample()
            if not result.get("complete") or result["elapsedMs"] < config["durationMs"] or result["cycles"] != math.ceil(config["durationMs"] / config["cycleMs"]):
                raise ValueError("Incomplete live workload")
            save(folder / "resources.json", samples)
            if measurement == "frames":
                Path(str(capture) + ".stop").touch()
                observer.wait(timeout=15)
                if observer.returncode:
                    raise RuntimeError("Frame observer failed; inspect observer.log")
                measured = summarize_frames(json.loads(capture.read_text()), started["startHostTimeNs"], config)
            else:
                measured = {"summary": summarize_resources(samples), "fullCoalitionAttributed": True,
                    "scope": "All live processes in an isolated resource coalition, with WebKit PID validation. Footprint sums exclude unassigned WindowServer/kernel/GPU costs; peak is sampled. CPU is time-weighted over the recorded interval, including effect setup."}
            save(folder / "measured.json", measured)
            return measured
        except BaseException as error:
            save(folder / "failure.json", {"error": str(error), "type": type(error).__name__})
            raise
        finally:
            original_error = sys.exc_info()[0]
            cleanup_errors = []
            if samples:
                save(folder / "resources.json", samples)
            if observer and observer.poll() is None:
                try:
                    Path(str(folder / "frames.json") + ".stop").touch()
                    observer.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    observer.terminate()
                    try:
                        observer.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        observer.kill()
                        observer.wait()
                    cleanup_errors.append("Frame observer required forced termination")
            if app_pid and app_pid > 0:
                try:
                    subprocess.run([str(OBSERVER), "terminate", str(app_pid), identifier], check=True, timeout=20)
                except (OSError, subprocess.SubprocessError) as error:
                    cleanup_errors.append(str(error))
            if cleanup_errors:
                save(folder / "cleanup-errors.json", cleanup_errors)
                if not original_error:
                    raise RuntimeError(f"Trial cleanup failed: {cleanup_errors}")


def run_profiles(binaries, configs, output, trials, capture_hz, report, report_path):
    report["profileTrials"] = []
    report["profilingScope"] = "Separate unrecorded resource and ScreenCaptureKit frame passes. Complete app-coalition accounting, not whole-system memory or physical-panel FPS. Protocol2 start barrier; warmups excluded."
    for scenario, config in configs.items():
        for measurement in ("resources", "frames"):
            for repetition in range(trials + 1):
                labels = list(binaries)
                offset = repetition % len(labels)
                for label in labels[offset:] + labels[:offset]:
                    name = f"{scenario}-{measurement}-{'warmup' if repetition == 0 else repetition}-{label}"
                    print(f"Profile {name}", flush=True)
                    result = profile_trial(binaries[label], config, output / name, label, measurement, capture_hz)
                    report["profileTrials"].append({"scenario": scenario, "measurement": measurement,
                        "candidate": label, "warmup": repetition == 0, **result})
                    save(report_path, report)
    report["profileMedians"] = []
    for scenario in configs:
        for measurement in ("resources", "frames"):
            for label in binaries:
                rows = [r for r in report["profileTrials"] if r["scenario"] == scenario and r["measurement"] == measurement and r["candidate"] == label and not r["warmup"]]
                if len(rows) != trials:
                    raise ValueError("Incomplete profile trial group")
                report["profileMedians"].append({"scenario": scenario, "measurement": measurement,
                    "candidate": label, "measuredSuccesses": len(rows),
                    **{k: statistics.median(r["summary"][k] for r in rows) for k in rows[0]["summary"]}})
    report["frameCaptureBackpressureDetected"] = any(r.get("captureBackpressureDetected") for r in report["profileTrials"] if not r["warmup"])
