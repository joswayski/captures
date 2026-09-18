#!/usr/bin/env python3
"""Native workbench diagnostics, not a cross-renderer performance acceptance gate.

No screen capture is performed. macOS/Linux RSS and Windows working set are NOT
physical footprint; CPU covers only the process. Process-tree and GPU costs are
also excluded. Keep #529's coalition/WindowServer observer for comparisons.
"""
import argparse
import ctypes
import hashlib
import json
import os
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


def ps_sample(pid, clock=time.monotonic):
    output = subprocess.check_output(
        ["ps", "-p", str(pid), "-o", "time=", "-o", "rss="], text=True
    ).split()
    if len(output) != 2:
        raise RuntimeError(f"Cannot sample live process {pid}")
    return {"monotonic": clock(), "cpuSeconds": cpu_seconds(output[0]),
            "rssBytes": int(output[1]) * 1024}


def filetime_seconds(high, low):
    """Convert the unsigned 64-bit Win32 FILETIME counter to seconds."""
    return ((int(high) << 32) | int(low)) / 10_000_000


def windows_sample(pid, api=None, clock=time.monotonic):
    """Sample process CPU and working set, closing the Win32 handle on all paths."""
    if api is None:
        from ctypes import wintypes

        class FILETIME(ctypes.Structure):
            _fields_ = [("dwLowDateTime", wintypes.DWORD),
                        ("dwHighDateTime", wintypes.DWORD)]

        class PROCESS_MEMORY_COUNTERS(ctypes.Structure):
            _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD),
                        ("PeakWorkingSetSize", ctypes.c_size_t),
                        ("WorkingSetSize", ctypes.c_size_t),
                        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                        ("QuotaPagedPoolUsage", ctypes.c_size_t),
                        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                        ("PagefileUsage", ctypes.c_size_t),
                        ("PeakPagefileUsage", ctypes.c_size_t)]

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        # ctypes otherwise assumes C ints and truncates 64-bit process handles.
        kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel32.OpenProcess.restype = wintypes.HANDLE
        kernel32.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(FILETIME)] * 4
        kernel32.GetProcessTimes.restype = wintypes.BOOL
        kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel32.CloseHandle.restype = wintypes.BOOL
        psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE,
                                             ctypes.POINTER(PROCESS_MEMORY_COUNTERS), wintypes.DWORD]
        psapi.GetProcessMemoryInfo.restype = wintypes.BOOL
        api = (kernel32, psapi, FILETIME, PROCESS_MEMORY_COUNTERS)
    kernel32, psapi, FILETIME, PROCESS_MEMORY_COUNTERS = api
    last_error = getattr(ctypes, "get_last_error", lambda: 0)
    handle = kernel32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        raise OSError(last_error(), f"Cannot open process {pid}")
    try:
        creation, exit_time, kernel, user = (FILETIME() for _ in range(4))
        if not kernel32.GetProcessTimes(handle, ctypes.byref(creation), ctypes.byref(exit_time),
                                        ctypes.byref(kernel), ctypes.byref(user)):
            raise OSError(last_error(), f"Cannot read CPU time for process {pid}")
        memory = PROCESS_MEMORY_COUNTERS()
        memory.cb = ctypes.sizeof(memory)
        if not psapi.GetProcessMemoryInfo(handle, ctypes.byref(memory), memory.cb):
            raise OSError(last_error(), f"Cannot read working set for process {pid}")
        cpu = filetime_seconds(kernel.dwHighDateTime, kernel.dwLowDateTime)
        cpu += filetime_seconds(user.dwHighDateTime, user.dwLowDateTime)
        return {"monotonic": clock(), "cpuSeconds": cpu,
                "workingSetBytes": int(memory.WorkingSetSize)}
    finally:
        kernel32.CloseHandle(handle)


def linux_sample(pid, clock=time.monotonic, read_text=None, clock_ticks=None):
    read_text = read_text or (lambda path: Path(path).read_text())
    stat = read_text(f"/proc/{pid}/stat")
    fields = stat.rsplit(")", 1)[1].split()
    if len(fields) < 13:
        raise RuntimeError(f"Cannot sample live process {pid}")
    ticks = clock_ticks or os.sysconf("SC_CLK_TCK")
    cpu = (int(fields[11]) + int(fields[12])) / ticks
    status = read_text(f"/proc/{pid}/status")
    rss_lines = [line.split() for line in status.splitlines() if line.startswith("VmRSS:")]
    if len(rss_lines) != 1 or len(rss_lines[0]) < 2:
        raise RuntimeError(f"Cannot sample live process {pid}")
    return {"monotonic": clock(), "cpuSeconds": cpu,
            "rssBytes": int(rss_lines[0][1]) * 1024}


def sample(pid, system=None):
    system = system or platform.system()
    if system == "Windows":
        return windows_sample(pid)
    if system == "Linux":
        return linux_sample(pid)
    if system == "Darwin":
        return ps_sample(pid)
    raise RuntimeError(f"Unsupported platform: {system}")


def summarize(samples):
    if len(samples) < 2:
        raise ValueError("At least two resource samples are required")
    elapsed = samples[-1]["monotonic"] - samples[0]["monotonic"]
    cpu = samples[-1]["cpuSeconds"] - samples[0]["cpuSeconds"]
    if elapsed <= 0 or cpu < 0:
        raise ValueError("Invalid sample interval or decreasing CPU counter")
    memory_key = "workingSetBytes" if "workingSetBytes" in samples[0] else "rssBytes"
    summary_names = (("medianProcessWorkingSetBytes", "peakSampledProcessWorkingSetBytes")
                     if memory_key == "workingSetBytes"
                     else ("medianProcessRSSBytes", "peakSampledProcessRSSBytes"))
    return {"sampledSeconds": elapsed, "processCPUPercentOneCore": cpu / elapsed * 100,
            summary_names[0]: statistics.median(s[memory_key] for s in samples),
            summary_names[1]: max(s[memory_key] for s in samples)}


def workloads_for(renderer):
    scenes = ["idle", "preferences", "history", "hud", "preview"]
    workloads = [(scene, False, False) for scene in scenes]
    workloads += [(scene, True, False) for scene in scenes[1:]]
    if renderer == "appkit":
        workloads.append(("preview", True, True))
    else:
        workloads += [("editor", False, False), ("editor", True, False)]
    if renderer == "dcomp":
        # DWM owns the floating fade. The ordinary preview only changes content.
        workloads.append(("preview-floating", True, False))
    return workloads


def validate_renderer_platform(renderer, system):
    supported = {"appkit": ("Darwin",), "wgpu": ("Darwin", "Windows", "Linux"),
                 "dcomp": ("Windows",), "gtk": ("Linux",)}
    if renderer not in supported or system not in supported[renderer]:
        raise ValueError(f"{renderer} diagnostics do not support {system}")


def events_at(path):
    rows = []
    for line in path.read_text().splitlines(keepends=True):
        # A concurrent FileHandle write may be visible only in part. Only
        # newline-terminated records are committed; malformed full records fail.
        if line.endswith("\n") and line.strip():
            rows.append(json.loads(line))
    return rows


def trial(binary, destination, scene, exercise, reference, seconds):
    args = [str(binary), "--scene", scene.removesuffix("-floating"), "--quit-after", str(seconds + 15)]
    if scene.endswith("-floating"):
        args.append("--floating")
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
    parser.add_argument("--renderer", choices=("appkit", "wgpu", "dcomp", "gtk"), default="appkit")
    args = parser.parse_args()
    try:
        validate_renderer_platform(args.renderer, platform.system())
    except ValueError as error:
        parser.error(str(error))
    if args.seconds < 30 or args.trials < 1:
        parser.error("Use at least 30 seconds and one trial")
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=False)
    memory_measurement = ("Windows process working set" if platform.system() == "Windows"
                          else "process RSS")
    metadata = {"schema": 1, "platform": platform.platform(), "renderer": args.renderer,
                "binarySHA256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                "measurement": (f"process CPU-time delta (one-core definition) and {memory_measurement}; "
                                "NOT physical footprint, process-tree/GPU cost, or displayed FPS"),
                "seconds": args.seconds, "trials": args.trials, "complete": False}
    report = args.output / "report.json"
    report.write_text(json.dumps(metadata, indent=2))
    workloads = workloads_for(args.renderer)
    results = []
    # One excluded warmup per workload, then rotate trial order.
    for iteration in range(args.trials + 1):
        order = workloads[iteration:] + workloads[:iteration]
        for scene, exercise, reference in order:
            effect = ('reference' if reference else 'atlas') if args.renderer == 'appkit' else 'probe'
            name = f"{iteration}-{scene}-{'active' if exercise else 'idle'}-{effect}"
            print(name, flush=True)
            result = trial(binary, args.output / name, scene, exercise, reference, args.seconds)
            if iteration:
                results.append(result)
    metadata.update(complete=True, results=results)
    report.write_text(json.dumps(metadata, indent=2))
    print(f"Diagnostics complete: {report}; no cross-renderer performance claim.")


if __name__ == "__main__":
    main()
