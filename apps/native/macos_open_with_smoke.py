#!/usr/bin/env python3
"""LaunchServices cold/warm/forced-secondary Open With in a disposable dev bundle.

Requires macOS graphical login. Registers only a disposable development .app;
never changes default handlers, installed Preview, or normal native data.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time

from instance_smoke import png
from package import stage


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--resources", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    bundle, binary = stage("macos", args.binary, output / "package space", args.resources)
    subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(bundle)], check=True)
    license = subprocess.run([str(binary), "--font-license"], capture_output=True, text=True, check=True).stdout
    assert "SIL OPEN FONT LICENSE" in license
    history = output / "history"
    settings = output / "settings.json"
    settings.write_text(json.dumps({"settings_schema_version": 5,
        "onboarding_completed": True,
        "appearance": "dark", "theme": "mustard", "output_directory": str(output / "exports"),
        "region_shortcut": "Ctrl+Shift+F7", "window_shortcut": "Ctrl+Shift+F8",
        "display_shortcut": "Ctrl+Shift+F9", "new_capture_shortcut": "Ctrl+Shift+F10",
        "launch_at_login": False, "show_mini_previews": False, "auto_copy_to_clipboard": False}))
    files = [output / name for name in ("cold é space.png", "warm %f.png", "secondary.png")]
    for index, file in enumerate(files):
        file.write_bytes(png(13 + index * 2, 7 + index))
    originals = [file.read_bytes() for file in files]
    common = ["--history-root", str(history)]
    log = output / "primary.log"
    env = {**os.environ, "NSUnbufferedIO": "YES"}
    command = ["open", "-n", "-W", "-a", str(bundle), "--stdout", str(log), "--stderr", str(log),
               str(files[0]), "--args", *common, "--settings-file", str(settings), "--quit-after", "35"]
    primary = subprocess.Popen(command, env=env)

    def wait_file(file):
        until = time.monotonic() + 15
        while time.monotonic() < until:
            assert primary.poll() is None, "LaunchServices primary exited early"
            for metadata in history.glob("*/metadata.json"):
                try:
                    entry = json.loads(metadata.read_text())
                    if Path(entry.get("saved_path", "")).resolve() == file.resolve():
                        index = files.index(file)
                        assert (entry["width"], entry["height"]) == (13 + index * 2, 7 + index)
                        return
                except (OSError, json.JSONDecodeError):
                    pass
            time.sleep(.05)
        raise AssertionError(f"No History item for {file}; {log.read_text() if log.exists() else 'no app log'}")

    try:
        wait_file(files[0])
        subprocess.run(["open", "-a", str(bundle), str(files[1])], check=True, timeout=10)
        wait_file(files[1])
        # Force a NEW bundle process while the profile is owned. Its Apple event
        # must be forwarded before it exits, without loading its settings file.
        unused = output / "must-not-create.json"
        secondary_log = output / "secondary.log"
        subprocess.run(["open", "-n", "-W", "-a", str(bundle), "--stdout", str(secondary_log),
                        "--stderr", str(secondary_log), str(files[2]), "--args", *common,
                        "--settings-file", str(unused), "--quit-after", "5"],
                       check=True, timeout=15, env=env)
        wait_file(files[2])
        assert not unused.exists(), "secondary initialized settings"
        for line in secondary_log.read_text().splitlines():
            if line.startswith("{"):
                assert json.loads(line).get("event") != "ready", "secondary created a workbench"
        subprocess.run(["open", "-a", str(bundle), str(files[0])], check=True, timeout=10)
        time.sleep(.5)
        assert len(list(history.glob("*/metadata.json"))) == 3
        assert [file.read_bytes() for file in files] == originals
        assert primary.wait(timeout=40) == 0
    finally:
        # Timed normal quit also cleans up after an assertion. Never send a
        # process-name kill that could hit a developer's other native instance.
        primary.wait(timeout=45)
        register = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
        subprocess.run([register, "-u", str(bundle)], check=True)
    checks = ["packaged-resources", "cold-apple-event", "running-app-apple-event",
              "secondary-apple-event-forwarded-before-exit", "secondary-no-workbench-or-settings",
              "alias-no-duplicate", "asymmetric-import-dimensions", "source-bytes-unchanged"]
    (output / "result.json").write_text(json.dumps({"passed": True, "checks": checks}, indent=2) + "\n")
    print(f"PASS native LaunchServices Open With: {len(checks)} checks")


if __name__ == "__main__":
    main()
