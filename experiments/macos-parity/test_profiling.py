import copy
import unittest
from profiling import MIB, summarize_frames, summarize_resources, validate_resources


class ProfilingTests(unittest.TestCase):
    def test_exact_webkit_ownership_includes_launchd_children_and_rejects_partial(self):
        ready = {"pid": 10, "renderer": "tauri-dom-waapi", "webkitProcesses": [
            {"role": "webContent", "pid": 20}, {"role": "gpu", "pid": 30}, {"role": "network", "pid": 40}]}
        sample = {"rootPid": 10, "rootStartMach": 110, "resourceCoalitionId": 99,
                  "samplerCoalitionId": 3, "unreadableSameUidPids": [], "hostTimeNs": 500,
                  "processes": [{"pid": p, "parent": 1, "startMach": p + 100,
                                 "startHostTimeNs": p + 100} for p in (10, 20, 30, 40)]}
        validate_resources(sample, ready)
        for mutation, message in [
            (lambda s: s.update(resourceCoalitionId=3), "isolated"),
            (lambda s: s["processes"].pop(), "outside"),
            (lambda s: s["processes"][1].update(startMach=999), "membership"),
            (lambda s: s.update(unreadableSameUidPids=[123]), "unreadable"),
        ]:
            broken = copy.deepcopy(sample)
            mutation(broken)
            with self.assertRaisesRegex(ValueError, message):
                validate_resources(broken, ready, sample)
        with self.assertRaisesRegex(ValueError, "explicit WebKit"):
            validate_resources(sample, {**ready, "webkitProcesses": []})
        absent = copy.deepcopy(ready)
        absent["webkitProcesses"][1]["pid"] = 0
        absent["webkitProcesses"][2]["pid"] = 0
        local_only = {**sample, "processes": sample["processes"][:2]}
        validate_resources(local_only, absent)
        with self.assertRaisesRegex(ValueError, "membership"):
            validate_resources(sample, absent, local_only)  # pre-existing helper joined later
        validate_resources(sample, absent, local_only, complete_lifetimes=False)
        newborn = copy.deepcopy(sample)
        newborn["hostTimeNs"] = 600
        for process in newborn["processes"][2:]:
            process["startHostTimeNs"] = 550
        validate_resources(newborn, absent, local_only)
        with self.assertRaisesRegex(ValueError, "disappeared"):
            validate_resources({**local_only, "hostTimeNs": 700}, absent, newborn)
        newborn["processes"][2]["startHostTimeNs"] = 601
        with self.assertRaisesRegex(ValueError, "invalid identity"):
            validate_resources(newborn, absent, local_only)
        reused = copy.deepcopy(sample)
        reused["processes"][1]["startMach"] = 999
        with self.assertRaisesRegex(ValueError, "identity/membership"):
            validate_resources(reused, absent, local_only, complete_lifetimes=False)
        absent["webkitProcesses"][0]["pid"] = 0
        with self.assertRaisesRegex(ValueError, "Invalid explicit"):
            validate_resources(local_only, absent)

    def test_cpu_is_time_weighted_and_memory_peak_is_simultaneous(self):
        samples = []
        # A third helper is born between samples 0 and 1. Include its entire
        # .4-second CPU lifetime, not only the .2 seconds since first observation.
        # CPU deltas: .8 seconds in 1 second, then 2.9 seconds in 3 seconds.
        for timestamp, cpu, memory in [(0, (1, .3), (100, 10)),
                                       (1, (1.1, .8, .2), (20, 120, 20)),
                                       (4, (1.1, 3.5, .4), (50, 100, 30))]:
            samples.append({"hostTimeNs": int(timestamp * 1e9), "elapsedProbeNs": 1000,
                "processes": [{"userCpuNs": round(c * 1e9), "systemCpuNs": 0,
                    "physicalFootprintBytes": m * MIB, "residentBytes": (m + 5) * MIB,
                    "lifetimePeakPhysicalFootprintBytes": 500 * MIB}
                    for c, m in zip(cpu, memory)]})
        summary = summarize_resources(samples)
        self.assertAlmostEqual(summary["cpuAveragePercentOneCore"], 92.5)
        self.assertAlmostEqual(summary["cpuSeconds"], 3.7)
        self.assertEqual(summary["physicalFootprintPeakSampledMiB"], 180)
        self.assertEqual(summary["summedRSSPeakSampledMiB"], 195)
        self.assertAlmostEqual(summary["physicalFootprintTailGrowthMiBPerSecond"], 20 / 3)
        self.assertEqual(summary["processCount"], 3)

    def test_frame_timestamps_not_arrivals_idle_or_identical_redraws(self):
        start = 10_000_000_000
        def frame(ms, digest, status=0, arrival=None):
            return {"status": status, "displayTimeNs": start + round(ms * 1e6),
                    "arrivalHostTimeNs": start + round((arrival if arrival is not None else ms + 2) * 1e6),
                    "pixelHash": digest, "processingNs": 1000}
        frames = [frame(-1, "a"), frame(0, "a"), frame(10, "b", arrival=15),
                  frame(20, "b", arrival=40), frame(25, None, status=1),
                  frame(30, "c", arrival=50), frame(600, "d"),
                  frame(3190, "d"), frame(3210, "e"), frame(3220, "f")]
        frames.insert(0, frame(-2, None, status=4))  # started is metadata, not image data
        frames.insert(4, frames[3].copy())  # duplicate display timestamps aren't new frames
        result = summarize_frames({"complete": True, "requestedHz": 120, "frames": frames}, start,
            {"scenario": "settle-top", "durationMs": 6400, "cycleMs": 3200})
        a, b = result["motionWindows"]
        self.assertEqual(a["changedFrames"], 2)
        self.assertEqual(a["changedIntervalP50Ms"], 20)  # not the35ms callback spacing
        self.assertEqual(a["maxUnchangedSpanMs"], 550)
        self.assertEqual(b["changedFrames"], 2)
        self.assertAlmostEqual(result["summary"]["settleObservedChangedHz"], 4 / 1.16)
        self.assertFalse(result["exhaustivePresentationMeasurement"])
        self.assertEqual(result["rawStatusCounts"]["4"], 1)
        frames[2]["processingNs"] = 20_000_000
        self.assertTrue(summarize_frames({"complete": True, "requestedHz": 120, "frames": frames}, start,
            {"scenario": "settle-top", "durationMs": 6400, "cycleMs": 3200})["captureBackpressureDetected"])

    def test_missing_presentation_metadata_and_wrong_clock_fail_closed(self):
        config = {"scenario": "settle-bottom", "durationMs": 6400, "cycleMs": 3200}
        base = {"status": 0, "displayTimeNs": 100, "arrivalHostTimeNs": 101,
                "pixelHash": "a", "processingNs": 1}
        for frame, message in [({**base, "displayTimeNs": None}, "timestamp"),
                               ({**base, "arrivalHostTimeNs": 10**12}, "clocks")]:
            with self.assertRaisesRegex(ValueError, message):
                summarize_frames({"complete": True, "requestedHz": 120, "frames": [frame]}, 1, config)

    def test_dust_phase_boundaries_exclude_idle_and_duplicate_conflicts(self):
        start = 1_000_000_000
        config = {"scenario": "dust-top-right", "durationMs": 6400, "cycleMs": 3200}
        frames = [{"status": 0, "displayTimeNs": start + round(ms * 1e6),
                   "arrivalHostTimeNs": start + round(ms * 1e6) + 1,
                   "pixelHash": str(i), "processingNs": 1}
                  for i, ms in enumerate((-1, 0, 1299, 1300, 1799, 1800, 2379, 2380, 3200))]
        capture = {"complete": True, "requestedHz": 120, "frames": frames}
        result = summarize_frames(capture, start, config)
        dust, settle, second_dust, second_settle = result["motionWindows"]
        self.assertEqual(dust["changedFrames"], 2)
        self.assertEqual(settle["changedFrames"], 2)
        self.assertEqual(second_dust["changedFrames"], 1)
        self.assertEqual(second_settle["maxUnchangedSpanMs"], 580)
        with self.assertRaisesRegex(ValueError, "baseline"):
            summarize_frames({**capture, "frames": frames[1:]}, start, config)
        with self.assertRaisesRegex(ValueError, "Conflicting"):
            summarize_frames({**capture, "frames": [frames[0], {**frames[0], "pixelHash": "other"}]}, start, config)


if __name__ == "__main__":
    unittest.main()
