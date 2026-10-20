"""Checks the headless DPI matrix: every scene at every scaling on both renderers.

Reads the folder run.ps1 wrote (`<scene>-<tag>-<renderer>.png`, the tag is the scale with an underscore
for its dot: 1, 1_25, 2_25) and checks each image:

  size        the image is round(W * s) by round(H * s), W x H the scene's logical size (1000 x 640, or the
              scene's own size from SCENE_SIZE: marks-overflow 700 x 640, update-window-small 1000 x 420, and the dictation\n              scenes that are windows of their own).
              The badge scene is a contact sheet, not a window, and is not checked here (NOT_WINDOWS)
  blank       more than two colours
  renderer    software against gpu at the same scene and scale: no pixel differs by more than 8 in any
              channel, except within 2 px (a square window) of a non-background pixel (the glyph edges;
              the background is the image's most common colour, a non-background pixel differs from it by
              more than 40 in some channel)
  drift       scale consistency: the image at scale s, resampled (Lanczos) to the 100% size, against the
              100% image; mean absolute difference per channel. A scene's threshold is 2.5 times the median
              of its eight errors (at least 1.0); a scale above it is a SUSPECT for a human to read (it is
              listed, not failed: a layout that drifts between scales shows here)

Failures exit 1. Suspects are printed and written to `suspects.txt` in the folder.

    python tools/dpi-matrix/check.py <folder> [--renderers software,gpu]
    python tools/dpi-matrix/check.py --selftest

Pillow and numpy (dev-only, this script only).
"""

from __future__ import annotations

import argparse
import math
import re
import statistics
import sys
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image

BASE_W = 1000
BASE_H = 640
SCALES = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0]
RENDERERS = ["software", "gpu"]
# A scene's own logical size, when it is not 1000 x 640 (scene_args in headless.rs).
SCENE_SIZE = {
    "marks-overflow": (700, 640),
    "update-window-small": (1000, 420),
    # The dictation scenes that are windows of their own (dictation_scenes.rs holds the same sizes; a test there
    # reads this table): the tab mark on a strip and on the sidebar, and the widget card at each face's height.
    "dictation-mark-strip": (400, 36),
    "dictation-mark-sidebar": (260, 148),
    "dictation-widget-listening": (480, 196),
    "dictation-widget-starting": (480, 196),
    "dictation-widget-pick": (480, 400),
    "dictation-widget-sent": (480, 104),
    "dictation-widget-warning": (480, 104),
}
# `badge` is a contact sheet of every taskbar disc at its own scales, not a window: --scale does not apply.
NOT_WINDOWS = {"badge"}
CROSS_TOL = 8
EDGE_PX = 2
BG_TOL = 40
DRIFT_K = 2.5
DRIFT_FLOOR = 1.0
NAME = re.compile(r"^(?P<scene>.+)-(?P<tag>\d+(?:_\d+)?)-(?P<renderer>software|gpu)\.png$")


def tag(scale: float) -> str:
    """1.0 -> '1', 1.25 -> '1_25', 2.25 -> '2_25'."""
    return f"{scale:g}".replace(".", "_")


def image_name(scene: str, scale: float, renderer: str) -> str:
    return f"{scene}-{tag(scale)}-{renderer}.png"


def round_half_up(x: float) -> int:
    """Rust's f32::round: halves go away from zero (all the sizes here are positive)."""
    return int(math.floor(x + 0.5))


def expected_size(scene: str, scale: float) -> tuple[int, int]:
    width, height = SCENE_SIZE.get(scene, (BASE_W, BASE_H))
    return round_half_up(width * scale), round_half_up(height * scale)


def packed(arr: np.ndarray) -> np.ndarray:
    """One integer per pixel (r, g, b packed), for counting colours fast."""
    a = arr.astype(np.uint32)
    return (a[..., 0] << 16) | (a[..., 1] << 8) | a[..., 2]


def background(arr: np.ndarray) -> np.ndarray:
    values, counts = np.unique(packed(arr), return_counts=True)
    bg = int(values[counts.argmax()])
    return np.array([(bg >> 16) & 255, (bg >> 8) & 255, bg & 255], dtype=np.int32)


def ink_mask(arr: np.ndarray) -> np.ndarray:
    """The non-background pixels: those differing from the most common colour by more than BG_TOL."""
    diff = np.abs(arr.astype(np.int32) - background(arr)).max(axis=2)
    return diff > BG_TOL


def grow(mask: np.ndarray, px: int) -> np.ndarray:
    """Every pixel within `px` (a square window) of a set pixel."""
    padded = np.pad(mask, px)
    h, w = mask.shape
    out = np.zeros_like(mask)
    for dy in range(2 * px + 1):
        for dx in range(2 * px + 1):
            out |= padded[dy : dy + h, dx : dx + w]
    return out


def check_size(img: Image.Image, scene: str, scale: float) -> str | None:
    want = expected_size(scene, scale)
    if img.size != want:
        return f"size: {img.size[0]}x{img.size[1]}, want {want[0]}x{want[1]}"
    return None


def check_not_blank(arr: np.ndarray) -> str | None:
    colours = len(np.unique(packed(arr)))
    if colours <= 2:
        return f"blank: {colours} colour(s)"
    return None


def check_renderers(software: np.ndarray, gpu: np.ndarray) -> str | None:
    if software.shape != gpu.shape:
        return f"renderer: size {gpu.shape[1]}x{gpu.shape[0]} against software {software.shape[1]}x{software.shape[0]}"
    over = np.abs(software.astype(np.int32) - gpu.astype(np.int32)).max(axis=2) > CROSS_TOL
    allowed = grow(ink_mask(software) | ink_mask(gpu), EDGE_PX)
    bad = over & ~allowed
    count = int(bad.sum())
    if count:
        ys, xs = np.nonzero(bad)
        return (
            f"renderer: {count} pixel(s) differ by more than {CROSS_TOL} away from any glyph edge, "
            f"first at x={int(xs[0])} y={int(ys[0])}"
        )
    return None


def scale_error(img: Image.Image, base: Image.Image) -> float:
    """Mean absolute difference per channel, the image resampled to the 100% size against the 100% image."""
    back = np.asarray(img.convert("RGB").resize(base.size, Image.LANCZOS), dtype=np.float64)
    ref = np.asarray(base.convert("RGB"), dtype=np.float64)
    return float(np.abs(back - ref).mean())


def drift_suspects(errors: dict[float, float]) -> list[tuple[float, float, float]]:
    """(scale, error, threshold) for every scale whose error is above the scene's threshold."""
    threshold = max(DRIFT_K * statistics.median(errors.values()), DRIFT_FLOOR)
    return [(s, e, threshold) for s, e in sorted(errors.items()) if e > threshold]


def scan(folder: Path) -> dict[tuple[str, str, float], Path]:
    found: dict[tuple[str, str, float], Path] = {}
    for path in folder.glob("*.png"):
        m = NAME.match(path.name)
        if m and m["scene"] not in NOT_WINDOWS:
            found[(m["scene"], m["renderer"], float(m["tag"].replace("_", ".")))] = path
    return found


def analyse(folder: Path, renderers: list[str]) -> tuple[list[str], list[str], int]:
    """Failures, suspect lines, and the number of images checked."""
    found = scan(folder)
    scenes = sorted({scene for (scene, _, _) in found})
    failures: list[str] = []
    suspects: list[str] = []
    checked = 0
    for scene in scenes:
        errors: dict[str, dict[float, float]] = {r: {} for r in renderers}
        for scale in SCALES:
            arrays: dict[str, np.ndarray] = {}
            images: dict[str, Image.Image] = {}
            for r in renderers:
                name = image_name(scene, scale, r)
                path = found.get((scene, r, scale))
                if path is None:
                    failures.append(f"{name}: missing image")
                    continue
                img = Image.open(path).convert("RGB")
                checked += 1
                images[r] = img
                arr = np.asarray(img)
                arrays[r] = arr
                msg = check_size(img, scene, scale) or check_not_blank(arr)
                if msg:
                    failures.append(f"{name}: {msg}")
            if "software" in arrays and "gpu" in arrays:
                msg = check_renderers(arrays["software"], arrays["gpu"])
                if msg:
                    failures.append(f"{scene}-{tag(scale)}: {msg}")
            for r, img in images.items():
                if scale == 1.0:
                    continue
                base = found.get((scene, r, 1.0))
                if base is not None:
                    errors[r][scale] = scale_error(img, Image.open(base).convert("RGB"))
        for r in renderers:
            if 1.0 not in errors[r] and found.get((scene, r, 1.0)) is None:
                failures.append(f"{image_name(scene, 1.0, r)}: missing the 100% reference")
                continue
            errors[r][1.0] = 0.0
            if len(errors[r]) < len(SCALES):
                continue
            for scale, err, threshold in drift_suspects(errors[r]):
                suspects.append(
                    f"SUSPECT {scene} {r} {tag(scale)}: drift {err:.2f} > {threshold:.2f} "
                    f"(scale {scale:g} against 100%)"
                )
    return failures, suspects, checked


def selftest() -> int:
    """Synthetic fixtures: a clean set passes; a mis-sized, a blank, a drifted and a renderer-mismatch set each fail."""
    rng = np.random.default_rng(7)
    base = np.full((BASE_H, BASE_W, 3), (30, 30, 30), dtype=np.uint8)
    first_rect = (0, 0)
    for i in range(12):
        x, y = int(rng.integers(120, 880)), int(rng.integers(80, 540))
        if i == 0:
            first_rect = (x, y)
        base[y : y + 18, x : x + 60] = (200 - 9 * i, 180, 90 + 5 * i)
    base_img = Image.fromarray(base)

    def scaled(scale: float, shift: int = 0) -> Image.Image:
        w, h = expected_size("scene", scale)
        img = base_img.resize((w, h), Image.LANCZOS)
        if shift:
            canvas = Image.new("RGB", (w, h), (30, 30, 30))
            canvas.paste(img, (shift, 0))
            img = canvas
        return img

    def write(folder: Path, scene: str, renderer: str, make) -> None:
        for scale in SCALES:
            make(scale).save(folder / image_name(scene, scale, renderer))

    checks = 0
    with tempfile.TemporaryDirectory(prefix="dpi-selftest-") as tmp:
        root = Path(tmp)

        clean = root / "clean"
        clean.mkdir()
        for r in RENDERERS:
            write(clean, "clean", r, scaled)
        failures, suspects, n = analyse(clean, RENDERERS)
        assert not failures and not suspects, (failures, suspects)
        assert n == 2 * len(SCALES), n
        checks += 1

        sized = root / "sized"
        sized.mkdir()
        write(sized, "sized", "software", scaled)
        # The 150% image is the 100% size: a scaling that was not applied.
        base_img.save(sized / image_name("sized", 1.5, "software"))
        failures, _, _ = analyse(sized, ["software"])
        assert any("size: 1000x640, want 1500x960" in f for f in failures), failures
        checks += 1

        blank = root / "blank"
        blank.mkdir()
        write(blank, "blank", "software", lambda s: Image.new("RGB", expected_size("blank", s), (9, 9, 9)))
        failures, _, _ = analyse(blank, ["software"])
        assert any("blank: 1 colour(s)" in f for f in failures), failures
        checks += 1

        drift = root / "drift"
        drift.mkdir()
        write(drift, "drift", "software", lambda s: scaled(s, shift=60 if s == 2.0 else 0))
        failures, suspects, _ = analyse(drift, ["software"])
        assert not failures, failures
        assert any(s.startswith("SUSPECT drift software 2:") for s in suspects), suspects
        assert not any("drift software 1" in s or "drift software 3" in s for s in suspects), suspects
        checks += 1

        mismatch = root / "mismatch"
        mismatch.mkdir()
        write(mismatch, "mismatch", "software", scaled)

        def gpu_patch(s: float) -> Image.Image:
            img = scaled(s).copy()
            # A 30 by 30 block in the background corner, far from every glyph, changed by 30 per channel.
            arr = np.asarray(img).copy()
            arr[5:35, 5:35] = (60, 60, 60)
            return Image.fromarray(arr)

        write(mismatch, "mismatch", "gpu", gpu_patch)
        failures, _, _ = analyse(mismatch, RENDERERS)
        assert any("renderer: " in f and "mismatch-1" in f for f in failures), failures
        checks += 1

        assert expected_size("marks-overflow", 1.5) == (1050, 960)
        assert expected_size("update-window-small", 2.0) == (2000, 840)
        checks += 1

        sheet = root / "sheet"
        sheet.mkdir()
        Image.new("RGB", (2000, 1600), (9, 9, 9)).save(sheet / image_name("badge", 1.0, "software"))
        failures, _, _ = analyse(sheet, ["software"])
        assert not failures, failures
        checks += 1

        glyph = root / "glyph"
        glyph.mkdir()
        write(glyph, "glyph", "software", scaled)

        def edge_patch(s: float) -> Image.Image:
            # The first rectangle and a 1 px ring round it differ by 60 on the gpu side: glyph-edge pixels, allowed.
            if s != 1.0:
                return scaled(s)
            arr = np.asarray(base_img).copy()
            x, y = first_rect
            arr[y - 1 : y + 19, x - 1 : x + 61] = (60, 60, 60)
            return Image.fromarray(arr)

        write(glyph, "glyph", "gpu", edge_patch)
        failures, _, _ = analyse(glyph, RENDERERS)
        assert not failures, failures
        checks += 1

    print(f"selftest: {checks} checks passed")
    return 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("folder", nargs="?", help="the folder run.ps1 wrote")
    ap.add_argument("--renderers", default=",".join(RENDERERS))
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args(argv)
    if args.selftest:
        return selftest()
    if not args.folder:
        ap.error("a folder is needed (or --selftest)")
    folder = Path(args.folder)
    renderers = [r for r in args.renderers.split(",") if r]
    failures, suspects, checked = analyse(folder, renderers)
    for line in failures:
        print(f"FAIL {line}")
    for line in suspects:
        print(line)
    (folder / "suspects.txt").write_text("\n".join(suspects) + ("\n" if suspects else ""), encoding="utf-8")
    print(f"checked {checked} images in {folder}: {len(failures)} failure(s), {len(suspects)} suspect(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
