#!/usr/bin/env python3
"""Click the real X11 probe windows; verify capture output and retain review images.

Requires xdotool and ImageMagick (`import`, `identify`, `convert`). Run on a
disposable 1280x800 Xvfb display, not on a personal desktop with private content.
"""
import argparse
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile
import time

from benchmark import stop


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def wait_for(predicate, process):
    deadline = time.monotonic() + 20
    while not predicate():
        if process.poll() is not None or time.monotonic() > deadline:
            raise RuntimeError('Probe exited or UI action timed out')
        time.sleep(0.05)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    args.artifacts.mkdir(parents=True, exist_ok=True)
    for name in ('gtk', 'tauri'):
        binary = Path(__file__).parent / 'target/release' / f'captures-{name}-probe'
        with tempfile.TemporaryDirectory(prefix='captures-ui-smoke-') as directory:
            output = Path(directory)
            captures = output / 'captures'
            ready = output / 'ready'
            env = dict(os.environ, CAPTURES_PROBE_READY=str(ready), CAPTURES_PROBE_OUTPUT=str(captures))
            process = subprocess.Popen([str(binary)], env=env, start_new_session=True)
            try:
                wait_for(ready.exists, process)
                window = command('xdotool', 'search', '--onlyvisible', '--name', '^Captures UI probe$').splitlines()
                assert len(window) == 1, f'Expected one isolated probe window: {window}'
                window = window[0]
                command('xdotool', 'windowmove', window, '0', '0')
                command('xdotool', 'windowfocus', '--sync', window)
                time.sleep(0.2)
                command('import', '-window', window, str(args.artifacts / f'{name}-empty.png'))

                # Both deliberately fixed 800x600 layouts put these controls here.
                command('xdotool', 'mousemove', '--window', window, '80', '104', 'click', '1')
                time.sleep(1)
                command('import', '-window', window, str(args.artifacts / f'{name}-preview.png'))
                first = captures / 'capture-1.png'
                command('xdotool', 'mousemove', '--window', window, '190', '104', 'click', '1')
                wait_for(first.exists, process)
                # Wait for the synchronous save handler to close the file.
                time.sleep(0.2)
                assert command('identify', '-format', '%m %wx%h', str(first)) == 'PNG 1280x800'
                # The far corner of the Xvfb desktop is black, outside the probe.
                pixel = command('convert', str(first), '-crop', '1x1+1200+750', '-depth', '8', 'txt:-')
                assert '#000000' in pixel, f'Unexpected background pixel: {pixel}'
                # Reject a blank buffer masquerading as a successful screenshot.
                mean = float(command('identify', '-format', '%[fx:mean]', str(first)))
                assert 0.05 < mean < 0.95, f'Expected both the window and desktop, got mean={mean}'
                digest = hashlib.sha256(first.read_bytes()).hexdigest()
                command('xdotool', 'mousemove', '--window', window, '190', '104', 'click', '1')
                second = captures / 'capture-2.png'
                wait_for(second.exists, process)
                time.sleep(0.2)
                assert first.read_bytes() == second.read_bytes(), 'Repeated save changed capture bytes'
                command('import', '-window', window, str(args.artifacts / f'{name}-saved.png'))
                print(f'{name}: PASS capture, preview, two non-overwriting PNG saves; 1280x800; SHA256 {digest}', flush=True)
                # Only disposable test output: make the destination invalid and
                # retain the resulting error state for visual inspection.
                first.unlink()
                second.unlink()
                captures.rmdir()
                captures.write_text('blocked destination')
                command('xdotool', 'mousemove', '--window', window, '190', '104', 'click', '1')
                time.sleep(0.2)
                command('import', '-window', window, str(args.artifacts / f'{name}-error.png'))
                assert captures.read_text() == 'blocked destination'
                captures.unlink()
                command('xdotool', 'mousemove', '--window', window, '190', '104', 'click', '1')
                wait_for(first.exists, process)
                time.sleep(0.2)
                assert hashlib.sha256(first.read_bytes()).hexdigest() == digest
                print(f'{name}: PASS retained capture and saved successfully after destination recovery', flush=True)
            finally:
                stop(process)


if __name__ == '__main__':
    main()
