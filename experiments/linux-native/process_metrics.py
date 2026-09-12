"""Process-tree resource helpers shared by native validation commands."""

import os
from pathlib import Path
import signal
import subprocess


def process_tree(root):
    processes = {}
    for path in Path('/proc').glob('[0-9]*/stat'):
        try:
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
        fields = {}
        for line in Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines()[1:]:
            key, value = line.split(':', 1)
            fields[key] = int(value.split()[0])
        totals['rss_mib'] += fields['Rss'] / 1024
        totals['pss_mib'] += fields['Pss'] / 1024
        totals['private_mib'] += (fields['Private_Clean'] + fields['Private_Dirty']) / 1024
    return dict(totals, processes=len(processes))


def stop(process):
    """Stop only the isolated process group created by a validation command."""
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=5)
    except ProcessLookupError:
        pass
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
