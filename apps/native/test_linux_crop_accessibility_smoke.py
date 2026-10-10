"""Regression tests for the public-provider evidence oracle (no GI/D-Bus needed)."""
from copy import deepcopy
import unittest

from linux_crop_accessibility_smoke import HANDLES, verify_handles, verify_screen_bounds


class CropProviderEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.crop = (40, 20, 160, 80)
        # Independent source-pixel expectations for the known 320x180 fixture.
        values = (40, 200, 200, 40, 20, 200, 100, 40)
        ranges = ((0, 198), (42, 320), (42, 320), (0, 198),
                  (0, 98), (42, 320), (22, 180), (0, 198))
        self.limits = dict(zip(HANDLES, ranges))
        self.nodes = [{"name": name, "role": "slider", "bounds": [150, 200, 12, 12],
                       "interfaces": ["org.a11y.atspi.Value"], "actions": [],
                       "description": f"{'Y' if name in ('Crop top', 'Crop bottom') else 'X'} edge {value} source pixels; "
                                      "X 40, Y 20, width 160, height 80",
                       "value": {"CurrentValue": value, "MinimumValue": low,
                                 "MaximumValue": high, "MinimumIncrement": 1}}
                      for name, value, (low, high) in zip(HANDLES, values, ranges)]

    def test_names_values_ranges_and_bounds_reject_plausible_faults(self):
        self.assertEqual(len(verify_handles(self.nodes, self.crop, self.limits)), 8)
        for field, wrong in (("CurrentValue", 80), ("MaximumValue", 640), ("MinimumIncrement", 10)):
            with self.subTest(field=field):
                nodes = deepcopy(self.nodes)
                nodes[0]["value"][field] = wrong
                with self.assertRaises(AssertionError):
                    verify_handles(nodes, self.crop, self.limits)
        for nodes in ([], self.nodes[:-1], self.nodes + self.nodes[:1]):
            with self.subTest(count=len(nodes)), self.assertRaises(AssertionError):
                verify_handles(nodes, self.crop, self.limits)
        nodes = deepcopy(self.nodes)
        nodes[0]["bounds"][3] = -12
        with self.assertRaises(AssertionError):
            verify_handles(nodes, self.crop, self.limits)

    def test_scrolled_projection_rejects_inside_but_misplaced_and_unclipped_bounds(self):
        image = (100, 50, 740, 410, 105, 95, 520, 220)
        # At 2x display scale, three north handles have only their last pixel
        # inside the clip; south handles are fully outside and must be absent.
        handles = [{"name": name, "bounds": rect} for name, rect in (
            ("Crop top left", [194, 125, 12, 1]), ("Crop top right", [514, 125, 12, 1]),
            ("Crop top", [354, 125, 12, 1]), ("Crop right", [514, 194, 12, 12]),
            ("Crop left", [194, 194, 12, 12]))]
        verify_screen_bounds(handles, self.crop, (320, 180), image, (20, 30))
        for index, value in ((0, 204), (3, 12)):
            changed = deepcopy(handles)
            changed[0]["bounds"][index] = value
            with self.subTest(index=index), self.assertRaises(AssertionError):
                verify_screen_bounds(changed, self.crop, (320, 180), image, (20, 30))
        with self.assertRaises(AssertionError):
            verify_screen_bounds(handles + [{"name": "Crop bottom", "bounds": [354, 274, 12, 12]}],
                                 self.crop, (320, 180), image, (20, 30))

    def test_value_transport_and_source_descriptions_are_required_not_action_interface(self):
        verify_handles(self.nodes, self.crop, self.limits)
        for description in ("", "Y edge 40 source pixels; X 40, Y 20, width 160, height 80",
                            "X edge 40 source pixels; X 40, Y 20, width 160, height 800"):
            nodes = deepcopy(self.nodes)
            nodes[0]["description"] = description
            with self.subTest(description=description), self.assertRaises(AssertionError):
                verify_handles(nodes, self.crop, self.limits)
        nodes = deepcopy(self.nodes)
        nodes[0]["interfaces"] = ["org.a11y.atspi.Action"]
        with self.assertRaises(AssertionError):
            verify_handles(nodes, self.crop, self.limits)


if __name__ == "__main__":
    unittest.main()
