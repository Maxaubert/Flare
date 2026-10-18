"""Checks of the icon pipeline (M9 Task 26). Run: python -I tools/test_make_icon.py

Stdlib unittest plus Pillow. The frames are the owner's artwork rendered from the SVG, so the
checks look for the artwork (a dark flame on a yellow square, transparent outside the square),
never for a badge or an outline.
"""

import struct
import unittest
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
ICO = ROOT / 'build' / 'icon.ico'
SOURCE = ROOT / 'build' / 'icon-source.png'
README = ROOT / 'docs' / 'assets' / 'flare-icon.png'

SIZES = [16, 20, 24, 28, 30, 32, 36, 40, 42, 48, 54, 56, 60, 64, 72, 80, 96, 128, 256]
VIEW = 1254  # the SVG's viewBox; the sample points below are in these units
FLAME_POINT = (440, 690)  # deep inside the flame (black in the artwork)
YELLOW_POINT = (1060, 260)  # inside the rounded square, clear of the flame


def at(point, n):
    """The pixel of an n-pixel frame that holds a point given in SVG units."""
    x = min(n - 1, int(point[0] * n / VIEW))
    y = min(n - 1, int(point[1] * n / VIEW))
    return x, y


def directory(path):
    """The ICO directory as (width, height, count) rows, read from the bytes."""
    data = path.read_bytes()
    reserved, kind, count = struct.unpack('<HHH', data[:6])
    rows = []
    for i in range(count):
        w, h = struct.unpack('<BB', data[6 + 16 * i:8 + 16 * i])
        rows.append((w or 256, h or 256))
    return reserved, kind, count, rows


class IconFrames(unittest.TestCase):
    def test_the_ico_has_19_frames_in_pt_order(self):
        reserved, kind, count, rows = directory(ICO)
        self.assertEqual((reserved, kind, count), (0, 1, 19))
        self.assertEqual([w for w, _ in rows], SIZES)
        self.assertTrue(all(w == h for w, h in rows))

    def test_every_frame_is_transparent_in_the_corner(self):
        im = Image.open(ICO)
        for n in SIZES:
            im.size = (n, n)
            im.load()
            frame = im.convert('RGBA')
            self.assertEqual(frame.getpixel((0, 0))[3], 0, f'frame {n} corner is not transparent')

    def test_every_frame_has_the_dark_flame_and_the_yellow_square(self):
        im = Image.open(ICO)
        for n in SIZES:
            im.size = (n, n)
            im.load()
            frame = im.convert('RGBA')
            r, g, b, a = frame.getpixel(at(FLAME_POINT, n))
            self.assertLess(max(r, g, b), 60, f'frame {n}: the flame is not dark')
            self.assertEqual(a, 255, f'frame {n}: the flame is not opaque')
            r, g, b, a = frame.getpixel(at(YELLOW_POINT, n))
            self.assertTrue(r > 200 and g > 180 and b < 120, f'frame {n}: not yellow')
            self.assertEqual(a, 255, f'frame {n}: the square is not opaque')

    def test_there_is_no_badge_or_outline(self):
        # PT's icon had a #202020 edge on a black badge; Flare's is the artwork as it is.
        im = Image.open(ICO)
        im.size = (256, 256)
        im.load()
        frame = im.convert('RGBA')
        for x, y in [(10, 128), (128, 10)]:
            r, g, b, a = frame.getpixel((x, y))
            self.assertEqual(a, 0, f'({x}, {y}) is not transparent: {frame.getpixel((x, y))}')
            self.assertNotEqual((r, g, b), (0x20, 0x20, 0x20))


class SourceAndReadme(unittest.TestCase):
    def test_the_source_is_square_1024_with_transparent_corners(self):
        im = Image.open(SOURCE).convert('RGBA')
        self.assertEqual(im.size, (1024, 1024))
        self.assertEqual(im.getpixel((0, 0))[3], 0)
        r, g, b, a = im.getpixel(at(FLAME_POINT, 1024))
        self.assertLess(max(r, g, b), 60)
        r, g, b, a = im.getpixel(at(YELLOW_POINT, 1024))
        self.assertTrue(r > 200 and g > 180 and b < 120)

    def test_the_readme_picture_is_256(self):
        im = Image.open(README)
        self.assertEqual(im.size, (256, 256))


if __name__ == '__main__':
    unittest.main()
