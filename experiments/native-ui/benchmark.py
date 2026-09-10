#!/usr/bin/env python3
"""Linux process-tree resource probe. Run inside a disposable X11/DBus session.

No pip dependencies. stdout is JSON; diagnostics go to stderr. This measures
warm launches of the two minimal probes, NOT feature-equivalent production apps.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import statistics
import subprocess
import sys
import tempfile
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


def stop(process):
    # Only the new session/process group created by this test is terminated.
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=5)
    except ProcessLookupError:
        pass
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()


def trial(binary, settle, duration):
    with tempfile.TemporaryDirectory(prefix='captures-probe-') as directory:
        ready = Path(directory) / 'ready'
        env = dict(os.environ, CAPTURES_PROBE_READY=str(ready))
        with tempfile.TemporaryFile() as log:
            started = time.perf_counter()
            process = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log, start_new_session=True)
            try:
                while not ready.exists():
                    if process.poll() is not None or time.perf_counter() - started > 30:
                        log.seek(0)
                        raise RuntimeError(f'{binary.name} did not become ready: {log.read().decode(errors="replace")}')
                    time.sleep(0.002)
                ready_ms = (time.perf_counter() - started) * 1000
                time.sleep(settle)
                memory = resources(process.pid)
                before = process_tree(process.pid)
                cpu_start = time.perf_counter()
                time.sleep(duration)
                after = process_tree(process.pid)
                if before.keys() != after.keys() or process.poll() is not None:
                    raise RuntimeError('Process tree changed during idle sample')
                elapsed = time.perf_counter() - cpu_start
                ticks = sum(after.values()) - sum(before.values())
                return dict(memory, ready_ms=ready_ms,
                            idle_cpu_percent_one_core=100 * ticks / os.sysconf('SC_CLK_TCK') / elapsed)
            finally:
                stop(process)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runs', type=int, default=10)
    parser.add_argument('--settle', type=float, default=2)
    parser.add_argument('--idle-seconds', type=float, default=3)
    args = parser.parse_args()
    if args.runs < 2 or args.settle < 0 or args.idle_seconds <= 0:
        parser.error('Use at least two runs, nonnegative settle, and positive idle duration')
    binaries = {name: Path(__file__).parent / 'target/release' / f'captures-{name}-probe'
                for name in ('gtk', 'tauri')}
    samples = {name: [] for name in binaries}
    for name, binary in binaries.items():
        print(f'Warmup: {name}', file=sys.stderr)
        trial(binary, args.settle, args.idle_seconds)
    for index in range(args.runs):
        order = ('gtk', 'tauri') if index % 2 == 0 else ('tauri', 'gtk')
        for name in order:
            result = trial(binaries[name], args.settle, args.idle_seconds)
            samples[name].append(result)
            print(f'{index + 1}/{args.runs} {name}: {result}', file=sys.stderr)
    summaries = {}
    for name, rows in samples.items():
        summaries[name] = {key: {'median': statistics.median(row[key] for row in rows),
                                 'min': min(row[key] for row in rows),
                                 'max': max(row[key] for row in rows)} for key in rows[0]}
        summaries[name]['binary_bytes'] = binaries[name].stat().st_size
        summaries[name]['binary_sha256'] = hashlib.sha256(binaries[name].read_bytes()).hexdigest()
    print(json.dumps(dict(recorded_at=datetime.now(timezone.utc).isoformat(),
                          lock_sha256=hashlib.sha256((Path(__file__).parent / 'Cargo.lock').read_bytes()).hexdigest(),
                          environment=dict(platform=platform.platform(), machine=platform.machine(),
                                          rustc=subprocess.check_output(['rustc', '--version'], text=True).strip(),
                                          gtk=subprocess.check_output(['pkg-config', '--modversion', 'gtk+-3.0'], text=True).strip(),
                                          webkit=subprocess.check_output(['pkg-config', '--modversion', 'webkit2gtk-4.1'], text=True).strip(),
                                          cpu_count=os.cpu_count(), display=os.environ.get('DISPLAY'),
                                          software_gl=os.environ.get('LIBGL_ALWAYS_SOFTWARE')),
                          configuration=vars(args), summary=summaries, samples=samples), indent=2))


if __name__ == '__main__':
    main()
