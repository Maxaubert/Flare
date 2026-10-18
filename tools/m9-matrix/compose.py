"""Side-by-sides (Flare left, Prism Terminal right) and the two contact sheets of the M9 matrix.

Reads docs/screenshots/m9/flare (from run.ps1) and docs/screenshots/m9/pt (from
tools/pt-shot/pt-update.mjs) and writes, under docs/screenshots/m9/:

  side/<scene>-<scale>.png   for 1.0, 1.5 and 2.25: Flare left, Prism Terminal right, a 24 px header
                             strip over each half carrying the two names
  matrix-chip.png            the update-chip scene at the eight Flare scales, in a labelled grid
  matrix-window.png          the update-window scene at the eight Flare scales, in a labelled grid

Pillow only. An image one side lacks is reported and left out (the run still exits 0 unless nothing
at all could be composed). Run it from anywhere:

    python tools/m9-matrix/compose.py [--shots docs/screenshots/m9] [--renderer software]
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

SCENES = [
    "update-chip",
    "update-chip-notice",
    "update-window",
    "update-window-running",
    "update-window-installing",
    "update-window-notice",
    "update-window-small",
    "update-window-empty",
]
SIDE_SCALES = [1.0, 1.5, 2.25]
ALL_SCALES = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0]
HEADER_H = 24
GROUND = (0x1C, 0x1C, 0x1C)
INK = (0xE6, 0xE6, 0xE6)
CELL_W = 480  # a grid cell's image width in the contact sheets
LABEL_H = 22


def tag(scale: float) -> str:
    """1.0 -> '1', 1.25 -> '1_25', 2.25 -> '2_25' (the tag both tools put in a file name)."""
    text = f"{scale:g}"
    return text.replace(".", "_")


def font(size: int) -> ImageFont.ImageFont | ImageFont.FreeTypeFont:
    for name in ("segoeui.ttf", "arial.ttf", "DejaVuSans.ttf"):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            continue
    return ImageFont.load_default()


def with_header(img: Image.Image, text: str) -> Image.Image:
    """The image under a 24 px strip that names it."""
    out = Image.new("RGB", (img.width, img.height + HEADER_H), GROUND)
    out.paste(img.convert("RGB"), (0, HEADER_H))
    ImageDraw.Draw(out).text((8, 4), text, fill=INK, font=font(14))
    return out


def side_by_side(flare: Path, pt: Path, pt_name: str) -> Image.Image:
    left = with_header(Image.open(flare), "Flare")
    right = with_header(Image.open(pt), pt_name)
    height = max(left.height, right.height)
    out = Image.new("RGB", (left.width + right.width, height), GROUND)
    out.paste(left, (0, 0))
    out.paste(right, (left.width, 0))
    return out


def contact_sheet(files: list[tuple[str, Path]], columns: int = 4) -> Image.Image:
    """The images, each scaled to CELL_W wide, under a label, in a grid."""
    cells: list[Image.Image] = []
    for label, path in files:
        img = Image.open(path).convert("RGB")
        h = round(img.height * CELL_W / img.width)
        img = img.resize((CELL_W, h), Image.LANCZOS)
        cell = Image.new("RGB", (CELL_W, h + LABEL_H), GROUND)
        cell.paste(img, (0, LABEL_H))
        ImageDraw.Draw(cell).text((6, 3), label, fill=INK, font=font(14))
        cells.append(cell)
    rows = (len(cells) + columns - 1) // columns
    row_h = [max(c.height for c in cells[r * columns : (r + 1) * columns]) for r in range(rows)]
    gap = 6
    sheet = Image.new(
        "RGB",
        (columns * CELL_W + (columns + 1) * gap, sum(row_h) + (rows + 1) * gap),
        (0x20, 0x20, 0x20),
    )
    y = gap
    for r in range(rows):
        for c in range(columns):
            i = r * columns + c
            if i < len(cells):
                sheet.paste(cells[i], (gap + c * (CELL_W + gap), y))
        y += row_h[r] + gap
    return sheet


def main(argv: list[str]) -> int:
    root = Path(__file__).resolve().parents[2]
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--shots", default=str(root / "docs" / "screenshots" / "m9"))
    ap.add_argument("--renderer", default="software", choices=["software", "gpu"])
    args = ap.parse_args(argv)
    shots = Path(args.shots)
    flare_dir, pt_dir, side_dir = shots / "flare", shots / "pt", shots / "side"
    side_dir.mkdir(parents=True, exist_ok=True)

    pt_name = "Prism Terminal"
    version = pt_dir / "VERSION.txt"
    if version.is_file():
        pt_name = f"Prism Terminal {version.read_text(encoding='utf-8').strip()}"

    made = skipped = 0
    for scene in SCENES:
        for scale in SIDE_SCALES:
            t = tag(scale)
            flare = flare_dir / f"{scene}-{t}-{args.renderer}.png"
            pt = pt_dir / f"{scene}-{t}.png"
            if not flare.is_file() or not pt.is_file():
                print(f"skip {scene}-{t}: an image is missing ({'flare' if not flare.is_file() else 'pt'})")
                skipped += 1
                continue
            side_by_side(flare, pt, pt_name).save(side_dir / f"{scene}-{t}.png")
            made += 1

    sheets = 0
    for name, scene in (("matrix-chip.png", "update-chip"), ("matrix-window.png", "update-window")):
        files = []
        for scale in ALL_SCALES:
            path = flare_dir / f"{scene}-{tag(scale)}-{args.renderer}.png"
            if path.is_file():
                files.append((f"{scene} {scale:g}x", path))
        if not files:
            print(f"skip {name}: no Flare images of {scene}")
            continue
        contact_sheet(files).save(shots / name)
        sheets += 1

    print(f"side-by-sides: {made} (skipped {skipped}); sheets: {sheets}; folder: {shots}")
    return 0 if (made or sheets) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
