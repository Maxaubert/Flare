# M10 screenshots: the M5 terminal scenes beside Prism Terminal (Tasks 58, 59)

Made on 2026-10-19. M5 never made its side-by-side images (`docs/screenshots/m5/README.md` says so); this folder closes that gap.

- **Prism Terminal side** (`<scene>-<pct>.png`): `node tools/pt-shot/pt-term.mjs --out docs/screenshots/m10/m5-pt --scales 1,1.5,2.25 --size 1000x640`, a scratch run of the installed Prism Terminal with a throwaway `--user-data-dir` under `%TEMP%`, `--e2e` (parked offscreen, unfocusable, its clipboard the in-memory stand-in), reaped by profile path. The installed copy reports 0.58.0 (`VERSION.txt`), not 0.55.6 (plan Q5). The run exited 0: profile removed, the two real profile folders under `%APPDATA%` untouched, every image the asked size. The Windows clipboard sequence number was 1992 before and after the run (Ctrl+C in `term-copied` reached the stand-in, never the machine's clipboard).
- **Flare side** (`flare/<scene>-<pct>.png`): rendered again from the current branch (`m10-parity` at `f1202c0`, release build in `.claude\targets\t1`) with `flare.exe --scene <scene> --scale <s> --size 1000x640 --renderer software --screenshot ...`, so the pairs show Flare as it is now, not as M5 left it (the M5 images in `docs/screenshots/m5/` predate the command-help button and the M7 and M10 chrome fixes).
- `side/<scene>-<pct>.png`: each pair, Prism Terminal left, Flare right (`tools/m5-compare/compose.ps1 -Flare docs\screenshots\m10\m5-pt\flare`).
- `sheet-100.png`, `sheet-150.png`, `sheet-225.png`: contact sheets of the side-by-sides.

`tools/m5-compare/check.ps1 -Flare docs\screenshots\m10\m5-pt\flare -Since <capture start>` passed: 36 pairs, none blank, every size `round(1000 * s)` by `round(640 * s)` on both sides, no real profile written. Its "Differ %" column (share of the page body that differs by more than 12 percent in luminance at height 200) is 0.1 to 1.1 for every pair. The folder is 11 MB.

## The image review (Task 59, 2026-10-19)

Read by Opus from the three contact sheets, the twelve 150 percent side-by-sides, and full-resolution crops of the 225 percent pairs (find bar and matches, menus, links, the Copied pill, the title bar ends). Colours were sampled and the menus measured in device pixels on both sides (a column scan through the panel at 100, 150 and 225).

| Scene | 100 / 150 / 225 | Verdict | Note |
|---|---|---|---|
| `term-selection` | ok / ok / ok | differs: selection fill | the band's cells, the full-width middle row and the last cell match; the fill is 78 grey in Flare, 62 in Prism Terminal (defect 1) |
| `term-selection-wide` | ok / ok / ok | differs: selection fill | the block covers the same 7 cells by 3 rows and both cells of the emoji; fill as defect 1. Flare's scene has no prompt below the rows (scene difference S) |
| `term-find` | differs / differs / differs | defects 1, 2 | the other matches (fill `#4a5160`, outline `#6b7482`), the wrapped match on both rows, the readout `2 of 5` and the bar match; the active match is a solid accent fill in Flare, an accent outline round the selection grey in Prism Terminal (defect 1); the cursor is hollow in Prism Terminal while the find field holds the focus (defect 2) |
| `term-find-empty` | differs / differs / differs | defect 2 | placeholder, dimmed buttons, empty readout match |
| `term-find-none` | differs / differs / differs | defect 2 | readout `none`, dimmed arrows match |
| `term-find-many` | differs / differs / differs | defects 1, 2 | readout `1000+` on both; no fills past the counted thousand on both; active match as `term-find`. Prism Terminal's shell adds a prompt row below the rows (scene difference S) |
| `term-links` | ok / ok / ok | matches | link ink, underlines and the url wrapped over two rows match; Prism Terminal's cursor is in its blink-off phase in this frame |
| `term-menu-selection` | differs / differs / differs | defects 3, 4 | rows, glyphs, hints and the order match |
| `term-menu-link` | differs / differs / differs | defects 3, 4 | as above |
| `term-menu-path` | differs / differs / differs | defects 3, 4 | as above |
| `term-menu-plain` | differs / differs / differs | defects 3, 4 | as above; Prism Terminal draws the row above its prompt dim in this frame only (not in any other scene; a capture artefact, not judged) |
| `term-copied` | ok / ok / ok | matches | the pill is 68 css px wide on the Prism Terminal side and 69 on Flare's (measured at 225), the same height and distance from the bottom, round, one hairline border, semibold; centred on each side's own window (difference A) |

### Defects (each needs a failing pixel test first, then the fix, in the defect pass)

1. **The selection fill is lighter than Prism Terminal's, and the active find match is a solid accent block.** Sampled at 100, 150 and 225 percent: Prism Terminal's selection is (62, 62, 62), Flare's (78, 78, 78), for the same `selectionBackground` `#bbbbbb55`. Cause, read in Prism Terminal's code and in the xterm bundle the installed app ships: when the panel paints the ground itself, `core/renderer/lib/termXterm.ts` hands xterm `background: '#00000000'`, and xterm's `_setTheme` makes `selectionBackgroundOpaque = blend(background, selection)`, so the selection is blended over transparent BLACK, not over the theme ground: 187 x 0x55 / 255 = 62. Flare blends over the ground (23 + 164 x 0.333 = 78). The same rule makes the active find match look different: xterm's search selects the active match, the selection (62) is painted over the accent decoration, and only the accent border shows round it (`activeMatchBorder`). Flare fills the active match with the accent. Fix: the selection's painted colour is `rgb(selection) x alpha` over black, as xterm's opaque blend computes it (only where Prism Terminal's panel paints the ground, which is every solid window; check the see-through case against `termXterm.ts` `clearGround`), and the active match draws the selection fill with a one-hair accent outline. On light themes this rule gives a dark band; confirm on one light preset in Prism Terminal before the fix lands (`pt-term.mjs` has no theme flag yet: add one, or capture one scene by hand in the scratch run). Tests: `selection_fill_is_the_selection_over_black_as_xterm_blends_it` (pixel, all eight scales, `term-selection`), `the_active_match_is_the_selection_with_an_accent_outline` (pixel, `term-find`). If the owner prefers the lighter band, this becomes a PROPOSED difference instead (it is Prism Terminal's look, not a rendering limit).
2. **The terminal cursor stays a solid block while another control holds the focus.** Prism Terminal's xterm draws the cursor as a one-pixel outline when the terminal is not focused (`cursorInactiveStyle`, default `outline`; Prism Terminal does not set it): every find scene shows the hollow box. Flare has no unfocused cursor at all (no cursor-focus state in `flare-render` or `term_ui`). Fix: an outline cursor in the cursor colour whenever the terminal's text input is not focused (find field focused, window inactive, a dialog over the tab). Test: `the_cursor_is_an_outline_while_find_has_the_focus` (pixel, all eight scales, `term-find-empty`), plus a unit test that the window losing focus draws the outline.
3. **Every context menu row after the first is 1 css px too tall.** Measured at 100 percent: Prism Terminal's rows repeat every 28 px (`h-[28px] border-t ... first:border-t-0` in `src/renderer/src/components/ContextMenu.tsx`: the hairline is inside the 28), so five rows are 142 px tall with the panel border; Flare's repeat every 29 (a 28 px row plus a separate divider, `menus.slint` line 233), 146 px. At 150 and 225 the same, by the hairline. The M5 test `ui_tests::menus::every_row_is_28_tall_including_the_last_and_the_border_closes_it` pins the wrong rule (28 plus a hairline): it is rewritten to Prism Terminal's (the first row 28, every later row a hairline plus 28 minus a hairline), which makes it fail on today's build, then the fix (the divider inside the row's 28) makes it pass. The terminal menu, the tab menu and the flyout share the panel, so all are covered.
4. **The context menu is about 19 css px wider than Prism Terminal's.** Measured at 100 percent on `term-menu-link`: Prism Terminal's panel is 198 px, Flare's 217; the labels (about 91 px for "Find in scrollback"), the hints (about 30 px for "Ctrl+F") and the paddings (11 px) agree to a pixel, but the gap from the longest label to its hint is 32 px in Prism Terminal (`gap-8`) and 51 in Flare: the row's 32 px stretch (`menus.slint` line 74) sits between two 8 px layout spacings, so the least gap is 48, and the hint's `letter-spacing: 0.4px` is wider than `tracking-wide` (0.025em of 10.5 px = 0.26 px). Fix: the gap is 32 in all, the hint's letter spacing 0.26 px. Test: `the_gap_from_the_longest_label_to_its_hint_is_32` (pixel, all eight scales) and `the_terminal_link_menu_is_198_wide` (within 1 css px, the label widths of both fonts agreeing as measured here).

### Rendering differences (kept)

- Cell size: Flare's whole-pixel cells are 8 by 16 at 100 percent, Prism Terminal's 7.625 by 15 (Q3, Task 56). Every row and column is a little wider and taller on Flare's side, so menus and the find bar open over slightly different text, and the url in `term-links` wraps at the same column but sits a few px further right.
- Text: Chromium draws ClearType colour fringes and its own hinting, Slint greyscale (DIFF-4). Glyph edges differ by up to 1 css px.
- Hairlines are `Px.hair()`: 2 device px at 225 percent where Chromium draws 3 (DIFF-4).
- Prism Terminal's cursor and the find field's caret blink; Flare's scenes are frozen with both shown. In `term-links` and the menu scenes Prism Terminal's frame caught the cursor in its off phase.
- Version drift: the installed Prism Terminal is 0.58.0. Nothing in these twelve scenes differs between 0.55.6's source and what 0.58.0 drew (the menu rows, the find decorations and `termXterm.ts`'s ground rule were checked against the 0.55.6 source and the 0.58.0 bundle).

### Capture differences (not Flare's)

- **A.** The Prism Terminal window ends up 8 to 27 css px wider than 1000 at 150 and 225 percent (`pt-term.mjs` asks for the content size four times and goes on when it never lands exactly, as `pt-settings.mjs` does in M7), and the screenshot is clipped to 1000 by 640. Everything anchored to the right edge or centred sits further right on the Prism Terminal side: the window buttons (the close button is cut off in some pairs), the find bar, the Copied pill.
- **S.** `term-selection-wide` and `term-find-many` end without a prompt row in Flare's scene (its text has none); Prism Terminal's shell prints its own prompt after the text, so its side has one more row.

## Regenerate

```
node tools/pt-shot/pt-term.mjs --out docs/screenshots/m10/m5-pt --scales 1,1.5,2.25 --size 1000x640
flare.exe --scene <scene> --scale <s> --size 1000x640 --renderer software --screenshot docs/screenshots/m10/m5-pt/flare/<scene>-<pct>.png
pwsh -File tools/m5-compare/compose.ps1 -Flare docs\screenshots\m10\m5-pt\flare
pwsh -File tools/m5-compare/check.ps1 -Flare docs\screenshots\m10\m5-pt\flare -Since <capture start>
```
