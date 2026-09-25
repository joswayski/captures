#!/usr/bin/env python3
"""Exercise native history confirmation on a private X11 desktop and disposable files.

No screen-capture permission or session bypass. Requires Xvfb, Openbox, xdotool
and ImageMagick. This is software-rendered UI evidence, not hardware acceptance.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

from history_fixture import write_completed_settings, write_history


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    assert os.geteuid() != 0, "run unprivileged so the deletion-permission test is meaningful"
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = {**os.environ, "WGPU_BACKEND": "gl"}
    env.pop("WAYLAND_DISPLAY", None)
    children = []
    with (output / "processes.log").open("w") as log:
        def spawn(command, announce=False):
            child = subprocess.Popen(command, env=env, stdout=subprocess.PIPE if announce else log, stderr=log)
            children.append(child)
            return child

        def run(*command):
            return subprocess.check_output(command, env=env, stderr=log, timeout=10)

        def wait(condition):
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                if result := condition():
                    return result
                time.sleep(.05)
            raise AssertionError("native history action did not settle")

        try:
            xserver = spawn(["Xvfb", "-displayfd", "1", "-screen", "0", "1280x900x24", "-nolisten", "tcp"], True)
            env["DISPLAY"] = ":" + xserver.stdout.readline().decode().strip()
            spawn(["openbox", "--sm-disable"])
            time.sleep(.5)
            for appearance, fail_partway in [("dark", False), ("light", False), ("dark", True)]:
                prefix = appearance + ("-partial" if fail_partway else "")
                with tempfile.TemporaryDirectory(prefix="hist-") as temporary:
                    root = Path(temporary)
                    history = root / "history"
                    write_history(history)
                    protected = next(history.iterdir())  # Older item is deleted last.
                    write_history(root / "second")
                    shutil.move(str(next((root / "second").iterdir())), history)
                    export = root / "export.png"
                    shutil.copyfile(next(history.glob("*/capture.png")), export)
                    original_export = export.read_bytes()
                    metadata = next(history.glob("*/metadata.json"))
                    entry = json.loads(metadata.read_text())
                    entry["saved_path"] = str(export)
                    metadata.write_text(json.dumps(entry))
                    write_completed_settings(root / "settings.json")
                    app = spawn([str(binary), "--live", "--history-root", str(history),
                        "--settings-file", str(root / "settings.json"), "--appearance", appearance])
                    window = run("xdotool", "search", "--sync", "--onlyvisible", "--pid", str(app.pid), "--name", "^Captures$").decode().splitlines()[0]
                    time.sleep(1)  # Font upload and asynchronous fixture decode.

                    def click(x, y):
                        run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
                            "mousemove", "--sync", "--window", window, str(x - 1), str(y),
                            "mousemove_relative", "--sync", "1", "0", "click", "1")
                        time.sleep(.2)

                    def screenshot(name):
                        run("import", "-window", window, str(output / f"{prefix}-{name}.png"))

                    screenshot("populated")
                    click(63, 336)
                    screenshot("confirmation")
                    assert len(list(history.glob("*/metadata.json"))) == 2, "opening confirmation deleted files"
                    run("xdotool", "key", "Escape")
                    time.sleep(.2)
                    assert len(list(history.glob("*/metadata.json"))) == 2, "Escape deleted files"
                    click(63, 336)
                    click(46, 460)  # Explicit Cancel.
                    assert len(list(history.glob("*/metadata.json"))) == 2, "Cancel deleted files"
                    screenshot("cancelled")
                    if fail_partway:
                        protected.chmod(0o555)
                    click(63, 336)
                    click(124, 460)  # Explicit Delete all.
                    if fail_partway:
                        try:
                            wait(lambda: len(list(history.glob("*/metadata.json"))) == 1)
                            time.sleep(.3)
                            screenshot("error")
                            assert (protected / "capture.png").is_file(), "failed item disappeared"
                        finally:
                            protected.chmod(0o755)
                        click(63, 336)
                        click(124, 460)  # Retry after restoring write access.
                    wait(lambda: not list(history.glob("*/metadata.json")))
                    time.sleep(.3)
                    screenshot("empty")
                    assert export.read_bytes() == original_export, "export changed during clear"
                    run("xdotool", "key", "alt+F4")
                    assert app.wait(timeout=10) == 0
                    assert not list(history.glob("*/metadata.json"))
                    print(f"PASS {prefix}: confirmation, Escape/Cancel, clear, export preserved, clean exit", flush=True)
            (output / "result.json").write_text(json.dumps({"passed": True, "appearances": 2, "partialFailureAndRetry": True,
                "scope": "Disposable native history on private X11/software GL; not hardware or other OS acceptance."}, indent=2))
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()


if __name__ == "__main__":
    main()
