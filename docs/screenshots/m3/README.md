# M3 screenshots and their review (Task 33)

Rendered with `target\release\flare.exe --scene <name> --scale <s> --size 1000x640 --renderer software
--screenshot docs\screenshots\m3\<name>-<pct>.png` (12 scenes at 100, 150 and 225 percent; `strip-3`,
`sidebar-open` and `start` also at 125, 175, 200, 250 and 300 percent as `-s125`, `-s175`, `-s20`, `-s25`,
`-s30`; `strip-3` and `sidebar-open` at 225 on the GPU renderer as `-225-gpu`).

Reviewed 2026-10-17 by reading every PNG, with 4x to 8x crops (PowerShell `System.Drawing`, nearest
neighbour) and pixel samples of the strip's top-left 300 x 40, the dividers, the menu border and the
sidebar corner at 150 and 225. Compared against Prism Terminal v0.55.6's components (`EmptyState.tsx`,
`TabStrip.tsx`, `ContextMenu.tsx`, `tabMenus.tsx`, `TabSidebar.tsx`, `cornerShapes.ts`) and
`side-by-side-225.png`. Every defect got a failing pixel test first, then the fix, then a re-render.

## Verdicts

| Scene | Scales | Verdict | What was fixed |
|---|---|---|---|
| strip-3 | 100, 150, 225, 125-300, 225 GPU | Pass. Names centred and unclipped; dividers 1 device px at 100/125/150/175, 2 at 200/225/250, 3 at 300; the active rule crisp; + aligned; tabs touch with no gaps; GPU matches software. | - |
| strip-hover | 100, 150, 225 | Pass. The hovered tab's fill and its X; nothing else moves. | - |
| strip-many | 100, 150, 225 | Fixed. Icon tabs 40 wide with the divider; the +N chevron sat at the button's top-right corner, 12 px past the count. | The chevron is centred on the button's middle, 6 px after the count (no empty dot slot). Test `strip::the_more_buttons_chevron_sits_beside_the_count_on_its_middle`. |
| strip-list | 100, 150, 225 | Fixed (scene and strip). The list hangs under +N with its field and rows; the scene typed "api", which matched nothing, so it only showed the empty line. | The scene now types "2" (Tab20 to Tab29). The chevron as above. |
| sidebar-open | 100, 150, 225, 125-300, 225 GPU | Pass. Rows 46 px with a square 46 px icon band; folder lines dim and elided; the active row tinted with its X; the terminal's cut corner and its line crisp. The default width (220) is PT's; the owner's PT window in the side-by-side is widened. | - |
| sidebar-find | 100, 150, 225 | Pass. Accent ring on the field, one kept row. | - |
| sidebar-collapsed | 100, 150, 225 | Pass. Slid out over the terminal (which keeps its full width, so its left columns sit under the sidebar, as in PT); the corner piece and the line from the bar down, 1 device px at 100/150 and 2 at 225. | - |
| start | 100, 150, 225, 125-300 | Fixed. Each row was centred on its own (the glyphs did not line up), the glyph sat at the row's top, the New and Settings keys stopped mid-row, and the footer read "v0.1.0". | Rows run from the left as PT's: glyph 10 px in and centred on the row, name 12 px after it, keys and Pinned at the right padding. Footer "Version 0.1.0". Tests `start::every_row_starts_at_the_left_with_its_glyph_centred_on_the_row`, `flows::the_start_screen_footer_says_version_in_words`. |
| menu-places | 100, 150, 225 | Pass. At the pointer, as PT; folders in the accent, pins at the right, hairlines between rows. | Width rule as below (no visible change here). |
| menu-tab | 100, 150, 225 | Fixed. The labels were cut to "Clo..." and "Copy folder..." beside Ctrl+W: the panel was 156 wide whatever its rows. | The panel grows to its rows (156 minimum, as PT), the label is measured whole, and the panel's width and place are snapped so its border stays on whole device pixels. Test `menus::a_label_is_never_cut_for_its_key_hint`. |
| dialog-close | 100, 150, 225 | Pass. Box 420 wide, centred; "Claude" and "Prism" in the brighter ink; Cancel and the red Close tab with its focus ring, right-aligned. | - |
| tip | 100, 150, 225 | Pass. Centred on the hovered tab, 6 px below the strip. | - |
| side-by-side-225 | 225 | Reference. PT (left, the owner's window) and Flare (right): same title row height, find box, row cards and tint; PT's sidebar is wider by the owner's own drag. | - |
