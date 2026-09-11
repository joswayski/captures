#!/usr/bin/env python3
"""Sample an already-open macOS app state. Does not launch, stop, or modify apps.

These are process RSS/CPU diagnostics, NOT PSS, physical footprint, energy, launch
latency, frame rate, or a complete app-memory comparison. WebKit XPC services may
be parented by launchd; missing attribution is explicitly reported, never hidden.
"""
import argparse
import hashlib
import json
import os
import platform
import statistics
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path


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
        rows[int(pid)] = {"pid": int(pid), "parent": int(parent), "rss_kib": int(rss),
                          "cpu_seconds": cpu_seconds(cpu), "name": Path(command).name}
    return rows


def descendants(rows, pid):
    if pid not in rows:
        raise ValueError("The selected process exited")
    selected = {pid}
    while True:
        children = {key for key, row in rows.items() if row["parent"] in selected}
        expanded = selected | children
        if expanded == selected:
            return {key: rows[key] for key in sorted(selected)}
        selected = expanded


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--label", choices=["tauri", "native"], required=True)
    parser.add_argument("--state", required=True, help="e.g. preferences-dark or editor-same-fixture")
    parser.add_argument("--output", type=Path, required=True, help="New JSON file; never overwritten")
    parser.add_argument("--fixture", type=Path, help="Optional shared input file to hash, not copy")
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--interval", type=float, default=2)
    parser.add_argument("--settle", type=float, default=5)
    parser.add_argument("--window-probe", type=Path, required=True)
    parser.add_argument("--window-id", type=int, help="Required when the app has multiple visible windows")
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("Run this sampler on the Mac being compared; Linux results are not macOS results")
    if args.samples < 2 or args.interval <= 0 or args.settle < 0:
        parser.error("Use at least 2 samples, positive interval, and nonnegative settling time")
    if args.output.exists() or args.output.with_suffix(".png").exists():
        parser.error("Choose a new output name; existing results/screenshots are preserved")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    windows = json.loads(subprocess.check_output([str(args.window_probe.resolve()), str(args.pid)]))
    eligible = [window for window in windows if args.window_id is None or window["window_id"] == args.window_id]
    if len(eligible) != 1:
        parser.error(f"Select one visible document window with --window-id. Found: {windows}")
    window = eligible[0]
    time.sleep(args.settle)
    raw = []
    unattributed = set()
    for _ in range(args.samples):
        all_before = snapshot()
        before = descendants(all_before, args.pid)
        started = time.monotonic()
        time.sleep(args.interval)
        all_after = snapshot()
        after = descendants(all_after, args.pid)
        elapsed = time.monotonic() - started
        if before.keys() != after.keys():
            raise RuntimeError("Process tree changed during sampling; rerun in a settled app state")
        unattributed.update(row["name"] for pid, row in all_after.items()
                            if pid not in after and "WebKit" in row["name"])
        cpu = sum(after[pid]["cpu_seconds"] - before[pid]["cpu_seconds"] for pid in after)
        if cpu < 0:
            raise RuntimeError("Process identity or CPU counter changed; sample is invalid")
        raw.append({"elapsed_seconds": elapsed, "tree_rss_mib": sum(row["rss_kib"] for row in after.values()) / 1024,
                    "root_rss_mib": after[args.pid]["rss_kib"] / 1024,
                    "tree_cpu_percent_one_core": cpu / elapsed * 100,
                    "processes": list(after.values())})
    final_windows = json.loads(subprocess.check_output([str(args.window_probe.resolve()), str(args.pid)]))
    if window not in final_windows:
        raise RuntimeError("The measured window changed size/title or disappeared; sample is invalid")
    screenshot = args.output.with_suffix(".png")
    subprocess.run(["screencapture", "-x", "-o", "-l", str(window["window_id"]), str(screenshot)], check=True)
    if not screenshot.exists() or screenshot.stat().st_size < 100:
        raise RuntimeError("No usable window screenshot; check Terminal Screen Recording permission")
    result = {"recorded_at": datetime.now(timezone.utc).isoformat(), "label": args.label, "state": args.state,
              "os": platform.platform(), "hardware": subprocess.check_output(["sysctl", "-n", "hw.model"], text=True).strip(),
              "logical_cpus": os.cpu_count(), "window": window, "settle_seconds": args.settle,
              "scope": __doc__.strip(), "unattributed_webkit_service_names": sorted(unattributed),
              "complete_app_memory": False,
              "fixture_sha256": hashlib.sha256(args.fixture.read_bytes()).hexdigest() if args.fixture else None,
              "screenshot": screenshot.name, "samples": raw,
              "medians": {key: statistics.median(row[key] for row in raw)
                          for key in ("tree_rss_mib", "root_rss_mib", "tree_cpu_percent_one_core")}}
    with args.output.open("x") as output:
        json.dump(result, output, indent=2); output.write("\n")
    print(json.dumps(result["medians"], indent=2))
    print("Inspect the screenshot before using these numbers. RSS is not whole-app physical memory.")


if __name__ == "__main__":
    main()
