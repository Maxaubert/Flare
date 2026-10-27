# New tab: a folder arrow on the sidebar button, and no "ask" mode: spec and plan

Date: 2026-10-27. Owner: Max. Mockup: `docs/surfaces/new-tab-arrow-mock.html` (Max picked N1).

## Owner decisions (2026-10-27)

1. **Sidebar New tab button is a split button (N1)**: the main part opens a tab in the default folder; an arrow on the right,
   behind a full-height divider, opens the folder menu. Right-click on the sidebar button no longer opens the menu.
2. **The horizontal strip's + keeps its look and its right-click menu.** Two small buttons side by side there would make
   New tab hard to hit.
3. **Settings drops "Ask where each time".** New tab only has a default folder (user folder, or one you choose). A stored
   `flare.newtab.mode = "ask"` now reads as folder mode.
4. **The folder menu**: the top row is always "Choose a folder…" (the folder picker, then a tab there, and the folder joins
   the recents). Below it the recent folders. While there are fewer real recents than the menu shows, the empty spots are
   filled with Documents, Downloads, Desktop and the user folder (those that exist, never twice); real recents push them out.
5. **The menu drops from the arrow like a normal dropdown** (Max, 2026-10-27: "the menu should go from the arrow like a
   drop down menu normally does"). Its right edge meets the button's right edge (the arrow's) and it opens toward the
   left and upward, 4 px clear of the button; only when there is no room above (a very short window) it hangs 4 px under
   the button instead, and the window's edges hold it 8 px inside (it may run past a narrow sidebar over the terminal).
   Its width fits its rows (at least 156 px, at most the window less 16 px). While it is open the arrow stays lit and
   its chevron points up. `ContextMenu.drop` with `at-foot`; held by
   `a_dropdown_hangs_from_its_buttons_right_edge_and_opens_upward_4_px_clear`,
   `a_dropdown_drops_below_when_there_is_no_room_above_and_stays_in_the_window` and
   `the_arrow_stays_pressed_and_points_up_while_its_menu_is_open`. The strip's + right-click menu is unchanged.

## My calls (Max can change these)

- Pinned folders stay as today, listed after "Choose a folder…" and before the recents (pinning is not retired).
- The same menu (with "Choose a folder…" and the defaults) is what the strip's right-click opens.
- The arrow has its own tooltip ("Open in a folder") and accessible label; Ctrl+T is unchanged.

## Plan

One PR on `feat/new-tab-arrow`: `new_tab.rs` (drop Ask, keep reading "ask" as folder), settings page row and its tests,
`menus_ui.rs` plus menu (choose row, default fill), `sidebar.slint` split button with the arrow anchoring the menu,
the strip unchanged, appctl wiring, ui tests, docs (features, decisions, regression rules if they name the ask mode).
Built, reviewed and fixed by a workflow, checked live, then the merge question. No install until Max says so.
