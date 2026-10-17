#!/usr/bin/python3
"""Native editor clipboard ownership and image input on private headless Sway.

Reuses the real-window/virtual-input host fixture. DISPLAY is unset: no X11
fallback, installed profile, global clipboard or physical acceptance is involved.
"""
import json
import select
import subprocess
import time

from gi.repository import GLib

from instance_smoke import png
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
                subprocess.run(["convert", "-size", size, "xc:" + color, str(path)], check=True)
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
                    "--quit-after", "120"], env=env, stdout=log, stderr=log)
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

                    def key(name, modifiers="none"):
                        code = KEYS[name]
                        keyboard.stdin.write(f"{code} {modifiers}\n".encode())
                        keyboard.stdin.flush()
                        assert select.select([keyboard.stdout], [], [], 5)[0], "keyboard stalled"
                        assert keyboard.stdout.readline().strip() == f"KEYED {code} {modifiers}".encode()
                        time.sleep(.2)

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
                    publish(image.read_bytes(), "image/png")
                    key("v", "ctrl")
                    imported = count(2)[-1]
                    assert (imported["naturalWidth"], imported["naturalHeight"], imported["y"]) == (90, 43, 360)
                    shot("clipboard-image-imported")
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
                    (profile / "result.json").write_text(json.dumps({"passed": True,
                        "appearance": appearance, "private_offer": types[0].decode(),
                        "checks": ["PNG-pixels", "private-MIME-no-text", "24px-layer-offset",
                            "text-html-empty-cleared-silent-noop", "corrupt-image-error-and-recovery",
                            "external-selection-replacement", "external-image-precedence", "Undo-Redo",
                            "one-undo-per-batch", "late-decode-and-render-failure-atomic",
                            "failed-batch-preserves-Redo", "batch-retry",
                            "picker-one-undo-per-batch", "picker-late-decode-render-failure-atomic",
                            "picker-failure-preserves-Redo", "picker-retry",
                            "CRLF-file-order", "source-cancellation", "reclaim-after-cancel", "original-unchanged"]}, indent=2) + "\n")
                    print(f"PASS {appearance}: Wayland clipboard image/file input, ownership replacement/cancellation", flush=True)
                except Exception:
                    subprocess.run(["grim", str(profile / "failed-desktop.png")], env=env, check=True, timeout=5)
                    raise
                finally:
                    stop(app)
    finally:
        stop(keyboard)
        chooser.remove_from_connection()
    return {"clipboard_appearances": 2, "core_selection_ownership": True,
            "image_read_protocol": "wlr-data-control", "physical_acceptance": False}


if __name__ == "__main__":
    main(cases, __doc__)
