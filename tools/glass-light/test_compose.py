import os
import sys
import tempfile
import unittest

from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import compose as c


class Compose(unittest.TestCase):
    def test_half_transparent_white_over_black_is_mid_grey(self):
        shot = Image.new("RGBA", (4, 4), (255, 255, 255, 128))
        px = c.over(shot, c.backdrop("black", shot.size)).getpixel((1, 1))
        self.assertTrue(all(127 <= v <= 129 for v in px), px)

    def test_a_flat_image_has_no_alpha(self):
        self.assertFalse(c.has_alpha(Image.new("RGBA", (2, 2), (1, 2, 3, 255))))
        self.assertFalse(c.has_alpha(Image.new("RGB", (2, 2))))
        self.assertTrue(c.has_alpha(Image.new("RGBA", (2, 2), (1, 2, 3, 100))))

    def test_the_wallpaper_is_deterministic_and_busy(self):
        a, b = c.wallpaper((64, 40)), c.wallpaper((64, 40))
        self.assertEqual(a.tobytes(), b.tobytes())
        self.assertGreater(len(set(a.getdata())), 200)

    def test_run_writes_four_backdrops_and_a_sheet_per_level(self):
        with tempfile.TemporaryDirectory() as src, tempfile.TemporaryDirectory() as out:
            Image.new("RGBA", (40, 24), (240, 240, 240, 77)).save(os.path.join(src, "paper-70.png"))
            c.run(src, out)
            names = sorted(os.listdir(out))
            self.assertEqual(names, ["contact-70.png"] + [f"paper-70-{b}.png" for b in sorted(c.BACKDROPS)])


if __name__ == "__main__":
    unittest.main()
