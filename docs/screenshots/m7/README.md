# M7 screenshots: the Settings tab beside Prism Terminal (Task 52)

Made on 2026-10-18 from the release build of `flare.exe` (target folder `.claude\targets\t1`) and from a scratch copy of the installed Prism Terminal (run offscreen, unfocusable, with its own temp profile; the real `%APPDATA%` profile folders were checked untouched by the capture script). The installed copy reports 0.58.0 (`pt/VERSION.txt`); the plan's numbers are 0.55.6's, so a few rows differ by version (see the review).

The Flare images were rendered again on 2026-10-18 by Task 53 after its fixes (target folder `.claude\targets\t15`); the Prism Terminal images are the Task 52 ones.

The matrix is 52 images, listed by `node tools/pt-shot/pt-settings.mjs --list`:

- `flare/` and `pt/`: one PNG per image, named `<page>-<variant>-<pct>.png`. Each side is 1000 by 640 css px at its scale (for example 2250 by 1440 at 225 percent).
- `side/`: each pair side by side, Prism Terminal left, Flare right.
- `sheet-100.png`, `sheet-150.png`, `sheet-225.png`: contact sheets of the side-by-sides at each main scaling.

The variants are `plain` (the six pages), `picker`, `editor`, `ask`, `help`, `wall-expanded` (Appearance), `select` and `help` (Terminal), and `find` (Appearance, the text "colour"). Appearance and Terminal are also `plain` at 125, 175, 200, 250 and 300 percent.

`check.ps1` passed: 52 images, none blank, every size equal to `round(1000 * s)` by `round(640 * s)` on both sides.

The folder is 37 MB, under the 60 MB limit, so every image is kept.

Regenerate with `tools/settings-matrix/run.ps1`, `pt-settings.mjs`, `compose.ps1` and `check.ps1` (see `tools/settings-matrix/README.md`).

## The image review (Task 53, 2026-10-18)

Read by Opus from the contact sheets, the full-size side-by-sides at 225 percent, the eight-scale pairs of Appearance and Terminal, and device-pixel crops and samples of both sides. Both painters were checked: Appearance and Terminal at all eight scalings and the picker and the editor at 100, 150, 225 and 300 were rendered with `--renderer gpu` as well, and every one is identical to the software image byte for byte (the Settings tab draws no terminal grid, so the painters cannot differ there).

| Page | 100 / 150 / 225 | Verdict | Note |
|---|---|---|---|
| Appearance | ok / ok / ok | matches after fixes | rail weight (defect 1), heading inset (defect 3); 125, 175, 200, 250, 300 also ok |
| Terminal | ok / ok / ok | matches after fixes | select trigger weight (defect 2), chevron size (defect 8); 125, 175, 200, 250, 300 also ok |
| Agents | ok / ok / ok | matches | Prism Terminal 0.58 shows an Orchestrator (beta) section, Flare a Mark colours section: version drift |
| Dictation | differs / differs / differs | by design | Flare shows the placeholder sentence until the dictation milestone |
| Diagnostics | ok / ok / ok | matches | the log folder is each scene's own fixed path |
| About | ok / ok / ok | matches | Flare's own icon and copyright lines and the Slint badge are by design |
| Appearance, picker | ok / ok / ok | matches after fixes | defects 4 to 7 |
| Appearance, editor | ok / ok / ok | matches | no backdrop blur behind the scrim (D9) |
| Appearance, ask | ok / ok / ok | matches | no backdrop blur behind the scrim (D9) |
| Appearance, help | ok / ok / ok | matches | the panel's left edge follows the capture's wider window (difference A) |
| Appearance, wall expanded | ok / ok / ok | matches | |
| Terminal, select | ok / ok / ok | matches after fixes | defect 9; the shell list is each side's own machine or fixed list |
| Terminal, help | ok / ok / ok | matches | as Appearance, help |
| Appearance, find | ok / ok / ok | matches | the field is not focused in the scene (difference C) |

### Defects (each fixed with a failing pixel test first, then the images rendered again)

1. **The rail's unchosen labels were drawn Regular.** PT sets them `font-medium` (500) and Segoe UI has no Medium face; Chromium draws 500 with the Semibold one, Slint with the Regular one. Fix: 600 for every rail label (`frame.slint`). Test: `settings_frame::every_rail_label_is_as_heavy_as_the_chosen_one` (all eight scales).
2. **The select trigger's word was drawn Regular** for the same reason (`font-medium`). Fix: 600 (`select.slint`). Test: `settings_select::the_trigger_word_is_semibold_and_the_menu_rows_regular`.
3. **The section heading sat on the panel's edge.** PT's heading row is `mx-0.5` (measured x 290 against the panel's 288). Fix: 2 px in from each edge, the heading's Save too (`view.slint`). Test: `settings_view::the_section_heading_starts_2_px_inside_the_panel_edge`.
4. **The colour square showed the accent focus ring when opened by a click.** PT's ring is `focus-visible`: a click opens with no ring, Enter or Space with it. Fix: the `PickerOpening` global records how the swatch was opened and the popover reads it on open; the scene opens as a click does (`colour.slint`, `headless.rs`). Test: `settings_colour::a_pointer_open_shows_no_focus_ring_and_a_keyboard_open_does`.
5. **The square had sharp corners and the hue bar a square right end.** Slint's software painter draws a gradient square whatever the radius or a rounded clip. Fix: a `RoundMask` of the popover's fill rounds the square (radius-sm) and both bars (a half circle), on both painters. Test: `settings_colour::the_square_has_round_corners_and_the_bars_round_ends`.
6. **The hue and alpha thumbs were filled with the ground.** PT's thumb is a ring (2 px of the ink, 1.5 px of the ground outside) with the bar showing through. Fix: the outer ring is a border, not a fill. Test: `settings_colour::the_hue_thumb_is_a_ring_the_bar_shows_through`.
7. **Dark specks in the hue bar at 225 percent**, where a stop of the seven-stop gradient falls between device pixels. Fix: the spectrum is six two-stop pieces on whole device pixels. Test: `settings_colour::the_hue_bar_has_no_dark_specks` (all eight scales).
8. **Every stroked icon of the Settings tab was drawn about 10 percent small** (the select chevron 6 css px wide against Chromium's 7.1). Slint fits a stroked Path into its box minus the stroke width. Fix: each 24-unit icon Path of `ui/settings/` grows its box by its stroke and moves back by half of it (13 icons: rail, find, clear, row tiles, chevron, menu check, eyedropper, pencil, Show all arrow, copy, check, close, the help field's magnifier). Test: `settings_select::the_chevron_is_drawn_at_its_full_12_px`. The chrome outside Settings (tabs, sidebar, menus, start screen) has the same inset and is left for a later milestone.
9. **The select's chevron did not turn over while the menu was open.** The software painter ignores `transform-rotation` on a Path. Fix: the open chevron is drawn pointing up (the wall's Show all arrow already did this); the 150 ms turn is the one thing lost. Test: `settings_select::the_chevron_turns_over_while_the_menu_is_open`.

### Rendering differences (kept)

- Chromium draws text with ClearType colour fringes and its own hinting; Slint greyscale. Glyph edges and widths differ by up to 1 css px, and Cascadia Mono's semibold (the format button, the card previews) reads lighter.
- No backdrop blur behind the editor's and the question's scrim, the select menu and the help panel (D9): the page behind reads crisper than Prism Terminal's.
- Hairlines are `Px.hair()`: 2 device px at 225 percent where Chromium draws 3, by the rule of the milestone.
- The select menu opens with the chosen row highlighted: Flare's menu is keyboard driven (Up, Down, Home, End, Enter, Space), Prism Terminal's has no keyboard highlight.

### Capture differences (not Flare's)

- **A.** At 125, 150, 175 and 225 percent the Prism Terminal window is a few css px wider than 1000 (`pt-settings.mjs` asks for the content size four times and goes on when it never lands exactly), and the screenshot is clipped to 1000 by 640. Everything anchored to the right edge or centred sits 4 to 20 css px further right on the Prism Terminal side (the window buttons, the panel's right edge, the editor, the question, the help panel). The panel measured at 100 and 300 percent, and in `pt_settings_metrics.json`, is 668 wide like Flare's.
- **B.** The Agents page of Prism Terminal 0.58 has sections 0.55.6 did not (version drift).
- **C.** The Find image types into Prism Terminal's field, so it is focused (the lighter edge); the Flare scene sets the text without focusing it. A focused field draws the same edge in Flare (`SettingsInk.focus-edge`).
