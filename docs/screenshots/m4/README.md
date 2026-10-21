# Theme matrix (M4, Task 30)

Every theme on the wall (39, wall order) at 100, 150 and 225 percent, on both grid painters (software and GPU), beside Prism Terminal 0.58.0.

- `flare/<id>-<pct>-<renderer>.png`: Flare's `theme-sample` scene. Made by `tools/theme-matrix/run.ps1`. The 150 percent images were removed to keep this folder under 60 MB; re-run `run.ps1` to regenerate them.
- `pt/<id>-<pct>.png`: the installed Prism Terminal wearing each theme, from `tools/pt-shot/pt-shot.mjs` (version in `pt/VERSION.txt`).
- `side/<id>-<pct>.png`: Prism Terminal left, Flare (GPU) right, from `tools/theme-matrix/compose.ps1`.
- `sheet-<pct>-<renderer>.png`: contact sheets, 39 tiles in wall order.
- `tools/theme-matrix/check.ps1` compares software against GPU for every theme and scale. Run before the 150 percent images were removed: 117 pairs, 0 failing. Re-running it now reports the 39 removed 150 percent pairs as MISSING until `run.ps1` regenerates them.

Sizes come from the 1000x640 window at each scale, so a 225 percent image is 2250x1440.

## Review (Task 31, 2026-10-17)

Read with the image reader: the six contact sheets, the six side by sides named by the plan at 225 (`pt-default`, `paper`,
`volt`, `dracula`, `catppuccin-latte`, `cinder`) and crops of them. Then measured, every theme at 100, 150 and 225 on both
painters against Prism Terminal 0.58.0's own capture (the 150 percent Flare images regenerated for the check, then removed
again): the strip ground (`--p-tabs`), the grid ground, the active tab rule (`--p-accent-solid`), the bar's bottom divider,
the tab body, the tab name ink, the cursor, the sixteen blocks in order, and the ink of every text cell in the sample
(prompt, title, both digit rows, the bold hues, underline, `prediction`). Colours agree within 1 unit everywhere except
the rows marked below. Columns 100 / 150 / 225 cover both painters (software and GPU agree on all 117 pairs, `check.ps1`).

| id | 100 / 150 / 225 | verdict | note |
|---|---|---|---|
| `pitch` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `volt` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `prism` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `campbell` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `pt-default` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `graphite` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `monokai` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `gruvbox-dark` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `mist` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT; `inverse` ink differs (PT bug, see below) |
| `catppuccin-latte` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT; `inverse` ink differs (PT bug, see below) |
| `paper` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT; `inverse` ink differs (PT bug, see below) |
| `phosphor` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `amber` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `garnet` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `plum` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `moss` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `rosewood` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `tokyonight` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `horizon` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `dracula` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `catppuccin-mocha` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `kanagawa` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `ocean` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `umber` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `solarized-dark` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `synthwave` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `marine` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `cobalt` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `nord` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `retro` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `cinder` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT |
| `fawn` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT; `inverse` ink differs (PT bug, see below) |
| `sage` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT; `inverse` ink differs (PT bug, see below) |
| `lavender` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT; `inverse` ink differs (PT bug, see below) |
| `peach` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT; `inverse` ink differs (PT bug, see below) |
| `mint` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT; `inverse` ink differs (PT bug, see below) |
| `blossom` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT; `inverse` ink differs (PT bug, see below) |
| `sky` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT; `inverse` ink differs (PT bug, see below) |
| `butter` | ok / ok / ok | match | ground, strip, accent rule, divider, tab ink, cursor and the 16 blocks equal PT; `inverse` ink differs (PT bug, see below) |

**Defect found and fixed.** The line after the title mark sat one hairline too far right (device x 43 / 65 / 97 against
PT's 42 / 63 / 95), so the whole tab strip started one hairline late. PT's TitleMark is a 43px box (`pl-3.5 pr-3` around
the 17px mark) whose line is drawn INSIDE it (`after:right-0 after:w-px`); Flare had added the line after the 43px. Failing
test first (`theme_pixels.rs` `the_mark_line_sits_where_prism_terminal_draws_it`), then `ui/app.slint` (the mark cell is
`Px.snap(42px) + Px.hair()`), the scene and flow helpers' strip origin (42), and Task 30 re-run for every theme: the line
now lands on PT's pixel at all three scales.

**Not defects, recorded:**
- **Link (M5).** PT draws `https://example.com/themes` (and the echoed path) in the link blue, underlined; Flare draws it as
  plain text. Link finding is M5 (`findLinks`); `link_colour` already exists (Task 11b).
- **`inverse` on the eleven light themes: a Prism Terminal bug.** PT paints the inverse cell's ink black (`#000000`) on the
  dark foreground block, near illegible (xterm takes the inverse ink from its transparent theme background). Flare paints
  the ground colour, the legible reading of SGR 7. Flare is not changed to copy it.
- **Cell size (rendering difference, M1).** Flare's cells are whole device pixels (M1 metrics): 8x16 at 100, 11x23 at 150,
  17x34 at 225, against Chromium's 7.625x15, 11.406x23 and 17.125x34. At 100 percent Flare's grid is about 5 percent wider
  and one pixel taller per row. Owner question for a later milestone, not an M4 change (M4 adds no geometry).
- **Text antialiasing (rendering difference).** PT uses ClearType (coloured fringes, visible at 100); Flare is greyscale.
  At 225 every glyph's full-coverage ink equals PT's.
- **Help button.** PT 0.58.0's title row has a `?` button; Flare's has settings, minimise, maximise, close. Version drift
  (0.58.0 against the 0.55.6 Flare follows) and outside the theme review.
- **Scrollbar.** Absent in both (not drawn in M4, and PT shows none with no scrollback).
- **Selection.** Not in the sample scene; held by the unit tests of `selection_for`.
- **Tab width.** The active tab's right edge is equal at 100 and within one device pixel at 150 and 225 (Chromium's
  fractional layout).

**Glass.** The DWM captures (levels 0 and 40, acrylic on and off) are in `material/` with their numbers in
`docs/material.md`. The plan's levels 0, 25 and 70 on `pt-default` and `paper` were rendered headless (`--see-through`,
both painters, scale 1.5) and composited over a red-to-blue gradient: level 0 is opaque and identical to the matrix;
at 25 and 70 the strip and the grid carry one coat of the ground at alpha 0.75 and 0.30 (`GROUND_ALPHA_MIN`), the same on
both, the blocks, the cursor and the inverse cell stay opaque, the tab rule and text keep their colours. Verdict: as
designed. Over a saturated backdrop without acrylic, `paper` at 70 loses contrast on the red and yellow ink, as PT's own glass
does; in the real window the acrylic layer sits under the ground (material.md). Expected, not a defect; worth a look
by eye when the owner tries glass on a light theme.
