import importlib.util
import json
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
        self.assertEqual(benchmark.command(binary, "native", "preferences", fixture, "dark"),
                         ["/opt/captures", "--preferences"])
        self.assertEqual(benchmark.command(binary, "native", "image", fixture, "light"),
                         ["/opt/captures", "--open", "/tmp/fixture.png"])

    def test_native_profile_configures_appearance_and_isolates_output(self):
        for appearance in ("light", "dark"):
            with self.subTest(appearance=appearance), tempfile.TemporaryDirectory() as directory:
                profile = Path(directory)
                env = benchmark.profile_environment(profile, appearance)
                settings = json.loads((Path(env["CAPTURES_NATIVE_DATA"]) / "settings.json").read_text())
                self.assertEqual(settings["appearance"], appearance)
                self.assertEqual(settings["output_directory"], str(profile / "captures"))
                self.assertFalse(settings["launch_at_login"])
                self.assertNotEqual(env["CAPTURES_NATIVE_DATA"], env["CAPTURES_GPUI_DATA"])

    def test_selection_commands_use_real_shortcut_entry_not_gpui_test_cli(self):
        binary = Path("/opt/captures")
        fixture = Path("/tmp/fixture.png")
        for state in ("screenshot-selection", "recording-selection"):
            self.assertEqual(benchmark.command(binary, "tauri", state, fixture, "dark"),
                             ["/opt/captures"])
            self.assertEqual(benchmark.command(binary, "gpui", state, fixture, "dark"),
                             ["/opt/captures", "--appearance", "dark", "--preferences"])
            self.assertEqual(benchmark.command(binary, "native", state, fixture, "dark"),
                             ["/opt/captures", "--preferences"])

    def test_video_uses_each_production_media_open_contract(self):
        binary = Path("/opt/captures")
        fixture = Path("/tmp/fixture.mp4")
        self.assertEqual(benchmark.command(binary, "tauri", "video-editor", fixture, "dark"),
                         ["/opt/captures", "/tmp/fixture.mp4"])
        self.assertEqual(benchmark.command(binary, "gpui", "video-editor", fixture, "dark"),
                         ["/opt/captures", "--appearance", "dark", "--open", "/tmp/fixture.mp4"])
        self.assertEqual(benchmark.command(binary, "native", "video-editor", fixture, "dark"),
                         ["/opt/captures", "--open", "/tmp/fixture.mp4"])

    def test_selection_shortcuts_are_discriminating(self):
        from unittest.mock import patch
        with patch.object(benchmark.subprocess, "run") as run:
            benchmark.enter_state("screenshot-selection")
            run.assert_called_once_with(
                ["xdotool", "keydown", "Print", "sleep", ".1", "keyup", "Print"], check=True)
        with patch.object(benchmark.subprocess, "run") as run:
            benchmark.enter_state("recording-selection")
            run.assert_called_once_with(
                ["xdotool", "keydown", "ctrl+shift+alt+r", "sleep", ".1", "keyup", "ctrl+shift+alt+r"], check=True)

    def test_preview_matching_rejects_same_title_fullscreen_selector(self):
        from unittest.mock import patch, Mock
        with patch.object(benchmark.subprocess, "run", return_value=Mock(returncode=0, stdout="11\n22\n")), \
             patch.object(benchmark.subprocess, "check_output", side_effect=["WIDTH=1600\n", "WIDTH=340\n"]):
            self.assertEqual(benchmark.find_window("^Captures$", 123, maximum_width=400), "11")

    def test_additional_surface_titles_are_distinct(self):
        self.assertEqual(benchmark.TITLES[("tauri", "history")], "^Capture History$")
        self.assertEqual(benchmark.TITLES[("gpui", "history")], "^Captures GPUI Capture History$")
        self.assertEqual(benchmark.TITLES[("tauri", "recording-hud")], "^Captures Recording Controls$")
        self.assertEqual(benchmark.TITLES[("gpui", "recording-hud")], "^Captures GPUI Recording controls$")

    def test_overlay_states_keep_native_dimensions(self):
        for state in ("previews-collapsed", "previews-expanded", "recording-hud"):
            self.assertNotIn(state, benchmark.WINDOW_SIZES)

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
