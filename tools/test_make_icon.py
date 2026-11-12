"""Checks of the icon pipeline (M9 Task 26). Run: python -I tools/test_make_icon.py

Stdlib unittest plus Pillow. The frames are the owner's Windows app icon rendered from
assets/icon/app-windows.svg (2026-11-12: a gold flame and a light gold flame on a dark rounded
square, transparent outside the square), so the checks look for that artwork.
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
# Sample points as fractions of the frame.
FLAME_POINT = (0.25, 0.65)  # deep inside the gold flame (left)
LIGHT_POINT = (0.62, 0.45)  # deep inside the light gold flame (right)
SQUARE_POINT = (0.10, 0.50)  # inside the dark rounded square, left of the flames, clear of its edge


def at(point, n):
    """The pixel of an n-pixel frame that holds a point given as fractions of the frame."""
    x = min(n - 1, int(point[0] * n))
    y = min(n - 1, int(point[1] * n))
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

    def test_every_frame_has_the_two_flames_and_the_dark_square(self):
        im = Image.open(ICO)
        for n in SIZES:
            im.size = (n, n)
            im.load()
            frame = im.convert('RGBA')
            self.assert_artwork(frame, n)

    def assert_artwork(self, frame, n):
        r, g, b, a = frame.getpixel(at(FLAME_POINT, n))
        self.assertTrue(r > 150 and 100 < g < r and b < 60, f'frame {n}: not gold {(r, g, b)}')
        self.assertEqual(a, 255, f'frame {n}: the flame is not opaque')
        r, g, b, a = frame.getpixel(at(LIGHT_POINT, n))
        self.assertTrue(r > 200 and g > 150 and b < 140, f'frame {n}: not light gold {(r, g, b)}')
        r, g, b, a = frame.getpixel(at(SQUARE_POINT, n))
        self.assertLess(max(r, g, b), 70, f'frame {n}: the square is not dark {(r, g, b)}')
        self.assertEqual(a, 255, f'frame {n}: the square is not opaque')

    def test_every_frame_is_as_sharp_as_the_vector(self):
        # Each frame against the 1024 px render averaged down to its size (each pixel the mean of
        # the area it covers: the coverage a vector rasteriser draws). M9 Task 41: ImageMagick's
        # `-density` path rendered 10 of the 19 sizes one pixel short and `-resize` stretched them
        # back, so 16, 20, 42, 48, 54, 56, 60, 96, 128 and 256 were blurred (16 px: 51 levels off
        # on average, the yellow stroke in the flame gone).
        ref = Image.open(SOURCE).convert('RGBA')
        im = Image.open(ICO)
        for n in SIZES:
            im.size = (n, n)
            im.load()
            frame = im.convert('RGBA')
            want = ref.resize((n, n), Image.BOX)
            a, b = frame.tobytes(), want.tobytes()
            mean = sum(abs(p - q) for p, q in zip(a, b)) / len(a)
            self.assertLess(mean, 3.0, f'frame {n} is {mean:.1f} levels from the vector on average')

    def test_there_is_no_badge_around_the_artwork(self):
        # PT's icon sat on a black badge; Flare's is the artwork as it is: the rounded square
        # reaches the frame's edges and its corners stay transparent.
        im = Image.open(ICO)
        im.size = (256, 256)
        im.load()
        frame = im.convert('RGBA')
        for x, y in [(3, 3), (252, 3), (3, 252), (252, 252)]:
            self.assertEqual(frame.getpixel((x, y))[3], 0, f'({x}, {y}) is not transparent')
        for x, y in [(4, 128), (128, 4)]:
            self.assertEqual(frame.getpixel((x, y))[3], 255, f'({x}, {y}): the square stops short')


class SourceAndReadme(unittest.TestCase):
    def test_the_source_is_square_1024_with_transparent_corners(self):
        im = Image.open(SOURCE).convert('RGBA')
        self.assertEqual(im.size, (1024, 1024))
        self.assertEqual(im.getpixel((0, 0))[3], 0)
        IconFrames.assert_artwork(self, im, 1024)


    def test_the_readme_picture_is_256(self):
        im = Image.open(README)
        self.assertEqual(im.size, (256, 256))


if __name__ == '__main__':
    unittest.main()
