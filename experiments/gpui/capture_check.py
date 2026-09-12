#!/usr/bin/env python3
"""Real GPUI/X11 capture checks against native_desktop.py's disposable session.

No session gate bypass, mocked capture pixels, or browser surrogate. Never run
against a personal desktop: --lab must contain the isolated lab environment.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from latency import X11, patch_has_yellow


SPEC = importlib.util.spec_from_file_location("gpui_benchmark", Path(__file__).with_name("benchmark.py"))
benchmark = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(benchmark)


def run(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout


def eventually(check, description, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.03)
    raise AssertionError(f"Timed out: {description}")


def lock(value):
    run("dbus-send", "--session", "--type=method_call", "--print-reply",
        "--dest=org.freedesktop.ScreenSaver", "/org/freedesktop/ScreenSaver",
        "org.freedesktop.ScreenSaver.SetActive", f"boolean:{str(value).lower()}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lab", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    args = parser.parse_args()
    os.environ.update(json.loads((args.lab / "environment.json").read_text()))
    args.artifacts.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="gpui-capture-check-") as directory:
        profile = Path(directory)
        env = benchmark.profile_environment(profile, "dark")
        # Exercise the normal XDG fallback, not only an explicit override: all
        # surfaces must agree on this root for history and draft recovery.
        env.pop("CAPTURES_GPUI_DATA")
        data = profile / "data/captures-gpui"
        data.mkdir(parents=True)
        (data / "settings.json").write_text(json.dumps({
            "appearance": "dark", "freeze_screen": True,
            "show_cursor_in_screenshots": False, "auto_copy_to_clipboard": False,
            "output_directory": str(profile / "saved-output"),
        }))
        run("xdotool", "mousemove", "0", "0")
        run("import", "-window", "root", str(profile / "before.png"))
        with (profile / "app.log").open("w+") as log:
            process = subprocess.Popen([str(args.binary.resolve()), "--capture"], env=env,
                                       stdout=log, stderr=log, start_new_session=True)
            def visible(title):
                return benchmark.find_window(f"^Captures GPUI {title}$", process.pid)

            def command(*arguments):
                with socket.socket(socket.AF_UNIX) as connection:
                    connection.connect(str(data / "instance.sock"))
                    connection.sendall(json.dumps(arguments).encode())

            try:
                eventually(lambda: visible("Select target"), "selector opens")
                pixels = X11()
                eventually(lambda: patch_has_yellow(pixels.patch((1205, 910, 3, 3))), "Capture button actually paints")
                run("xdotool", "mousemove", "320", "180", "mousedown", "1", "sleep", ".08",
                    "mousemove", "1050", "630", "sleep", ".1", "mouseup", "1")
                eventually(lambda: patch_has_yellow(pixels.patch()), "selection border actually paints")
                pixels.close()
                run("import", "-window", "root", str(args.artifacts / "gpui-selector.png"))
                run("xdotool", "key", "Return")
                capture = eventually(lambda: next((data / "unsaved").glob("*.png"), None), "private PNG publication")
                assert benchmark.png_dimensions(capture) == (730, 450)
                run("convert", str(profile / "before.png"), "-crop", "730x450+320+180", "+repage", str(profile / "expected.png"))
                run("compare", "-metric", "AE", str(profile / "expected.png"), str(capture), "null:")
                eventually(lambda: (data / "history.json").exists(), "history index")
                entries = json.loads((data / "history.json").read_text())
                assert len(entries) == 1, entries
                assert not (profile / "saved-output").exists(), "capture must not auto-save to user output"
                eventually(lambda: visible("Previews"), "preview after capture")
                time.sleep(2)
                run("import", "-window", "root", str(args.artifacts / "gpui-capture-preview.png"))
                print("PASS: real 730×450 capture matches source pixels exactly; private history, no automatic user-file save; preview opens", flush=True)

                command("--capture")
                eventually(lambda: visible("Select target"), "second selector via IPC")
                lock(True)
                eventually(lambda: not visible("Select target") and not visible("Previews"), "lock closes selector and hides preview")
                assert process.poll() is None, "tray process exited when last visible window closed"
                assert len(list((data / "unsaved").glob("*.png"))) == 1
                lock(False)
                command("--capture")
                eventually(lambda: visible("Select target"), "capture after unlock")
                run("xdotool", "key", "Escape")
                eventually(lambda: not visible("Select target"), "Escape cancellation")
                assert process.poll() is None
                assert len(list((data / "unsaved").glob("*.png"))) == 1
                print("PASS: IPC reopening, fail-closed lock cancellation, Escape, and zero-visible-window tray lifetime", flush=True)

                invalid = profile / "invalid.png"
                invalid.write_bytes(b"not an image; never replace with a placeholder")
                command("--open", str(invalid))
                editor = eventually(lambda: visible("Image"), "asynchronous editor error surface")
                time.sleep(1)
                # The old placeholder editor exposed Save even after decode
                # failed. The error surface must have no editing/save controls.
                run("xdotool", "mousemove", "--window", editor, "1238", "736", "click", "1")
                time.sleep(.3)
                assert invalid.read_bytes() == b"not an image; never replace with a placeholder"
                run("import", "-window", editor, str(args.artifacts / "gpui-image-open-error.png"))
                print("PASS: failed asynchronous image open cannot overwrite the source with a placeholder", flush=True)
            except Exception:
                log.flush()
                log.seek(0)
                print(log.read())
                raise
            finally:
                lock(False)
                benchmark.stop(process)


if __name__ == "__main__":
    main()
