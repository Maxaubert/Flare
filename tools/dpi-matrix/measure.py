"""The measurements behind the Task 73 reading of the scaling images (docs/screenshots/m10/dpi/README.md).

usage: python tools/dpi-matrix/measure.py <image folder>   (the folder run.ps1 wrote, holding scenes.txt)

Prints three tables, all in logical (100%) pixels unless said otherwise:
1. strip: the tab strip's bottom line under the window controls, in device pixels, per scene and scale; the
   rule is max(1, floor(scale)) solid pixels of one colour (no blended row). Scenes with vertical tabs have no
   strip there and print `n/a`.
2. ink: the ink box of the chrome icons of `strip-3` (the logo, a tab's terminal glyph, help, settings and the
   three window controls); a box that drifts by more than one device pixel between scales is a sizing defect.
3. skeleton: the top of the resume skeleton's lines; the line pitch is snapped per line to whole device pixels
   (Px.snap), so at fractional scales the page grows a little taller, by design.
Exits 1 when a strip line breaks its rule.
"""
import math
import sys
from pathlib import Path

from PIL import Image

SCALES = ["1", "1_25", "1_5", "1_75", "2", "2_25", "2_5", "3"]
ICONS = {
    "logo": (8, 4, 36, 32),
    "term": (50, 6, 76, 30),
    "help": (820, 6, 848, 30),
    "gear": (856, 6, 884, 30),
    "min": (892, 6, 920, 30),
    "max": (928, 6, 956, 30),
    "close": (964, 6, 992, 30),
}


def scale_of(s):
    return float(s.replace("_", "."))


def load(d, scene, s, r):
    p = d / f"{scene}-{s}-{r}.png"
    return Image.open(p).convert("RGB") if p.exists() else None


def strip_table(d, scenes):
    bad = 0
    print("# strip bottom line (device px; want max(1, floor(scale)))")
    for scene in scenes:
        for r in ("gpu", "software"):
            cells = []
            for s in SCALES:
                f = scale_of(s)
                im = load(d, scene, s, r)
                x = round(960 * f)
                if im is None or x >= im.width:
                    cells.append("n/a")
                    continue
                col = [im.getpixel((x, y))[0] for y in range(round(28 * f), round(44 * f))]
                run = [v for v in col if v != col[0]]
                if len(run) > 4 * max(1, math.floor(f)):
                    cells.append("n/a")  # a control, not a line (vertical tabs)
                    continue
                ok = len(set(run)) == 1 and len(run) == max(1, math.floor(f))
                bad += 0 if ok else 1
                cells.append(f"{len(run)}" + ("" if ok else "!"))
            print(f"{scene:28} {r:8} " + " ".join(f"{c:>4}" for c in cells))
    return bad


def ink_table(d):
    print("# chrome icon ink boxes in strip-3 (logical px, threshold 40 over the ground)")
    for r in ("gpu", "software"):
        for name, (x0, y0, x1, y1) in ICONS.items():
            out = []
            for s in SCALES:
                f = scale_of(s)
                im = load(d, "strip-3", s, r)
                c = im.convert("L").crop((round(x0 * f), round(y0 * f), round(x1 * f), round(y1 * f)))
                bg, px = c.getpixel((0, 0)), c.load()
                pts = [(x, y) for y in range(c.height) for x in range(c.width) if px[x, y] - bg > 40]
                if not pts:
                    out.append("none")
                    continue
                xs, ys = [p[0] for p in pts], [p[1] for p in pts]
                out.append(f"{(max(xs) - min(xs) + 1) / f:.1f}x{(max(ys) - min(ys) + 1) / f:.1f}")
            print(f"{r:8} {name:6} " + " ".join(out))


def skeleton_table(d):
    print("# resume skeleton line tops (logical px)")
    for s in SCALES:
        f = scale_of(s)
        im = load(d, "marks-skeleton", s, "gpu")
        if im is None:
            continue
        x, bg = round(50 * f), im.getpixel((round(990 * f), im.height - 2))
        runs, start = [], None
        for y in range(round(45 * f), im.height):
            on = max(abs(a - b) for a, b in zip(im.getpixel((x, y)), bg)) > 6
            if on and start is None:
                start = y
            if not on and start is not None:
                runs.append(start)
                start = None
        print(f"{s:5} " + " ".join(f"{t / f:.1f}" for t in runs[:12]))


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    d = Path(sys.argv[1])
    # `badge` is a sheet of taskbar discs, not a window (run.ps1 leaves it out of the matrix).
    scenes = [s for s in (d / "scenes.txt").read_text().split() if s != "badge"]
    bad = strip_table(d, scenes)
    ink_table(d)
    skeleton_table(d)
    print(f"strip lines off the rule: {bad}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
