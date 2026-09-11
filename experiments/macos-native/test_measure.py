import unittest
from measure import cpu_seconds, descendants


class MeasureTest(unittest.TestCase):
    def test_cpu_clock_units(self):
        self.assertAlmostEqual(cpu_seconds("02:03.45"), 123.45)
        self.assertAlmostEqual(cpu_seconds("1-02:03:04.5"), 93784.5)

    def test_tree_excludes_unrelated_process_and_keeps_grandchild(self):
        rows = {1:{"parent":0}, 71:{"parent":1}, 12:{"parent":71}, 92:{"parent":12}, 99:{"parent":1}}
        self.assertEqual(set(descendants(rows, 71)), {71, 12, 92})
        with self.assertRaises(ValueError):
            descendants(rows, 100)


if __name__ == "__main__":
    unittest.main()
