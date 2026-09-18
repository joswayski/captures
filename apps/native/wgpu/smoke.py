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


def run(binary, output, name, arguments, allow_unsupported_hidden=False):
    result = subprocess.run([str(binary), *arguments], capture_output=True, timeout=40)
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
    shots = {
        "live-empty": ["--live", "--history-root", str(args.output / "empty-history"), "--appearance", "light"],
        "live-populated": ["--live", "--history-root", str(populated)],
        "preferences-dark": ["--scene", "preferences"],
        "preferences-light": ["--scene", "preferences", "--appearance", "light", "--theme", "cobalt"],
        "history-empty": ["--scene", "history", "--history-count", "0"],
        "history-populated": ["--scene", "history", "--history-count", "1000"],
        "hud-running": ["--scene", "hud", "--appearance", "light"],
        "hud-paused": ["--scene", "hud", "--exercise", "--screenshot-after", "3"],
        "preview-before": ["--scene", "preview"],
        "preview-after": ["--scene", "preview", "--exercise", "--screenshot-after", "3"],
        "preview-reduced": ["--scene", "preview", "--exercise", "--reduced-motion", "--screenshot-after", "3"],
        "preview-floating": ["--scene", "preview", "--floating"],
        "editor-transformed": ["--scene", "editor", "--exercise", "--screenshot-after", "7"],
    }
    for name, arguments in shots.items():
        screenshot = args.output / f"{name}.png"
        events = run(binary, args.output, name, [*arguments, "--screenshot", str(screenshot)])
        if not any(e["event"] == "screenshot-saved" for e in events):
            raise RuntimeError(f"{name}: missing screenshot acknowledgement")
        data = screenshot.read_bytes()
        if data[:8] != b"\x89PNG\r\n\x1a\n" or min(struct.unpack(">II", data[16:24])) < 400:
            raise RuntimeError(f"{name}: invalid or undersized viewport capture")

    # Verify all six scheduled actions, not only that the process survived.
    for scene in ["preferences", "history", "hud", "preview", "editor"]:
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
    print("PASS: static redraw guard, 13 viewport captures, 30 scripted actions; "
          + ("hidden visibility verified" if hidden_supported else "hidden idle UNSUPPORTED, not accepted"))


if __name__ == "__main__":
    main()
