# Surfaces decisions

Each entry: the date, the stage, the choice, the reason. Owner decisions are marked (owner).

- 2026-10-21, plan: build in Flare first, document for Prism Terminal, port after (owner).
- 2026-10-21, plan: the agent must be able to USE surfaces, not only show them, modelled on the iOS
  simulator MCP (describe, tap, type, screenshot) (owner).
- 2026-10-21, plan: video must play at full frame rate, so web content is a real WebView2 panel, not
  frames drawn into the grid (owner: "it can't just be a frame here and there").
- 2026-10-21, plan: one panel per tab in v1; more later.

## Stage 1: panels (`surfaces-1-panels`, issues #15 and #16)

- 2026-10-21, stage 1, chords: **Ctrl+Shift+P** shows or hides the front tab's panel, **Ctrl+Shift+O** moves the
  keys between the terminal and the panel. Why: a program cannot tell Ctrl+Shift+letter from Ctrl+letter in the
  legacy key encoding, so no shell or agent loses a key it could have used; plain Ctrl+P and Ctrl+O stay the
  shell's. P is "panel"; O is "the other side". Both are always the app's (rule 7: a host chord never reaches the
  pty), so with no panel to bring back, or none open, they do nothing and the shell does not hear them (a ^P would
  walk the shell's history). A chord with Alt too (AltGr on some layouts) is the shell's.
  Both live in the one chord table (`flare-tabs` `keymap.rs`, rule 7), and the table's own tests cover them.
- 2026-10-21, stage 1, focus: a panel never takes the keys by itself (a click on a link leaves them in the
  terminal). A click in the panel, or Ctrl+Shift+O, gives it the keys; Esc, or the chord again, gives them back.
  While the panel has the keys a key that is no chord goes to Slint (the panel scrolls with the arrows, Page
  keys, Home and End; Enter or Space zooms a picture), never to the shell. Closing the panel, or leaving its tab,
  while it has the keys returns them to the terminal at once. Why: typing into the wrong place is the worst
  thing a split view can do.
- 2026-10-21, stage 1, state file: **`panels.json` beside `tabs.json`**, not inside it. Why: `tabs.json`'s key
  order is a shape other code depends on, and a panel is not a tab property that must survive a downgrade. A tab
  has no id across a restart, so an entry carries its folder and is matched to a restored tab by folder, in
  order (`match_restored`); a tab dropped from the restore only skips its entry. The file has a `version`. It is
  written the way `tabs.json` is (temp file, rename, 400 ms debounce, synchronous flush at close; rules 36 and
  41). The writer thread is made on the first save, and a session that never opens a panel writes no file.
- 2026-10-21, stage 1, what is stored: kind, target, open, width (the share of the page). A closed panel keeps
  its kind and target so the toggle chord can bring it back; it holds no picture and no rows. Unknown kinds are
  dropped on load; a kind this build cannot draw yet (web, changes, tasks, board, mod) is kept in memory and in
  the file and is not shown. A width that is not a fraction reads as "not set" and the default applies, it is never
  written back as a number (rule 40). A stored target is re-checked like a click: a local drive path, no NUL, no
  network path (it would make Windows reach out), a name of the stored kind.
- 2026-10-21, stage 1, layout numbers (all in `flare-surfaces` `layout.rs`, handed to the window once):
  default share 40 percent of the page; the slot is at least 240 css px unless the page cannot give it so much;
  the terminal always keeps 320 css px; the slot is at most 70 percent; under 120 css px of room the slot is not
  shown (the panel stays open in the model, and the terminal keeps the whole page); the splitter's hit area is 8
  css px centred on the slot's left edge; the header is 32 css px. Why 320: about 40 columns, the least a prompt
  stays readable in. The `.slint` file evaluates the same formulas in logical pixels with `Px.snap`; a test pins
  the two to each other at all eight scales, five widths and four shares, and a probe reports both.
- 2026-10-21, stage 1, one fit (rule 24): while the splitter moves, the slot follows the pointer and the grid
  keeps its width (the slot is drawn over it, opaque); the share is stored on the release and the grid fits
  once. The held-width trick of the sidebar's slide is not reused: it freezes the whole page, and here only the
  slot moves.
- 2026-10-21, stage 1, previews: a plain left click on a path link asks a worker to resolve the path again (rule
  13), and the app shows it in the panel only when the file is on the click allowlist (`may_open`) as named AND as
  the file system names it, and both names are of the same previewable kind (`classify_resolved`). So a link
  called `a.png` that points at `run.exe` is revealed, not shown. Pictures: png, jpg, jpeg, gif, bmp, webp, ico.
  Text: txt, md, markdown, rst, log, json, jsonc, yaml, yml, toml, csv, tsv, ini, cfg, conf, xml, lock, diff, patch,
  rs, ts, tsx, c, h, cpp, hpp, cc, cs, go, java, kt, swift, css, scss, sql, slint, vue, svelte. Not svg (a vector
  file can carry script and is not in the picture list), not pdf, not any script (rule 14). Folders and
  everything else keep today's behaviour. The right-click menu's Open still hands the file to Windows: that is an
  explicit ask. The setting is **Open previews in Flare** (`flare.window.openPreviews`, default on) in a new
  **Links** section of Settings > Terminal; no page held link behaviour before.
- 2026-10-21, stage 1, pictures: decoded on a worker with the `image` crate (png, jpeg, gif, bmp, webp, ico; the
  rest of its formats are off). Limits: a 64 MB file, 100 megapixels, and a side over 8192 px is scaled down
  (a GPU texture limit). The decoded bytes go into a Slint pixel buffer on the worker, so the UI thread copies
  nothing. A GIF shows its first frame. Fitted: never enlarged, kept in shape, centred; a click shows one image
  pixel per device pixel with scrolling; a click again fits it. A small picture is shown as it is and a click does
  nothing. A picture with any transparent pixel sits on a checkerboard (a tiled 16 px tile, two neutral greys).
- 2026-10-21, stage 1, text: read on a worker, first 4 MB, 100 000 lines, 2000 characters a line (a longer line is cut
  with a mark); tabs are four spaces; bytes that are not UTF-8 show as replacement marks; a NUL in the first 8 KB
  means "This file is not text." and a missing, denied or unreadable file shows its reason in the panel, never a
  blank. Monospace (Cascadia Mono), a line-number gutter, only the rows in view are built (100 000 lines draw in
  milliseconds), no wrapping, a horizontal scroll, no selection and no copy yet (v1 is a viewer). A cut file says
  so in a line under the text.
- 2026-10-21, stage 1, one panel per tab; only the front tab's is loaded. A tab behind keeps its path and reads it
  again when it comes to the front (no cache: a file may have changed, and memory stays flat). A newer click
  replaces the panel; an answer for an older one is dropped by a generation number.
- 2026-10-21, stage 1, the overlay flag: one `out` property of the window, `overlay-open`, the OR of every overlay:
  menu, tab list, dialog, update window, update notice, the Settings page in front, Settings dropdown, theme
  editor, theme question, Dictation question, colour picker, Command help, find bar. `flare-surfaces`
  `overlay::Overlay::ALL` names them; a test opens each and checks the flag; the match in that test has no
  catch-all arm, so a new overlay cannot be added without being counted; another test reads the property's line
  from `app.slint` and counts its terms. Native panels ignore the flag (Slint orders them with the rest); stage 3's
  web panel hides while it is true. The find bar spans the terminal only, so it never overlaps the slot, but it
  counts (it is one of the overlays the task names). The Copied badge and tooltips are not counted: they are small
  and brief; stage 3 should check them against a child window.
