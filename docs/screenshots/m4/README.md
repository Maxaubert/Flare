# Theme matrix (M4, Task 30)

Every theme on the wall (39, wall order) at 100, 150 and 225 percent, on both grid painters (software and GPU), beside Prism Terminal 0.58.0.

- `flare/<id>-<pct>-<renderer>.png`: Flare's `theme-sample` scene. Made by `tools/theme-matrix/run.ps1`. The 150 percent images were removed to keep this folder under 60 MB; re-run `run.ps1` to regenerate them.
- `pt/<id>-<pct>.png`: the installed Prism Terminal wearing each theme, from `tools/pt-shot/pt-shot.mjs` (version in `pt/VERSION.txt`).
- `side/<id>-<pct>.png`: Prism Terminal left, Flare (GPU) right, from `tools/theme-matrix/compose.ps1`.
- `sheet-<pct>-<renderer>.png`: contact sheets, 39 tiles in wall order.
- `tools/theme-matrix/check.ps1` compares software against GPU for every theme and scale. Run before the 150 percent images were removed: 117 pairs, 0 failing. Re-running it now reports the 39 removed 150 percent pairs as MISSING until `run.ps1` regenerates them.

Sizes come from the 1000x640 window at each scale, so a 225 percent image is 2250x1440.
