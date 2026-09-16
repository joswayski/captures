"""Opt-in graphical Mac smoke test; no Screen Recording permission required."""
import json
import os
import platform
import subprocess
import time
import unittest
from pathlib import Path

from benchmark import BINARIES, LAB
from profiling import OBSERVER, package_app, probe, profile_trial, save, validate_resources, wait_json


@unittest.skipUnless(platform.system() == "Darwin" and os.environ.get("CAPTURES_NATIVE_PROFILE_TEST") == "1",
                     "requires opt-in logged-in Mac session and built helpers")
class NativeProfileTests(unittest.TestCase):
    def test_launchservices_gate_and_complete_resource_accounting(self):
        display = json.loads(subprocess.check_output([str(OBSERVER), "display"], text=True))
        folder = LAB / ".build" / f"native-profile-{time.time_ns()}"
        folder.mkdir()
        save(folder / "display.json", display)
        config = json.loads((LAB / ".build/public/dust-bottom-left.json").read_text())
        config.update(scale=display["scale"], mode="run", checkpointMs=0, durationMs=6400)
        # Retain raw evidence under .build for CI upload, including on failure.
        for label, executable in BINARIES.items():
            with self.subTest(candidate=label):
                gated = folder / f"{label}-gate"
                gated.mkdir()
                app, identifier = package_app(executable, gated, label)
                output = gated / "renderer.json"
                gate_config = {**config, "profileResources": True, "startGatePath": str(gated / "start.gate")}
                save(gated / "config.json", gate_config)
                before = probe(os.getpid())
                launched = json.loads(subprocess.check_output([str(OBSERVER), "launch", str(app),
                    str(gated / "config.json"), str(output)], text=True, timeout=40))
                pid = launched["pid"]
                try:
                    ready = wait_json(Path(str(output) + ".ready.json"), pid, 90)
                    self.assertEqual(ready["measurementProtocol"], 2)
                    self.assertEqual(ready["pid"], pid)
                    time.sleep(1)
                    self.assertFalse(output.exists(), "workload finished before gate release")
                    self.assertFalse(Path(str(output) + ".started.json").exists(), "workload started before release")
                    sample = probe(pid)
                    sample["unreadableBeforeLaunch"] = before["unreadableSameUidPids"]
                    save(gated / "resources.json", sample)
                    validate_resources(sample, ready)
                    print(f"{label}: gate held; isolated coalition={sample['resourceCoalitionId']}, "
                          f"processes={[p['name'] for p in sample['processes']]}", flush=True)
                finally:
                    if pid > 0:
                        subprocess.run([str(OBSERVER), "terminate", str(pid), identifier], check=True, timeout=20)
                measured = profile_trial(executable, config, folder / f"{label}-resources", label, "resources", 120)
                self.assertTrue(measured["fullCoalitionAttributed"])
                self.assertGreater(measured["summary"]["cpuSeconds"], 0)
                self.assertGreaterEqual(measured["summary"]["sampledSeconds"], 6.4)
                print(f"{label}: {json.dumps(measured)}", flush=True)

    def test_frame_observation_when_screen_recording_is_already_allowed(self):
        display = json.loads(subprocess.check_output([str(OBSERVER), "display"], text=True))
        if not display["screenCaptureAllowed"]:
            self.skipTest("Screen Recording not authorized; frame runtime remains unverified")
        folder = LAB / ".build" / f"native-profile-frames-{time.time_ns()}"
        folder.mkdir()
        save(folder / "display.json", display)
        config = json.loads((LAB / ".build/public/dust-bottom-left.json").read_text())
        config.update(scale=display["scale"], mode="run", checkpointMs=0, durationMs=6400)
        for label, executable in BINARIES.items():
            with self.subTest(candidate=label):
                measured = profile_trial(executable, config, folder / label, label, "frames", 120)
                self.assertGreater(measured["summary"]["observedChangedFrames"], 10)
                self.assertEqual(len(measured["motionWindows"]), 4)
                print(f"{label} frame smoke at {display['scale']}x: {json.dumps(measured)}", flush=True)


if __name__ == "__main__":
    unittest.main()
