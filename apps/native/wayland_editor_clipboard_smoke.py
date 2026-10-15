#!/usr/bin/python3
"""Native editor clipboard ownership and image input on private headless Sway.

Reuses the real-window/virtual-input host fixture. DISPLAY is unset: no X11
fallback, installed profile, global clipboard or physical acceptance is involved.
"""
import json
import select
import subprocess
import time

from wayland_keyboard_smoke import KEYS
from wayland_native_capture_smoke import wait, windows
from wayland_screenshot_smoke import ready, stop
from wayland_shortcuts_host_smoke import HEIGHT, WIDTH, main


def cases(binary, pointer, root, env, bus):
    assert "DISPLAY" not in env
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
                    publish(b"ordinary filename", "text/plain")
                    key("v", "ctrl")
                    time.sleep(.7)
                    assert len(layers()) == 3, "external selection must invalidate the internal snapshot"
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
                    assert [layer["name"] for layer in count(6)[-2:]] == [image.name, second.name]
                    key("c", "ctrl")
                    subprocess.run(["wl-copy", "--clear"], env=env, check=True, timeout=5)
                    key("v", "ctrl")
                    time.sleep(.7)
                    assert len(layers()) == 6, "cleared selection must cancel snapshot authorization"
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
                    (profile / "result.json").write_text(json.dumps({"passed": True,
                        "appearance": appearance, "private_offer": types[0].decode(),
                        "checks": ["PNG-pixels", "private-MIME-no-text", "24px-layer-offset",
                            "external-selection-replacement", "external-image-precedence", "Undo-Redo",
                            "CRLF-file-order", "source-cancellation", "reclaim-after-cancel", "original-unchanged"]}, indent=2) + "\n")
                    print(f"PASS {appearance}: Wayland clipboard image/file input, ownership replacement/cancellation", flush=True)
                except Exception:
                    subprocess.run(["grim", str(profile / "failed-desktop.png")], env=env, check=True, timeout=5)
                    raise
                finally:
                    stop(app)
    finally:
        stop(keyboard)
    return {"clipboard_appearances": 2, "core_selection_ownership": True,
            "image_read_protocol": "wlr-data-control", "physical_acceptance": False}


if __name__ == "__main__":
    main(cases, __doc__)
