#!/usr/bin/env python3
"""Real recording/screenshot/recovery integration in the disposable X11 lab."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

from capture_check import benchmark, eventually, lock, run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lab", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    args = parser.parse_args()
    os.environ.update(json.loads((args.lab / "environment.json").read_text()))
    args.artifacts.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="gpui-recording-check-") as directory:
        profile = Path(directory)
        env = benchmark.profile_environment(profile, "dark")
        env.pop("CAPTURES_GPUI_DATA")
        data = profile / "data/captures-gpui"
        data.mkdir(parents=True)
        output = profile / "published-recordings"
        (data / "settings.json").write_text(json.dumps({
            "appearance": "dark", "output_directory": str(output),
            "auto_copy_to_clipboard": False,
            "recording": {"countdown_seconds": 0, "video_fps": 15,
                          "capture_system_audio": False, "microphone_device_id": None},
        }))
        with (profile / "app.log").open("w+") as log:
            def launch(*arguments):
                return subprocess.Popen([str(args.binary.resolve()), *arguments], env=env,
                                        stdout=log, stderr=log, start_new_session=True)

            process = launch("--previews", str((args.lab / "fixture.png").resolve()))

            def visible(title):
                return benchmark.find_window(f"^Captures GPUI {title}$", process.pid)

            def click(window, x, y):
                run("xdotool", "mousemove", "--window", window, str(x), str(y), "click", "1")

            try:
                eventually(lambda: visible("Previews"), "initial preview")
                subprocess.run([str(args.binary.resolve()), "--record"], env=env, check=True)
                eventually(lambda: visible("Select target"), "recording selector")
                time.sleep(1)
                run("xdotool", "mousemove", "320", "180", "mousedown", "1", "sleep", ".1",
                    "mousemove", "1050", "630", "sleep", ".2", "mouseup", "1", "key", "Return")
                hud = eventually(lambda: visible("Recording controls"), "recording HUD")
                manifest_path = eventually(
                    lambda: next((data / "recording-drafts").glob("*/manifest.json"), None),
                    "draft in default XDG profile, not output directory")

                def manifest():
                    return json.loads(manifest_path.read_text())

                eventually(lambda: manifest()["state"] == "recording", "real xcap recording", 30)
                assert not visible("Previews"), "excluded preview reopened when recording started"
                guides = run("xdotool", "search", "--onlyvisible", "--all", "--pid", str(process.pid),
                             "--name", "^Captures GPUI Recording region ").splitlines()
                assert len(guides) == 4

                def guide_bounds():
                    result = []
                    for guide in guides:
                        fields = dict(line.split("=", 1) for line in
                                      run("xdotool", "getwindowgeometry", "--shell", guide).splitlines())
                        result.append(tuple(int(fields[key]) for key in ("X", "Y", "WIDTH", "HEIGHT")))
                    return sorted(result)

                expected_guides = sorted([(320, 180, 730, 3), (320, 627, 730, 3),
                                          (320, 180, 3, 450), (1047, 180, 3, 450)])
                eventually(lambda: guide_bounds() == expected_guides, "guide bounds match selected region")
                run("xdotool", "windowmove", hud, "490", "800")
                time.sleep(2)
                run("import", "-window", "root", str(args.artifacts / "gpui-recording-integrated.png"))
                print("PASS: recording started with four region guides and preview exclusion", flush=True)

                # The real parent callback pauses the segment, opens capture, and
                # resumes after Escape, without creating a screenshot/history row.
                click(hud, 368, 60)
                selector = eventually(lambda: visible("Select target"), "screenshot hook opens selector", 30)
                run("xdotool", "windowactivate", "--sync", selector)
                time.sleep(.3)
                run("xdotool", "key", "Escape")
                eventually(lambda: not visible("Select target"), "screenshot selector closes on Escape")
                eventually(lambda: len(manifest()["segments"]) == 2
                           and manifest()["state"] == "recording", "screenshot cancellation resumes", 30)
                assert not visible("Previews"), "excluded preview reopened on screenshot cancellation"
                assert not list((data / "unsaved").glob("*.png"))
                print("PASS: screenshot callback cancellation resumed into a second segment", flush=True)
                time.sleep(2)
                lock(True)
                eventually(lambda: manifest()["state"] == "paused", "lock auto-pause", 30)
                completed = manifest()["segments"]
                assert len(completed) == 2 and all(segment["complete"] for segment in completed)
                assert all(segment["duration_ms"] > 0 and segment["size_bytes"] > 0 for segment in completed)
                run("import", "-window", "root", str(args.artifacts / "gpui-recording-lock-integrated.png"))
                lock(False)
                print("PASS: lock preserved two completed nonempty segments", flush=True)

                # Interrupt a safely paused process. Recovery must find the same
                # default-profile manifest after restart and preserve both segments.
                benchmark.stop(process)
                process = launch("--background")
                recovery = eventually(lambda: visible("Recording recovery"), "default-profile recovery")
                time.sleep(1)
                run("import", "-window", recovery, str(args.artifacts / "gpui-recording-recovery.png"))
                click(recovery, 541, 177)
                saved = eventually(lambda: next(output.glob("*.mp4"), None), "recovered MP4 publication", 40)
                probe = json.loads(run("ffprobe", "-v", "error", "-show_streams", "-show_format",
                                       "-of", "json", str(saved)))
                video = next(stream for stream in probe["streams"] if stream["codec_type"] == "video")
                assert (video["width"], video["height"]) == (730, 450), video
                assert float(probe["format"]["duration"]) > 1
                eventually(lambda: not manifest_path.exists(), "draft removed only after publication")
                print("PASS: real 730×450 recording; four region guides; screenshot cancellation resumes; "
                      "lock finalizes two segments; default-XDG restart/recovery publishes a probed MP4")
            except Exception:
                for manifest in (data / "recording-drafts").glob("*/manifest.json"):
                    print(manifest.read_text())
                log.flush()
                log.seek(0)
                print(log.read())
                raise
            finally:
                lock(False)
                benchmark.stop(process)


if __name__ == "__main__":
    main()
