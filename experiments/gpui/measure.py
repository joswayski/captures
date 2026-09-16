#!/usr/bin/env python3
"""Sample an already rendered Linux app and its entire process tree.

Launch the app separately in a disposable profile, inspect its window, and pass
its PID. This does not launch/stop apps or infer feature equivalence. No pip deps.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import time


def process_tree(root):
    processes = {}
    for path in Path('/proc').glob('[0-9]*/stat'):
        try:
            # comm may itself contain spaces or parentheses.
            fields = path.read_text().rsplit(')', 1)[1].split()
            processes[int(path.parent.name)] = (int(fields[1]), int(fields[11]) + int(fields[12]))
        except FileNotFoundError:
            continue
    found = {root}
    while True:
        children = {pid for pid, (parent, _) in processes.items() if parent in found}
        expanded = found | children
        if expanded == found:
            break
        found = expanded
    return {pid: processes[pid][1] for pid in found if pid in processes}


def resources(root):
    processes = process_tree(root)
    totals = dict(rss_mib=0.0, pss_mib=0.0, private_mib=0.0)
    for pid in processes:
        # Missing/denied smaps is a failed measurement, never a zero-memory child.
        fields = {}
        for line in Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines()[1:]:
            key, value = line.split(':', 1)
            fields[key] = int(value.split()[0])
        totals['rss_mib'] += fields['Rss'] / 1024
        totals['pss_mib'] += fields['Pss'] / 1024
        totals['private_mib'] += (fields['Private_Clean'] + fields['Private_Dirty']) / 1024
    return dict(totals, processes=len(processes))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pid", type=int)
    parser.add_argument("--state", required=True, help="Exact rendered workload and limitations")
    parser.add_argument("--samples", type=int, default=3)
    parser.add_argument("--seconds", type=float, default=5)
    parser.add_argument("--screenshot", required=True, type=Path,
                        help="Previously inspected screenshot for this state")
    args = parser.parse_args()
    if args.samples < 2 or args.seconds <= 0 or not args.screenshot.is_file():
        parser.error("Need at least two samples, positive seconds, and a real screenshot")
    exe = Path(f"/proc/{args.pid}/exe")
    executable_hash = hashlib.sha256(exe.read_bytes()).hexdigest()
    executable_bytes = exe.stat().st_size
    rows = []
    for _ in range(args.samples):
        before = process_tree(args.pid)
        if args.pid not in before:
            raise RuntimeError("Root process exited before sampling")
        memory = resources(args.pid)
        start = time.perf_counter()
        time.sleep(args.seconds)
        elapsed = time.perf_counter() - start
        after = process_tree(args.pid)
        if before.keys() != after.keys():
            raise RuntimeError("Process tree changed; sample rejected rather than undercounted")
        memory["cpu_percent_one_core"] = (
            100 * (sum(after.values()) - sum(before.values()))
            / os.sysconf("SC_CLK_TCK") / elapsed
        )
        memory["interval_seconds"] = elapsed
        rows.append(memory)
    print(json.dumps({
        "recorded_at": datetime.now(timezone.utc).isoformat(),
        "state": args.state,
        "sampling": "Repeated intervals in one warm process, not independent launches",
        "environment": {"kernel": platform.release(), "architecture": platform.machine(),
                        "logical_cpus": os.cpu_count(), "display": os.environ.get("DISPLAY")},
        "executable_sha256": executable_hash,
        "executable_bytes": executable_bytes,
        "screenshot_sha256": hashlib.sha256(args.screenshot.read_bytes()).hexdigest(),
        "samples": rows,
        "median": {key: statistics.median(row[key] for row in rows) for key in rows[0]},
    }, indent=2))


if __name__ == "__main__":
    main()
