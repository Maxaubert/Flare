"""Contact sheets for the visual tour (audit 2026-10-20).

    python tools/visual/sheet.py <cell-folder> [<cell-folder> ...] [--out sheet.png] [--cols 5] [--width 440]

A cell folder is `docs/screenshots/visual/<surface>/<scale>-<renderer>/` and holds `frames/*.png`. Every
folder gets a captioned strip of its frames in file-name order (the names carry the time), tiled `--cols`
across; several folders stack under each other, so Flare's cell and Prism Terminal's sit in one sheet.
The sheet is written beside the first folder as `sheet.png` unless `--out` says otherwise, reduced to a
256-colour palette (a terminal window is flat colour; the sheet is a third of the size).
"""
import argparse
import os
import re
import sys

from PIL import Image, ImageDraw, ImageFont

BG = (24, 24, 24)
INK = (230, 230, 230)
DIM = (150, 150, 150)


def font(size):
    try:
        return ImageFont.load_default(size=size)
    except TypeError:  # an older Pillow has one size only
        return ImageFont.load_default()


def label_of(name):
    stem = os.path.splitext(name)[0]
    return re.sub(r"^(\d+)-", r"\1 ", stem)


def frames_of(folder):
    fdir = os.path.join(folder, "frames")
    if not os.path.isdir(fdir):
        return []
    return sorted(
        os.path.join(fdir, f) for f in os.listdir(fdir) if f.lower().endswith(".png")
    )


def strip(folder, caption, cols, width):
    files = frames_of(folder)
    if not files:
        return None
    small = []
    for f in files:
        im = Image.open(f).convert("RGB")
        h = round(im.height * width / im.width)
        small.append((os.path.basename(f), im.resize((width, h), Image.LANCZOS)))
    cell_h = max(i.height for _, i in small)
    pad, cap_h = 8, 22
    rows = (len(small) + cols - 1) // cols
    w = cols * (width + pad) + pad
    h = 30 + rows * (cell_h + cap_h + pad) + pad
    sheet = Image.new("RGB", (w, h), BG)
    d = ImageDraw.Draw(sheet)
    d.text((pad, 6), caption, fill=INK, font=font(16))
    for i, (name, im) in enumerate(small):
        x = pad + (i % cols) * (width + pad)
        y = 30 + (i // cols) * (cell_h + cap_h + pad)
        d.text((x, y), label_of(name), fill=DIM, font=font(12))
        sheet.paste(im, (x, y + cap_h - 4))
    return sheet


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("folders", nargs="+")
    ap.add_argument("--out")
    ap.add_argument("--cols", type=int, default=5)
    ap.add_argument("--width", type=int, default=440)
    a = ap.parse_args(argv)
    strips = []
    for folder in a.folders:
        caption = "/".join(os.path.normpath(folder).split(os.sep)[-3:])
        s = strip(folder, caption, a.cols, a.width)
        if s is None:
            print(f"skip {folder}: no frames")
            continue
        strips.append(s)
    if not strips:
        print("nothing to sheet")
        return 1
    w = max(s.width for s in strips)
    h = sum(s.height for s in strips)
    out = Image.new("RGB", (w, h), BG)
    y = 0
    for s in strips:
        out.paste(s, (0, y))
        y += s.height
    path = a.out or os.path.join(a.folders[0], "sheet.png")
    out.quantize(colors=256, method=Image.Quantize.MEDIANCUT).save(path, optimize=True)
    print(f"{path}: {out.width}x{out.height}, {sum(len(frames_of(f)) for f in a.folders)} frames")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
