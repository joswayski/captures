#!/usr/bin/env python3
"""Separate whole-app Preferences reference, NOT a matched native comparison.

Run a locally built Captures executable on disposable Xvfb/DBus with an empty
profile. Does not modify production code. Requires xdotool and ImageMagick.
Inspect the saved screenshot before accepting results: a visible native window
alone does not prove that the React frontend rendered successfully.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time

from benchmark import process_tree, resources, stop


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('--artifacts', required=True, type=Path)
    args = parser.parse_args()
    args.artifacts.mkdir(parents=True, exist_ok=True)
    samples = []
    for index in range(3):
        with tempfile.TemporaryDirectory(prefix='captures-production-reference-') as directory:
            profile = Path(directory)
            config = profile / 'config/captures'
            config.mkdir(parents=True)
            # Required fields/default shortcuts from src-tauri/src/models.rs.
            # Only this disposable fixture starts with onboarding completed.
            (config / 'settings.json').write_text(json.dumps(dict(
                settings_schema_version=5, onboarding_completed=True,
                output_directory=str(profile / 'captures'), launch_at_login=False,
                region_shortcut='Super+Shift+S', window_shortcut='Alt+PrintScreen',
                display_shortcut='Shift+PrintScreen',
            )))
            env = dict(os.environ, HOME=str(profile), XDG_CONFIG_HOME=str(profile / 'config'),
                       XDG_DATA_HOME=str(profile / 'data'), XDG_CACHE_HOME=str(profile / 'cache'),
                       HTTP_PROXY='http://127.0.0.1:9', HTTPS_PROXY='http://127.0.0.1:9',
                       ALL_PROXY='http://127.0.0.1:9', NO_PROXY='localhost,127.0.0.1')
            with tempfile.TemporaryFile() as log:
                process = subprocess.Popen([str(args.binary.resolve())], env=env, stdout=log,
                                           stderr=log, start_new_session=True)
                try:
                    deadline = time.monotonic() + 30
                    while True:
                        windows = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name',
                                                  '^Captures Preferences$'], capture_output=True, text=True)
                        if windows.returncode == 0:
                            window = windows.stdout.strip().splitlines()[0]
                            break
                        if process.poll() is not None or time.monotonic() > deadline:
                            log.seek(0)
                            raise RuntimeError(log.read().decode(errors='replace'))
                        time.sleep(0.1)
                    time.sleep(10)
                    log.seek(0)
                    diagnostics = log.read().decode(errors='replace')
                    if 'failed to prepare capture' in diagnostics:
                        raise RuntimeError(f'Hidden capture window initialization failed: {diagnostics}')
                    memory = resources(process.pid)
                    before = process_tree(process.pid)
                    started = time.perf_counter()
                    time.sleep(5)
                    after = process_tree(process.pid)
                    elapsed = time.perf_counter() - started
                    assert before.keys() == after.keys() and process.poll() is None
                    memory['idle_cpu_percent_one_core'] = (
                        100 * (sum(after.values()) - sum(before.values())) / os.sysconf('SC_CLK_TCK') / elapsed)
                    samples.append(memory)
                    if index == 0:
                        subprocess.run(['import', '-window', window,
                                        str(args.artifacts / 'production-preferences.png')], check=True)
                finally:
                    stop(process)
    print(json.dumps(dict(state='Current source app, empty Preferences, pre-created hidden webviews; '
                               'disposable profile; outbound HTTP proxy unavailable; unpackaged',
                          binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                          samples=samples,
                          median={key: statistics.median(row[key] for row in samples) for key in samples[0]}), indent=2))


if __name__ == '__main__':
    main()
