#!/usr/bin/env python3
"""Exercise native History drag, Restore and confirmation on private X11 and disposable files.

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
    # Live hosts must never unbind the developer's real OS screenshot keys.
    env["CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER"] = "1"
    env["CAPTURES_NATIVE_TRACE"] = "1"
    env.pop("WAYLAND_DISPLAY", None)
    children = []
    with (output / "processes.log").open("w") as log:
        def spawn(command, announce=False):
            child = subprocess.Popen(command, env=env, stdout=subprocess.PIPE if announce else log, stderr=log)
            children.append(child)
            return child

        def run(*command):
            return subprocess.check_output(command, env=env, stderr=log, timeout=10)

        def previews():
            result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "^Captures Mini Preview$"],
                                    env=env, stderr=log, stdout=subprocess.PIPE, text=True, timeout=5)
            return result.stdout.split()

        def preview_at_height(height):
            visible = previews()
            assert len(visible) <= 1, "Restore duplicated the preview window"
            if not visible:
                return False
            window = visible[0]
            geometry = subprocess.run(["xdotool", "getwindowgeometry", "--shell", window],
                                      env=env, stderr=log, stdout=subprocess.PIPE, text=True, timeout=5)
            if geometry.returncode and window not in previews():
                return False  # The retiring Clear all viewport vanished during this query.
            geometry.check_returncode()
            actual = int(dict(line.split("=", 1) for line in geometry.stdout.splitlines())["HEIGHT"])
            return window if actual == height else False

        def wait(condition):
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                if result := condition():
                    return result
                time.sleep(.05)
            raise AssertionError("native history action did not settle")

        def begin_drag(window, x, y):
            run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
                "mousemove", "--sync", "--window", window, str(x), str(y), "sleep", ".6", "mousedown", "1",
                "sleep", ".6", "mousemove_relative", "--sync", "30", "0")

            def started():
                # Synchronize against the real OS source, not a fixed delay:
                # the worker/renderer may still be processing the gesture.
                run("xdotool", "mousemove", "--sync", "--window", window, str(x + 31), str(y),
                    "mousemove", "--sync", "--window", window, str(x + 30), str(y))
                owner = run("/usr/bin/python3", "-c",
                    "from Xlib import display; d = display.Display(); "
                    "owner = d.get_selection_owner(d.intern_atom('XdndSelection')); "
                    "print(owner.id if owner else 0)")
                return owner.decode().strip() == window

            wait(started)

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
                    neighbor = next((root / "second").iterdir())
                    run("convert", str(neighbor / "capture.png"), "-channel", "B", "-evaluate", "multiply", ".65",
                        "+channel", str(neighbor / "capture.png"))
                    shutil.copyfile(neighbor / "capture.png", neighbor / "preview.png")
                    shutil.move(str(neighbor), history)
                    write_history(root / "third")
                    third = next((root / "third").iterdir())
                    # A distinct oldest source makes Copy reject a stale old card,
                    # independently of the window-height assertion below.
                    run("convert", str(protected / "capture.png"), "-channel", "R", "-evaluate", "multiply", ".7",
                        "+channel", str(protected / "capture.png"))
                    shutil.copyfile(protected / "capture.png", protected / "preview.png")
                    shutil.move(str(third), history)
                    for preview_path in history.glob("*/preview.png"):
                        run("convert", str(preview_path), "-resize", "160x90", str(preview_path))
                    export = root / "Café capture.png"
                    shutil.copyfile(protected / "capture.png", export)
                    original_export = export.read_bytes()
                    metadata = protected / "metadata.json"
                    entry = json.loads(metadata.read_text())
                    entry["saved_path"] = str(export)
                    entry["size_bytes"] = len(original_export)
                    metadata.write_text(json.dumps(entry))
                    write_completed_settings(root / "settings.json")
                    app = spawn([str(binary), "--live", "--open-history", "--history-root", str(history),
                        "--settings-file", str(root / "settings.json"), "--appearance", appearance])
                    window = run("xdotool", "search", "--all", "--sync", "--onlyvisible", "--pid", str(app.pid), "--name", "^Capture History$").decode().splitlines()[0]
                    time.sleep(1)  # Font upload and asynchronous fixture decode.

                    def click(x, y, settle=True):
                        run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
                            "mousemove", "--sync", "--window", window, str(x - 1), str(y),
                            "mousemove_relative", "--sync", "1", "0", "sleep", ".12",
                            "mousedown", "1", "sleep", ".12", "mouseup", "1")
                        if settle:
                            time.sleep(.2)

                    def screenshot(name):
                        run("import", "-window", window, str(output / f"{prefix}-{name}.png"))

                    screenshot("populated")
                    assert not previews(), "a preview opened before Restore"
                    entries = sorted(history.glob("*/metadata.json"),
                                     key=lambda path: json.loads(path.read_text())["created_at"], reverse=True)
                    # Exercise the real OS drag source, not a synthetic editor
                    # import: both recovery-only and saved Unicode files.
                    for slot, source in enumerate(entries):
                        destination = output / f"{prefix}-history-drag-{slot}.bin"
                        receiver = spawn(["/usr/bin/python3", str(Path(__file__).with_name("x11_drag_receiver.py")),
                                          "--output", str(destination)], True)
                        receiver.stdout.readline()  # Mapped XDND receiver.
                        x = 90 + slot * 320
                        begin_drag(window, x, 280)
                        run("xdotool", "mousemove", "--sync", "700", "550", "sleep", ".3")
                        events = destination.with_suffix(".jsonl")
                        def negotiated():
                            # The renderer/worker may accept the gesture after
                            # the scripted pointer reaches the receiver. Keep
                            # delivering motion as a real held drag would.
                            run("xdotool", "mousemove", "--sync", "701", "550",
                                "mousemove", "--sync", "700", "550")
                            return events.exists() and '"position"' in events.read_text()
                        wait(negotiated)
                        run("xdotool", "mouseup", "1")
                        wait(destination.exists)
                        assert destination.read_bytes() == (source.parent / "capture.png").read_bytes(), "History drag transferred a thumbnail"
                        received = next(json.loads(line) for line in events.read_text().splitlines()
                                        if json.loads(line)["event"] == "received")
                        saved = json.loads(source.read_text()).get("saved_path")
                        if saved:
                            assert Path(received["path"]) == Path(saved), "saved export did not win"
                        assert not previews(), "History drag must not Restore"
                        assert len(list(history.glob("*/metadata.json"))) == 3, "History drag removed its source"
                        receiver.terminate(); receiver.wait(timeout=5)
                    # Cancel a repeat gesture and verify it neither opens an
                    # editor nor consumes History or pointer state.
                    begin_drag(window, 90, 280)
                    run("xdotool", "key", "Escape", "mouseup", "1")
                    time.sleep(.3)
                    assert not previews(), "cancelled History drag restored a preview"
                    screenshot("drag-retained")
                    click(254, 433)  # First card's Restore brings back a floating preview.
                    preview = wait(previews)[0]
                    time.sleep(.5)  # Arrival motion.
                    screenshot("restored")
                    run("import", "-window", preview, str(output / f"{prefix}-restored-preview.png"))
                    click(254, 433)  # Already showing: no duplicate window, still confirms.
                    time.sleep(.3)
                    assert len(previews()) == 1, "Restore duplicated the preview window"
                    assert len(list(history.glob("*/metadata.json"))) == 3, "Restore changed History"
                    # An individual Delete is not Clear all. Restore cancels
                    # only that ID's dust, keeping a different Delete's slot.
                    click(574, 433)
                    preview = wait(previews)[0]
                    time.sleep(1.1)
                    def preview_height():
                        return int(dict(line.split("=", 1) for line in run(
                            "xdotool", "getwindowgeometry", "--shell", preview).decode().splitlines())["HEIGHT"])
                    assert preview_height() == 424
                    preserved_restore = {path: path.read_bytes() for path in history.glob("*/*")
                                         if path.is_file() and not path.parent.name.startswith(".")}
                    run("xdotool", "mousemove", "--sync", "--window", preview, "50", "50", "sleep", ".12",
                        "mousedown", "1", "sleep", ".12", "mouseup", "1")
                    deleted_at = time.monotonic()
                    deleted_id = json.loads(entries[0].read_text())["id"]
                    wait(lambda: any('"name":"preview-exit"' in line and deleted_id in line
                                    for line in (output / "processes.log").read_text().splitlines()))
                    run("xdotool", "mousemove", "--sync", "--window", preview, "50", "234", "sleep", ".12",
                        "mousedown", "1", "sleep", ".12", "mouseup", "1")
                    neighbor_id = json.loads(entries[1].read_text())["id"]
                    wait(lambda: any('"name":"preview-exit"' in line and neighbor_id in line
                                    for line in (output / "processes.log").read_text().splitlines()))
                    click(254, 433)
                    preview = wait(previews)[0]
                    run("xdotool", "mousemove", "--sync", "640", "440")
                    # The restored card is the newest/front slot (bottom).
                    # Its blue interior has no text/chrome or resized edges.
                    # A stale old slot paints the differently colored neighbor.
                    expected_pixel = run("convert", str(entries[0].parent / "capture.png"),
                                         "-crop", "1x1+100+180", "-depth", "8", "rgb:-")
                    wait(lambda: run("import", "-window", preview, "-crop", "1x1+100+292",
                                     "-depth", "8", "rgb:-") == expected_pixel)
                    elapsed = time.monotonic() - deleted_at
                    run("import", "-window", preview, str(output / f"{prefix}-delete-restored-held.png"))
                    assert elapsed < 2.4, f"Restore waited for the original dust instead of cancelling it: {elapsed:.3f}s"
                    wait(lambda: preview_height() == 240)
                    run("import", "-window", preview, str(output / f"{prefix}-delete-restored-settled.png"))
                    assert all(path.read_bytes() == data for path, data in preserved_restore.items())
                    print(f"PASS {prefix}: same-ID Restore cancels dust in {elapsed:.3f}s; unrelated Delete settles; History unchanged", flush=True)
                    # Drop the first History capture into the second capture's
                    # editor. The first already has a mini preview: this drag
                    # must not run that preview's dismissal callback.
                    click(420, 280)
                    editor = run("xdotool", "search", "--all", "--sync", "--onlyvisible", "--pid", str(app.pid),
                                 "--name", "^Captures Screenshot Editor$").decode().splitlines()[0]
                    run("xdotool", "windowmove", window, "0", "30", "windowmove", editor, "350", "40")
                    run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window)
                    # Keep the source focused but expose the overlapping drop
                    # target. Activating History otherwise covers the editor.
                    run("xdotool", "windowraise", editor)
                    time.sleep(1)
                    preview_count = len(previews())
                    draft_id = json.loads(entries[1].read_text())["id"]
                    draft = root / "editor-drafts" / draft_id / "manifest.json"
                    begin_drag(window, 90, 280)
                    run("xdotool", "windowraise", editor,
                        "mousemove", "--sync", "--window", editor, "300", "300", "sleep", ".6",
                        "mousemove_relative", "--sync", "1", "0", "sleep", ".3", "mouseup", "1")
                    wait(lambda: draft.exists() and len(json.loads(draft.read_text())["document"]["elements"]) == 2)
                    assert len(previews()) == preview_count, "History-to-editor drag dismissed a mini preview"
                    assert len(list(history.glob("*/metadata.json"))) == 3, "editor drop republished History"
                    run("import", "-window", editor, str(output / f"{prefix}-history-editor-drop.png"))
                    run("xdotool", "windowactivate", "--sync", editor, "key", "alt+F4")
                    time.sleep(.5)
                    # Edit restored the second card, so Clear all has two
                    # visible cards. Restore a different third during its hold.
                    preview = wait(previews)[0]
                    wait(lambda: int(dict(line.split("=", 1) for line in run(
                        "xdotool", "getwindowgeometry", "--shell", preview).decode().splitlines())["HEIGHT"]) == 424)
                    time.sleep(1.2)
                    preserved = {path: path.read_bytes() for path in history.glob("*/*")
                                 if path.is_file() and not path.parent.name.startswith(".")}
                    previous_exits = (output / "processes.log").read_text().count('"settles":false')
                    run("xdotool", "mousemove", "--sync", "--window", preview, "42", "398", "sleep", ".12",
                        "mousedown", "1", "sleep", ".04", "mouseup", "1")
                    cleared_at = time.monotonic()
                    wait(lambda: (output / "processes.log").read_text().count('"settles":false') >= previous_exits + 2)
                    click(894, 433, settle=False)
                    # Restore releases Clear all's exit cards before decoding
                    # its new image. The viewport can retire and reopen with a
                    # new XID, so locate the ready window rather than polling
                    # the outgoing one. Keep the same hold and pixel checks.
                    preview = wait(lambda: preview_at_height(240))
                    elapsed = time.monotonic() - cleared_at
                    height = int(dict(line.split("=", 1) for line in run(
                        "xdotool", "getwindowgeometry", "--shell", preview).decode().splitlines())["HEIGHT"])
                    assert elapsed < .8, f"Restore missed the Clear all hold: {elapsed:.3f}s"
                    run("import", "-window", preview, str(output / f"{prefix}-clear-arrival.png"))
                    assert height == 240, f"Restore inherited Clear all held slots: {height}px, not 240px"
                    assert all(path.read_bytes() == data for path, data in preserved.items()), "Clear all or Restore changed History"
                    expected = run("convert", str(entries[2].parent / "capture.png"), "-depth", "8", "rgb:-")
                    run("xdotool", "mousemove", "--sync", "--window", preview, "169", "89",
                        "mousemove_relative", "--sync", "1", "0", "sleep", ".12",
                        "mousedown", "1", "sleep", ".12", "mouseup", "1")

                    def copied_restored():
                        value = subprocess.run(["xclip", "-selection", "clipboard", "-t", "image/png", "-o"],
                                               env=env, capture_output=True, timeout=5)
                        if value.returncode != 0:
                            return False
                        pixels = subprocess.run(["convert", "png:-", "-depth", "8", "rgb:-"],
                                                env=env, input=value.stdout, capture_output=True, timeout=5)
                        return pixels.returncode == 0 and pixels.stdout == expected

                    wait(copied_restored)
                    print(f"PASS {prefix}: Restore during Clear all opens one actionable 240px card in {elapsed:.3f}s; all source files unchanged", flush=True)
                    click(948, 79)  # Delete all arms "Delete all forever" in place.
                    screenshot("confirmation")
                    assert len(list(history.glob("*/metadata.json"))) == 3, "arming confirmation deleted files"
                    run("xdotool", "key", "Escape")
                    time.sleep(.2)
                    assert len(list(history.glob("*/metadata.json"))) == 3, "Escape deleted files"
                    click(948, 79)
                    click(820, 79)  # Explicit Cancel.
                    assert len(list(history.glob("*/metadata.json"))) == 3, "Cancel deleted files"
                    screenshot("cancelled")
                    if fail_partway:
                        protected.chmod(0o555)
                    click(948, 79)
                    click(948, 79)  # Explicit Delete all forever.
                    if fail_partway:
                        try:
                            wait(lambda: len(list(history.glob("*/metadata.json"))) == 1)
                            time.sleep(.3)
                            screenshot("error")
                            assert (protected / "capture.png").is_file(), "failed item disappeared"
                        finally:
                            protected.chmod(0o755)
                        click(948, 79)
                        click(948, 79)  # Retry after restoring write access.
                    wait(lambda: not list(history.glob("*/metadata.json")))
                    wait(lambda: not previews())  # Clearing History drops restored previews.
                    time.sleep(.3)
                    screenshot("empty")
                    assert export.read_bytes() == original_export, "export changed during clear"
                    run("xdotool", "key", "alt+F4")
                    assert app.wait(timeout=10) == 0
                    assert not list(history.glob("*/metadata.json"))
                    print(f"PASS {prefix}: direct History COPY (saved/recovery bytes), editor layer drop with preview retained, cancelled drag, Restore, confirmation, Escape/Cancel, clear, export preserved, clean exit", flush=True)
            (output / "result.json").write_text(json.dumps({"passed": True, "appearances": 2, "partialFailureAndRetry": True, "restore": True,
                "historyDrag": True, "historyEditorDrop": True, "restoreDuringClearAll": True, "restoreDuringDelete": True,
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
