import unittest
import tempfile
from pathlib import Path
from profile import cpu_seconds, summarize, events_at


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


if __name__ == "__main__":
    unittest.main()
