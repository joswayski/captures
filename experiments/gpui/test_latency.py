import importlib.util
from pathlib import Path
import unittest


SPEC = importlib.util.spec_from_file_location(
    "gpui_latency", Path(__file__).with_name("latency.py"))
latency = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(latency)


class LatencyTests(unittest.TestCase):
    def test_pixel_color_detection_discriminates_channels_and_tolerance(self):
        self.assertTrue(latency.pixel_is_yellow(0xFFCA28))
        self.assertTrue(latency.pixel_is_yellow(0xF5C030))
        self.assertFalse(latency.pixel_is_yellow(0xFF28CA))
        self.assertFalse(latency.pixel_is_yellow(0xE0CA28))
        self.assertTrue(latency.patch_has_yellow((0, 0xFFCA28, 0)))
        self.assertFalse(latency.patch_has_yellow((0, 0xFFFFFF, 0)))

    def test_summary_orders_values_and_excludes_failures(self):
        rows = [{"status": "success", "latency_ms": value}
                for value in (9.0, 1.0, 7.0, 3.0, 5.0)]
        rows += [{"status": "timeout", "latency_ms": None},
                 {"status": "no_change", "latency_ms": None}]
        summary = latency.summarize(rows)
        self.assertEqual(summary["median_ms"], 5.0)
        self.assertEqual(summary["p95_ms"], 9.0)
        self.assertEqual(summary["min_ms"], 1.0)
        self.assertEqual(summary["max_ms"], 9.0)
        self.assertEqual(summary["successful_trials"], 5)
        self.assertEqual(summary["status_counts"],
                         {"success": 5, "no_change": 1, "timeout": 1})


if __name__ == "__main__":
    unittest.main()
