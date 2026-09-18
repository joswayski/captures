#!/usr/bin/env python3
"""Windows-only DirectComposition functional/render checks, not performance acceptance."""
import argparse
import ctypes
import json
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from wgpu.smoke import run
from profile import events_at


def client_bounds(pid):
    """Find this process's visible HWND; use physical client pixels, not a desktop dump."""
    from ctypes import wintypes as w
    user = ctypes.WinDLL("user32", use_last_error=True)
    callback = ctypes.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
    user.EnumWindows.argtypes = [callback, w.LPARAM]
    user.EnumWindows.restype = w.BOOL
    user.GetWindowThreadProcessId.argtypes = [w.HWND, ctypes.POINTER(w.DWORD)]
    user.GetWindowThreadProcessId.restype = w.DWORD
    user.IsWindowVisible.argtypes = [w.HWND]
    user.IsWindowVisible.restype = w.BOOL
    user.GetClientRect.argtypes = [w.HWND, ctypes.POINTER(w.RECT)]
    user.GetClientRect.restype = w.BOOL
    user.ClientToScreen.argtypes = [w.HWND, ctypes.POINTER(w.POINT)]
    user.ClientToScreen.restype = w.BOOL
    found = []

    @callback
    def visit(hwnd, _):
        owner = w.DWORD()
        user.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user.IsWindowVisible(hwnd):
            found.append(hwnd)
        return True

    user.EnumWindows(visit, 0)
    if len(found) != 1:
        raise RuntimeError(f"Expected one visible probe HWND, got {len(found)}")
    rect, origin = w.RECT(), w.POINT()
    if not user.GetClientRect(found[0], ctypes.byref(rect)) or not user.ClientToScreen(found[0], ctypes.byref(origin)):
        raise ctypes.WinError(ctypes.get_last_error())
    return origin.x, origin.y, origin.x + rect.right, origin.y + rect.bottom


def composition(binary, output, reduced=False):
    from PIL import ImageGrab, ImageChops, ImageStat
    name = "composition-reduced" if reduced else "composition-fade"
    log = output / f"{name}.jsonl"
    with log.open("w") as stdout, (output / f"{name}.stderr.txt").open("w") as stderr:
        args = [str(binary), "--scene", "preview", "--floating", "--exercise", "--quit-after", "8"]
        if reduced:
            args.append("--reduced-motion")
        start = time.monotonic()
        process = subprocess.Popen(args, stdout=stdout, stderr=stderr)
        frames, times, shots = [], [], {}
        try:
            while not any(e["event"] == "ready" for e in events_at(log)):
                if process.poll() is not None or time.monotonic() - start > 10:
                    raise RuntimeError(f"{name}: no readiness; see {log}")
                time.sleep(.02)
            bounds = client_bounds(process.pid)
            # The app schedules relative to startup, not observer readiness.
            for deadline, key in [(1., "before"), *[(1.65 + n * .05, None) for n in range(22)],
                                  (3., "after"), (6.7, "restored")]:
                time.sleep(max(0, start + deadline - time.monotonic()))
                if process.poll() is not None:
                    raise RuntimeError(f"{name}: exited before composition capture")
                image = ImageGrab.grab(bbox=bounds, all_screens=True).convert("RGB")
                if key:
                    shots[key] = image
                    image.save(output / f"{name}-{key}.png")
                else:
                    times.append(time.monotonic() - start)
                    frames.append(image)
            if process.wait(timeout=10) != 0:
                raise RuntimeError(f"{name}: nonzero exit; see {log}")
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
        # Keep the actual sampling times. This is an observer recording, not FPS.
        (output / f"{name}-observer-times.json").write_text(json.dumps(times))
        frames[0].save(output / f"{name}.gif", save_all=True, append_images=frames[1:],
                       duration=[max(10, round((b-a)*1000)) for a, b in zip(times, times[1:])] + [500], loop=0)
        scale = shots["before"].width / 640
        box = tuple(round(v * scale) for v in (48, 48, 200, 160))
        before = shots["before"].crop(box)
        faded = ImageStat.Stat(ImageChops.difference(before, shots["after"].crop(box))).mean
        restored = ImageStat.Stat(ImageChops.difference(before, shots["restored"].crop(box))).mean
        if max(faded) < 5 or max(restored) > 2:
            raise RuntimeError(f"{name}: final desktop composition did not fade/restore: {faded}, {restored}")
        events = events_at(log)
        actions = [e["detail"] for e in events if e["event"] == "scripted-action"]
        if [e["deleted"] for e in actions] != [True, False] or not all(e["compositorFade"] for e in actions):
            raise RuntimeError(f"{name}: missing compositor actions")
        if not any(e["event"] == "exit" for e in events):
            raise RuntimeError(f"{name}: missing clean shutdown")
        print(f"{name}: desktop pixels fade and restore; recording saved (not displayed-FPS measurement)", flush=True)


def main():
    from PIL import Image
    if sys.platform != "win32":
        raise RuntimeError("Run on Windows with a real interactive desktop")
    user = ctypes.WinDLL("user32", use_last_error=True)
    user.SetProcessDpiAwarenessContext.argtypes = [ctypes.c_void_p]
    user.SetProcessDpiAwarenessContext.restype = ctypes.c_bool
    if not user.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4)):
        raise ctypes.WinError(ctypes.get_last_error())
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=False)
    for scene in ("idle", "preferences"):
        events = run(binary, args.output, f"{scene}-idle", ["--scene", scene, "--quit-after", "5"])
        lifecycle = next(e["detail"] for e in events if e["event"] == "lifecycle-check")
        metrics = next(e["detail"] for e in events if e["event"] == "exit")
        if lifecycle["nativeVisible"] != (scene != "idle") or metrics["uiPassesAfterTwoSeconds"] > 1:
            raise RuntimeError(f"{scene}: recurring redraw or incorrect visibility: {lifecycle}, {metrics}")
    shots = {
        "preferences-dark": ["--scene", "preferences"],
        "preferences-light": ["--scene", "preferences", "--appearance", "light"],
        "history-empty": ["--scene", "history", "--history-count", "0"],
        "history-populated": ["--scene", "history"],
        "hud-running": ["--scene", "hud", "--appearance", "light"],
        "hud-paused": ["--scene", "hud", "--exercise", "--screenshot-after", "3"],
        "hud-floating": ["--scene", "hud", "--floating"],
        "preview-before": ["--scene", "preview"],
        "preview-after": ["--scene", "preview", "--exercise", "--screenshot-after", "3"],
        "preview-floating": ["--scene", "preview", "--floating"],
        "editor-transformed": ["--scene", "editor", "--exercise", "--screenshot-after", "7"],
        "countdown-light": ["--scene", "countdown", "--appearance", "light"],
        "countdown-dark": ["--scene", "countdown"],
    }
    for name, arguments in shots.items():
        path = args.output / f"{name}.png"
        events = run(binary, args.output, name, [*arguments, "--screenshot", str(path)])
        if not any(e["event"] == "screenshot-saved" for e in events):
            raise RuntimeError(f"{name}: missing readback acknowledgement")
        with Image.open(path) as image:
            if min(image.size) < 400 or image.convert("RGB").getextrema() == ((0, 0),) * 3:
                raise RuntimeError(f"{name}: blank/undersized surface readback")
            if "floating" in name and image.convert("RGBA").getpixel((0, 0))[3] != 0:
                raise RuntimeError(f"{name}: floating corner is not transparent")
    for scene in ("preferences", "history", "hud", "preview", "editor"):
        events = run(binary, args.output, f"{scene}-exercise", ["--scene", scene, "--exercise", "--quit-after", "24"])
        actions = [e["detail"] for e in events if e["event"] == "scripted-action"]
        field, expected = {"preferences": ("appearance", ["light", "dark"] * 3),
                           "history": ("historyEnd", [True, False] * 3),
                           "hud": ("paused", [True, False] * 3),
                           "preview": ("deleted", [True, False] * 3),
                           "editor": ("zoom", [1.5, .75] * 3)}[scene]
        if [e["cycle"] for e in actions] != list(range(6)) or [e[field] for e in actions] != expected:
            raise RuntimeError(f"{scene}: missing/incorrect scripted actions")
    composition(binary, args.output)
    composition(binary, args.output, reduced=True)
    print("PASS: hidden visibility, static redraw guard, 13 surface readbacks, 30 actions, desktop fade/restore")


if __name__ == "__main__":
    main()
