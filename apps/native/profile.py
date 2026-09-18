#!/usr/bin/env python3
"""Native workbench diagnostics, not a cross-renderer performance acceptance gate.

No screen capture is performed. ps RSS is NOT physical footprint, and CPU covers
only the process. Keep #529's coalition/WindowServer observer for comparisons.
"""
import argparse
import hashlib
import json
import platform
import statistics
import subprocess
import time
from pathlib import Path


def cpu_seconds(value):
    """macOS ps time: [[days-]hours:]minutes:seconds.hundredths."""
    days = 0
    if "-" in value:
        day, value = value.split("-", 1)
        days = int(day)
    fields = value.split(":")
    if len(fields) not in (2, 3):
        raise ValueError(f"Unexpected ps CPU time: {value}")
    total = 0.0
    for field in fields:
        total = total * 60 + float(field)
    return total + days * 86400


def sample(pid):
    output = subprocess.check_output(
        ["ps", "-p", str(pid), "-o", "time=", "-o", "rss="], text=True
    ).split()
    if len(output) != 2:
        raise RuntimeError(f"Cannot sample live process {pid}")
    return {"monotonic": time.monotonic(), "cpuSeconds": cpu_seconds(output[0]),
            "rssBytes": int(output[1]) * 1024}


def summarize(samples):
    if len(samples) < 2:
        raise ValueError("At least two resource samples are required")
    elapsed = samples[-1]["monotonic"] - samples[0]["monotonic"]
    cpu = samples[-1]["cpuSeconds"] - samples[0]["cpuSeconds"]
    if elapsed <= 0 or cpu < 0:
        raise ValueError("Invalid sample interval or decreasing CPU counter")
    return {"sampledSeconds": elapsed, "processCPUPercentOneCore": cpu / elapsed * 100,
            "medianProcessRSSBytes": statistics.median(s["rssBytes"] for s in samples),
            "peakSampledProcessRSSBytes": max(s["rssBytes"] for s in samples)}


def events_at(path):
    rows = []
    for line in path.read_text().splitlines(keepends=True):
        # A concurrent FileHandle write may be visible only in part. Only
        # newline-terminated records are committed; malformed full records fail.
        if line.endswith("\n") and line.strip():
            rows.append(json.loads(line))
    return rows


def trial(binary, destination, scene, exercise, reference, seconds):
    args = [str(binary), "--scene", scene, "--quit-after", str(seconds + 15)]
    if exercise:
        args.append("--exercise")
    if reference:
        args.append("--reference-chips")
    log = destination.with_suffix(".jsonl")
    errors = destination.with_suffix(".stderr.txt")
    with log.open("w") as stdout, errors.open("w") as stderr:
        started = time.monotonic()
        process = subprocess.Popen(args, stdout=stdout, stderr=stderr)
        try:
            while not any(e.get("event") == "ready" for e in events_at(log)):
                if process.poll() is not None or time.monotonic() - started > 15:
                    raise RuntimeError(f"Workbench did not become ready; see {errors}")
                time.sleep(.05)
            ready_ms = (time.monotonic() - started) * 1000
            samples = [sample(process.pid)]
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                time.sleep(min(.5, max(0, deadline - time.monotonic())))
                if process.poll() is not None:
                    raise RuntimeError("Workbench exited before completing resource measurement")
                samples.append(sample(process.pid))
            events = events_at(log)
            if any(e.get("event") == "effect-error" for e in events):
                raise RuntimeError(f"Effect failed; see {log}")
            if exercise and len([e for e in events if e.get("event") == "scripted-action"]) != 6:
                raise RuntimeError("Missing scripted actions; refusing an incomplete workload")
            result = {"scene": scene, "exercise": exercise, "referenceChips": reference,
                      "launchToReadyObservedMs": ready_ms, "samples": samples,
                      "summary": summarize(samples)}
            destination.with_suffix(".json").write_text(json.dumps(result, indent=2))
            return result
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--seconds", type=int, default=60)
    parser.add_argument("--trials", type=int, default=3)
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("Run on macOS; this runner does not emulate AppKit")
    if args.seconds < 30 or args.trials < 1:
        parser.error("Use at least 30 seconds and one trial")
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=False)
    metadata = {"schema": 1, "platform": platform.platform(),
                "binarySHA256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                "measurement": "process CPU-time delta and RSS; NOT coalition physical footprint or displayed FPS",
                "seconds": args.seconds, "trials": args.trials, "complete": False}
    report = args.output / "report.json"
    report.write_text(json.dumps(metadata, indent=2))
    workloads = [(s, False, False) for s in ["idle", "preferences", "history", "hud", "preview"]]
    workloads += [(s, True, False) for s in ["preferences", "history", "hud", "preview"]]
    workloads += [("preview", True, True)]
    results = []
    # One excluded warmup per workload, then rotate trial order.
    for iteration in range(args.trials + 1):
        order = workloads[iteration:] + workloads[:iteration]
        for scene, exercise, reference in order:
            name = f"{iteration}-{scene}-{'active' if exercise else 'idle'}-{'reference' if reference else 'atlas'}"
            print(name, flush=True)
            result = trial(binary, args.output / name, scene, exercise, reference, args.seconds)
            if iteration:
                results.append(result)
    metadata.update(complete=True, results=results)
    report.write_text(json.dumps(metadata, indent=2))
    print(f"Diagnostics complete: {report}; no cross-renderer performance claim.")


if __name__ == "__main__":
    main()
