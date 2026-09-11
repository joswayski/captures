#!/usr/bin/python3
"""Focused GTK/AT-SPI history action check for the disposable native lab."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

import pyatspi

from benchmark import stop
from native_check import click, cmd, find, wait


def checked(name):
    node = find(name, frame="Captures — History")
    return node and node.getState().contains(pyatspi.STATE_CHECKED)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lab", type=Path, required=True)
    args = parser.parse_args()
    os.environ.update(json.loads((args.lab / "environment.json").read_text()))
    binary = Path(__file__).resolve().parent / "target/release/captures-linux-native"
    with tempfile.TemporaryDirectory(prefix="captures-history-") as temp:
        profile = Path(temp)
        output = profile / "captures"
        output.mkdir()
        first = output / "history-first.png"
        second = output / "history-second.png"
        shutil.copy2(args.lab / "fixture.png", first)
        shutil.copy2(args.lab / "fixture.png", second)
        stamp = int(time.time() * 1000)
        (profile / "history.json").write_text(json.dumps([
            {"path": str(first), "saved_ms": stamp},
            {"path": str(second), "saved_ms": stamp - 1},
        ]))
        (profile / "settings.json").write_text(json.dumps({"output_directory": str(output)}))
        env = dict(os.environ, CAPTURES_NATIVE_DATA=str(profile), XDG_CONFIG_HOME=str(profile / "config"))
        process = subprocess.Popen([str(binary), "--history"], env=env, start_new_session=True)
        try:
            wait(lambda: find("Captures — History", "frame"))
            for name in ("Restore history-first.png", "Delete history-first.png from History", "Delete all captures", "All 2", "Screenshots 2"):
                wait(lambda name=name: find(name, frame="Captures — History"))
            edit = find('Edit history-first.png', frame='Captures — History').queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            trash = find('Delete history-first.png from History', frame='Captures — History').queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            cmd('xdotool', 'mousemove', 0, 0)
            time.sleep(.3)
            cmd('import', '-window', 'root', profile/'idle.png')
            pixel = cmd('convert', profile/'idle.png', '-format', f'%[hex:p{{{edit.x+10},{edit.y+10}}}]', 'info:')
            assert pixel[:6].lower() == 'ffca28', ('Edit lost the primary accent', pixel)
            assert trash.width == trash.height == 32, trash
            cmd('xdotool', 'mousemove', trash.x+16, trash.y+16)
            time.sleep(.3)
            cmd('import', '-window', 'root', profile/'hover.png')
            pixel = cmd('convert', profile/'hover.png', '-format', f'%[hex:p{{{trash.x+7},{trash.y+7}}}]', 'info:')
            assert pixel[:6].lower() == 'ef4650', ('Trash lost its destructive hover', pixel)
            assert checked("All 2")
            click("All 2", "Captures — History", pointer=True)
            wait(lambda: checked("All 2"))
            click("Restore history-first.png", "Captures — History")
            wait(lambda: find("Expand preview", frame="Captures — Mini previews"))
            assert len(json.loads((profile / "history.json").read_text())) == 2
            click("Delete history-first.png from History", "Captures — History")
            wait(lambda: find("Confirm removal of history-first.png from History", frame="Captures — History"))
            click("Confirm removal of history-first.png from History", "Captures — History")
            wait(lambda: len(json.loads((profile / "history.json").read_text())) == 1)
            wait(lambda: find("All 1", frame="Captures — History"))
            assert first.read_bytes() and second.read_bytes()
            click("Delete all captures", "Captures — History", pointer=True)
            wait(lambda: find("Confirm delete all captures", frame="Captures — History"))
            click("Confirm delete all captures", "Captures — History", pointer=True)
            wait(lambda: json.loads((profile / "history.json").read_text()) == [])
            assert first.is_file() and second.is_file()
            wait(lambda: find("No captures yet", frame="Captures — History"))
            print("PASS native history remove/clear actions preserve saved files")
        finally:
            stop(process)


if __name__ == "__main__":
    main()
