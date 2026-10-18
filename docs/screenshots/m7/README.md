# M7 screenshots: the Settings tab beside Prism Terminal (Task 52)

Made on 2026-10-18 from the release build of `flare.exe` (target folder `.claude\targets\t1`) and from a scratch copy of the installed Prism Terminal 0.55.6 (run offscreen, unfocusable, with its own temp profile; the real `%APPDATA%` profile folders were checked untouched by the capture script).

The matrix is 52 images, listed by `node tools/pt-shot/pt-settings.mjs --list`:

- `flare/` and `pt/`: one PNG per image, named `<page>-<variant>-<pct>.png`. Each side is 1000 by 640 css px at its scale (for example 2250 by 1440 at 225 percent).
- `side/`: each pair side by side, Prism Terminal left, Flare right.
- `sheet-100.png`, `sheet-150.png`, `sheet-225.png`: contact sheets of the side-by-sides at each main scaling.

The variants are `plain` (the six pages), `picker`, `editor`, `ask`, `help`, `wall-expanded` (Appearance), `select` and `help` (Terminal), and `find` (Appearance, the text "colour"). Appearance and Terminal are also `plain` at 125, 175, 200, 250 and 300 percent.

`check.ps1` passed: 52 images, none blank, every size equal to `round(1000 * s)` by `round(640 * s)` on both sides.

The folder is 37 MB, under the 60 MB limit, so every image is kept.

Regenerate with `tools/settings-matrix/run.ps1`, `pt-settings.mjs`, `compose.ps1` and `check.ps1` (see `tools/settings-matrix/README.md`).

The review table (page, scale, verdict, note) is written in Task 53.
