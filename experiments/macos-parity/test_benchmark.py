import tempfile
import unittest
from pathlib import Path
from PIL import Image, ImageDraw
from benchmark import BG, compare, cpu_seconds, descendants, order, percentile


class BenchmarkTests(unittest.TestCase):
    def test_cpu_time_and_deep_process_tree(self):
        self.assertAlmostEqual(cpu_seconds("2-03:04:05.25"), 183845.25)
        rows = {9: {"pid": 9, "parent": 1}, 12: {"pid": 12, "parent": 9},
                15: {"pid": 15, "parent": 12}, 19: {"pid": 19, "parent": 15},
                20: {"pid": 20, "parent": 2}}
        self.assertEqual([r["pid"] for r in descendants(rows, 9)], [9, 12, 15, 19])
        self.assertEqual(descendants(rows, 5), [])

    def test_balanced_order_and_tail_not_mean(self):
        self.assertEqual(order(["tauri", "native"], 1), ["native", "tauri"])
        self.assertEqual(order(["tauri", "native", "gpui"], 2), ["gpui", "tauri", "native"])
        self.assertEqual(percentile([16] * 18 + [47, 103], .95), 47)

    def test_pixel_gate_rejects_blank_missing_and_moved_content(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reference = Image.new("RGB", (1280, 1440), BG)
            draw = ImageDraw.Draw(reference)
            draw.rectangle((356, 272, 923, 591), fill=(241, 194, 37))
            draw.rectangle((356, 640, 923, 959), fill=(61, 130, 178))
            reference.save(root / "reference.png")
            reference.save(root / "same.png")
            self.assertTrue(compare(root / "reference.png", root / "same.png", root / "same")["pixelGatePassed"])
            missing = reference.copy()
            ImageDraw.Draw(missing).rectangle((356, 640, 923, 959), fill=BG)
            missing.save(root / "missing.png")
            self.assertFalse(compare(root / "reference.png", root / "missing.png", root / "missing")["pixelGatePassed"])
            shifted = Image.new("RGB", reference.size, BG)
            shifted.paste(reference, (9, 0))
            shifted.save(root / "shifted.png")
            self.assertFalse(compare(root / "reference.png", root / "shifted.png", root / "shifted")["pixelGatePassed"])
            Image.new("RGB", reference.size, BG).save(root / "blank.png")
            with self.assertRaisesRegex(ValueError, "Missing visible"):
                compare(root / "blank.png", root / "blank.png", root / "blank")
            Image.new("RGB", (640, 720), "white").save(root / "small.png")
            with self.assertRaisesRegex(ValueError, "physical content size"):
                compare(root / "reference.png", root / "small.png", root / "small")


if __name__ == "__main__":
    unittest.main()
