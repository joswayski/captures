import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location("gpui_benchmark", Path(__file__).with_name("benchmark.py"))
benchmark = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(benchmark)


class BenchmarkHelpersTest(unittest.TestCase):
    def test_summary_keeps_asymmetric_metric_ranges(self):
        rows = [
            dict(rss_mib=9, pss_mib=3, private_mib=8, processes=1,
                 first_window_mapped_ms=90, idle_cpu_percent_one_core=0.9),
            dict(rss_mib=1, pss_mib=7, private_mib=4, processes=5,
                 first_window_mapped_ms=10, idle_cpu_percent_one_core=0.1),
            dict(rss_mib=5, pss_mib=2, private_mib=6, processes=3,
                 first_window_mapped_ms=30, idle_cpu_percent_one_core=0.4),
        ]
        result = benchmark.summarize(rows)
        self.assertEqual(result["rss_mib"], {"median": 5, "min": 1, "max": 9})
        self.assertEqual(result["pss_mib"], {"median": 3, "min": 2, "max": 7})
        self.assertEqual(result["processes"], {"median": 3, "min": 1, "max": 5})
        self.assertEqual(result["first_window_mapped_ms"]["median"], 30)

    def test_summary_rejects_incomplete_resource_row(self):
        row = {key: 1 for key in benchmark.METRICS}
        del row["private_mib"]
        with self.assertRaisesRegex(ValueError, "missing or unexpected"):
            benchmark.summarize([row])

    def test_commands_preserve_distinct_cli_contracts(self):
        binary = Path("/opt/captures")
        fixture = Path("/tmp/fixture.png")
        self.assertEqual(benchmark.command(binary, "tauri", "preferences", fixture, "dark"),
                         ["/opt/captures"])
        self.assertEqual(benchmark.command(binary, "tauri", "image", fixture, "dark"),
                         ["/opt/captures", "/tmp/fixture.png"])
        self.assertEqual(benchmark.command(binary, "gpui", "preferences", fixture, "dark"),
                         ["/opt/captures", "--appearance", "dark", "--preferences"])
        self.assertEqual(benchmark.command(binary, "gpui", "image", fixture, "light"),
                         ["/opt/captures", "--appearance", "light", "--open", "/tmp/fixture.png"])

    def test_png_dimensions_validates_signature_and_asymmetric_size(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "shot.png"
            path.write_bytes(b"\x89PNG\r\n\x1a\n" + b"\0\0\0\rIHDR" + struct.pack(">II", 1280, 760))
            self.assertEqual(benchmark.png_dimensions(path), (1280, 760))
            path.write_bytes(b"not a png")
            with self.assertRaisesRegex(RuntimeError, "valid PNG"):
                benchmark.png_dimensions(path)


if __name__ == "__main__":
    unittest.main()
