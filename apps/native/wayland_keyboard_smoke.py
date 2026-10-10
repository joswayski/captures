#!/usr/bin/python3
"""Compositor-delivered native Find/editor keys on private Sway 1.9+.

Reuses the shortcut host's disposable bus, tray, profile and persistent pointer.
Keyboard input crosses Sway's virtual-keyboard protocol; no X11, egui event
injection, installed profile, login registration or capture consent is used.
"""
import hashlib
import json
import re
import select
import subprocess
import time

from wayland_lifecycle_smoke import menu_action, watcher
from wayland_native_capture_smoke import wait, windows
from wayland_recording_host_smoke import events
from wayland_screenshot_smoke import ready, stop
from wayland_shortcuts_host_smoke import HEIGHT, PREFERENCES, WIDTH, main

# Linux evdev codes, not X11 keycodes. The injector publishes its own US keymap.
KEYS = dict(zip("qwertyuiopasdfghjklzxcvbnm", [16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 30, 31, 32, 33, 34, 35, 36, 37, 38, 44, 45, 46, 47, 48, 49, 50], strict=True))
KEYS.update(dict(zip("1234567890", range(2, 12), strict=True)))
KEYS.update(enter=28, escape=1, tab=15, backspace=14, space=57, f3=61,
            home=102, end=107, left=105, right=106, up=103, down=108,
            pageup=104, pagedown=109, delete=111)


def snapshot(root):
    # Artifact bytes, including every metadata/original/preview file. The
    # hidden crash-diagnostics marker is intentionally removed by clean Quit.
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in root.glob("*/**/*") if path.is_file()
            and not path.relative_to(root).parts[0].startswith(".")}


def cases(binary, pointer, root, env, bus):
    version = subprocess.check_output(["sway", "--version"], env=env, text=True).strip()
    match = re.search(r"version (\d+)\.(\d+)", version)
    assert match and tuple(map(int, match.groups())) >= (1, 9), version
    assert "DISPLAY" not in env and env["XDG_SESSION_TYPE"] == "wayland"
    keyboard = subprocess.Popen([pointer.args[0], "keyboard"], env=env,
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    try:
        ready(keyboard)
        for appearance in ("dark", "light"):
            profile = root / appearance
            profile.mkdir()
            source = profile / "Source.png"
            movie = profile / "Source.mp4"
            subprocess.run(["convert", "-size", "640x360", "xc:#286ea6", "-fill", "#e5b344",
                            "-draw", "rectangle 80,60 220,200", "-strip", "PNG32:" + str(source)], check=True)
            subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", "color=red:s=320x180:r=10:d=2",
                            "-f", "lavfi", "-i", "color=green:s=320x180:r=10:d=2", "-f", "lavfi", "-i",
                            "color=blue:s=320x180:r=10:d=2", "-filter_complex", "[0:v][1:v][2:v]concat=n=3:v=1:a=0",
                            "-c:v", "mpeg4", "-q:v", "2", "-an", str(movie)], check=True, timeout=30)
            originals = {path.name: path.read_bytes() for path in (source, movie)}
            settings = profile / "settings.json"
            settings.write_text(json.dumps({"settings_schema_version": 5, "onboarding_completed": True,
                "appearance": appearance, "launch_at_login": False, "show_mini_previews": False,
                "auto_copy_to_clipboard": False, "output_directory": str(profile / "exports"),
                "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
                "display_shortcut": "Ctrl+Shift+F9"}))
            settings_before = settings.read_bytes()
            history = profile / "history"
            host_log = profile / "host.log"
            observations = []
            with host_log.open("w") as log:
                app = subprocess.Popen([binary, "--live", "--open-preferences", "--open-media", str(source),
                    "--open-media", str(movie), "--settings-file", str(settings), "--history-root", str(history),
                    "--quit-after", "300"], env=env, stdout=log, stderr=log)
                try:
                    def window(title):
                        assert app.poll() is None, host_log.read_text()
                        return next((node for node in windows(env) if node["pid"] == app.pid
                                     and node["name"].startswith(title)), None)

                    def layout(event):
                        found = events(host_log, event)
                        return found[-1]["detail"] if found else {}

                    def arrange(title, width, height):
                        node = wait(lambda: window(title), title)
                        subprocess.run(["swaymsg", f'[con_id={node["id"]}] floating enable, '
                            f'resize set {width} {height}, move position 80 80, focus'],
                            env=env, check=True, stdout=subprocess.DEVNULL)
                        wait(lambda: window(title)["rect"]["width"] == width
                            and window(title)["rect"]["height"] == height
                            and window(title)["focused"], title + " resize/focus")
                        time.sleep(.5)

                    def key(name, modifiers="none"):
                        code = KEYS[name]
                        keyboard.stdin.write(f"{code} {modifiers}\n".encode())
                        keyboard.stdin.flush()
                        assert select.select([keyboard.stdout], [], [], 5)[0], "keyboard stalled"
                        assert keyboard.stdout.readline().strip() == f"KEYED {code} {modifiers}".encode()

                    def type_text(value):
                        for char in value:
                            key("space" if char == " " else char.lower(), "shift" if char.isupper() else "none")

                    def click(title, x, y, scroll=None):
                        rect = window(title)["rect"]
                        suffix = f" {scroll}" if scroll is not None else ""
                        pointer.stdin.write(f'{round(rect["x"] + x)} {round(rect["y"] + y)} '
                                            f"{WIDTH} {HEIGHT}{suffix}\n".encode())
                        pointer.stdin.flush()
                        assert select.select([pointer.stdout], [], [], 5)[0], "pointer stalled"
                        assert pointer.stdout.readline().strip() == (b"SCROLLED" if scroll is not None else b"CLICKED")

                    def shot(title, name):
                        time.sleep(.3)
                        rect = window(title)["rect"]
                        subprocess.run(["grim", "-g", f'{rect["x"]},{rect["y"]} '
                            f'{rect["width"]}x{rect["height"]}', str(profile / f"{name}.png")],
                            env=env, check=True, timeout=5)

                    def clipboard():
                        result = subprocess.run(["wl-paste", "--no-newline"], env=env,
                                                capture_output=True, timeout=3)
                        return result.stdout.decode() if result.returncode == 0 else None

                    wait(lambda: watcher(bus) and watcher(bus).Get("org.kde.StatusNotifierWatcher",
                        "RegisteredStatusNotifierItems"), "real tray registration")
                    wait(lambda: len(list(history.glob("*/metadata.json"))) == 2, "two imported sources")
                    wait(lambda: window("Captures Screenshot Editor") and window("Captures Editor"), "native editors")
                    history_before = snapshot(history)
                    (profile / "history-before.json").write_text(json.dumps(history_before, indent=2) + "\n")

                    def expect_find(count, index, focused=True, opened=True):
                        expected = dict(open=opened, count=count, index=index, focused=focused)
                        def matches():
                            state = layout("preferences-shortcuts-layout").get("find", {})
                            return state if all(state.get(k) == v for k, v in expected.items()) else None
                        result = wait(matches, f"Find {expected}; got {layout('preferences-shortcuts-layout')}")
                        observations.append({"find": result})

                    for width, height, size in ((880, 660, "normal"), (560, 440, "minimum")):
                        arrange(PREFERENCES, width, height)
                        expect_find(0, 0, False, False)
                        for chord in (("f", "ctrl+shift"), ("f", "ctrl+alt"), ("f3", "none"), ("g", "ctrl")):
                            key(*chord)
                            time.sleep(.2)
                            expect_find(0, 0, False, False)
                        key("f", "ctrl")
                        expect_find(0, 0)
                        type_text("screenshots")
                        expect_find(5, 0)
                        # Four capture rows plus Wayland's shortcut limitation
                        # copy match; distinct rows expose direction/duplicate errors.
                        for name, mods, index in (("enter", "none", 1), ("enter", "none", 2),
                            ("enter", "shift", 1), ("enter", "shift", 0), ("enter", "shift", 4),
                            ("f3", "shift", 3), ("g", "ctrl", 4), ("g", "ctrl+shift", 3)):
                            key(name, mods)
                            expect_find(5, index)
                        for chord in (("f3", "ctrl"), ("f3", "alt"), ("g", "ctrl+alt")):
                            key(*chord)
                            time.sleep(.2)
                            expect_find(5, 3)
                        key("f3")
                        expect_find(5, 4)
                        def revealed():
                            detail = layout("preferences-shortcuts-layout")
                            current, page = detail["find"]["current"], detail["page"]
                            return current and page[1] <= current[1] < current[3] <= page[3]
                        wait(revealed, "Find match visible")
                        shot(PREFERENCES, f"find-{size}")
                        key("tab")
                        expect_find(5, 4, False)
                        key("enter")  # Previous button owns Enter, not the query's Next.
                        expect_find(5, 3, False)
                        key("tab", "shift")
                        expect_find(5, 3)
                        key("enter")
                        expect_find(5, 4)
                        type_text("zz")  # Subsequent typing proves Enter retained TextEdit focus.
                        expect_find(0, 0)
                        key("a", "ctrl")
                        type_text("cursor")
                        expect_find(2, 0)
                        key("escape")
                        expect_find(0, 0, False, False)
                        key("f", "ctrl")
                        expect_find(0, 0)
                        key("escape")
                        expect_find(0, 0, False, False)

                    # Real inline Text entry: repeated Enter is text, Shift navigation
                    # is selection, and document shortcuts must wait for field blur.
                    screenshot = "Captures Screenshot Editor"
                    artifact = next(path.parent for path in history.glob("*/metadata.json")
                                    if json.loads(path.read_text())["kind"] == "screenshot")
                    draft = profile / "editor-drafts" / artifact.name / "manifest.json"
                    arrange(screenshot, 1000, 800)
                    click(screenshot, 28, 159)  # Shipping rail Text tool.
                    click(screenshot, 350, 320)  # Canvas; no Properties or source mutation.
                    type_text("Alpha")
                    key("enter")
                    type_text("Beta")
                    key("enter")
                    type_text("Gamma")
                    key("home", "shift")
                    key("c", "ctrl")
                    wait(lambda: clipboard() == "Gamma", "Shift Home selection after repeated Enter")
                    assert not draft.exists(), "inline typing persisted before commit"
                    shot(screenshot, "text-normal")
                    arrange(screenshot, 760, 540)
                    key("end")
                    key("left", "shift")
                    key("left", "shift")
                    key("c", "ctrl")
                    wait(lambda: clipboard() == "ma", "repeated Shift Left at minimum size")
                    shot(screenshot, "text-minimum")
                    key("escape")
                    def layers():
                        return json.loads(draft.read_text())["document"]["elements"] if draft.exists() else []
                    created = wait(lambda: values if len(values := layers()) == 2 else None, "one Text commit")[-1]
                    assert created["text"] == "Alpha\nBeta\nGamma", created
                    click(screenshot, 28, 430)  # Empty rail: release text/tool focus.
                    key("z", "ctrl")
                    wait(lambda: not draft.exists(), "single document Undo clears the only edit's draft")
                    key("z", "ctrl+shift")
                    wait(lambda: len(layers()) == 2 and layers()[-1] == created, "exact document Redo")
                    observations.append({"text": created["text"], "selection": ["Gamma", "ma"], "undo_redo": True})

                    recording = "Captures Editor"
                    def controls():
                        return layout("recording-editor-layout").get("controls", {})

                    def settled():
                        since = None
                        def idle():
                            nonlocal since
                            if "Working…" in window(recording)["name"]:
                                since = None
                            elif since is None:
                                since = time.monotonic()
                            return since is not None and time.monotonic() - since >= .5
                        wait(idle, "recording worker settled")

                    def press(name):
                        settled()
                        for _ in range(40):
                            control = controls().get(name)
                            page = controls().get("Page")
                            if control and page and page[1] <= control[1] < control[3] <= page[3]:
                                click(recording, (control[0] + control[2]) / 2, (control[1] + control[3]) / 2)
                                time.sleep(.3)
                                return
                            rect = window(recording)["rect"]
                            distance = 200 if not control or control[1] > rect["height"] / 2 else -200
                            click(recording, rect["width"] - 40, rect["height"] / 2, distance)
                            time.sleep(.2)
                        raise AssertionError(f"{name} not visible: {controls()}")

                    def read_time(name, expected):
                        press(name)
                        # A failed Copy must not pass by reading the prior field.
                        subprocess.run(["wl-copy", "--clear"], env=env, check=True, timeout=3)
                        key("a", "ctrl")
                        time.sleep(.2)
                        key("c", "ctrl")
                        actual = wait(lambda: value if (value := clipboard()) == str(expected) else None,
                                      f"{name} expected {expected}; got {clipboard()}")
                        observations.append({"field": name, "value": int(actual)})

                    for width, height, size in ((960, 820, "normal"), (760, 580, "minimum")):
                        arrange(recording, width, height)
                        read_time("Start (ms)", 0)
                        read_time("End (ms)", 6000)
                        # No refocus between repeated keys: decoding must retain grip focus.
                        for handle, field, commands, expected in (
                            ("Trim start", "Start (ms)", [("right", "shift"), ("right", "shift")], 2),
                            ("Trim start", "Start (ms)", [("down", "ctrl")], 1),
                            ("Trim start", "Start (ms)", [("pageup", "shift")], 1001),
                            ("Trim start", "Start (ms)", [("pagedown", "shift")], 1),
                            ("Trim start", "Start (ms)", [("left", "ctrl+alt+shift")], 0),
                            ("Trim end", "End (ms)", [("down", "shift")], 5999),
                            ("Trim end", "End (ms)", [("up", "ctrl")], 6000),
                            ("Trim end", "End (ms)", [("pagedown", "ctrl+shift")], 5000),
                            ("Trim end", "End (ms)", [("pageup", "shift")], 6000)):
                            press(handle)
                            for command in commands:
                                key(*command)
                                settled()
                            observations.append({"handle": handle, "commands": commands})
                            read_time(field, expected)
                        # Read-back focuses numeric TextEdit. Its cursor keys cannot trim.
                        # Check directions separately; opposite stolen keys could cancel.
                        key("left", "ctrl")
                        read_time("End (ms)", 6000)
                        read_time("Start (ms)", 0)
                        press("Trim start")
                        key("left", "ctrl+shift")
                        read_time("Start (ms)", 0)
                        key("right", "shift")
                        read_time("Start (ms)", 0)
                        shot(recording, f"trim-{size}")

                    menu_action(bus, "Quit Captures")
                    assert app.wait(timeout=15) == 0
                    assert settings.read_bytes() == settings_before, "keyboard work edited settings"
                    history_after = snapshot(history)
                    (profile / "history-after.json").write_text(json.dumps(history_after, indent=2) + "\n")
                    assert history_after == history_before, ("keyboard work edited History/source/preview bytes",
                        {path: (history_before.get(path), history_after.get(path))
                         for path in history_before.keys() | history_after.keys()
                         if history_before.get(path) != history_after.get(path)})
                    assert all(path.read_bytes() == originals[path.name] for path in (source, movie)), "external source modified"
                    result = {"passed": True, "appearance": appearance, "sway": version,
                        "history_sha256": history_before, "settings_sha256": hashlib.sha256(settings_before).hexdigest(),
                        "source_sha256": {name: hashlib.sha256(data).hexdigest() for name, data in originals.items()},
                        "observations": observations, "sizes": {"find": [[880, 660], [560, 440]],
                            "text": [[1000, 800], [760, 540]], "trim": [[960, 820], [760, 580]]}}
                    (profile / "result.json").write_text(json.dumps(result, indent=2) + "\n")
                    print(f"PASS {appearance}: Wayland Find, inline Text, modified trim, immutable source/settings/History", flush=True)
                except Exception:
                    (profile / "failed-windows.json").write_text(json.dumps(windows(env), indent=2) + "\n")
                    subprocess.run(["grim", str(profile / "failed-desktop.png")], env=env, check=True, timeout=5)
                    raise
                finally:
                    stop(app)
    finally:
        stop(keyboard)
    return {"keyboard_appearances": 2, "sizes_per_surface": 2, "sway": version,
            "screen_reader_acceptance": False}


if __name__ == "__main__":
    main(cases, __doc__)
