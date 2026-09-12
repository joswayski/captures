import importlib.util
from pathlib import Path
import unittest
from unittest.mock import Mock, patch


SPEC = importlib.util.spec_from_file_location("gpui_effects", Path(__file__).with_name("effects.py"))
effects = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(effects)


class EffectsTests(unittest.TestCase):
    def test_sampling_uses_whole_tree_cpu_and_observed_memory_maximum(self):
        action = Mock()
        with patch.object(effects.bench, "process_tree", side_effect=[{1: 100, 2: 40}, {1: 180, 2: 60}]), \
             patch.object(effects.bench, "resources", side_effect=[{"pss_mib": n} for n in (3, 12, 7, 5, 6, 4, 2, 1)]), \
             patch.object(effects.time, "perf_counter", side_effect=[10] * 9 + [14]), \
             patch.object(effects.time, "sleep"), \
             patch.object(effects.os, "sysconf", return_value=100):
            result = effects.sample_cpu(1, action)
        action.assert_called_once_with()
        self.assertEqual(result, {"window_seconds": 4, "cpu_seconds": 1,
                                  "cpu_percent_one_core": 25, "max_sampled_pss_mib": 12})

    def test_sampling_rejects_disappearing_children(self):
        with patch.object(effects.bench, "process_tree", side_effect=[{1: 100, 2: 40}, {1: 120}]), \
             patch.object(effects.bench, "resources", return_value={"pss_mib": 1}), \
             patch.object(effects.time, "perf_counter", side_effect=[10] * 9 + [14]), \
             patch.object(effects.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "process tree changed"):
                effects.sample_cpu(1, lambda: None)


if __name__ == "__main__":
    unittest.main()
