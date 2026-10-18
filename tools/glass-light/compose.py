"""Task 57, step 1: composite the see-through window images over four backdrops.

The orchestrator makes the RGBA images (no desktop needed), one per light preset and level:

    flare.exe --scene term-sample --renderer gpu --see-through <30|50|70> --theme <id> --screenshot <in>/<id>-<level>.png

then:

    python tools/glass-light/compose.py --in <dir> --out docs/screenshots/m10/glass-light

writes <theme>-<level>-<backdrop>.png (backdrops: black, grey, white, wallpaper) and one contact sheet per
level, contact-<level>.png. The wallpaper is built here, deterministically, so the sheets can be redone.
If an image has no alpha (the headless path flattened it) the run stops: that needs `--keep-alpha`.
"""
import argparse
import math
import os
import re
import sys

from PIL import Image, ImageDraw

BACKDROPS = ("black", "grey", "white", "wallpaper")
NAME = re.compile(r"^(?P<theme>.+)-(?P<level>30|50|70)\.png$")


def wallpaper(size):
    """A busy, deterministic stand-in for a desktop picture: diagonal gradient, colour blobs, stripes."""
    w, h = size
    img = Image.new("RGB", size)
    px = img.load()
    for y in range(h):
        for x in range(w):
            t = (x / max(w - 1, 1) + y / max(h - 1, 1)) / 2
            px[x, y] = (int(40 + 180 * t), int(90 + 100 * (1 - t)), int(200 - 150 * t))
    d = ImageDraw.Draw(img, "RGBA")
    blobs = [(0.15, 0.2, 0.2, (230, 60, 60, 200)), (0.5, 0.5, 0.25, (250, 220, 60, 200)),
             (0.8, 0.3, 0.18, (40, 200, 120, 200)), (0.3, 0.8, 0.22, (250, 250, 250, 200)),
             (0.85, 0.85, 0.2, (20, 20, 40, 220))]
    for cx, cy, r, col in blobs:
        rr = r * min(w, h)
        d.ellipse((cx * w - rr, cy * h - rr, cx * w + rr, cy * h + rr), fill=col)
    step = max(6, min(w, h) // 40)
    for i in range(-h, w, step * 2):
        d.line((i, 0, i + h, h), fill=(255, 255, 255, 60), width=max(1, step // 3))
    return img


def backdrop(name, size):
    if name == "black":
        return Image.new("RGB", size, (0, 0, 0))
    if name == "grey":
        return Image.new("RGB", size, (128, 128, 128))
    if name == "white":
        return Image.new("RGB", size, (255, 255, 255))
    if name == "wallpaper":
        return wallpaper(size)
    raise ValueError(name)


def over(shot, back):
    """The window image (RGBA, straight alpha) over a backdrop of its size."""
    return Image.alpha_composite(back.convert("RGBA"), shot.convert("RGBA")).convert("RGB")


def has_alpha(shot):
    if shot.mode != "RGBA":
        return False
    lo, _ = shot.getchannel("A").getextrema()
    return lo < 255


def contact_sheet(tiles, cols, thumb_w):
    """tiles: [(label, image)] in rows of `cols`; every tile is scaled to thumb_w wide."""
    scaled = []
    for label, im in tiles:
        h = max(1, round(im.height * thumb_w / im.width))
        scaled.append((label, im.resize((thumb_w, h), Image.LANCZOS)))
    th = max(im.height for _, im in scaled)
    label_h = 14
    rows = math.ceil(len(scaled) / cols)
    sheet = Image.new("RGB", (cols * thumb_w, rows * (th + label_h)), (32, 32, 32))
    d = ImageDraw.Draw(sheet)
    for n, (label, im) in enumerate(scaled):
        x, y = (n % cols) * thumb_w, (n // cols) * (th + label_h)
        d.text((x + 3, y + 1), label, fill=(230, 230, 230))
        sheet.paste(im, (x, y + label_h))
    return sheet


def run(src, out):
    files = sorted(f for f in os.listdir(src) if NAME.match(f))
    if not files:
        raise SystemExit(f"no <theme>-<30|50|70>.png in {src}")
    os.makedirs(out, exist_ok=True)
    by_level = {}
    for f in files:
        m = NAME.match(f)
        shot = Image.open(os.path.join(src, f))
        if not has_alpha(shot):
            raise SystemExit(f"{f} has no alpha: the headless path flattened it; add --keep-alpha to the scene flags")
        for b in BACKDROPS:
            img = over(shot, backdrop(b, shot.size))
            img.save(os.path.join(out, f"{m['theme']}-{m['level']}-{b}.png"))
            by_level.setdefault(m["level"], []).append((f"{m['theme']} {b}", img))
    for level, tiles in by_level.items():
        contact_sheet(tiles, len(BACKDROPS), 480).save(os.path.join(out, f"contact-{level}.png"))
    print(f"{len(files)} images, {len(files) * len(BACKDROPS)} composites, {len(by_level)} sheets -> {out}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--in", dest="src", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    run(a.src, a.out)


if __name__ == "__main__":
    sys.exit(main())
