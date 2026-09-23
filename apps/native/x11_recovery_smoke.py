#!/usr/bin/env python3
"""Recover/discard disposable real recordings on private X11, not physical acceptance."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import uuid

from history_fixture import write_history


def write_bundle(root, kind, created):
    bundle = root / str(uuid.uuid4())
    bundle.mkdir(parents=True)
    segments = []
    for index, color in enumerate(("red", "blue")):
        video = bundle / f"segment-{index:03}.mp4"
        subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i",
                        f"color=c={color}:size=160x90:rate=15:duration=0.8",
                        "-c:v", "mpeg4", str(video)], check=True, timeout=15)
        segments.append({"index": index, "relative_path": video.name, "started_at_ms": index * 800,
                         "duration_ms": 800, "width": 160, "height": 90,
                         "size_bytes": video.stat().st_size, "complete": True})
    (bundle / "manifest.json").write_text(json.dumps({
        "schema_version": 1, "session_id": bundle.name, "created_at_ms": created,
        "updated_at_ms": created, "state": "failed", "segments": segments,
        "last_error": "Interrupted during finalization", "options": {
            "kind": kind, "target": {"type": "display", "display_id": "disposable-fixture"},
            "frames_per_second": 15, "max_resolution": "original", "countdown_seconds": 0,
            "show_cursor": False,
        },
    }))
    return bundle


def digest_tree(root):
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in root.rglob("*") if path.is_file()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    assert os.geteuid() != 0, "run unprivileged for the History permission failure"
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = {**os.environ, "WGPU_BACKEND": "gl", "WINIT_X11_SCALE_FACTOR": "1"}
    env.pop("WAYLAND_DISPLAY", None)
    children = []
    with (output / "processes.log").open("w") as log:
        def spawn(command, announce=False):
            child = subprocess.Popen(command, env=env, stdout=subprocess.PIPE if announce else log, stderr=log)
            children.append(child)
            return child

        def run(*command):
            return subprocess.check_output(command, env=env, stderr=log, timeout=15)

        def wait(condition):
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                if result := condition():
                    return result
                time.sleep(.05)
            run("import", "-window", "root", str(output / "timeout.png"))
            raise AssertionError("recovery did not settle")

        try:
            server = spawn(["Xvfb", "-displayfd", "1", "-screen", "0", "1280x1000x24", "-nolisten", "tcp"], True)
            env["DISPLAY"] = ":" + server.stdout.readline().decode().strip()
            spawn(["openbox", "--sm-disable"])
            time.sleep(.5)
            for appearance in ("dark", "light"):
                with tempfile.TemporaryDirectory(prefix="recovery-") as temporary:
                    root = Path(temporary)
                    history = root / "history"
                    write_history(history)
                    original_history = digest_tree(history)
                    recovery = root / "recording-recovery"
                    now = int(time.time() * 1000)
                    video = write_bundle(recovery, "video", now)
                    gif = write_bundle(recovery, "gif", now - 1000)
                    corrupt = recovery / str(uuid.uuid4())
                    corrupt.mkdir()
                    (corrupt / "manifest.json").write_text("not valid JSON; preserve me")
                    video_before, gif_before = digest_tree(video), digest_tree(gif)
                    tools = root / "bin"
                    tools.mkdir()
                    gate, entered = root / "encode-gate", root / "encode-entered"
                    wrapper = tools / "ffmpeg"
                    wrapper.write_text(f'''#!/bin/sh
case "$*" in
  *assembled.gif*) if [ -f "{gate}" ]; then
    touch "{entered}"
    while [ -f "{gate}" ]; do sleep 0.05; done
  fi;;
esac
exec /usr/bin/ffmpeg "$@"
''')
                    wrapper.chmod(0o755)
                    previous_path = env["PATH"]
                    env["PATH"] = str(tools) + os.pathsep + previous_path
                    app = spawn([str(binary), "--live", "--history-root", str(history),
                                 "--settings-file", str(root / "settings.json"), "--appearance", appearance])
                    env["PATH"] = previous_path
                    window = run("xdotool", "search", "--sync", "--onlyvisible", "--pid", str(app.pid),
                                 "--name", "^Captures$").decode().splitlines()[0]
                    time.sleep(1)

                    def click(x, y):
                        run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
                            "mousemove", "--sync", "--window", window, str(x - 1), str(y),
                            "mousemove_relative", "--sync", "1", "0",
                            "sleep", ".1", "mousedown", "1", "sleep", ".1", "mouseup", "1")
                        time.sleep(.3)

                    def screenshot(name):
                        run("import", "-window", window, str(output / f"{appearance}-{name}.png"))

                    screenshot("populated")
                    click(134, 503)  # First bundle's Discard… action.
                    screenshot("confirmation")
                    assert digest_tree(video) == video_before
                    run("xdotool", "key", "Escape")
                    time.sleep(.2)
                    assert digest_tree(video) == video_before
                    click(134, 503)
                    click(395, 410)  # Keep recording.
                    assert digest_tree(video) == video_before
                    click(134, 503)
                    click(545, 410)  # Discard permanently.
                    wait(lambda: not video.exists())
                    assert digest_tree(gif) == gif_before
                    assert digest_tree(history) == original_history
                    screenshot("discarded")
                    gate.touch()
                    click(50, 531)  # Recover the remaining GIF.
                    wait(entered.exists)
                    screenshot("busy")
                    click(65, 461)  # Cancel recovery while the encoder is gated.
                    wait(lambda: not list(gif.glob(".recovery-*")))
                    gate.unlink()
                    assert digest_tree(gif) == gif_before
                    assert digest_tree(history) == original_history
                    screenshot("cancelled")
                    click(180, 398)  # Refresh clears the previous action's error.
                    history.chmod(0o555)
                    try:
                        click(50, 503)
                        wait(lambda: (gif / "publication-intent-v1.json").exists())
                        time.sleep(.5)
                        assert digest_tree(history) == original_history
                        screenshot("publication-error")
                    finally:
                        history.chmod(0o755)
                    click(180, 398)
                    if appearance == "dark":
                        entered.unlink()
                        gate.touch()
                    click(50, 503)  # Retry the same publication intent after restoring access.
                    if appearance == "dark":
                        wait(entered.exists)
                        click(100, 595)  # A newer History selection must prevent automatic editor focus.
                        gate.unlink()
                    wait(lambda: len(list(history.glob("*/metadata.json"))) == 2)
                    entries = [json.loads(path.read_text()) for path in history.glob("*/metadata.json")]
                    entry = next(entry for entry in entries if entry["kind"] == "gif")
                    media = next((history / entry["id"]).glob("*.gif"))
                    assert (entry["width"], entry["height"]) == (160, 90), entry
                    assert not entry["has_system_audio"] and not entry["has_microphone_audio"]
                    for at, channel in (("0.2", 0), ("1.1", 2)):
                        pixels = run("ffmpeg", "-v", "error", "-ss", at, "-i", str(media), "-frames:v", "1",
                                     "-vf", "scale=1:1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-")
                        assert len(pixels) == 3 and pixels[channel] > 150, pixels
                        assert all(pixels[channel] > pixels[c] + 60 for c in range(3) if c != channel), pixels
                    if appearance == "dark":
                        time.sleep(.5)
                        found = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "Recording editor"],
                                               env=env, stdout=subprocess.PIPE, stderr=log, timeout=5)
                        assert found.returncode == 1, "stale recovery completion stole editor focus"
                        click(100, 595)  # Explicitly select the newly recovered GIF.
                        click(800, 195)  # Explicit Edit recording.
                    editor = run("xdotool", "search", "--sync", "--onlyvisible", "--name", "Recording editor").decode().splitlines()[0]
                    time.sleep(1)
                    run("import", "-window", editor, str(output / f"{appearance}-recovered-editor.png"))
                    run("xdotool", "windowactivate", "--sync", editor, "key", "alt+F4")
                    time.sleep(.5)
                    screenshot("recovered")
                    assert (corrupt / "manifest.json").read_text() == "not valid JSON; preserve me"
                    for path, digest in original_history.items():
                        assert hashlib.sha256((history / path).read_bytes()).hexdigest() == digest
                    run("xdotool", "windowactivate", "--sync", window, "key", "alt+F4")
                    assert app.wait(timeout=10) == 0
                    print(f"PASS {appearance}: discard confirmation/cancel, encoder cancellation, History failure/retry, GIF red-blue frames/editor, unrelated files intact", flush=True)
            (output / "result.json").write_text(json.dumps({"passed": True, "appearances": 2,
                "scope": "Private X11/software GL with disposable real MP4/GIF, not physical acceptance."}, indent=2))
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
