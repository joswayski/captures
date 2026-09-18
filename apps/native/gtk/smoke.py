#!/usr/bin/env python3
"""Functional GTK4 native-window probe; screenshots are separate from profiling."""
import argparse
import json
import os
import struct
import subprocess
import time
from pathlib import Path


def events(stdout):
    return [json.loads(line) for line in stdout.decode().splitlines() if line.strip()]


def run(binary, output, name, arguments, input_text=None):
    process = subprocess.Popen([str(binary), *arguments], stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, env=os.environ.copy())
    if input_text:
        time.sleep(1)
        if os.environ.get("GDK_BACKEND") == "wayland":
            subprocess.run(["wtype", "-k", "tab"], check=True)
            subprocess.run(["wtype", input_text], check=True)
        else:
            window = subprocess.check_output(
                ["xdotool", "search", "--name", "Captures GTK4 comparison workbench"],
                text=True).splitlines()[-1]
            subprocess.run(["xdotool", "mousemove", "--window", window, "300", "106", "click", "1"], check=True)
            subprocess.run(["xdotool", "type", "--window", window, "--delay", "20", input_text], check=True)
    stdout, stderr = process.communicate(timeout=35)
    (output / f"{name}.jsonl").write_bytes(stdout)
    (output / f"{name}.stderr.txt").write_bytes(stderr)
    result = events(stdout)
    if process.returncode:
        raise RuntimeError(f"{name} exited {process.returncode}: {stderr.decode(errors='replace')}")
    if not any(row["event"] == "ready" for row in result) or not any(row["event"] == "exit" for row in result):
        raise RuntimeError(f"{name}: missing ready or clean exit event")
    return result


def png_size(path):
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise RuntimeError(f"{path}: not a PNG")
    return struct.unpack(">II", data[16:24])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=False)

    rejected = subprocess.run([str(binary), "--appearance", "system"], capture_output=True,
                              timeout=10)
    if rejected.returncode == 0 or b"Invalid scene, appearance" not in rejected.stderr:
        raise RuntimeError("system appearance must fail explicitly in this bounded candidate")

    # A visible static scene and a genuinely hidden root must settle without a
    # recurring application-owned snapshot callback.
    for scene in ("preferences", "idle"):
        rows = run(binary, args.output, f"{scene}-idle",
                   ["--scene", scene, "--quit-after", "5"])
        lifecycle = next(row["detail"] for row in rows if row["event"] == "lifecycle-check")
        exit_event = next(row["detail"] for row in rows if row["event"] == "exit")
        expected = scene != "idle"
        if lifecycle["nativeVisible"] is not expected or lifecycle["nativeMapped"] is not expected:
            raise RuntimeError(f"{scene}: native visibility mismatch: {lifecycle}")
        if exit_event["snapshotPasses"] > 3:
            raise RuntimeError(f"{scene}: recurring redraw after settle: {exit_event}")

    wayland = os.environ.get("GDK_BACKEND") == "wayland"
    typed = "Keyboard GTK4 focus probe"
    rows = run(binary, args.output, "preferences-keyboard",
               ["--scene", "preferences", "--quit-after", "4"],
               input_text=None if wayland else typed)
    changes = [row["detail"] for row in rows if row["event"] == "text-changed"]
    if not wayland and (not changes or changes[-1]["value"] != typed or not changes[-1]["focused"]):
        raise RuntimeError(f"keyboard editing/focus did not reach the search field: {changes[-3:]}")
    accessibility = next(row["detail"] for row in rows if row["event"] == "declared-accessibility")
    if (accessibility["rootRole"], accessibility["searchRole"], accessibility["annotationRole"]) != (
            "group", "text-box", "text-box") or "not AT-SPI" not in accessibility["acceptance"]:
        raise RuntimeError(f"unexpected accessibility declarations: {accessibility}")

    shots = {
        "preferences-dark": ["--scene", "preferences"],
        "preferences-light": ["--scene", "preferences", "--appearance", "light", "--theme", "cobalt"],
        "preferences-focused": ["--scene", "preferences", "--theme", "ember", "--exercise", "--screenshot-after", "3"],
        "history-empty": ["--scene", "history", "--history-count", "0"],
        "history-100": ["--scene", "history", "--history-count", "100"],
        "history-1000": ["--scene", "history", "--history-count", "1000", "--exercise", "--screenshot-after", "3"],
        "hud-paused-muted": ["--scene", "hud", "--floating", "--exercise", "--screenshot-after", "3"],
        "preview-visible": ["--scene", "preview", "--floating"],
        "preview-hidden": ["--scene", "preview", "--floating", "--exercise", "--screenshot-after", "3"],
        "preview-reduced": ["--scene", "preview", "--floating", "--exercise", "--reduced-motion", "--screenshot-after", "3"],
        "editor-transformed": ["--scene", "editor", "--exercise", "--screenshot-after", "7"],
    }
    contracts = {}
    for name, arguments in shots.items():
        screenshot = args.output / f"{name}.png"
        rows = run(binary, args.output, name, [*arguments, "--screenshot", str(screenshot)])
        contract = next(row["detail"] for row in rows if row["event"] == "probe-contract")
        contracts[name] = contract
        expected_appearance = "light" if "--appearance" in arguments else "dark"
        if (contract["selectedAppearance"] != expected_appearance or
                (contract["textureWidth"], contract["textureHeight"]) != (2048, 1152) or
                contract["exerciseScheduleSeconds"] != "2,6,10,14,18,22"):
            raise RuntimeError(f"{name}: mismatched shared probe contract: {contract}")
        saved = next(row["detail"] for row in rows if row["event"] == "screenshot-saved")
        if min(png_size(screenshot)) < 480:
            raise RuntimeError(f"{name}: undersized screenshot")
        floating = "--floating" in arguments
        if saved["cornerAlpha"] != (0 if floating else 255):
            raise RuntimeError(f"{name}: unexpected render-target corner alpha: {saved}")
        actions = [row["detail"] for row in rows if row["event"] == "scripted-action"]
        if "--exercise" in arguments and not actions:
            raise RuntimeError(f"{name}: expected exercised non-default state")
        if name == "history-1000" and (actions[-1].get("cycle") != 0 or
                                       actions[-1].get("selectedRow") != 999 or
                                       actions[-1].get("scroll", 0) <= 0):
            raise RuntimeError("history did not select/scroll to its non-default endpoint")
        if name == "editor-transformed" and (actions[-1]["rotation"] != 15 or actions[-1]["zoom"] != .75):
            raise RuntimeError("editor transform did not reach the expected asymmetric state")

    dark = contracts["preferences-dark"]
    light = contracts["preferences-light"]
    if dark["entrySurface"] == light["entrySurface"] or dark["entryText"] == light["entryText"]:
        raise RuntimeError("entry surface/text did not follow light and dark tokens")
    if dark["entryFocus"] == light["entryFocus"]:
        raise RuntimeError("entry focus did not follow mustard and cobalt theme tokens")

    for scene in ("preferences", "history", "hud", "preview", "editor"):
        rows = run(binary, args.output, f"{scene}-exercise",
                   ["--scene", scene, "--exercise", "--quit-after", "24"])
        actions = [row["detail"] for row in rows if row["event"] == "scripted-action"]
        if [row["cycle"] for row in actions] != list(range(6)):
            raise RuntimeError(f"{scene}: missing/repeated actions: {actions}")
        for action, expected in zip(actions, (2, 6, 10, 14, 18, 22)):
            if abs(action["elapsedSeconds"] - expected) > .35:
                raise RuntimeError(f"{scene}: action cadence drifted from shared probes: {actions}")
        if scene == "preview" and len([row for row in rows if row["event"] == "animation-settled"]) != 6:
            raise RuntimeError("preview retained or skipped a frame callback")

    backend = next(row["detail"]["backend"] for row in rows if row["event"] == "ready")
    input_result = ("keyboard injection unavailable in the headless Wayland compositor; editable focus/text "
                    "covered by scripted GTK actions" if wayland else "keyboard focus/editing")
    print(f"PASS {backend}: hidden/visible lifecycle, static redraw guard, {input_result}, "
          "accessibility declarations, 11 rendered states with alpha checks, 30 scripted actions")


if __name__ == "__main__":
    main()
