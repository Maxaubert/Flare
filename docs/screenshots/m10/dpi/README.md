# The display-scaling images, read (M10 Task 73)

What was looked at, what was measured and the verdict for every scene of the headless matrix (Task 71,
`tools/dpi-matrix`) at the eight scalings on both renderers, plus the live window's `monitorMove`
snapshots (Task 72). Read by Opus on 2026-10-19 from the build at `f65ec7f` (no `ui/` or `flare-render`
change since; the matrix images were rendered 16:18 to 16:30 that day).

**Verdict: no defect.** Nothing at any scaling, on either renderer, needs a fix. Two things differ between
scalings on purpose and are kept (K1, K2 below).

## How it was read

- **read**: the image (or a crop of the same logical region at several scales, enlarged with nearest-neighbour
  so device pixels show) was looked at by eye. A scale group counts as read when at least one of its scales was.
- **measured**: a script measured it. Every one of the 704 images passed `check.py` (size, not blank, software
  against GPU outside the glyph edges, scale drift; 0 failures, 2 suspects), and `tools/dpi-matrix/measure.py`
  (added by this task) measured the tab strip's bottom line in every horizontal-tab scene at every scale and both
  renderers: always `max(1, floor(scale))` solid device pixels of one colour (1 1 1 1 2 2 2 3), never a blended
  row. Every scene's 8-scale sheet (`sheet-<scene>.png`) and the three grids (`sheet-100/150/225.png`) were also
  looked at, at sheet resolution.
- Software against GPU: `check.py`'s renderer rule passed for every scene and scale, so one verdict covers both;
  the last column says which renderer the full-size reading used.

Run the measurements again with:

```powershell
python tools\dpi-matrix\measure.py <scratch>\dpi    # the folder run.ps1 wrote
```

## Verdict table
| Scene | 100 | 125-175 | 200 | 225-250 | 300 | Software / GPU |
|---|---|---|---|---|---|---|
| `strip-3` | ok (read) | ok (read) | ok (read) | ok (read) | ok (read) | same (check.py renderer rule); read on gpu |
| `strip-hover` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `strip-many` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `strip-list` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `sidebar-open` | ok (read) | ok (read) | ok (read) | ok (read) | ok (read) | same (check.py renderer rule); read on software |
| `sidebar-find` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `sidebar-collapsed` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `start` | ok (read) | ok (read) | ok (measured) | ok (read) | ok (measured) | same (check.py renderer rule); read on software |
| `menu-places` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `menu-tab` | ok (read) | ok (read) | ok (read) | ok (read) | ok (read) | same (check.py renderer rule); read on software |
| `dialog-close` | ok (read) | ok (read) | ok (measured) | ok (read) | ok (read) | same (check.py renderer rule); read on gpu |
| `tip` | ok (read) | ok (read) | ok (measured) | ok (read) | ok (read) | same (check.py renderer rule); read on gpu |
| `theme-sample` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `term-selection` | ok (read) | ok (read) | ok (read) | ok (read) | ok (read) | same (check.py renderer rule); read on software |
| `term-selection-wide` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `term-find` | ok (read) | ok (read) | ok (measured) | ok (read) | ok (read) | same (check.py renderer rule); read on gpu |
| `term-find-none` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `term-find-many` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `term-find-empty` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `term-links` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `term-menu-plain` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `term-menu-selection` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `term-menu-link` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `term-menu-path` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `term-copied` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `marks-ring` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `marks-bar` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `marks-bar-flow` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `marks-sidebar` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `marks-sidebar-bar` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `marks-icons` | ok (read) | ok (read) | ok (read) | ok (read) | ok (read) | same (check.py renderer rule); read on gpu |
| `marks-overflow` | ok (read) | ok (read) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule); read on gpu |
| `marks-tip` | ok (read) | ok (read) | ok (measured) | ok (read) | ok (read) | same (check.py renderer rule); read on gpu |
| `marks-skeleton` | ok (read) | ok, kept K1 (read) | ok (measured) | ok, kept K1 (measured) | ok (measured) | same (check.py renderer rule); read on gpu |
| `marks-close` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `settings` | ok (read) | ok (read) | ok (read) | ok (read) | ok (read) | same (check.py renderer rule); read on gpu |
| `update-chip` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `update-chip-notice` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `update-window` | ok (measured) | ok (read) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule); read on gpu |
| `update-window-running` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `update-window-installing` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `update-window-notice` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |
| `update-window-small` | ok (read) | ok (read) | ok (measured) | ok (measured) | ok (read) | same (check.py renderer rule); read on gpu |
| `update-window-empty` | ok (measured) | ok (measured) | ok (measured) | ok (measured) | ok (measured) | same (check.py renderer rule) |

## What the reading checked, scene by scene

- **Tab strip** (`strip-3`, `marks-icons`): the logo, the tab glyphs, the agent marks, the accent bar, the
  separators and the window controls at all eight scales. Ink boxes of the chrome icons (`measure.py`, logical px)
  stay within one device pixel of each other across scales (logo 15.0 to 15.4 wide, gear 14.0 to 15.4, close
  5.6 to 6.7): the stroke fix of Tasks 50 to 52 holds at every scaling, not only at 100 and 225.
- **Sidebar** (`sidebar-open`): the terminal panel's rounded corner and its hairline at all eight scales, the
  find field, the rows and the New tab button at 175%.
- **Terminal** (`term-find`, `term-selection`): the find highlights cover exactly the three cells of `err` at
  every scale read; the selection band runs whole rows and starts on the same column at every scale.
- **Menus, dialog, tooltips** (`menu-tab`, `dialog-close`, `tip`, `marks-tip`): position, borders, the shortcut
  column and the button outlines agree across scales within a device pixel.
- **Settings** (Appearance): the segmented controls' pills and outlines at all eight scales; the cards, icons
  and the theme tiles at 125%.
- **Update window** (`update-window`, `update-window-small`): the notes wrap at the same words at 100, 175 and
  300; the buttons stay inside the window.
- **Overflow list** (`marks-overflow`): the list, its sections, marks and the spinner at 100 and 175.
- **`monitorMove`** (the live window, Task 72): a snapshot after each of the eight moves on both renderers
  (`dpi.rs` now saves them as `monitorMove-<renderer>-move<n>-<pct>.png`, plus `-after-move`, into `--shots`).
  Read: GPU at 225, 100 and 300, software at 225 after the resize and the final 100 with `FLOOD-END` and the
  `after-move` echo. Every one shows a full grid of whole rows, sharp text, the strip at its scale, the
  scrollbar on the right edge and the cursor on the last row; the run itself passed 81 of 81 checks
  (2026-10-19, 59 s).

## Suspects and kept differences

| Id | What | Why it is kept |
|---|---|---|
| K1 | `marks-skeleton` at 125% (the only `check.py` suspects, both renderers, drift 3.32 against a threshold of 3.04): the resume skeleton's lines sit a little lower further down the page at fractional scales (line 12 at 352.0 px at 125% against 347.0 at 100%) | Each line's height and gap are snapped to whole device pixels (`Px.snap` in `skeleton.slint`), so at 125% a line is 14 + 13 device px (21.6 px) instead of 21. The lines stay crisp and evenly spaced; it is a placeholder page, not aligned to anything else. At 100, 200 and 300 the tops are identical (`measure.py`). Not a defect. |
| K2 | The grid holds slightly more or fewer columns and rows per logical pixel at different scales (for example the truecolour bar is 512 px wide at 100% and about 475 px at 175%) | Cells are whole device pixels (owner question Q3, Task 56): 8 x 16 at 100%, 13 device px wide at 175%. `dpiMatrix` checks the cell metrics and the grid size at every scale. Not a defect. |

No scene shows a missing hairline, a blurred hairline, a misplaced overlay, a clipped label or a mis-sized
icon at any scaling. No defect task is filed for the defect pass.
