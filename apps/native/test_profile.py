import unittest
import tempfile
import ctypes
import os
from pathlib import Path
from profile import (cpu_seconds, summarize, events_at, filetime_seconds,
                     linux_sample, validate_renderer_platform, windows_sample,
                     workloads_for, sample)


class ProfileTests(unittest.TestCase):
    def test_ps_time_formats(self):
        self.assertAlmostEqual(cpu_seconds("2:03.45"), 123.45)
        self.assertAlmostEqual(cpu_seconds("1:02:03.45"), 3723.45)
        self.assertAlmostEqual(cpu_seconds("2-01:02:03.45"), 176523.45)
        with self.assertRaises(ValueError):
            cpu_seconds("garbage")

    def test_cpu_uses_counter_delta_not_lifetime_average(self):
        result = summarize([
            {"monotonic": 10, "cpuSeconds": 8, "rssBytes": 300},
            {"monotonic": 14, "cpuSeconds": 8.2, "rssBytes": 900},
            {"monotonic": 20, "cpuSeconds": 8.5, "rssBytes": 400},
        ])
        self.assertEqual(result["processCPUPercentOneCore"], 5)
        self.assertEqual(result["medianProcessRSSBytes"], 400)
        self.assertEqual(result["peakSampledProcessRSSBytes"], 900)

    def test_partial_or_invalid_intervals_fail(self):
        row = {"monotonic": 10, "cpuSeconds": 8, "rssBytes": 300}
        with self.assertRaises(ValueError):
            summarize([row])
        with self.assertRaises(ValueError):
            summarize([row, row])
        with self.assertRaises(ValueError):
            summarize([row, {**row, "monotonic": 11, "cpuSeconds": 7}])

    def test_reader_waits_for_complete_record_but_rejects_malformed_record(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "events.jsonl"
            path.write_text('{"event":"ready"}\n{"event":')
            self.assertEqual(events_at(path), [{"event": "ready"}])
            path.write_text('{"event":\n')
            with self.assertRaises(ValueError):
                events_at(path)

    def test_workloads_are_renderer_specific(self):
        appkit = workloads_for("appkit")
        wgpu = workloads_for("wgpu")
        self.assertEqual(len(appkit), 10)
        self.assertIn(("preview", True, True), appkit)
        self.assertNotIn(("preview", True, True), wgpu)
        self.assertIn(("editor", False, False), wgpu)
        self.assertIn(("editor", True, False), wgpu)

    def test_platform_validation(self):
        validate_renderer_platform("appkit", "Darwin")
        for system in ("Darwin", "Windows", "Linux"):
            validate_renderer_platform("wgpu", system)
        with self.assertRaises(ValueError):
            validate_renderer_platform("appkit", "Linux")
        with self.assertRaises(ValueError):
            validate_renderer_platform("wgpu", "Plan9")

    def test_linux_counter_and_rss_units(self):
        files = {"/proc/42/stat": "42 (name with spaces) S " + " ".join(
                     ["0"] * 10 + ["125", "75"] + ["0"] * 20),
                 "/proc/42/status": "Name:\ttest\nVmRSS:\t321 kB\n"}
        result = linux_sample(42, clock=lambda: 7.0, read_text=files.__getitem__, clock_ticks=100)
        self.assertEqual(result, {"monotonic": 7.0, "cpuSeconds": 2.0,
                                  "rssBytes": 321 * 1024})

    def test_filetime_conversion_preserves_high_word(self):
        self.assertEqual(filetime_seconds(1, 0), 2 ** 32 / 10_000_000)

    def test_live_process_sampler_uses_native_api(self):
        # Runs on each CI OS; mocks alone cannot catch a wrong Win32 signature.
        result = sample(os.getpid())
        self.assertGreater(result.get("workingSetBytes", result.get("rssBytes", 0)), 0)
        self.assertGreaterEqual(result["cpuSeconds"], 0)
        self.assertGreater(result["monotonic"], 0)

    def test_windows_sample_units_elapsed_and_handle_close(self):
        class FileTime(ctypes.Structure):
            _fields_ = [("dwLowDateTime", ctypes.c_uint32),
                        ("dwHighDateTime", ctypes.c_uint32)]

        class Memory(ctypes.Structure):
            _fields_ = [("cb", ctypes.c_uint32), ("WorkingSetSize", ctypes.c_size_t)]

        class Kernel:
            closed = []
            def OpenProcess(self, access, inherit, pid): return 99
            def GetProcessTimes(self, handle, creation, exit_time, kernel, user):
                kernel._obj.dwHighDateTime = 1
                user._obj.dwLowDateTime = 10_000_000
                return 1
            def CloseHandle(self, handle): self.closed.append(handle)

        class Psapi:
            def GetProcessMemoryInfo(self, handle, memory, size):
                memory._obj.WorkingSetSize = 123456
                return 1

        kernel = Kernel()
        api = (kernel, Psapi(), FileTime, Memory)
        first = windows_sample(42, api=api, clock=lambda: 10.0)
        second = {**first, "monotonic": 11.0, "cpuSeconds": first["cpuSeconds"] + .25}
        result = summarize([first, second])
        self.assertEqual(first["workingSetBytes"], 123456)
        self.assertEqual(result["sampledSeconds"], 1.0)
        self.assertEqual(result["processCPUPercentOneCore"], 25.0)
        self.assertEqual(kernel.closed, [99])

    def test_windows_handle_closes_when_memory_read_fails(self):
        class Value(ctypes.Structure):
            _fields_ = [("dwLowDateTime", ctypes.c_uint32),
                        ("dwHighDateTime", ctypes.c_uint32)]
        class Memory(ctypes.Structure):
            _fields_ = [("cb", ctypes.c_uint32), ("WorkingSetSize", ctypes.c_size_t)]
        class Kernel:
            closed = False
            def OpenProcess(self, *args): return 12
            def GetProcessTimes(self, *args): return 1
            def CloseHandle(self, handle): self.closed = True
        class Psapi:
            def GetProcessMemoryInfo(self, *args): return 0
        kernel = Kernel()
        with self.assertRaises(OSError):
            windows_sample(1, api=(kernel, Psapi(), Value, Memory))
        self.assertTrue(kernel.closed)


if __name__ == "__main__":
    unittest.main()
