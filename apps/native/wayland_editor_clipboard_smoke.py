#!/usr/bin/python3
"""Native editor clipboard ownership and image input on private headless Sway.

Reuses the real-window/virtual-input host fixture. DISPLAY is unset: no X11
fallback, installed profile, global clipboard or physical acceptance is involved.
"""
import errno
import json
import os
from pathlib import Path
import select
import subprocess
import time

from gi.repository import GLib

from instance_smoke import png
from wayland_drag_smoke import Output
from wayland_keyboard_smoke import KEYS
from wayland_native_capture_smoke import wait, windows
from wayland_screenshot_smoke import ready, stop
from wayland_shortcuts_host_smoke import HEIGHT, WIDTH, main
from x11_editor_smoke import FileChooser


def cases(binary, pointer, root, env, bus):
    assert "DISPLAY" not in env
    chooser = FileChooser(bus, [])
    keyboard = subprocess.Popen([pointer.args[0], "keyboard"], env=env,
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    ready(keyboard)
    try:
        for appearance in ("dark", "light"):
            profile = root / appearance
            profile.mkdir()
            source = profile / "Source.png"
            image = profile / "Clipboard image.png"
            second = profile / "Second.BMP"
            for path, size, color in ((source, "640x360", "#286ea6"),
                                      (image, "90x43", "#d53e55"),
                                      (second, "31x17", "#268752")):
                # Untagged sRGB samples: ImageMagick's default gAMA-only PNG
                # describes gamma 2.2, not the exact sRGB values asserted below.
                subprocess.run(["convert", "-size", size, "xc:" + color, "-strip", str(path)], check=True)
            original = source.read_bytes()
            settings = profile / "settings.json"
            settings.write_text(json.dumps({"settings_schema_version": 5, "onboarding_completed": True,
                "appearance": appearance, "launch_at_login": False, "show_mini_previews": False,
                "auto_copy_to_clipboard": False, "output_directory": str(profile / "exports"),
                "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
                "display_shortcut": "Ctrl+Shift+F9"}))
            history = profile / "history"
            host_log = profile / "host.log"
            with host_log.open("w") as log:
                app = subprocess.Popen([binary, "--live", "--open-media", str(source),
                    "--settings-file", str(settings), "--history-root", str(history),
                    "--quit-after", "180"], env=env, stdout=log, stderr=log)
                try:
                    def window():
                        assert app.poll() is None, host_log.read_text()
                        return next((node for node in windows(env) if node["pid"] == app.pid
                                     and node["name"].startswith("Captures Screenshot Editor")), None)

                    node = wait(window, "screenshot editor")
                    subprocess.run(["swaymsg", f'[con_id={node["id"]}] floating enable, '
                        'resize set 1100 800, move position 80 60, focus'],
                        env=env, check=True, stdout=subprocess.DEVNULL)
                    wait(lambda: window()["focused"] and window()["rect"]["width"] == 1100,
                         "editor input focus")
                    artifact = wait(lambda: next(iter(history.glob("*/metadata.json")), None), "artifact").parent
                    draft = profile / "editor-drafts" / artifact.name / "manifest.json"

                    def key(name, modifiers="none", delay=.2):
                        code = KEYS[name]
                        keyboard.stdin.write(f"{code} {modifiers}\n".encode())
                        keyboard.stdin.flush()
                        assert select.select([keyboard.stdout], [], [], 5)[0], "keyboard stalled"
                        assert keyboard.stdout.readline().strip() == f"KEYED {code} {modifiers}".encode()
                        time.sleep(delay)

                    def click(x, y):
                        rect = window()["rect"]
                        pointer.stdin.write(f'{rect["x"] + x} {rect["y"] + y} {WIDTH} {HEIGHT}\n'.encode())
                        pointer.stdin.flush()
                        assert select.select([pointer.stdout], [], [], 5)[0], "pointer stalled"
                        assert pointer.stdout.readline().strip() == b"CLICKED"

                    def publish(data, mime):
                        subprocess.run(["wl-copy", "--type", mime], input=data,
                                       env=env, check=True, timeout=5)
                        time.sleep(.2)

                    def layers():
                        return json.loads(draft.read_text())["document"]["elements"] if draft.exists() else []

                    def count(expected):
                        return wait(lambda: values if len(values := layers()) == expected else None,
                                    f"{expected} draft layers")

                    def shot(name):
                        rect = window()["rect"]
                        subprocess.run(["grim", "-g", f'{rect["x"]},{rect["y"]} '
                            f'{rect["width"]}x{rect["height"]}', str(profile / f"{name}.png")],
                            env=env, check=True, timeout=5)

                    def status_pixels():
                        rect = window()["rect"]
                        return subprocess.check_output(["grim", "-t", "ppm", "-g",
                            f'{rect["x"] + rect["width"] - 440},{rect["y"] + rect["height"] - 80} 440x80', "-"],
                            env=env, timeout=5)

                    # Move across neighboring rail controls before focusing blank
                    # rail space. All paste commands below cross the compositor.
                    click(28, 430)
                    replacement = profile / "Replacement.png"
                    subprocess.run(["convert", str(second), str(replacement)], check=True)
                    owner = subprocess.Popen([pointer.args[0], "clipboard-replace", str(image),
                                              str(replacement)], env=env,
                                             stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                    output = Output(owner)
                    try:
                        output.wait("READY")
                        key("v", "ctrl")
                        output.wait("REPLACED")
                        frozen = count(2)[-1]
                        assert (frozen["naturalWidth"], frozen["naturalHeight"]) == (90, 43), \
                            "PNG fallback read the replacement clipboard instead of the accepted offer"
                        asset = draft.parent / "assets" / (frozen["src"].split(":", 1)[1] + ".png")
                        assert subprocess.check_output(["convert", str(asset), "-crop", "1x1+89+42",
                                                        "-depth", "8", "rgba:-"]) == bytes((213, 62, 85, 255))
                        shot("clipboard-offer-retained")
                        key("v", "ctrl")
                        replaced = count(3)[-1]
                        assert (replaced["naturalWidth"], replaced["naturalHeight"]) == (31, 17)
                        key("z", "ctrl")
                        assert count(2)[-1] == frozen
                        key("z", "ctrl")
                        wait(lambda: not draft.exists(), "undo both clipboard offers")
                    finally:
                        (profile / "clipboard-replacement-protocol.json").write_text(
                            json.dumps(output.seen, indent=2) + "\n")
                        stop(owner)
                    publish(image.read_bytes(), "image/png")
                    key("v", "ctrl")
                    imported = count(2)[-1]
                    assert (imported["naturalWidth"], imported["naturalHeight"], imported["y"]) == (90, 43, 360)
                    shot("clipboard-image-imported")
                    for _ in range(3):
                        key("v", "ctrl", delay=0)
                    rapid = count(5)
                    assert all((layer["naturalWidth"], layer["naturalHeight"]) == (90, 43)
                               for layer in rapid[2:])
                    shot("clipboard-rapid-external-pastes")
                    for size in (4, 3, 2):
                        key("z", "ctrl")
                        count(size)
                    click(window()["rect"]["width"] - 200, 149)
                    key("c", "ctrl")
                    def marker_types():
                        types = subprocess.check_output(["wl-paste", "--list-types"], env=env, timeout=5).splitlines()
                        return types if any(mime.startswith(b"application/x-captures-editor-layer-") for mime in types) else None
                    types = wait(marker_types, "private clipboard MIME offer")
                    assert len(types) == 1 and types[0].startswith(b"application/x-captures-editor-layer-"), types
                    assert subprocess.check_output(["wl-paste", "--type", types[0].decode(), "--no-newline"],
                                                   env=env, timeout=5) == types[0]
                    key("v", "ctrl")
                    copied = count(3)[-1]
                    assert (copied["x"], copied["y"]) == (imported["x"] + 24, imported["y"] + 24)
                    time.sleep(.5)
                    unchanged_status = status_pixels()
                    unchanged_layers = layers()
                    for name, data, mime in [("text", b"ordinary filename", "text/plain"),
                                             ("html", b'<img src="https://example.invalid/no-fetch.png">', "text/html"),
                                             ("empty", b"", "text/plain")]:
                        publish(data, mime)
                        key("v", "ctrl")
                        time.sleep(.7)
                        shot(f"clipboard-{name}-ignored")
                        assert layers() == unchanged_layers, "unsupported input pasted the retained snapshot"
                        assert status_pixels() == unchanged_status, f"{name}-only Paste changed the export status"
                    publish(b"not a PNG", "image/png")
                    key("v", "ctrl")
                    time.sleep(.7)
                    shot("clipboard-invalid-image-error")
                    assert layers() == unchanged_layers, "failed image decode changed the document"
                    assert status_pixels() != unchanged_status, "corrupt image must report an error"
                    publish(image.read_bytes(), "image/png")
                    key("v", "ctrl")
                    external = count(4)[-1]
                    assert external["y"] == copied["y"] + copied["height"]
                    key("z", "ctrl")
                    count(3)
                    key("z", "ctrl+shift")
                    assert count(4)[-1] == external
                    publish((image.as_uri() + "\r\n" + second.as_uri() + "\r\n").encode(), "text/uri-list")
                    key("v", "ctrl")
                    pasted_files = count(6)
                    assert [layer["name"] for layer in pasted_files[-2:]] == [image.name, second.name]
                    shot("clipboard-batch-pasted")
                    key("z", "ctrl")
                    unchanged_layers = count(4)
                    shot("clipboard-batch-undone")
                    corrupt = profile / "Corrupt.png"
                    corrupt.write_bytes(b"not an image")
                    tall = profile / "Tall.png"
                    tall.write_bytes(png(1, 16_384))
                    for name, invalid in (("decode", corrupt), ("render", tall)):
                        publish(b"ordinary text", "text/plain")
                        key("v", "ctrl")
                        time.sleep(.7)
                        unchanged_status = status_pixels()
                        publish((image.as_uri() + "\r\n" + invalid.as_uri() + "\r\n").encode(), "text/uri-list")
                        key("v", "ctrl")
                        time.sleep(.8)
                        shot(f"clipboard-batch-{name}-error")
                        assert layers() == unchanged_layers, f"late {name} failure imported a partial batch"
                        assert status_pixels() != unchanged_status, f"late {name} failure must report an error"
                    key("z", "ctrl+shift")
                    assert count(6) == pasted_files, "failed batch must preserve Redo"
                    key("z", "ctrl")
                    count(4)
                    publish((image.as_uri() + "\r\n" + second.as_uri() + "\r\n").encode(), "text/uri-list")
                    key("v", "ctrl")
                    assert [layer["name"] for layer in count(6)[-2:]] == [image.name, second.name]
                    shot("clipboard-batch-retried")
                    key("c", "ctrl")
                    wait(marker_types, "clipboard claim before cancellation")
                    time.sleep(.5)
                    unchanged_status = status_pixels()
                    subprocess.run(["wl-copy", "--clear"], env=env, check=True, timeout=5)
                    key("v", "ctrl")
                    time.sleep(.7)
                    assert len(layers()) == 6, "cleared selection must cancel snapshot authorization"
                    assert status_pixels() == unchanged_status, "cleared clipboard Paste changed the status"
                    key("c", "ctrl")
                    key("v", "ctrl")
                    count(7)
                    shot("clipboard-reclaimed-after-cancellation")
                    # Check actual owned pixels, not only layer metadata.
                    asset = draft.parent / "assets" / (imported["src"].split(":", 1)[1] + ".png")
                    pixel = subprocess.check_output(["convert", str(asset), "-crop", "1x1+89+42",
                                                     "-depth", "8", "rgba:-"])
                    assert pixel == bytes((213, 62, 85, 255)), pixel
                    assert source.read_bytes() == original

                    def pick(paths):
                        chooser.selected = paths
                        click(window()["rect"]["width"] - 70, 26)
                        wait(lambda: chooser.pending, "native multi-file picker request")
                        assert chooser.calls[-1][1].get("multiple", False)
                        GLib.idle_add(chooser.respond, False)
                        time.sleep(1.5)

                    # The real rfd portal transport receives an isolated chooser
                    # response; no physical file dialog or X11 input is involved.
                    pick([image, second])
                    picked = count(9)
                    assert [layer["name"] for layer in picked[-2:]] == [image.name, second.name]
                    shot("picker-batch-imported")
                    click(28, 430)
                    key("z", "ctrl")
                    unchanged_layers = count(7)
                    shot("picker-batch-undone")
                    for name, invalid in [("decode", corrupt), ("render", tall)]:
                        pick([image, invalid])
                        assert layers() == unchanged_layers, f"late picker {name} failure published partial layers"
                        shot(f"picker-batch-{name}-error")
                    click(28, 430)
                    key("z", "ctrl+shift")
                    assert count(9) == picked, "failed picker batches must preserve Redo"
                    key("z", "ctrl")
                    count(7)
                    pick([image, second])
                    assert [layer["name"] for layer in count(9)[-2:]] == [image.name, second.name]
                    shot("picker-batch-retried")

                    # Restore the original canvas before exercising a separate
                    # GTK client's core Wayland drag offer (not clipboard paste).
                    click(28, 430)
                    while len(layers()) > 1:
                        previous_count = len(layers())
                        key("z", "ctrl")
                        wait(lambda: len(layers()) < previous_count, "undo before external drop")
                    # Undo back to the untouched original removes the draft;
                    # the rendered original still counts as one UI layer.
                    assert not draft.exists(), "undo must restore the untouched original"
                    unchanged_layers = layers()

                    def drop(paths, name, options=(), destination="accept", move_after=False,
                             during_receive=None, settle=True):
                        client = subprocess.Popen(["/usr/bin/python3",
                            str(Path(__file__).with_name("wayland_drag_source.py")),
                            *options, *map(str, paths)], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                        output = Output(client)
                        try:
                            output.wait("READY")
                            source_window = wait(lambda: next((node for node in windows(env)
                                if node["pid"] == client.pid), None), "external GTK drag source")
                            subprocess.run(["swaymsg", f'[con_id={source_window["id"]}] '
                                'resize set 200 150, move position 160 160, focus'],
                                env=env, check=True, stdout=subprocess.DEVNULL)
                            time.sleep(.3)
                            # The existing probe's normalized drag moves from
                            # (256,270) in GTK to (811,270) on this 1280x900 output.
                            subprocess.run([pointer.args[0], "inject", destination],
                                           env=env, check=True, timeout=5)
                            if move_after:
                                click(28, 430)
                            if during_receive:
                                during_receive()
                            output.wait("STARTED")
                            sent = output.wait("SENT ")
                            assert json.loads(sent.removeprefix("SENT ")) == [path.as_uri() for path in paths]
                            output.wait("FINISHED")
                            (profile / f"{name}-protocol.json").write_text(json.dumps(output.seen, indent=2) + "\n")
                        finally:
                            stop(client)
                        if settle:
                            click(28, 430)
                            time.sleep(.8)

                    # Hold the real decoder on a FIFO, then complete a picker
                    # before a later drop. Completion order must survive a busy
                    # renderer; every accepted batch retains its own Undo.
                    chooser.selected = [second]
                    click(window()["rect"]["width"] - 70, 26)
                    wait(lambda: chooser.pending, "picker before held import")
                    gate = profile / "Held.png"
                    os.mkfifo(gate)
                    writer = None
                    try:
                        drop([gate], "drop-held-import")

                        def connect_writer():
                            try:
                                return os.open(gate, os.O_WRONLY | os.O_NONBLOCK)
                            except OSError as error:
                                if error.errno != errno.ENXIO:
                                    raise
                                return None

                        writer = wait(connect_writer, "worker reading held import")
                        GLib.idle_add(chooser.respond, False)
                        wait(lambda: not chooser.pending, "picker completed behind held import")
                        click(28, 430)
                        drop([image], "drop-after-picker")
                        payload = image.read_bytes()
                        assert os.write(writer, payload) == len(payload)
                    finally:
                        if writer is not None:
                            os.close(writer)
                        gate.unlink()
                    ordered = count(4)
                    assert [layer["name"] for layer in ordered[-3:]] == [gate.name, second.name, image.name], \
                        "a later drop overtook the completed picker while the renderer was busy"
                    shot("picker-drop-completion-order")
                    key("z", "ctrl")
                    assert [layer["name"] for layer in count(3)[-2:]] == [gate.name, second.name]
                    key("z", "ctrl")
                    assert count(2)[-1]["name"] == gate.name
                    key("z", "ctrl")
                    wait(lambda: not draft.exists(), "one Undo per queued import")

                    drop([image, second], "drop-batch")
                    dropped = count(3)
                    assert [layer["name"] for layer in dropped[-2:]] == [image.name, second.name]
                    shot("drop-batch-imported")
                    key("z", "ctrl")
                    wait(lambda: not draft.exists(), "one Undo removes the complete drop")
                    shot("drop-batch-undone")
                    for name, invalid in [("decode", corrupt), ("render", tall)]:
                        unchanged_status = status_pixels()
                        drop([image, invalid], f"drop-batch-{name}")
                        assert layers() == unchanged_layers, f"late drop {name} failure published partial layers"
                        assert status_pixels() != unchanged_status, f"late drop {name} failure must report an error"
                        shot(f"drop-batch-{name}-error")
                    key("z", "ctrl+shift")
                    assert count(3) == dropped, "failed drops must preserve Redo"
                    key("z", "ctrl")
                    wait(lambda: not draft.exists(), "undo before retry")
                    drop([image, second], "drop-batch-retry")
                    retried = count(3)
                    assert [layer["name"] for layer in retried[-2:]] == [image.name, second.name]
                    shot("drop-batch-retried")
                    key("z", "ctrl")
                    wait(lambda: not draft.exists(), "undo before transfer rejection cases")
                    for name, payload in [
                        ("remote", image.as_uri().encode() + b"\r\nfile://remote/tmp/a.png\r\n"),
                        ("malformed", image.as_uri().encode() + b"\r\nfile:///tmp/bad%Q0.png\r\n"),
                        ("nul", b"file:///tmp/bad%00.png\r\n"),
                        ("empty", b"# comment only\r\n"),
                        ("oversized", b"#" + b"x" * (1024 * 1024) + b"\r\n"),
                    ]:
                        raw = profile / f"{name}.uri-list"
                        raw.write_bytes(payload)
                        drop([image], f"drop-reject-{name}", ["--payload", str(raw)])
                        assert not draft.exists(), f"{name} transfer must not import even its valid prefix"
                    drop([image], "drop-reject-move", ["--action", "move"])
                    assert not draft.exists(), "COPY-only receiver must reject MOVE"
                    drop([image], "drop-cancel-hover", destination="cancel-hover")
                    assert not draft.exists(), "leaving a valid hover must not import"
                    drop([image], "drop-timeout", ["--delay", "3"])
                    assert not draft.exists(), "late receive must not import after timeout"
                    key("z", "ctrl+shift")
                    assert count(3) == retried, "protocol rejection must preserve the entire Redo document"
                    key("z", "ctrl")
                    wait(lambda: not draft.exists(), "undo before delayed drop")
                    drop([image], "drop-delayed-center", ["--delay", "1"], destination="center", move_after=True)
                    delayed = count(2)[-1]
                    assert 0 < delayed["y"] < 360 - 43, "delayed drop must stack at release, not the later rail pointer"
                    assert 0 < delayed["x"] < 640 - 90
                    shot("drop-delayed-center")
                    key("z", "ctrl")
                    wait(lambda: not draft.exists(), "undo before overlapping offers")

                    def second_offer():
                        drop([second], "drop-overlap-fast", settle=False)

                    drop([image], "drop-overlap-slow", ["--delay", "1.8"], destination="center",
                         during_receive=second_offer)
                    overlapping = count(3)
                    assert [layer["name"] for layer in overlapping[-2:]] == [second.name, image.name]
                    assert overlapping[-2]["y"] == 360
                    assert 0 < overlapping[-1]["y"] < 360 - 43
                    key("z", "ctrl")
                    remaining = count(2)
                    assert [layer["name"] for layer in remaining] == ["Original screenshot", second.name]
                    assert (remaining[-1]["x"], remaining[-1]["y"]) == (305, 360)
                    key("z", "ctrl+shift")
                    assert count(3) == overlapping, "overlapping offers must retain independent Undo/Redo steps"
                    key("z", "ctrl")
                    count(2)
                    key("z", "ctrl")
                    wait(lambda: not draft.exists(), "undo second independent offer")
                    escaped = profile / "drop-é%#\n.png"
                    escaped.write_bytes(image.read_bytes())
                    uri_list = profile / "escaped.uri-list"
                    uri_list.write_bytes(("# local files\r\n" + escaped.as_uri().replace(
                        "file://", "file://localhost", 1) + "\r\n" + second.as_uri() + "\r\n").encode())
                    drop([escaped, second], "drop-escaped-paths", ["--payload", str(uri_list)])
                    assert [layer["name"] for layer in count(3)[-2:]] == [escaped.name, second.name]
                    key("z", "ctrl")
                    wait(lambda: not draft.exists(), "undo escaped-path batch")

                    def close_during_receive():
                        subprocess.run(["swaymsg", f'[con_id={window()["id"]}] kill'],
                                       env=env, check=True, stdout=subprocess.DEVNULL)
                        wait(lambda: window() is None, "close destination before transfer finishes")

                    drop([image], "drop-closed-window", ["--delay", "1.8"],
                         during_receive=close_during_receive, settle=False)
                    assert not draft.exists() and window() is None, "late transfer must not restore a closed editor"
                    assert source.read_bytes() == original
                    (profile / "result.json").write_text(json.dumps({"passed": True,
                        "appearance": appearance, "private_offer": types[0].decode(),
                        "checks": ["PNG-pixels", "private-MIME-no-text", "24px-layer-offset",
                            "rapid-external-pastes", "individual-Undo-for-external-pastes",
                            "text-html-empty-cleared-silent-noop", "corrupt-image-error-and-recovery",
                            "external-selection-replacement", "external-image-precedence", "Undo-Redo",
                            "PNG-fallback-retains-offer-across-selection-replacement",
                            "one-undo-per-batch", "late-decode-and-render-failure-atomic",
                            "failed-batch-preserves-Redo", "batch-retry",
                            "picker-one-undo-per-batch", "picker-late-decode-render-failure-atomic",
                            "picker-failure-preserves-Redo", "picker-retry",
                            "busy-picker-completion-precedes-later-drop",
                            "external-GTK-drop-one-undo-per-batch", "drop-late-decode-render-failure-atomic",
                            "drop-failure-preserves-Redo", "drop-retry",
                            "URI-rejection-atomic", "COPY-only", "hover-cancel", "receive-timeout",
                            "delayed-drop-frozen-position", "recovery-after-rejected-transfers",
                            "overlapping-offers-independent-targets-and-undo",
                            "percent-encoded-localhost-URI-order", "closed-destination-cancels-transfer",
                            "CRLF-file-order", "source-cancellation", "reclaim-after-cancel", "original-unchanged"]}, indent=2) + "\n")
                    print(f"PASS {appearance}: Wayland clipboard, picker and external GTK batch drops", flush=True)
                except Exception:
                    subprocess.run(["grim", str(profile / "failed-desktop.png")], env=env, check=True, timeout=5)
                    raise
                finally:
                    stop(app)
    finally:
        stop(keyboard)
        chooser.remove_from_connection()
    return {"clipboard_appearances": 2, "core_selection_ownership": True,
            "image_read_protocol": "wl_data_device", "physical_acceptance": False}


if __name__ == "__main__":
    main(cases, __doc__)
