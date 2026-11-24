# Plan: panel launcher and "Open in"

Spec: `docs/superpowers/specs/2026-11-24-panel-launcher-design.md`. One PR (`feat/panel-launcher`). Tests first for the
pure parts; then the window; then the rules and docs. Worktree, headless tests only, no install.

## 1. Pure rules (`crates/flare/src/launcher.rs`), tests first
1. `Hover`: the menu's state machine with the clock passed in: `button(on, now)`, `menu(on, now)` (ignored while
   closed), `click()`, `close()`, `tick(now)`, `due()`, `keyed()`. `GRACE_MS = 250`.
2. `shown(Where)`: shell tab in front, panel closed, slot fits, no remote view, no skeleton.
3. `items(Offer)`, `tops`, `menu_h`, `row_at`, `step`: the rows and where the pointer and the keys land.
4. `last_ok(PanelSnap)`: a page (not `about:blank`), a device or a file, closed.
5. `open_in(PanelSnap)` and `open_in_label`: http or https only (`is_web_url`); a file opens when `may_open`, else
   it is revealed; `clean_app_name`, `ext_key`.

## 2. Platform (`flare-win`)
1. `pick::pick_any_file` (no filter), sharing `pick_one` with `pick_file`; seam test.
2. `openers::default_app_name(ext)` (`AssocQueryStringW`, Windows; None on a Mac); `plain_ext` test and a real
   `.txt` lookup on Windows.

## 3. Window
1. `ui/launcher.slint`: `PanelLauncher` (button, menu, one touch area over the menu and its 6 px bridge, rows laid
   out at the controller's `top`s), `LauncherRow`.
2. `ui/app.slint`: the `launcher-*` properties and callbacks, the launcher in `grid-box`, the Preview chip moved
   left while it shows, `panel-open-in*`.
3. `ui/panel.slint`: `OpenInButton`; in the header (leftmost of the right group, counted in `head-extra`) and in the
   web bar (left of Draw arrow and the close, narrow rules).

## 4. Controller (`appctl/launcher_glue.rs`)
1. `sync_launcher` from `refresh` after `sync_panels`: shown, rows (pushed only when they change), open, lit row,
   "Open in" words (app name from a per-extension cache filled by a worker, `UiMsg::Launcher`).
2. Pointer: `launcher_hover`, `launcher_clicked`, `launcher_menu_moved`, `launcher_menu_clicked`; the grace on the
   timer registry (`panel.launcher-grace`), re-armed when it fires early.
3. Keys: `launcher_key` in `on_key` after the context menu's Escape.
4. Rows: `browser_open` + `focus_address`, `device_open`, `set_pick_file` (window.rs: `pick_any_file`, seam
   `FLARE_E2E_PICK_FILE`) then `previewable` on a worker and `TermMsg::Preview { asked: true }`, `panels.reopen`.
5. `panel_open_in`: `open_url`, or `open_term_path` (Open or Reveal) on a worker.

## 5. Tests through the window (`appctl/tests/launcher_tests.rs`)
Shown and hidden; the corner at 100, 150 and 225 percent; the hover rules with a real pointer; the keys; Browser;
Open a file (picture, refused zip, cancel); Last panel; Open in for a file and a page.

## 6. Docs and gates
Rules 1372 to 1376 in `docs/regression-rules.md`, `docs/features.md`, README's panel line, the e2e seam in
`tests/names.rs`. Gates: `cargo fmt`, `cargo clippy -D warnings` for `flare` and `flare-win`, `cargo test -p flare --bin
flare`, `cargo test -p flare-win`.
