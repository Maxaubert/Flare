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

## My calls (Max can change these)

- Pinned folders stay as today, listed after "Choose a folder…" and before the recents (pinning is not retired).
- The same menu (with "Choose a folder…" and the defaults) is what the strip's right-click opens.
- The arrow has its own tooltip ("Open in a folder") and accessible label; Ctrl+T is unchanged.

## Plan

One PR on `feat/new-tab-arrow`: `new_tab.rs` (drop Ask, keep reading "ask" as folder), settings page row and its tests,
`menus_ui.rs` plus menu (choose row, default fill), `sidebar.slint` split button with the arrow anchoring the menu,
the strip unchanged, appctl wiring, ui tests, docs (features, decisions, regression rules if they name the ask mode).
Built, reviewed and fixed by a workflow, checked live, then the merge question. No install until Max says so.
