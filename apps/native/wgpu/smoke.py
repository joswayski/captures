#!/usr/bin/env python3
"""Exercise real native windows; save viewport-only images and JSONL for review.

Requires a desktop/compositor, not a browser. This is a functional smoke test,
not hardware performance, visual parity, accessibility, or compositor acceptance.
"""
import argparse
import json
import struct
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from history_fixture import write_completed_settings


def run(binary, output, name, arguments, allow_unsupported_hidden=False):
    try:
        result = subprocess.run([str(binary), *arguments], capture_output=True, timeout=40)
    except subprocess.TimeoutExpired as error:
        # Keep the actual failure evidence; never turn a hung workload into a pass.
        (output / f"{name}.jsonl").write_bytes(error.stdout or b"")
        (output / f"{name}.stderr.txt").write_bytes(error.stderr or b"")
        raise
    (output / f"{name}.jsonl").write_bytes(result.stdout)
    (output / f"{name}.stderr.txt").write_bytes(result.stderr)
    events = [json.loads(line) for line in result.stdout.decode().splitlines() if line.strip()]
    if (allow_unsupported_hidden and result.returncode == 3 and
            any(e["event"] == "unsupported" and e["detail"]["capability"] == "hidden-idle" for e in events)):
        return events
    if result.returncode:
        raise RuntimeError(f"{name} exited {result.returncode}: {result.stderr.decode(errors='replace')}")
    if not any(e["event"] == "ready" for e in events) or not any(e["event"] == "exit" for e in events):
        raise RuntimeError(f"{name}: missing readiness or clean shutdown")
    return events


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=False)

    hidden_supported = True
    for scene in ["idle", "preferences"]:
        events = run(binary, args.output, f"{scene}-idle", ["--scene", scene, "--quit-after", "5"],
                     allow_unsupported_hidden=scene == "idle")
        if any(e["event"] == "unsupported" for e in events):
            hidden_supported = False
            print("UNSUPPORTED: hidden idle; this parity gate remains open", flush=True)
            continue
        metrics = next(e["detail"] for e in events if e["event"] == "exit")
        passes = metrics["uiPassesAfterTwoSeconds"]
        lifecycle = [e["detail"] for e in events if e["event"] == "lifecycle-check"]
        if not lifecycle or any(e["nativeVisible"] is not False for e in lifecycle if scene == "idle"):
            raise RuntimeError(f"{scene}: native visibility is incorrect or unverified: {lifecycle}")
        if scene != "idle" and any(e["nativeVisible"] is False for e in lifecycle):
            raise RuntimeError(f"{scene}: expected a visible window")
        # Exclude startup font/layout/async settings work, not recurring redraw.
        # The quit deadline itself legitimately causes a small number of passes.
        if passes > 6:
            raise RuntimeError(f"{scene}: {passes} UI passes after settling; investigate recurring redraw")
        print(f"{scene}: {metrics['uiPasses']} total / {passes} settled UI passes", flush=True)

    populated = args.output / "populated-history"
    subprocess.run([sys.executable, str(Path(__file__).resolve().parents[1] / "history_fixture.py"), str(populated)], check=True)
    completed = args.output / "completed-settings.json"
    write_completed_settings(completed)
    broken = args.output / "onboarding-error.json"
    broken.write_text("invalid-json")
    shots = {
        "live-empty": ["--live", "--settings-file", str(completed), "--history-root", str(args.output / "empty-history"), "--appearance", "light"],
        "live-populated": ["--live", "--settings-file", str(completed), "--history-root", str(populated)],
        "permission-dialog-ready": ["--live", "--settings-file", str(completed), "--history-root", str(args.output / "permission-ready-history"), "--permission-dialog", "ready"],
        "permission-dialog-error": ["--live", "--settings-file", str(completed), "--history-root", str(args.output / "permission-error-history"), "--permission-dialog", "error"],
        "onboarding-light": ["--live", "--history-root", str(args.output / "onboarding-history"), "--settings-file", str(args.output / "fresh-light.json"), "--appearance", "light"],
        "onboarding-dark": ["--live", "--history-root", str(args.output / "onboarding-history"), "--settings-file", str(args.output / "fresh-dark.json")],
        "onboarding-error": ["--live", "--history-root", str(args.output / "onboarding-history"), "--settings-file", str(broken)],
        "countdown-light": ["--scene", "countdown", "--appearance", "light"],
        "countdown-dark": ["--scene", "countdown", "--appearance", "dark"],
        "preferences-dark": ["--scene", "preferences"],
        "preferences-light": ["--scene", "preferences", "--appearance", "light", "--theme", "cobalt"],
        "history-empty": ["--scene", "history", "--history-count", "0"],
        "history-populated": ["--scene", "history", "--history-count", "1000"],
        "hud-running": ["--scene", "hud", "--appearance", "light"],
        "hud-paused": ["--scene", "hud", "--exercise", "--screenshot-after", "3"],
        "hud-muted": ["--scene", "hud", "--hud-state", "muted"],
        "hud-busy": ["--scene", "hud", "--hud-state", "busy"],
        "hud-no-microphone": ["--scene", "hud", "--hud-state", "no-microphone"],
        "hud-saving": ["--scene", "hud", "--hud-state", "saving"],
        "hud-failed": ["--scene", "hud", "--hud-state", "failed"],
        "preview-before": ["--scene", "preview"],
        "preview-after": ["--scene", "preview", "--exercise", "--screenshot-after", "3"],
        "preview-reduced": ["--scene", "preview", "--exercise", "--reduced-motion", "--screenshot-after", "3"],
        "preview-floating": ["--scene", "preview", "--floating"],
        "editor-transformed": ["--scene", "editor", "--exercise", "--screenshot-after", "7"],
        "region-blank": ["--scene", "region"],
        "region-drawn": ["--scene", "region", "--exercise", "--screenshot-after", "3"],
        "region-moved": ["--scene", "region", "--exercise", "--screenshot-after", "7"],
        "region-resized": ["--scene", "region", "--exercise", "--screenshot-after", "11"],
        "region-aspect": ["--scene", "region", "--exercise", "--screenshot-after", "15"],
        "region-shift-square": ["--scene", "region", "--exercise", "--screenshot-after", "19"],
        "region-cancelled": ["--scene", "region", "--exercise", "--screenshot-after", "23"],
        "window-blank": ["--scene", "window"],
        "window-project": ["--scene", "window", "--exercise", "--screenshot-after", "3"],
        "window-frontmost": ["--scene", "window", "--exercise", "--screenshot-after", "7"],
        "window-shell-display": ["--scene", "window", "--exercise", "--screenshot-after", "11"],
        "window-desktop-display": ["--scene", "window", "--exercise", "--screenshot-after", "15"],
        "window-terminal": ["--scene", "window", "--exercise", "--screenshot-after", "19"],
        "window-cancelled": ["--scene", "window", "--exercise", "--screenshot-after", "23"],
    }
    for name, arguments in shots.items():
        screenshot = args.output / f"{name}.png"
        events = run(binary, args.output, name, [*arguments, "--screenshot", str(screenshot)])
        if not any(e["event"] == "screenshot-saved" for e in events):
            raise RuntimeError(f"{name}: missing screenshot acknowledgement")
        if name.startswith(("region-", "window-")):
            texture_sizes = [e["detail"]["pixels"] for e in events if e["event"] == "texture-preparation"]
            if texture_sizes != [[2048, 1152]]:
                raise RuntimeError(f"{name}: expected one retained 2048×1152 source texture: {texture_sizes}")
        data = screenshot.read_bytes()
        if data[:8] != b"\x89PNG\r\n\x1a\n" or min(struct.unpack(">II", data[16:24])) < 400:
            raise RuntimeError(f"{name}: invalid or undersized viewport capture")

    assert not (args.output / "fresh-light.json").exists(), "rendering completed first-run setup"
    assert not (args.output / "fresh-dark.json").exists(), "rendering completed first-run setup"
    assert broken.read_text() == "invalid-json", "setup replaced malformed settings"
    assert not list((args.output / "onboarding-history").glob("*/metadata.json")), "setup captured media"

    # Verify all six scheduled actions, not only that the process survived.
    for scene in ["preferences", "history", "hud", "preview", "editor", "region", "window"]:
        events = run(binary, args.output, f"{scene}-exercise", ["--scene", scene, "--exercise", "--quit-after", "24"])
        actions = [e["detail"] for e in events if e["event"] == "scripted-action"]
        if [e["cycle"] for e in actions] != list(range(6)):
            raise RuntimeError(f"{scene}: missing or repeated actions")
        if scene == "hud" and [e["paused"] for e in actions] != [True, False] * 3:
            raise RuntimeError("HUD did not alternate pause/resume")
        if scene == "preferences" and [e["appearance"] for e in actions] != ["light", "dark"] * 3:
            raise RuntimeError("Preferences did not alternate appearance")
        if scene == "history" and [e["historyEnd"] for e in actions] != [True, False] * 3:
            raise RuntimeError("History did not alternate scroll endpoints")
        if scene == "editor" and [e["rotation"] for e in actions] != [0, 15, 30, 45, 60, 75]:
            raise RuntimeError("Editor rotation did not advance")
        if scene == "preview" and len([e for e in events if e["event"] == "first-action-total"]) != 6:
            raise RuntimeError("Preview did not submit six effects")
        if scene == "region":
            regions = [e["regionSelection"] for e in actions]
            if any(region is None for region in regions[:5]) or regions[5] is not None:
                raise RuntimeError(f"Region fixture did not exercise selection then cancellation: {regions}")
            aspect = regions[3]
            square = regions[4]
            if abs(aspect["width"] / aspect["height"] - 16 / 9) > 1e-6:
                raise RuntimeError(f"Region fixture did not apply 16:9: {aspect}")
            if abs(square["width"] - square["height"]) > 1e-6:
                raise RuntimeError(f"Region fixture did not apply Shift square: {square}")
        if scene == "window":
            targets = [e["windowSelection"] for e in actions]
            if targets != ["project", "export", "display", "display", "terminal", None]:
                raise RuntimeError(f"Window fixture did not exercise frontmost/window/display/cancel: {targets}")
    print(f"PASS: static redraw guard, {len(shots)} viewport captures, 42 scripted actions; "
          + ("hidden visibility verified" if hidden_supported else "hidden idle UNSUPPORTED, not accepted"))


if __name__ == "__main__":
    main()
