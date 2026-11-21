import contextlib
import io
import os
import sys
import tempfile
import unittest
from pathlib import Path

from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure as m


class Measure(unittest.TestCase):
    def test_ink_table_says_na_for_a_missing_image(self):
        # Rule 1336: run.ps1 writes no gpu pass when there is no adapter, and `-Only` can leave strip-3 out.
        # Only the software image at scale 1 is here; every other cell is n/a, and nothing crashes.
        with tempfile.TemporaryDirectory() as d:
            Image.new("RGB", (1000, 40), (20, 20, 20)).save(Path(d) / "strip-3-1-software.png")
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                m.ink_table(Path(d))
        lines = out.getvalue().splitlines()
        gpu = [l for l in lines if l.startswith("gpu")]
        soft = [l for l in lines if l.startswith("software")]
        self.assertEqual(len(gpu), len(m.ICONS))
        self.assertTrue(all(l.split()[2:] == ["n/a"] * len(m.SCALES) for l in gpu), gpu)
        self.assertTrue(all(l.split()[2] == "none" and l.split()[3:] == ["n/a"] * 7 for l in soft), soft)


if __name__ == "__main__":
    unittest.main()
