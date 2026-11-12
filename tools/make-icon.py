"""Build Flare's Windows app icon files from the owner's artwork, assets/icon/app-windows.svg.

The app icon has its own sources (2026-11-12): app-windows.svg here for Windows, and the Icon Composer
icon assets/icon/app-macos.icon for the Mac, which flare-mac-app compiles with actool
(crates/flare-cef/src/macos/release.rs). assets/icon/flare-color.svg stays the in-app mark and is
not read here.

Writes build/icon-source.png (1024 px), build/icon.ico (19 frames, the sizes Windows asks for
between 100% and 300% scaling) and docs/assets/flare-icon.png (256 px). The app's exe reads
build/icon.ico through crates/flare/build.rs, so the exe, the taskbar and the Explorer verb
all show the same artwork.

Every frame is rendered straight from the SVG at its own size, never by shrinking another
bitmap, so the 16 px frame is as sharp as the vector allows. The rasteriser is librsvg:
`rsvg-convert` when it is on PATH (Homebrew's ImageMagick has no librsvg and its own reader
draws the artwork's gradient fills as nothing), else ImageMagick, whose Windows build reads SVG
with librsvg. The artwork is used as it is: the outside of the rounded square stays transparent.

Run: python tools/make-icon.py   (needs rsvg-convert or ImageMagick 7, `magick`, on PATH)
"""

import os
import shutil
import subprocess
import tempfile
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
SVG = ROOT / 'assets' / 'icon' / 'app-windows.svg'
SOURCE = ROOT / 'build' / 'icon-source.png'
OUT = ROOT / 'build' / 'icon.ico'
README = ROOT / 'docs' / 'assets' / 'flare-icon.png'

# What Windows asks for between 100% and 300% scaling (small 16, taskbar 24, large 32,
# each times the scale), so the shell never resamples a neighbour.
SIZES = [16, 20, 24, 28, 30, 32, 36, 40, 42, 48, 54, 56, 60, 64, 72, 80, 96, 128, 256]


def magick() -> str:
    found = os.environ.get('MAGICK') or shutil.which('magick')
    if not found:
        raise SystemExit('ImageMagick 7 (magick) was not found; put it on PATH or set MAGICK')
    return found


def render(px: int, out: Path) -> None:
    """Rasterise the SVG at exactly px x px, straight from the vector artwork.

    `-size` hands librsvg the output size itself. A `-density` derived from the size (the
    earlier way) rounds 10 of the 19 sizes one pixel short, and a `-resize` back to the size
    then blurred them (M9 Task 41), so nothing here resizes: a size that comes out wrong fails.
    """
    rsvg = os.environ.get('RSVG_CONVERT') or shutil.which('rsvg-convert')
    if rsvg:
        cmd = [rsvg, '-w', str(px), '-h', str(px), '-f', 'png', '-o', str(out), str(SVG)]
    else:
        cmd = [
            magick(),
            '-background', 'none',
            '-size', f'{px}x{px}',
            str(SVG),
            '-depth', '8',
            f'PNG32:{out}',
        ]
    subprocess.run(cmd, check=True)
    got = Image.open(out).size
    if got != (px, px):
        raise SystemExit(f'the SVG rendered at {got}, not {px}x{px}')


def main() -> None:
    SOURCE.parent.mkdir(parents=True, exist_ok=True)
    README.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        frames = {}
        for n in SIZES:
            path = Path(tmp) / f'frame-{n}.png'
            render(n, path)
            frames[n] = Image.open(path).convert('RGBA')
            frames[n].load()
        render(1024, SOURCE)
        frames[256].save(
            OUT,
            format='ICO',
            sizes=[(n, n) for n in SIZES],
            append_images=[frames[n] for n in SIZES if n != 256],
        )
        frames[256].save(README)
    print(f'{OUT.relative_to(ROOT)}: {len(SIZES)} frames, {OUT.stat().st_size} bytes')


if __name__ == '__main__':
    main()
