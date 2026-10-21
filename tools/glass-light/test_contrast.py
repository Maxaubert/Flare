import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import contrast as c


class Contrast(unittest.TestCase):
    def test_black_on_white_is_21(self):
        self.assertAlmostEqual(c.contrast((0, 0, 0), (255, 255, 255)), 21.0, places=6)

    def test_alpha_one_is_the_ground(self):
        self.assertEqual(c.effective_ground((0, 0, 0), (246, 244, 238), 1.0), (246, 244, 238))

    def test_alpha_zero_is_the_desktop(self):
        self.assertEqual(c.effective_ground((128, 128, 128), (246, 244, 238), 0.0), (128, 128, 128))

    def test_level_70_on_grey_halves_toward_the_desktop(self):
        self.assertEqual(c.effective_ground((128, 128, 128), (255, 255, 255), 0.3), (166, 166, 166))

    def test_rows_cover_twelve_colours_three_desktops_three_levels(self):
        pal = [{"id": "t", "bg": "#ffffff", **{f"a{i}": "#808080" for i in range(16)}}]
        self.assertEqual(len(c.rows(pal)), 12 * 3 * 3)

    def test_worst_is_sorted_ascending(self):
        w = c.worst([{"ratio": 3}, {"ratio": 1}, {"ratio": 2}], 2)
        self.assertEqual([r["ratio"] for r in w], [1, 2])

    def test_the_real_table_loads(self):
        t = c.rows(c.load(os.path.join(c.HERE, "palettes.csv")))
        self.assertTrue(t)


if __name__ == "__main__":
    unittest.main()
