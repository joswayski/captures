#!/usr/bin/env python3
"""Real native first-run input on private X11; no permission bypass or real user data."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time

from instance_smoke import png


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = {**os.environ, "WGPU_BACKEND": "gl", "WINIT_X11_SCALE_FACTOR": "1", "XDG_SESSION_TYPE": "x11"}
    env.pop("WAYLAND_DISPLAY", None)
    children = []
    with (output / "processes.log").open("w") as log:
        def spawn(command, announce=False):
            process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE if announce else log, stderr=log)
            children.append(process)
            return process

        def run(*command):
            return subprocess.check_output(command, env=env, stderr=log, timeout=15)

        def wait(predicate, description):
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                if result := predicate():
                    return result
                time.sleep(.05)
            run("import", "-window", "root", str(output / "timeout.png"))
            raise AssertionError(description)

        def windows(pid, name="^Captures$"):
            result = subprocess.run(["xdotool", "search", "--all", "--onlyvisible", "--pid", str(pid), "--name", name],
                                    env=env, capture_output=True, text=True, timeout=5)
            assert result.returncode in (0, 1)
            return result.stdout.splitlines()

        def click(window, x, y):
            run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
                "mousemove", "--sync", "--window", window, str(x - 1), str(y),
                "mousemove_relative", "--sync", "1", "0", "sleep", ".15", "mousedown", "1",
                "sleep", ".15", "mouseup", "1")

        try:
            server = spawn(["Xvfb", "-displayfd", "1", "-screen", "0", "1280x900x24", "-nolisten", "tcp"], True)
            env["DISPLAY"] = ":" + server.stdout.readline().decode().strip()
            run("xdpyinfo")
            spawn(["openbox", "--sm-disable"])
            wait(lambda: b"window id" in run("xprop", "-root", "_NET_SUPPORTING_WM_CHECK"), "window manager ready")
            for appearance, hidden in (("dark", False), ("light", True)):
                root = output / appearance
                root.mkdir()
                settings = root / "fresh settings %.json"
                history = root / "history"
                source = root / "cold source.png"
                forwarded = root / "forwarded source.png"
                source.write_bytes(png(13, 7))
                forwarded.write_bytes(png(17, 11))
                common = [str(binary), "--live", "--history-root", str(history), "--settings-file", str(settings),
                          "--appearance", appearance]
                app = spawn(common + (["--scene", "idle"] if hidden else []) + ["--", str(source)])
                window = wait(lambda: windows(app.pid), "first-run setup must be visible even for idle launch")[0]
                time.sleep(1)
                run("import", "-window", window, str(output / f"onboarding-{appearance}.png"))
                assert not settings.exists(), "checking first run completed/wrote settings"
                assert not list(history.glob("*/metadata.json")), "cold media imported before setup"
                secondary = subprocess.run(common + ["--", str(forwarded)], env=env, capture_output=True, timeout=15)
                assert secondary.returncode == 0, secondary.stderr
                run("xdotool", "key", "super+shift+s")
                time.sleep(.4)
                assert not list(history.glob("*/metadata.json")), "capture/forwarding bypassed setup"
                assert len(windows(app.pid, ".*")) == 1, "capture selector/editor opened before setup"
                click(window, 500, 330)
                wait(lambda: settings.exists() and json.loads(settings.read_text()).get("onboarding_completed"),
                     "setup completion persisted")
                wait(lambda: len(list(history.glob("*/metadata.json"))) == 2, "queued cold and forwarded media imported")
                assert source.read_bytes() == png(13, 7) and forwarded.read_bytes() == png(17, 11)
                # Publication precedes the editor's asynchronous presentation.
                # Let that focus transfer settle before quitting from the root.
                time.sleep(1)
                run("xdotool", "windowactivate", "--sync", window, "windowfocus", "--sync", window,
                    "sleep", ".4", "key", "ctrl+q")
                assert app.wait(timeout=20) == 0
                again = spawn(common)
                window = wait(lambda: windows(again.pid), "completed profile workspace")[0]
                time.sleep(1)
                run("import", "-window", window, str(output / f"completed-{appearance}.png"))
                accepted_settings = settings.read_bytes()
                recovery_media = root / "during permission recovery.png"
                recovery_media.write_bytes(png(19, 9))
                click(window, 835, 126)  # Capture permissions, without an OS prompt.
                time.sleep(.5)
                secondary = subprocess.run(common + ["--", str(recovery_media)], env=env,
                                           capture_output=True, timeout=15)
                assert secondary.returncode == 0, secondary.stderr
                click(window, 196, 18)  # Navigation behind the dialog stays disabled.
                run("xdotool", "key", "super+shift+s")
                time.sleep(.5)
                assert len(list(history.glob("*/metadata.json"))) == 2, "recovery imported queued media"
                assert len(windows(again.pid, ".*")) == 1, "recovery launched capture/editor"
                run("import", "-window", window, str(output / f"permission-recovery-{appearance}.png"))
                click(window, 305, 440)  # Refresh is prompt-free and does not complete setup.
                time.sleep(.4)
                assert settings.read_bytes() == accepted_settings, "recovery changed setup/settings"
                click(window, 287, 484)  # Done, including when no upfront permission is required.
                wait(lambda: len(list(history.glob("*/metadata.json"))) == 3, "recovery releases queued media")
                assert recovery_media.read_bytes() == png(19, 9), "recovery changed the input"
                time.sleep(1)
                click(window, 196, 18)  # Real Preferences navigation, absent on setup.
                time.sleep(.4)
                run("import", "-window", window, str(output / f"preferences-{appearance}.png"))
                run("xdotool", "key", "ctrl+q")
                assert again.wait(timeout=20) == 0
                print(f"PASS {appearance}: first run, hidden={hidden}, capture gate, queued media, persistence, relaunch and permission recovery", flush=True)
            broken = output / "malformed.json"
            broken.write_text("invalid-json")
            app = spawn([str(binary), "--live", "--history-root", str(output / "error-history"),
                         "--settings-file", str(broken), "--appearance", "dark"])
            window = wait(lambda: windows(app.pid), "settings error visible")[0]
            time.sleep(1)
            run("import", "-window", window, str(output / "onboarding-error.png"))
            assert broken.read_text() == "invalid-json", "failed setup replaced malformed settings"
            # Another app may retain focus while setup is open (no repaint-driven activation).
            spawn(["xmessage", "-title", "Setup focus probe", "Other application"])
            probe = run("xdotool", "search", "--sync", "--onlyvisible", "--name", "^Setup focus probe$").decode().splitlines()[0]
            run("xdotool", "windowactivate", "--sync", probe, "windowfocus", "--sync", probe)
            time.sleep(.5)
            assert run("xdotool", "getwindowfocus").decode().strip() == probe, "setup stole focus"
            broken.unlink()  # User fixes the file; retry must reload and recheck.
            click(window, 500, 384)
            time.sleep(.5)
            run("import", "-window", window, str(output / "onboarding-retry.png"))
            assert not broken.exists(), "retry silently completed setup"
            click(window, 500, 330)
            wait(lambda: broken.exists() and json.loads(broken.read_text()).get("onboarding_completed"), "completion after retry")
            run("xdotool", "key", "ctrl+q")
            assert app.wait(timeout=20) == 0
            print("PASS malformed settings preserved, focus retained, explicit retry and completion", flush=True)
            (output / "result.json").write_text(json.dumps({"passed": True,
                "scope": "Private X11/software GL; not macOS TCC, Windows or physical acceptance."}, indent=2))
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
