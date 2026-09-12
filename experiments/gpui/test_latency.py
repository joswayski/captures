import importlib.util
from pathlib import Path
import unittest
from unittest.mock import Mock, patch


SPEC = importlib.util.spec_from_file_location(
    "gpui_latency", Path(__file__).with_name("latency.py"))
latency = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(latency)


class LatencyTests(unittest.TestCase):
    def test_patch_observes_moving_handle_not_initial_pointer_down(self):
        x, y, width, height = latency.PATCH
        self.assertLessEqual(x, latency.END[0])
        self.assertLess(latency.END[0], x + width)
        self.assertLessEqual(y, latency.START[1])
        self.assertLess(latency.START[1], y + height)
        self.assertGreater(x, latency.START[0])

    def test_native_selector_uses_its_own_title_and_rejects_stale_pixels(self):
        x11 = Mock()
        x11.patch.return_value = (0xFFCA28,)
        process = Mock(pid=123)
        process.poll.return_value = None
        with patch.object(latency, "exact_window", return_value="42") as find:
            with self.assertRaisesRegex(RuntimeError, "baseline patch is already yellow"):
                latency.open_fresh_selector(x11, process, "native", Mock(), inject_shortcut=False)
            find.assert_called_once_with("Captures — Select target", 123)

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
