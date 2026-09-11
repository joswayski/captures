import unittest
from pathlib import Path
from tokens import generate, resolve


class TokensTest(unittest.TestCase):
    def test_semantics_resolve_independently_and_glass_is_fixed(self):
        tokens = generate(Path(__file__).resolve().parents[2])
        self.assertEqual(tokens["light"]["surface-raised"], "#ffffff")
        self.assertEqual(tokens["dark"]["surface-raised"], "#16161b")
        self.assertEqual(tokens["light"]["text"], "#131318")
        self.assertEqual(tokens["dark"]["text"], "#f2f2f4")
        self.assertEqual(tokens["dark"]["glass"], "rgba(20, 20, 24, 0.82)")
        self.assertEqual(tokens["dark"]["glass"], tokens["light"]["glass"])
        self.assertEqual(tokens["dark"]["dur-4"], "280ms")
        self.assertEqual(tokens["dark"]["h-lg"], "36px")
        self.assertEqual(set(tokens["themes"]), {"mustard", "ember", "rose", "violet", "cobalt", "aqua", "mint", "lime", "mono"})
        self.assertEqual(tokens["themes"]["aqua"]["accent"], "#31cbd8")

    def test_cycles_are_rejected_and_rgb_aliases_expand(self):
        self.assertEqual(resolve("--a", {"--a":"rgba(var(--b), 0.5)", "--b":"1, 7, 13"}), "rgba(1, 7, 13, 0.5)")
        with self.assertRaises(ValueError):
            resolve("--a", {"--a":"var(--b)", "--b":"var(--a)"})


if __name__ == "__main__":
    unittest.main()
