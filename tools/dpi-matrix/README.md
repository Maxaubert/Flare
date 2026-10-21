# The headless DPI matrix

Every headless scene at every display scaling, on both renderers: the images, a script that checks
them, and contact sheets for the reader (M10 Task 71). The live window at every scaling and across a
monitor move is Task 72 (`flare-e2e dpi`), not this.

## Run it

```powershell
# 1. the images (one flare.exe at a time; nothing opens a window)
powershell -File tools\dpi-matrix\run.ps1 -Exe target\release\flare.exe -Out <scratch>\dpi

# 2. the checks (run.ps1 runs them unless -NoCheck; the self-test proves the checks themselves)
python tools\dpi-matrix\check.py <scratch>\dpi
python tools\dpi-matrix\check.py --selftest

# 3. the sheets for docs/screenshots/m10/dpi (add -CopySuspects for the suspects' full images)
powershell -File tools\dpi-matrix\compose.ps1 -Images <scratch>\dpi -CopySuspects

# the M8 dictation sheets (both renderers, readable size, the real images at 1:1 for 1x, 2.25x and 3x)
powershell -File tools\dpi-matrix\run.ps1 -Exe <flare.exe> -Out <scratch>\dpi -Only dictation
foreach ($r in 'software', 'gpu') { powershell -File tools\dpi-matrix\compose.ps1 -Images <scratch>\dpi -Sheets docs\screenshots\m8\dictation -Only dictation -Renderer $r -Height 320 -Cell 480 -Native 1,2_25,3 -CopySuspects }
```

The M8 dictation scenes are real scenes: the pill in the window, the Dictation page through the Settings frame, the
widget card and the tab mark on the strip and the sidebar (`dictation_scenes.rs`). The widget and the mark are windows of
their own size, so `SCENE_SIZE` in `check.py` carries those sizes (a test in `dictation_scenes.rs` keeps the two equal).

The scenes are the names `flare.exe --scene list` prints (`SCENES` in `crates/flare/src/headless.rs`),
so the matrix cannot drift from the scene list. `badge` is left out: it is one contact sheet of every
taskbar disc at its own scales, not a window, so `--scale` does not apply to it (59 windows are rendered: the 44 of M10 and the 15 dictation scenes of M8).
The scales are 1, 1.25, 1.5, 1.75, 2, 2.25, 2.5 and 3
(`--scale`), the logical size 1000 x 640 (`--size`), the renderers `software` and `gpu` (the gpu pass
is skipped with a note when `--probe-backend` finds no adapter).

The 944 full images (59 scenes x 8 scales x 2 renderers) stay in the scratch folder. Only the sheets (`sheet-<scene>.png` for every
scene, and `sheet-100.png`, `sheet-150.png`, `sheet-225.png` as grids) and the suspects' full images
are committed, under `docs/screenshots/m10/dpi/`.

## What `check.py` verifies, per image

| Check | Rule | Message |
|---|---|---|
| size | `round(1000 * s)` by `round(640 * s)`; `marks-overflow` is 700 by 640 and `update-window-small` 1000 by 420, by their own rules | `size: WxH, want WxH` |
| blank | more than two colours | `blank: N colour(s)` |
| renderer | software against gpu at the same scene and scale: no pixel differs by more than 8 in any channel, except within 2 px (a square window) of a non-background pixel (the glyph edges; the background is the image's most common colour, a non-background pixel differs from it by more than 40 in some channel) | `renderer: N pixel(s) ...` |
| drift | scale consistency: the image at scale `s`, resampled (Lanczos) to the 100% size, against the 100% image, as the mean absolute difference per channel. A scene's threshold is 2.5 times the median of its eight errors, at least 1.0 | `SUSPECT <scene> <renderer> <scale>: drift ...` |

A failure exits 1. A suspect is listed in the output and in `suspects.txt` in the image folder; it is
not a failure. The suspects are read by a person (Task 73): a layout that drifts between scales shows up as a
suspect, and the reader decides whether the drift is a defect.

The renderer rule is an approximation of M5's glyph-edge rule: it allows a mismatch only near ink, so
a difference in a flat area is always reported.

## Files

- `run.ps1`: renders every scene at every scale and renderer, one process at a time.
- `check.py`: the checks above, and `--selftest` (a clean set passes; a mis-sized, a blank, a drifted,
  and a renderer-mismatch set each fail with the right message, and a glyph-edge difference passes).
- `compose.ps1`: the sheets, with System.Drawing (Windows, bicubic downscale). `-Only`, `-Name`, `-Height`, `-Cell` and
  `-Native` make a readable partial set; every sheet cell is labelled with the image's real size; a gpu pass writes
  `-gpu` sheets beside the software ones.
