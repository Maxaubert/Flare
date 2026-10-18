# M9 Task 41: reading the images

Read on 2026-10-18 by Task 41 (Opus). Flare rendered by `tools/m9-matrix/run.ps1` (8 scenes x 8
scales x software and gpu, 128 PNGs in `flare/`), composed by `tools/m9-matrix/compose.py` into
`side/` and the two contact sheets. The PT reference is Task 3's capture in `pt/` (100, 150 and
225 percent of `chip`, `chip-notice`, `window`, `window-running`, `window-notice`, `window-many`)
and its numbers in `tools/pt-metrics/update.json`. Every image below was opened, and every
measurement was read off the pixels, not the layout properties.

`tools/pt-shot/pt-update.mjs` was NOT run: it starts the installed PT, and this pass never starts
or touches the owner's own windows. So `installing` and `empty` have no PT pair (Flare-only read),
and `update-window-small` (1000 x 420) is paired with Task 3's 1000 x 640 `window-many`, a near pair.
compose.py now falls back to Task 3's names, so the side-by-sides build from the files in the repo.

## Files opened

- `side/update-chip-{1,1_5,2_25}.png`, `side/update-chip-notice-{1,1_5,2_25}.png`,
  `side/update-window-{1,1_5,2_25}.png`, `side/update-window-running-{1,1_5,2_25}.png`,
  `side/update-window-notice-{1,1_5,2_25}.png`, `side/update-window-small-{1,1_5,2_25}.png`
- `flare/update-window-installing-1-software.png`, `flare/update-window-empty-1-software.png`,
  `flare/update-window-notice-1-software.png`, `flare/update-window-small-1-software.png`
- `matrix-chip.png`, `matrix-window.png`
- zoomed crops (nearest neighbour, 4 to 14x) of the chip, arrow, label and title buttons at 1.0,
  1.5, 2.25 and 3.0 beside PT; the notice at 1.0, 1.5, 3.0; the progress block at 2.5 (gpu) and
  3.0 (software); the title-bar flame mark at 1.0, 2.25, 3.0 beside `assets/icon/flare-white-on-black.png`
- `build/icon.ico`: all 19 frames, on dark and light grounds, 16 to 48 at 4 to 10x
- software against gpu: all 64 pairs diffed pixel by pixel

## Defects found and fixed (each: a failing test first, then the fix, then re-rendered and reread)

| # | Defect | Files | Test (failed before, passes after) | Fix |
|---|---|---|---|---|
| 1 | The chip sat 8 css px too far left: 200 px from the right edge, PT 192 (`toRightEdge`). The 10 px gap was taken from the buttons block's padded edge, not from the first button (`toNextControl`). | `side/update-chip-*.png`, `side/update-chip-notice-*.png` | `the_chip_ends_192_css_px_from_the_right_edge_as_in_pt`; `the_chip_is_the_leftmost_control_of_the_right_group_and_clears_the_buttons` now measures to the first button | `app.slint`: the chip slot (and the side-bar chip) add `snap(10 - 8)`, the block's own 8 px padding giving the rest |
| 2 | The chip's arrow was drawn about 10 percent small (6.7 x 7.5 css px against PT's 7.5 x 8.4): Slint's `Path` fits its geometry into its box minus the stroke width. | `side/update-chip-2_25.png` (zoomed) | `the_arrow_is_drawn_at_pt_size` (fails at 1.5 and up) | `update.slint`: the Path box is the icon grown by one stroke, offset by half of it |
| 3 | The same Path rule shrank the title-bar icons right of the chip (help, settings, window buttons) by about 7 percent (help circle 11.3 css px at 300 percent, PT 12.4). | `side/update-chip-2_25.png` (zoomed) | `the_help_circle_beside_the_chip_is_drawn_at_pt_size` (fails at 3.0) | `app.slint` `TitleButton`: the Path box grows by one stroke, still centred; the whole flare suite stays green |
| 4 | The window clipped the last note: "Under the hood" showed its heading with no entry. Through a `HorizontalLayout` a wrapping entry reports its one-line height, so the list measured 244 px for 267 px of content, and the scroller could never reach the clipped entry. | `side/update-window-1.png`, `side/update-window-notice-1.png`, `side/update-window-running-1_5.png` | `every_entry_of_a_release_that_fits_is_drawn` (counts the dots: 6 of 7 before) | `update.slint`: an entry is a Rectangle whose height is its Text's preferred height at the Text's own width |
| 5 | The notice under the chip cast no shadow. Slint drops `drop-shadow-*` set on a component's ROOT element (a compiler warning), so the GPU chrome drew it flat; PT's capture reads the ground 7 levels darker under it. | `side/update-chip-notice-1_5.png`, column read under the notice | `no_component_sets_its_shadow_on_its_root_element` (source scan: the headless software renderer draws no `BoxShadow` at all in Slint 1.18, `draw_box_shadow` is a TODO, so no frame can show it) | `update.slint`: the surface, border and shadow move to a child Rectangle |
| 6 | 10 of the 19 icon frames were blurred (16, 20, 42, 48, 54, 56, 60, 96, 128, 256; at 16 px the flame's yellow stroke was gone, 51 levels off on average): ImageMagick's `-density` path rendered them one pixel short and `-resize` stretched them back. | `build/icon.ico` frames | `test_every_frame_is_as_sharp_as_the_vector` in `tools/test_make_icon.py` (each frame within 3 levels of the 1024 render area-averaged to its size) | `tools/make-icon.py` renders with `-size NxN` (librsvg at the size itself), no resize; `build/icon.ico`, `build/icon-source.png`, `docs/assets/flare-icon.png` regenerated |

## Checked and good

- **Chip** (after fixes 1 to 3): 24 px high at every scale; arrow centred (box 11 px, drawn 7.5 x 8.4
  as PT's); label baseline on PT's row at 100, 150 and 225 percent; fill `#fe8f34`, PT's exact
  value; no hairline at the edge (the first fill column is the fill); right edge 192 px from the
  window edge (807.56 at 225 percent, PT 191.78); 10 px to the help button. The focus ring is held
  by the unit tests (the scene shows the chip unfocused; Task 3's PT chip-notice shot shows its ring).
- **Notice**: right aligned to the chip to the pixel at 1.0, 1.5, 2.25 and 3.0; 8 px under it; the
  border is one device pixel below 200 percent and `floor(scale)` above, the same 1 css px PT draws;
  the text is not clipped at 3.0.
- **Window**: the box 460 wide and centred; title, sub, three sections with 4 px accent-hi dots,
  every entry (after fix 4); `+ N more` is not reached by the sample; the 6 px track and the fill end
  on a device pixel at 42 percent (2.5 and 3.0 crops); status and percentage on one line; Cancel and
  Install right aligned 8 apart; Cancel disabled and the fill full while installing; the preview
  notice line and "Close" in the notice state; "No notes were published with this release." alone
  in the empty state. Vertical rhythm within 3 px of PT's at 100 percent (the fonts' line boxes differ).
- **1000 x 420**: the box fits with its buttons on screen; the notes stop at the 184.8 px cap
  (0.44 x 420) and the rest scrolls (held by `the_notes_list_scrolls_inside_its_cap`).
- **2.5 and 3.0**: nothing overlaps, no 1 px seams, the arrow and the flame mark are crisp.
- **The in-app flame mark**: the owner's flame, tinted, at every scale; the shape matches the SVG.
- **The app icon**: transparent outside the rounded square, no badge, no outline; crisp at every
  size after fix 6 (16 px stays as sharp as a 16 px render of the artwork allows).
- **Software against gpu**: the 64 pairs differ by at most 2 levels, only in the terminal grid
  (the gpu flag changes the grid painter; the chrome is the software renderer in both, so
  `--renderer gpu` screenshots cannot show a shadow either).

## Not fixed here

- None of the findings above is open. The PT `installing` and `empty` pairs, and a true 1000 x 420
  PT pair, need `pt-update.mjs` run while the owner's PT is free to start; until then those three
  are judged against PT's code (`UpdateDialog.tsx`) and the plan's checklist only.
