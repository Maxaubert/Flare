# M5 screenshots: the scaling matrix (Task 27)

Made by `flare.exe --scene <name> --scale <s> --size 1000x640 --renderer software --screenshot <name>-<pct>.png` for the
12 scenes at 100, 125, 150, 175, 200, 225, 250 and 300 percent (96 images), plus `--renderer gpu` for `term-find`,
`term-selection` and `term-links` at the same eight scales (`<name>-gpu-<pct>.png`, 24 images). A 225 percent image is 2250 x 1440.

## How they were reviewed (2026-10-17)

- Read with the image reader, whole and cropped at 2x to 6x: every scene at 100, 150 and 225, and the key scenes
  (`term-selection`, `term-find`, `term-links`, `term-copied`, both painters) at all eight scales.
- Measured with scripts at every scale (device pixels): the selection band (no gap between rows, block a rectangle),
  the find outlines (`max(1, floor(scale))`: 1 at 100 to 175, 2 at 200 to 250, 3 at 300, outside the box), the find bar
  (12 px from the strip and the right edge, hairline border, 334 x 30 css), the link underline (hair thick; faint, full
  when hovered), the menu rows (28 css px each, hairlines between), the Copied pill (size, centre, 24 px up, border).
- Software against GPU: every pair differs only in glyph anti-aliasing (no pixel more than 8 units apart); all
  rectangles, outlines and underlines are identical.
- Prism Terminal side by side: NOT made. The scratch Prism Terminal run (its own offscreen e2e and a `ref-shots.mjs`)
  was not set up in this pass, and the owner's running window is never driven, so the `compare-<name>-<pct>.png` pairs
  are missing. The look was judged against Prism Terminal's source instead: `TermFind.tsx` (bar `right-3 top-3`,
  `px-2 py-1`, 180 px input, 52 px right-aligned readout, three `h-5 w-5` buttons, no shadow), `ContextMenu.tsx` and
  `MenuIcon.tsx` (28 px rows, `border-t` hairlines, a 13 px glyph on EVERY row, 10.5 px hints), `CopiedBadge.tsx`
  (`bottom-6`, `rounded-full`, `px-3.5 py-1.5`, 12 px semibold).

## Defects found and fixed

| defect | test first (failed, then passed) | fix |
|---|---|---|
| Terminal menu rows had no glyph except `Copy`/`Copy path` (labels shifted left) | `tests/headless.rs` `every_terminal_menu_row_has_its_glyph` (pixels in the glyph column and the gap), `term_menu::every_row_has_an_icon_that_exists_in_glyphs` | `menus_ui::glyph_path` knows `menu-open`, `menu-paste`, `menu-link`, `menu-folder`, `menu-find`, `menu-help` |
| Every context menu's last row was 2 css px taller (border added on top of the layout's padding) | `ui_tests::menus::every_row_is_28_tall_including_the_last_and_the_border_closes_it` (all 8 scales) | `ui/menus.slint` panel height is the layout's own height |
| `term-find-many` read `1 of 1000`, not the plan's `1000+` | `headless::tests::the_many_scene_reads_a_thousand_plus` | the scene steps back once, past the counted thousand |

## Verdicts

`read` = looked at with the image reader; `measured` = checked by script only. Every row passes after the fixes.

| scene | scale | renderer | verdict | checked | what was fixed |
|---|---|---|---|---|---|
| `term-selection` | 100 | software | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 100 | gpu | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 125 | software | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 125 | gpu | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 150 | software | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 150 | gpu | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 175 | software | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 175 | gpu | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 200 | software | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 200 | gpu | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 225 | software | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 225 | gpu | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 250 | software | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 250 | gpu | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 300 | software | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection` | 300 | gpu | ok: one band, no seam between rows, last column whole, no sliver; selected text keeps its ink | read |  |
| `term-selection-wide` | 100 | software | ok: block is one rectangle (7 cells x 3 rows), starts on the wide emoji and covers both its cells | read |  |
| `term-selection-wide` | 125 | software | ok: block is one rectangle (7 cells x 3 rows), starts on the wide emoji and covers both its cells | measured |  |
| `term-selection-wide` | 150 | software | ok: block is one rectangle (7 cells x 3 rows), starts on the wide emoji and covers both its cells | read |  |
| `term-selection-wide` | 175 | software | ok: block is one rectangle (7 cells x 3 rows), starts on the wide emoji and covers both its cells | measured |  |
| `term-selection-wide` | 200 | software | ok: block is one rectangle (7 cells x 3 rows), starts on the wide emoji and covers both its cells | measured |  |
| `term-selection-wide` | 225 | software | ok: block is one rectangle (7 cells x 3 rows), starts on the wide emoji and covers both its cells | read |  |
| `term-selection-wide` | 250 | software | ok: block is one rectangle (7 cells x 3 rows), starts on the wide emoji and covers both its cells | measured |  |
| `term-selection-wide` | 300 | software | ok: block is one rectangle (7 cells x 3 rows), starts on the wide emoji and covers both its cells | measured |  |
| `term-find` | 100 | software | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 100 | gpu | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 125 | software | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 125 | gpu | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 150 | software | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 150 | gpu | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 175 | software | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 175 | gpu | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 200 | software | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 200 | gpu | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 225 | software | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 225 | gpu | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 250 | software | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 250 | gpu | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 300 | software | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find` | 300 | gpu | ok: fills tile cells; outline hair px outside the box; active is the accent; wrapped match on both rows; bar 12 px from top and right, hairline border, no shadow, 334 x 30 | read |  |
| `term-find-none` | 100 | software | ok: readout `none` right-aligned; previous and next dimmed | read |  |
| `term-find-none` | 125 | software | ok: readout `none` right-aligned; previous and next dimmed | measured |  |
| `term-find-none` | 150 | software | ok: readout `none` right-aligned; previous and next dimmed | read |  |
| `term-find-none` | 175 | software | ok: readout `none` right-aligned; previous and next dimmed | measured |  |
| `term-find-none` | 200 | software | ok: readout `none` right-aligned; previous and next dimmed | measured |  |
| `term-find-none` | 225 | software | ok: readout `none` right-aligned; previous and next dimmed | read |  |
| `term-find-none` | 250 | software | ok: readout `none` right-aligned; previous and next dimmed | measured |  |
| `term-find-none` | 300 | software | ok: readout `none` right-aligned; previous and next dimmed | measured |  |
| `term-find-many` | 100 | software | ok: readout `1000+`; no match fills past the limit (xterm clears them) | read | scene now steps back past the thousand (read `1 of 1000` before) |
| `term-find-many` | 125 | software | ok: readout `1000+`; no match fills past the limit (xterm clears them) | measured | scene now steps back past the thousand (read `1 of 1000` before) |
| `term-find-many` | 150 | software | ok: readout `1000+`; no match fills past the limit (xterm clears them) | read | scene now steps back past the thousand (read `1 of 1000` before) |
| `term-find-many` | 175 | software | ok: readout `1000+`; no match fills past the limit (xterm clears them) | measured | scene now steps back past the thousand (read `1 of 1000` before) |
| `term-find-many` | 200 | software | ok: readout `1000+`; no match fills past the limit (xterm clears them) | measured | scene now steps back past the thousand (read `1 of 1000` before) |
| `term-find-many` | 225 | software | ok: readout `1000+`; no match fills past the limit (xterm clears them) | read | scene now steps back past the thousand (read `1 of 1000` before) |
| `term-find-many` | 250 | software | ok: readout `1000+`; no match fills past the limit (xterm clears them) | measured | scene now steps back past the thousand (read `1 of 1000` before) |
| `term-find-many` | 300 | software | ok: readout `1000+`; no match fills past the limit (xterm clears them) | measured | scene now steps back past the thousand (read `1 of 1000` before) |
| `term-find-empty` | 100 | software | ok: placeholder shown, empty readout, caret before it | read |  |
| `term-find-empty` | 125 | software | ok: placeholder shown, empty readout, caret before it | measured |  |
| `term-find-empty` | 150 | software | ok: placeholder shown, empty readout, caret before it | read |  |
| `term-find-empty` | 175 | software | ok: placeholder shown, empty readout, caret before it | measured |  |
| `term-find-empty` | 200 | software | ok: placeholder shown, empty readout, caret before it | measured |  |
| `term-find-empty` | 225 | software | ok: placeholder shown, empty readout, caret before it | read |  |
| `term-find-empty` | 250 | software | ok: placeholder shown, empty readout, caret before it | measured |  |
| `term-find-empty` | 300 | software | ok: placeholder shown, empty readout, caret before it | measured |  |
| `term-links` | 100 | software | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 100 | gpu | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 125 | software | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 125 | gpu | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 150 | software | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 150 | gpu | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 175 | software | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 175 | gpu | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 200 | software | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 200 | gpu | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 225 | software | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 225 | gpu | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 250 | software | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 250 | gpu | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 300 | software | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-links` | 300 | gpu | ok: link blue ink; hairline underline (faint), hovered url full ink; wrapped url inked and underlined on both rows | read |  |
| `term-menu-plain` | 100 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | read | glyphs on every row; last row no longer 2 px taller |
| `term-menu-plain` | 125 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-plain` | 150 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | read | glyphs on every row; last row no longer 2 px taller |
| `term-menu-plain` | 175 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-plain` | 200 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-plain` | 225 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | read | glyphs on every row; last row no longer 2 px taller |
| `term-menu-plain` | 250 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-plain` | 300 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-selection` | 100 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | read | glyphs on every row; last row no longer 2 px taller |
| `term-menu-selection` | 125 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-selection` | 150 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | read | glyphs on every row; last row no longer 2 px taller |
| `term-menu-selection` | 175 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-selection` | 200 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-selection` | 225 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | read | glyphs on every row; last row no longer 2 px taller |
| `term-menu-selection` | 250 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-selection` | 300 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-link` | 100 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | read | glyphs on every row; last row no longer 2 px taller |
| `term-menu-link` | 125 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-link` | 150 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | read | glyphs on every row; last row no longer 2 px taller |
| `term-menu-link` | 175 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-link` | 200 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-link` | 225 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | read | glyphs on every row; last row no longer 2 px taller |
| `term-menu-link` | 250 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-link` | 300 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-path` | 100 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | read | glyphs on every row; last row no longer 2 px taller |
| `term-menu-path` | 125 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-path` | 150 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | read | glyphs on every row; last row no longer 2 px taller |
| `term-menu-path` | 175 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-path` | 200 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-path` | 225 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | read | glyphs on every row; last row no longer 2 px taller |
| `term-menu-path` | 250 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-menu-path` | 300 | software | ok: rows 28 px, hairlines, 13 px glyphs, hints right-aligned | measured | glyphs on every row; last row no longer 2 px taller |
| `term-copied` | 100 | software | ok: pill 69 x 32 css, centred (within half a px), 24 px up, fully round, one hairline border, semibold | read |  |
| `term-copied` | 125 | software | ok: pill 69 x 32 css, centred (within half a px), 24 px up, fully round, one hairline border, semibold | read |  |
| `term-copied` | 150 | software | ok: pill 69 x 32 css, centred (within half a px), 24 px up, fully round, one hairline border, semibold | read |  |
| `term-copied` | 175 | software | ok: pill 69 x 32 css, centred (within half a px), 24 px up, fully round, one hairline border, semibold | read |  |
| `term-copied` | 200 | software | ok: pill 69 x 32 css, centred (within half a px), 24 px up, fully round, one hairline border, semibold | read |  |
| `term-copied` | 225 | software | ok: pill 69 x 32 css, centred (within half a px), 24 px up, fully round, one hairline border, semibold | read |  |
| `term-copied` | 250 | software | ok: pill 69 x 32 css, centred (within half a px), 24 px up, fully round, one hairline border, semibold | read |  |
| `term-copied` | 300 | software | ok: pill 69 x 32 css, centred (within half a px), 24 px up, fully round, one hairline border, semibold | read |  |

## Notes, no change made

- The active match's text keeps its own (light) ink on the accent, as in Prism Terminal (xterm decorations sit under the text).
- Outlines of matches on adjacent rows overlap by the outline width; they are drawn outside the box, as xterm's `outline` is.
- The menu panel is `max(156, content) + 2 * hair` wide, 2 px wider than Prism Terminal's border-box `min-w-[156px]` when the rows are narrower than 156; not visible, left as is.
