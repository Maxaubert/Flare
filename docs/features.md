# Features

What Flare does, decided by the owner, with the date and the test that holds each decision. Entries
are added as milestones land.

## Rendering (M1), 2026-10-17

- **GPU by default, software as the fallback.** Flare starts on Slint's `femtovg-wgpu` renderer
  and paints the terminal grid on the GPU through a shelf-packed glyph atlas. When the GPU cannot
  start (RDP, a VM, WARP only), Flare falls back to the CPU painter and logs
  `renderer: software (fallback: <reason>)`; the debug overlay shows the same reason.
- **The glyph caches are bounded** (audit 2026-10-20, M1, M2, L7). A cell keeps at most 16 combining marks
  (`MAX_MARKS`); the rasteriser's cluster cache starts over at 8192 entries; the glyph atlas starts at
  2048 px square and doubles to 4096 (or the device limit) when a frame does not fit, keeping what it
  holds instead of clearing and re-uploading every frame. A lazy fallback font is tried again for a
  character it was not asked about; a character no family covers is walked once. The walk (cmap reads, font file parsing) runs on a worker\n  thread: a paint never waits on it, the cell is blank until the family arrives, and the host then\n  repaints both painters from the same source (audit M2). Held by paint_never_blocks_on_a_family_load,\n  _family_that_arrives_repaints_the_blank_cell, _fallback_font_that_arrives_repaints_without_any_other_change,
  `marks_beyond_the_cap_are_dropped_in_put_str_and_cluster`, `a_flood_of_combining_marks_is_capped_per_cell`,
  `the_glyph_cache_is_bounded`, `a_char_no_family_covers_does_not_burn_the_families_for_another_script`,
  `an_overflowing_atlas_grows_once_and_stops_re_uploading`.
- **Powerline symbols are Flare's own shapes** (owner bug 2026-10-21). U+E0B0 to U+E0BF (arrows, thin chevrons, half circles, corner triangles) are drawn by `shapes.rs` to the exact cell at every scale, in the cell's foreground, by both painters through the same glyph path, so a status line's caps meet the bar with no seam and never show a tofu box or a symbol font's star. Other private-use characters come from the stack's own families only. Held by the shapes tests, `powerline_shapes_are_drawn_to_the_cell_at_every_scale`, `the_statusline_pill_paints_the_same_on_both_painters` and e2e `powerlineGlyphs`. Details and Prism Terminal's behaviour: `docs/parity.md` (Audit additions).
- **Debug overlay (F12).** Shows the renderer, the grid painter, frame statistics and memory.
- **Flags and variables.** `--renderer gpu|software` (or `FLARE_RENDERER`, which applies only when
  the flag is absent); `--screenshot <png>` renders headlessly to a PNG for visual checks;
  `--bench-frames <n>` reports frame time; `--probe-backend` reports which Slint renderer would start
  (gpu, or software with the fallback reason).
  `FLARE_FORCE_GPU_FAIL=1` makes the GPU start fail, to test the fallback.
- **Release build is a GUI app.** No console window appears in a release build (`windows_subsystem`
  is set outside debug builds). The headless modes and start-up errors attach to the parent console,
  so their output still shows when run from a terminal. The software path redraws on a 16 ms timer,
  and only when the grid is dirty.
- **Tests that hold it.** Config tests for the flags (`crates/flare/src/config.rs`), the
  `--probe-backend` and `FLARE_FORCE_GPU_FAIL` binary tests (`crates/flare/tests/headless.rs`, no
  window), host dirty-tracking tests (`crates/flare/src/host.rs`), and GPU versus CPU pixel parity
  within 2 per channel at 100% and 225% (`crates/flare-render/tests/gpu_parity.rs`).
- **Regression rules.** `docs/regression-rules.md` rules 130 to 132: the raised wgpu limits, the
  physical-size grid texture, and no redraw without a dirty grid.

## Terminal core (M2), 2026-10-17

- **O1, the cd line is kept.** A PowerShell or cmd tab whose folder changes gets a `Set-Location` or
  `cd /d` line typed at an idle prompt, as Prism Terminal does. Held by `cd_writes_the_line_for_powershell_and_nothing_for_wsl`,
  (`crates/flare-term/src/session.rs`), `cd_is_not_typing` (`handle.rs`) and `the_cd_line_moves_a_powershell`
  (`crates/flare-term/tests/real_shells.rs`).
  The idle-prompt guard itself lands in M6.
- **O2, physical keys.** Ctrl+letter keys read the physical key, so they work on a Russian or any
  other layout. Held by `ctrl_c_reads_the_physical_key` (`crates/flare-term/src/keys.rs`),
  `ctrl_letters_use_the_physical_key` (`crates/flare-term/src/key_encode.rs`) and `ctrl_c_on_a_russian_layout_keeps_the_physical_key`
  (`crates/flare/src/keymap.rs`).
- **O3, the prompt line survives a resize (resolved, 2026-10-17).** alacritty's reflow is
  bottom-anchored and ConPTY's is top-anchored, so the cursor line drifted. The test showed it:
  with a plain `Term::resize` against the real bundled ConPTY and a 73-character cmd prompt, the
  banner was pushed out (`history 1`, cursor (3, 13)), a stray copy of the typed text appeared
  (cursor (22, 82)), and the prompt sat on the wrong row (cursor (18, 2)). No cursor-line carry is
  needed: `resize_conpty` computes the row ConPTY keeps the cursor on and shifts the grid to
  match, a vertical re-anchor. Held by the `o3_*` tests (`crates/flare-term/tests/o3_resize.rs`) and
  the `resize.rs` unit tests. The RED output is in commit `ce79fbc`.
- **O21, no Git Bash.** Shells are PowerShell 7, Windows PowerShell, cmd and the WSL distros, as in
  Prism Terminal. Held by `probes_exactly_where_pwsh_and_wsl_list_and_never_git_bash` (`crates/flare-shells/src/detect.rs`).
- **Measured on this PC, 2026-10-17 (release build).**
  - Flood through the real bundled ConPTY (`a_100_mb_flood_keeps_snapshots_fast_and_arrives_complete`):
    100 MB in 1.12 s, 89.5 MB/s end to end, worst snapshot 0.6 ms over 209 samples; the last line
    and `SPEW-DONE` arrive complete.
  - Bench (`bench_throughput`, ignored, `cargo test --release -p flare-term --test flood -- --ignored --nocapture`):
    90.1 MB/s end to end, parse 0.34 s for 64 MB, worst snapshot 0.1 ms.
  - Parser alone (`the_parser_alone_keeps_up`): 64 MB in 0.37 s, about 170 MB/s.
  - Spawn to first output (`spawn_and_first_output_time_is_reported`, prints only): cold cmd spawn
    returned after 65 to 81 ms, first output 19 to 30 ms later. Warm: the spare's first output was
    already queued when the spawn returned (0 ms, 0 ms), so the warm figure is the floor, not a time.
  - Cold start to the first prompt row (headless `--live-screenshot`, cmd, release, three runs):
    348 to 580 ms from process start to a written PNG, with `flare-m2-ok` on screen twice.
  - The 15.9 MB/s ConPTY ceiling of the prototype (111-byte lines, 40x120, `scratchpad\proto-m2`) was
    not reproduced by this flood, which measured 89.5 MB/s through the same ConPTY; the prototype
    figure is kept for the record and the difference is open.
- **Tests that hold the rest.** The Prism Terminal ledger is in `docs/test-port.md` (section "M2
  ledger"); every M2 line of `docs/parity.md` names its test or the milestone that holds it.
- **Regression rules.** `docs/regression-rules.md` rule 22 is amended (resolved in M2), and rules 137
  to 141 hold the ConPTY start-up, environment, query and synchronized-update lessons; rules 133
  to 136 hold the security fixes (cd lines, UNC reports, the click-to-open allowlist, inert paste,
  the PowerShell 7 lookup).

## Tabs, restore, single instance (M3), 2026-10-17

- **O12, Settings is a tab, as in Prism Terminal.** One reused tab with no shell, never saved;
  the cog, Ctrl+, and the start screen row toggle it. Held by `settings_opens_once_and_is_reused`
  (`flare-tabs`), `the_cog_toggles_and_goes_back_to_the_tab_used_before` and `the_cog_and_ctrl_comma_toggle_settings`.
  Its page body is M7 (decision D11).
- **O20, no rename.** Prism Terminal has no rename (a tab is always its folder name), so Flare has
  none. The roadmap's M3 "rename" line is answered by this, not built.
- **Ctrl+W closes a tab and reads "Ctrl+W" on its label.** Both Ctrl+W and Ctrl+Shift+W close it
  (owner question Q3 in `docs/parity.md`, assumed). Held by
  `ctrl_w_and_ctrl_shift_w_both_close_ctrl_t_and_ctrl_shift_t_both_open` and
  `every_label_is_the_key_that_chord_answers_to`.
- **Tabs in one row, one width.** Every tab is the same width (150 px, floor 84 px, shrinking
  equally); when the strip is short it becomes icons (40 px), then the least-needed tabs move to the
  `+N` list. The front tab is never hidden. Held by `strip_plan` and `strip_layout` tests and the
  `sixteen_tabs_icons_plus_n_and_the_list` flow.
- **Drag to reorder with the pointer.** A press must travel 4 px before it is a drag; the carried
  tab is clamped to its row, neighbours slide, and a drop slot follows the carried tab's leading
  edge so the first place is always reachable. Held by `the_slop_is_four_pixels_exactly`,
  `the_carried_tab_is_clamped_to_its_row` and `reaches_first_place_when_the_carried_tab_is_wider`.
- **Most recent switching.** `In order` (default) or `Most recent`; the list reorders only when the
  Ctrl hold ends, and any other activation counts at once. Held by the `tab_mru` tests and
  `no_ping_pong_the_order_changes_when_the_hold_ends`.
- **Restore.** `tabs.json` is Flare's own file, written 400 ms after a change, atomically, and
  flushed before the shells are killed. Nothing is written until the restore has answered. A gone
  folder is dropped silently; a Claude tab resumes its newest session (two tabs in one folder take
  the newest and the second newest); a tab without an agent never resumes. The restore draws the
  saved strip as placeholders at once and settles them in place. Held by the `restore` and
  `tabs_store` tests and `nothing_is_written_before_the_restore_answered`.
  The transcript reader and the 16-at-a-time lookup are M6 (decision D10).
- **Single instance per profile.** The lock and the hand-off pipe are named by a hash of the
  profile folder, so an e2e profile never reaches the owner's Flare. A second launch hands its
  folders over and exits 0; the first instance raises its window. Held by
  `a_second_launch_hands_its_folder_over_and_exits_zero`, `names_differ_per_profile...` and
  `the_lock_names_carry_no_prism_name`. The hand-off is verified by the security rule 142.
- **Command-line folders.** Every path is taken once, in order, at most 32; switches are skipped;
  a file means its folder; a missing path is ignored; relative paths resolve against the folder the
  command was typed in. Held by the `argv` tests.
- **Window state.** `window.json` keeps the size in logical pixels, the position in physical pixels
  and the scale it was saved at. Minimum 560 x 400, default 1180 x 780. A position off every
  monitor is dropped; a negative position on a second monitor is kept. Held by the `window_state`
  tests and `the_overlap_check_uses_device_pixels`.
  A size is capped at a quarter over the virtual screen (all monitors) at the saved scale, never below the
  minimum (audit 2026-10-20, L16). Held by `window_state_clamps_the_size_to_the_virtual_screen_at_every_scale`.
- **Resize edges (audit 2026-10-21).** The frameless main window resizes from its edges and corners
  (native cursors, Aero Snap by drag and Win+arrow intact; no snap layouts hover menu yet): `flare_win::resize_edges` answers `WM_NCHITTEST` in a DPI-aware band
  (Prism Terminal's sizes: `round(5 * dpi / 96)` px edges, `round(16 * dpi / 96)` px corners, inside the window), never while maximized, minimized or full
  screen (the minimum clamp skips a borderless full-screen position too), and holds a programmatic resize to the 560 x 400 minimum (`WM_WINDOWPOSCHANGING`). The dictation
  widget and popups are not resizable. Sizes at 100 to 300 percent are held by the unit tests (literals: edge 5, 8, 11, 15; corner 16, 24, 36, 48); the e2e `resizeEdges` reads the band at the host monitor's dpi and probes the Close button's last pixel. Chromium floors the pointer to DIP, so PT's band can be 1 px wider at fractional scales (parity.md).
  Held by the `resize_edges` tests and the e2e `resizeEdges`.
- **Diagnostics and hand-off hardening (audit 2026-10-20).** A panic line keeps the payload's length and hash, never its
  text (L12). The lock mutex has an explicit one-ACE DACL and a medium label, so an elevated first instance
  still shows its lock to a normal launch (L11). A hand-off server that is not this user in this session
  refuses the launch instead of starting a second instance on the same profile (L41). The taskbar badge is
  set up once the window has a handle and drawn again whenever the window gains focus (L14; the
  `TaskbarButtonCreated` subclass is gone: beside AccessKit's it crashed the window, 2026-10-26). The stack sampler's abandon wait is bounded (L15).
- **Close.** One rule, not a setting: a plain shell closes unasked; a tab with an agent asks; the
  window close is held only while an agent is working. Held by the `close` tests (flare-agent) and
  `close_flow` (flare-tabs). The shells are killed and waited for, with a 3 s cap.
- **Vertical tabs.** A sidebar with folders and a find box, open or collapsed; a collapsed sidebar
  slides over the terminal from the left edge (24 px zone, 60 ms open, 100 ms grace), and the
  terminal keeps its size during the 240 ms slide and fits once at the end (decision D9). Held by the
  `reveal` and `sidebar_state` tests and `the_slide_holds_the_terminals_size_and_fits_once`.
- **Folder drops.** A folder dropped on the tab row or the sidebar opens a new tab in it; a file
  opens the folder that holds it (decision D12).
- **Decisions D1 to D14** are the planner's and are listed in `docs/parity.md` (Decisions); the
  owner may overrule any of them.
- **Measured on this PC, 2026-10-17 (release build, RTX 5090, Windows 11).**
  - `--scene strip-3`, process start to written PNG (wall clock, three runs each): GPU 606, 660 and
    915 ms (median 660 ms); software 252, 254 and 256 ms.
  - Cold start to a live `cmd` prompt (`--live-screenshot` with `--live-shell cmd`, three runs):
    720, 731 and 747 ms. M2 measured 348 to 580 ms on the same kind of run; this is higher and
    the cause is not yet known (a regression to check before M10).
  - Idle windowed app for 10 s with the sidebar collapsed (vertical tabs, zero tabs, `--e2e`):
    0 ms of process CPU time, the process alive with 47 threads. The 50 ms timer is stopped when
    nothing is pending.
- **Tests that hold it.** The whole workspace: 1107 run, 1107 passed, 2 ignored (M2 ended at 589).
  The Prism Terminal ledger is in `docs/test-port.md` ("M3 ledger"); every M3 line of
  `docs/parity.md` names its tests or the milestone that holds it. Ten lines stay open with a
  reason: the focus hand-back after a drag, the Failed tooltip line, the agent icons, the shared tab
  look, the sidebar ground (M4), the Settings rows (M7), the greeting check, the transcript reader
  and its lookup limit (M6), and the once-per-window guard.
- **Regression rules.** `docs/regression-rules.md` rules 143 to 147 (the profile-keyed lock, the
  whole device pixel edges, the slide holding the terminal's size, the e2e variable gate, and
  `tabs.json` written only after the restore answers); rules 48 and 125 are amended with their tests.
  The plan's numbers 138 to 142 were taken by M2, so these moved to 143 to 147.

## Themes, fonts, material (M4), 2026-10-17

- **Flare wears Prism Terminal's look.** All 39 themes with their exact values (generated from
  Prism Terminal's own code by `tools/oracle`, so no value is typed by hand), the Custom theme, and
  the 31 window chrome tokens derived from the terminal theme. Every ink is measured against the
  ground it sits on: light or dark comes from the measured ground, never from a theme name. Text
  4.5:1, marks 3:1, sidebar tab names 7:1, name flip under 2:1.
- **PT Default is the default and leads the wall after Custom.** A fresh profile wears PT Default
  (owner brief, 2026-10-17). The wall is Custom (when saved), PT Default, then the rest in measured
  order. Held by `the_wall_is_custom_then_the_default_then_the_rest` and `a_fresh_store_wears_pt_default_at_100_percent`.
- **Unset means the theme's.** Background, Accent and Secondary are optional; unset follows the
  theme. A theme pick forgets all three, except that Custom restores what it kept. Each row shows the
  theme's colour until a pick, then a plain Reset forgets it. Held by `unset_colours_follow_the_theme_exactly`,
  `custom_keeps_no_window_colours_until_save_keeps_some` and `accent_forgets_when_handed_none_and_tells_the_painter_both_times`.
- **See-through 0 to 70.** 0 is solid and reads "Off"; other values read "N%". The desktop shows through
  every surface with one coat at the slider value (ground alpha = 1 - level/100). Each theme's own
  see-through is 0; Custom keeps its own. Held by `see_through_reads_clamped_and_nonsense_as_zero`,
  `ground_alpha_is_one_minus_the_level` and `glass_paints_the_ground_once`.
  Under glass the title buttons' and the new-tab "+" strokes are at least 2 device pixels (audit 2026-10-20,
  V7): a 1.1 px stroke at 100% anti-aliased into two part-covered rows and dropped out on the GPU window
  (the minimise glyph vanished, the "+" went faint). The solid window keeps each glyph's own width. Held by
  `glyph_stroke_floor_device_px` and `the_glyph_stroke_floor_is_set_only_under_glass`.
- **Edges: None, Faint, Hairline (default), Solid.** The window line and the one-pixel DWM border step
  follow the ground. Nothing is stored until a choice is made. Held by `edges_are_offered_weakest_to_strongest_by_every_number`,
  `default_to_hairline_and_read_anything_else_as_it` and `apply_is_debounced_250_ms`.
- **Acrylic on Windows 11 through DWM, GPU renderer only.** The row reads "Needs Windows 11." on builds
  under 22000 and "Needs Windows 11 22H2." on 22000 to 22620 (decision D6); the window stays solid there.
  Material is set before colour, so no frame shows a translucent ground without a backdrop behind it
  (held by `order_only_material_first_when_glass_turns_on`); a failed DWM call leaves the window solid
  (`a_failed_backdrop_call_leaves_the_window_solid`).
- **Fonts and size belong to no theme.** Fifteen faces with fallback stacks (default Cascadia Mono),
  base 13 px, size steps 50 to 200 percent. Ctrl+wheel (Cmd+wheel and Cmd+plus, minus and 0 on the
  Mac) moves one zoom that every tab shares, between 7 and 64 px: every open tab, hidden ones included, takes the new
  size at once and new tabs open at it. The zoom is held for the run and never saved (rule 1154). Held by
  `a_theme_leaves_the_font_and_its_size_alone`, `zoom_is_never_saved`, `zoom_stops_at_7_and_64` and
  `a_zoom_resizes_every_shell_hidden_ones_included_and_a_reset_brings_all_back`.
- **Planner decisions D1 to D16 and four owner questions** are in `docs/parity.md` (Decisions). The
  owner may overrule any of them. Q1 (see-through on the software renderer stays solid), Q2 (21H2
  reads "Needs Windows 11 22H2."), Q3 (the reference is Prism Terminal 0.58.0) and Q4 (the transparent
  DirectComposition swapchain is used from start) wait for the owner's answer.
- **Developer flags until M7.** `--theme`, `--see-through`, `--font`, `--font-pct`, `--accent`,
  `--background`, `--secondary` and `--edges` wear a theme for one run; F9 and Shift+F9 walk the wall,
  F8 steps the see-through by 10 (with the F12 overlay open). The keys write the store.
- **Tests that hold it.** The whole workspace: 1548 run, 1548 passed, 2 skipped (M3 ended at 1107).
  `flare-theme` 235, `flare-settings` 131, `flare-win` 93, `flare` 365 (the app crate, which holds the
  look controller, the pixel tests and the screenshots). The Prism Terminal ledger is in
  `docs/test-port.md` ("M4 ledger"); every M4 line of `docs/parity.md` names its tests, or is open with
  its milestone (M7 for the settings UI and the picker; O7 for the eyedropper).
- **Screenshots.** Every theme at 100, 150 and 225 percent on both grid renderers, beside Prism Terminal
  0.58.0 (`docs/screenshots/m4`: six contact sheets, one per size and renderer; the `side` and `pt`
  folders; the `flare` and `material` folders). Pixel assertions hold the colours; the screenshots are for the eye.
- **Measured on this PC, 2026-10-17 (release build, RTX 5090, Windows 11).**
  - Cold start of the real window to its first frame, `--e2e` with `RUST_LOG=info`, 10 runs each:
    visual swapchain (glass plan) median 1257 ms (range 1214 to 1744); without glass
    (`FLARE_NO_GLASS=1`) median 1228 ms (range 1194 to 1334). The visual swapchain is about 30 ms
    slower to the first frame, inside the run-to-run spread. The first-frame mean was not measured
    (the window was not driven through 300 forced redraws); the M4 spike's figure (no measurable
    frame cost, `docs/material.md` E7) stands.
  - `apply_look` for all 39 themes: 1.97 ms in total (about 0.05 ms each), release.
  - The cost of a theme switch on a 120 x 30 grid to the next frame is not timed separately yet (open).
- **Regression rules.** `docs/regression-rules.md` rules 148 to 152 (the oracle, no frame over a
  translucent ground without a backdrop, no DWM call on Windows 10, no theme colour typed outside the
  theme, glass only when it can be used). Rules 67, 68, 69, 73, 74, 75, 126 and 127 are amended with
  their test names. The plan's numbers 143 to 147 were taken by M3, so these moved to 148 to 152.

## Input (M5), 2026-10-17

- **Selection.** Character, word (double click), line (triple click) and block (Alt drag) on the
  grid, with drag scroll past the edge and Shift to extend. A selection is stored in alacritty's
  `Term::selection` and written from cell boundaries (planner D1, D3). Held by
  `double_and_triple_click_select_word_and_line`, `shift_click_extends_and_alt_drag_is_a_block`,
  `a_wide_character_is_taken_whole` and `selection_lands_on_the_right_cells_at_every_scale`.
- **Known limit (D2, open as O23).** A program that erases the lines a selection covers (Claude Code
  redrawing its inline screen) can clear that selection on the live screen. A selection in the
  scrollback is unaffected. The owner's answer is to fix it in M10. Held by `an_erase_over_the_selection_clears_it`.
- **Copy.** Ctrl+C copies and clears a selection and is the interrupt without one. It matches the
  physical key, so Russian and Greek layouts still copy (O2). The "Copied" badge sits at the bottom
  centre and leaves by itself. A program's own selection is announced when the clipboard changes after
  a selecting release. Held by `ctrl_c_over_a_selection_is_not_the_interrupt_and_without_one_it_is`,
  `ctrl_c_by_physical_key_copies`, `a_drag_then_ctrl_c_copies_and_says_copied`,
  `a_program_selection_copy_is_announced` and `the_copy_watch_announces_when_the_clipboard_changes_after_a_selecting_release`.
- **Backspace over a selection** deletes the selected text on the edited line (Left, then one DEL per
  character) and is swallowed; with nothing deletable the selection clears and the key is still
  swallowed. Held by `backspace_deletes_the_selection_on_the_edited_line` and `nothing_deletable_is_allowed_and_empty`.
- **Click to caret.** A plain left click on the typed line moves the caret with arrow keys; every
  refusal (a drag, a modifier, a mouse program, the alternate screen, scrollback, a hidden cursor,
  printed text below, a link) sends nothing. Held by `a_click_on_the_typed_line_moves_the_caret` and
  `every_refusal_sends_nothing`.
- **Difference (D10, O26).** An empty cell and a typed space read as the same character, so a prompt's
  trailing space is not printed text for the caret end or the selection trim.
- **Alt+click moves the cursor (D8 reversed, O25).** xterm's `altClickMovesCursor` is on in Prism
  Terminal, so a quick Alt press and release (under 500 ms, at most one character selected) sends
  the arrows from the cursor to the clicked cell, counted as xterm's `moveToCellSequence` does:
  left and right across rows on the normal screen, up or down then across on the alternate screen,
  nothing while scrolled back. The plain click-to-caret above is unchanged. Held by
  `an_alt_click_moves_the_cursor_with_arrows_and_selects_nothing`, `xterm_normal_buffer_table` and
  `an_alt_click_moves_the_cursor_by_arrows_as_xterm`.
- **Paste.** Ctrl+V, Ctrl+Shift+V (plain, even with an image) and right-click Paste share one path
  and deliver exactly once. Text is one bracketed paste when the shell asked for it, sanitised (no
  ESC, no early bracket end) and has its newlines turned into carriage returns. Copied files paste as
  quoted paths for the shell that runs in the tab. An image sends the agent's image key only. Held by
  `ctrl_v_ctrl_shift_v_and_the_menu_paste_each_deliver_exactly_once`, `plain_pastes_only_the_text`,
  `copied_files_paste_quoted_paths_for_each_shell`, `an_image_pastes_the_agent_key_and_shift_enter_is_a_newline_only_for_agents`
  and `a_hostile_paste_is_inert`.
- **Drop.** Dropping a file on a tab types its quoted path into a live shell, never Enter and never a
  trailing space. Held by `a_dropped_file_types_its_quoted_path_and_nothing_runs` and
  `several_files_are_joined_by_one_space_and_never_end_in_enter`.
- **Multi-line paste (D15, O27).** No confirmation and no "paste is large" notice, as in Prism
  Terminal. Bracketed paste is the safety for shells that asked for it. Held by `a_multi_line_paste_is_one_bracketed_block`.
- **Find (Ctrl+F).** Ctrl+F opens the bar except in a full-screen program (alternate screen or mouse
  reporting); Ctrl+Shift+F always finds. The bar is 332 by 28 px plus hairlines at the top right, 12 px
  from the edges, with no shadow and no case button. Incremental search keeps the current match,
  Enter goes next, Shift+Enter previous, Escape closes. Readout words: empty, "none", "N+" past the
  threshold, "i of n". The search covers the whole scrollback, wrapped lines and the alternate screen,
  and runs in slices so no pass holds the window. Closing clears the highlights and leaves the
  selection. The accepted cost: at a readline prompt Ctrl+F no longer means cursor right. Held by
  `ctrl_f_opens_the_bar_escape_closes_it_and_a_full_screen_program_keeps_ctrl_f`,
  `typing_highlights_every_match_and_enter_steps`, `previous_and_next_do_nothing_without_hits`,
  `the_bar_is_332_by_28_plus_two_hairlines` and `find_and_a_big_scrollback_never_hold_the_window`.
- **No ruler.** Prism Terminal sets no overview ruler, so none is painted. The highlight colours are
  the Prism Terminal ones (match, border, accent for the active match) through `DecorInk`.
- **Links.** Left click opens; right click opens nothing (D6: a plain click, path links too;
  `link_click_needs_ctrl` returns false). Links are painted in the link ink with a faint underline that
  never blocks a click. Scans cover the viewport only, from a per-line cache, sliced under 8 ms; the
  scrollback needs no stored decoration. OSC 8 links come from alacritty's cell hyperlinks (D18) and
  open on a plain left click when http or https; a label showing another host opens the address it shows.
  Held by `a_printed_link_wears_the_link_ink_and_its_full_stop_does_not`, `an_unchanged_line_is_not_scanned_again`,
  `a_new_ink_scans_everything_again`, `a_burst_of_scrollback_is_not_scanned`, `an_osc8_label_wears_the_ink_and_opens_once_on_a_left_click`,
  `an_osc8_label_showing_another_host_opens_what_it_shows` and `any_other_button_opens_nothing`.
- **Paths.** Since 2026-11-11 a path is a link only when a session writes it between the marks `⟦` and `⟧`
  (section "File links by marker" below); Flare guesses no file path from screen text. The backend resolves
  and checks it again on the click; a click
  opens only allowlisted types and reveals everything else in Explorer (O22, rule 134). A folder opens
  in Explorer; when an Explorer window already shows that folder, that window comes forward (restored if
  minimized) and no second Explorer starts, because the foreground lock could stop Explorer raising it itself
  (issue #55; `a_folder_already_showing_is_brought_forward_and_explorer_is_not_started`). A folder sitting in a
  background Explorer tab brings its window forward without switching tab. The user's chosen folder handler is M9. Held by `a_path_is_a_link_only_between_marks_and_opens_by_the_allowlist`
  and `a_path_menu_open_and_show_go_through_the_allowlist`. A web address a program breaks at the window's
  edge by hand (Claude Code writes each row on its own, so the next row is not marked wrapped) is
  read across the break and is ONE link on both rows (review #37, `cells::by_hand`); an OSC 8 label cut that
  way stays one mark per row. Held by `a_url_broken_by_hand_at_the_edge_is_one_mark_to_the_whole_address` and
  `an_osc8_label_cut_by_hand_is_a_mark_per_row`.
- **Terminal menu.** Right click opens the menu for what was clicked: path rows, link rows, Copy with a
  selection, Paste, Find in scrollback, and Command help last while it is on (M7). No Close tab. A path
  to a picture or text file also gets "Show in side panel" after Open (Surfaces round 9), and a tab that runs
  Claude gets "Show files Claude changed" in a group of its own at the bottom ("Show task list" was removed with the
  Tasks panel, 2026-10-26). It is
  bound to the tab it opened on. Held by `the_menu_fits_what_was_clicked_and_copies_exactly` and the
  flare `term_menu.rs` tests.
- **A right click outside an open menu opens the next menu (owner, 2026-10-23).** "Right click should open the
  right click menu each time, now it alternates, one click opens it the next closes it, other apps open it again
  when you click right click with one menu open already." The cause: the menu's full-window scrim swallowed every
  press, so the surface under the pointer never saw the right click. Now, with a menu (or the +N list) open, a
  right click released outside it closes it AND the same right click goes on to whatever is under the pointer
  (a tab, the + button, the terminal, a link), in one click. The scrim reports the point (`outside-right` in
  `ui/menus.slint`); `AppCtl::outside_right_click` closes the menu and, from a zero timer, hands the click to the
  window through Slint's own event path (`Window::dispatch_event`: move, press, release of the right button; no
  OS input), so every surface decides as it does for a real click: the terminal's mouse reporting still reports
  it and the menu still opens (rule: a report never marks the tab as typed). A left press outside only closes.
  A right click inside the panel is unchanged, except on the menu's own anchor (a 5 px square at the point it
  hangs from, which the window edge can leave inside the panel): that is the same click again and reopens the
  menu. Held by `a_right_click_outside_reports_its_point_and_a_left_press_only_closes`,
  `the_overflow_list_hands_on_a_right_click_outside_too`, `a_right_click_outside_an_open_menu_opens_the_menu_of_the_tab_under_it`,
  `a_right_click_outside_an_open_menu_reopens_it_for_what_is_under_the_pointer`,
  `a_right_click_on_the_same_spot_reopens_it_every_time` and `an_outside_right_click_goes_where_a_normal_one_would`
  (100 and 225 percent), and e2e `selectionEdit` ("a right click outside an open menu opens the menu of the link
  under it"). The Settings dropdown (a different popup, no right-click menus under it) is unchanged.
- **Mouse reporting.** A program that owns the mouse gets press, release, motion and wheel as bytes;
  a right click still opens the menu, and a report never marks the tab as typed. Shift gives the
  selection back. Held by `a_mouse_program_gets_reports_and_shift_selects` and `write_report_is_not_typing`.
- **IME caret hold.** A key opens a 250 ms window that holds the caret on the input line for IME and
  screen readers; it is released on the alternate screen, a resize, a buffer change or scrollback.
  Held by `the_hold_is_released_on_the_alternate_screen_a_resize_and_scrollback` and the flare-term `caret_hold.rs` tests.
  The cursor stops a tab records between frames are on the UI's monotonic clock, so a key's echo no frame showed
  still holds the caret (rule 1206).
- **Clipboard.** Reads and writes run on a worker. A copy is refused, never trimmed, when it is empty or
  too large. The test flag uses an in-memory clipboard and records openers. Held by
  `copy_async_refuses_empty_and_oversize_without_touching_the_clipboard` and
  `under_e2e_the_clipboard_is_in_memory_and_openers_record`.
- **OSC 52.** Write-only; a read query is swallowed and never answered; the accepted write shows the
  badge. Held by `osc_52_lands_with_utf8_intact_says_copied_and_a_read_gets_no_reply`.
  Only the tab in front may write the clipboard; a background tab's OSC 52 is dropped, and a flood shares one worker where the last wins (audit 2026-10-20, L4 L39 L48). Held by `osc_52_from_a_background_tab_is_ignored` and `an_osc_52_flood_uses_one_worker_and_the_last_write_wins`.
- **Regression rules.** `docs/regression-rules.md` rules 154 to 159 (cell boundaries, the typed drop,
  mouse reports, link opening and no held UI, the off-thread clipboard, opaque decorations in whole
  pixels). Rules 8 and 19 are amended with their native meaning. The plan's numbers 142 to 147 were
  taken by M2 and M3, and M4 used 148 to 153, so these moved to 154 to 159.

## The agent indicator, hooks, marks and resume (M6), 2026-10-17

- **Title first, hooks above it, scoring last.** An agent is recognised by the title it sets first, by
  Claude Code's own hook signal above that, and only for an unhooked session by scoring its output.
  Held by `a_claude_title_is_the_agent_present_ahead_of_the_poll_and_asks_a_look`,
  `a_hooked_session_is_never_scored_and_its_screen_is_not_read_for_output` and
  `a_poll_report_makes_the_agent_present_and_births_it`. (The plan records this order without a date.)
- **Ring or Bar only (2026-10-12).** A tab's mark is a ring by default, or a bar along the tab foot.
  `off`, `full` and the Powerline `prompt` style do not exist here, and a stored word `minimal` is
  Bar, anything else Ring. Held by `the_stored_word_minimal_is_bar_everything_else_ring` and
  `the_indicator_pick_turns_a_ring_row_into_a_run_bar`.
- **Mark colours are a Bar setting (2026-11-11).** Settings > Agents shows the Mark colours section (working,
  finished and question colour) only while the marks are Bar; Ring marks are mono in the tab name's ink, so the
  section and its Find entries are hidden with Ring and come back live when Bar is picked (the +N button's dots
  still wear a pick). Owner: "these should be hidden if you use the ring style". Rule 1140.
- **Finished is news and Question is state (2026-10-09).** Finished and Failed are raised once and
  cleared by the next turn or by looking; a question stays until it is answered or the agent leaves.
  The front tab never gets a finished or failed mark while it is being looked at, but a question is
  raised there. **A mark raised on the tab in front while the window was unfocused is held until the tab in front changes
  (2026-10-21, narrowed 2026-10-23, owner: "it should only disappear once I click a new tab ... if I'm
  already in the terminal and I'm on the tab that completes, it should not show the completion badge,
  only when I have another program in focus"):** looking is the window focused AND that tab in front, and
  while looking no Finished or Failed mark is raised. Away, the front tab finishing raises the mark and
  holds it; coming back and the pointer moving do not clear it, the badge counts it, and it comes down
  only when the front tab changes to a DIFFERENT tab (a click, a key, an overflow pick), work restarts,
  the agent leaves or the tab closes. Clicking the tab already in front does nothing, and closing another
  tab with its X keeps it. Background-tab marks are unchanged. Held by `coming_back_does_not_take_a_held_mark_down`,
  `a_click_on_the_same_tab_keeps_a_held_mark`, `a_click_on_another_tab_releases_the_mark_on_the_tab_left`,
  `a_keyboard_switch_releases_it_like_a_click`, `closing_another_tab_keeps_a_held_mark_but_forgetting_the_marked_one_drops_it`
  (flare-agent), `a_pick_of_the_same_tab_keeps_a_held_mark`, `a_pick_of_another_tab_takes_a_held_mark_down_on_the_tab_left`,
  `closing_another_tab_keeps_a_held_mark` (appctl) and e2e `agentHooks`. Also held by
  `finished_is_not_raised_on_the_tab_in_front_but_a_question_is`,
  `each_attention_mark_is_behind_its_own_switch` and `working_outranks_question_outranks_failed_outranks_done`.
- **Keep agent sessions open (2026-10-16).** With an agent in the front tab Ctrl+D never reaches it, and
  Ctrl+C is taken unless the agent works or a selection is held. Never in a plain shell. Held by
  `keep_agent_sessions_open`.
- **One check per claim (2026-10-23).** A hook line or a Claude title can mark a tab as an agent before the
  process poll has looked. Each such claim asks the poll to look at that tab once, and the answer is sent even
  when unchanged, so a tab with no agent loses the false Claude icon and Question. The owner chose "yes, one
  check per claim" and does not want scheduled checking scripts: no new timer or polling loop. Held by
  `a_hook_claim_asks_one_look_by_name_and_a_no_agent_answer_takes_it_back` and the e2e agentHooks (which runs
  a real stand-in `claude.exe`, plus a cat'ed OSC 777 capture in a plain shell). A WSL tab is the exception: the
  poll cannot see an agent inside it, so no one asks it about that tab and the claim stays (as `arm.rs` keeps a
  titled agent there). Held by `a_wsl_tab_keeps_its_claim_because_no_one_asks_the_poll_about_it`. ssh in a
  Windows shell is not covered yet.
- **Finished at turn end (2026-10-23).** Claude's `· N shell(s)` count in the footer alone is no longer
  background work: at Stop a tab whose only sign is that count (a run_in_background dev server) is Finished,
  and closing it follows the normal rule for an idle agent. A workflow's progress row and Claude's own
  `· N shells still running` summary still keep it Working. This supersedes the 2026-10-16 background rule
  where it counted footer shells. Held by `a_dev_server_in_the_footer_alone_is_not_work`,
  `a_stop_with_only_the_footer_shell_count_raises_finished`, `a_stop_with_a_workflow_progress_row_stays_working`
  and the e2e backgroundWork.
- **The rainbow finish (2026-10-10).** A finished mark may flow through the seven rainbow stops, one tab
  in eight seconds; reduced motion and a minimised window stop it. Held by
  `the_rainbow_line_flows_one_tab_in_eight_seconds` and `animation_effects_off_stills_every_mark`.
- **"Exact status from Claude Code" is on by default (2026-10-05).** The hooks that Claude Code runs
  report working, question, finished and failed; `flare.term.agentHooks` turns them off, and only a
  new shell sees the change. Held by `hooks_json_matches_the_table`, `hooks_json_has_25_entries` and
  `hook_cmd_is_crlf_and_static`.
- **The launch skeleton (2026-09-30).** A restoring agent tab shows the resume skeleton from the first
  frame and the clear reaches the grid only, never the shell. The reveal waits for the agent's own title
  (150 ms after it), or a key, an exit, a failed spawn, or 12 s. A resume is not new work. Held by
  `the_first_frame_has_the_tabs_the_skeleton_and_the_ring`, `a_resume_is_not_new_work` and
  `restored_tabs_carry_no_done_question_or_failed`.
### Planner decisions D1 to D16

These are the planner's decisions for the owner to overrule. Each is held by the tests named in the
M6 plan's Tasks 2 to 35.

- **D1** `flare-agent` holds no Win32 or UI types; the process snapshot, the overlay and the animation
  switch are `flare-win`'s, passed in through traits and closures.
- **D2** The indicator is one struct with injected time, replayable without a clock or a window
  (`timeline_*`).
- **D3** The poll runs on its own worker thread; an ask that arrives during a look waits in a shared set.
- **D4** The process query is native (ToolHelp32 and the command lines of the live shells' descendants),
  not PowerShell or WMI. The compact text listing and its five tests are dropped.
- **D5** Mark motion is one clock, owned by Rust. It runs only while a mark moves, the window is not
  minimised and Windows' animation effects are on (`the_clock_runs_only_while_something_moves`).
- **D6** Ring and Bar only (see the owner's decision above); the other PT cells are dropped in the ledger.
- **D7** The resume reveal splice lives in the terminal handle, the one place that sees the raw chunks.
- **D8** The plugin ships in `crates/flare-agent/claude-plugin/` and is copied beside the exe. It is
  handed to a shell as `CLAUDE_CODE_PLUGIN_DIRS` (`an_inherited_copy_of_our_plugin_is_dropped`).
- **D9** The question widget is watched for tabs recognised as Claude Code and posted to Sonara on ONE worker
  thread, to 127.0.0.1 only, with the token refused if it holds a control character (`only_loopback_is_ever_contacted`).
  Audit 2026-10-20 (A15): the post also needs the listener on the port to be the process runtime.json names in
  `pid` (`a_port_held_by_another_process_gets_neither_the_token_nor_the_question`), and a plain shell cannot
  forge a post (`a_plain_shell_cannot_forge_a_question_shown_post`). The dictation engine likewise trusts its
  port only when its own server is the listener (`a_stranger_answering_on_the_engines_port_is_never_trusted`),
  and a plugin refresh writes a temp file and renames it (`a_refresh_goes_through_a_temp_file_and_a_rename`).
- **D10** Diagnostics go through `DiagSink`; M9 implements it over `diag.jsonl`
  (`diagnostics_get_the_four_kinds_and_no_text`).
- **D11** The agent colours are M4's; the picks replace the theme's working, finished and question
  colours, and failed is always the theme's.
- **D12** The taskbar badge is a disc drawn by Flare as an overlay (`ITaskbarList3`), never a window
  icon, at 16 logical px times the scale.
- **D13** Settings rows are M7's; M6 owns the switch keys and getters only.
- **D14** Codex has no hooks. It is found by the poll and its title dialect, and is resumable with
  `codex resume --last`. Gemini, aider and other agents light the indicator and have nothing to resume.
  agy (the Antigravity CLI) became the third resumable agent on 2026-10-26: see "Antigravity (agy) as an agent".
- **D15** The cd guard is kept uncalled, so a later decision can wire it with one call.
- **D16** `hooks.json` is generated once and guarded forever (`hooks_json_matches_the_table`).

### What the probe in Task 24 found

The probe asked whether Slint's `@linear-gradient` takes dynamic stops, and whether the software
renderer draws a gradient with eight stops. The code records the outcome in `ui/tabmark.slint`: the
finished rainbow is 42 two-stop gradients (one per seventh of a 114 px tile), with the colours read
through `root.stop(...)`. The eight-stop form drops spans in the software renderer; the two-stop form
is exact in both renderers. So the image fallback was not needed. The scenes that draw the ribbon are
in `ui_tests/marks.rs`.

### The owner's questions (open)

1. Tab-icon artwork: the OpenAI mark (Simple Icons 15, CC0) for Codex and the Clawd pixel art in
   `#d77757` for Claude, both named in `THIRD-PARTY-NOTICES.md`. Confirm both may ship in a private repo.
   The agy arch (2026-10-26, after Antigravity's banner) is named there too and has the same question.
2. The taskbar disc counts Finished, Question and Failed tabs as Prism Terminal does, and is on by
   default. Confirm it stays on. No answer is recorded yet.
3. Sonara still reads `PRISM_TAB_ID` (decision O6). Flare posts `host_tab` with `FLARE_TAB_ID`, so a
   Flare tab's question is not read aloud until Sonara learns the new name. Raise it in Sonara's repo.

### Numbers for the M6 PR (measured 2026-10-17)

- **Tests.** `cargo nextest run --workspace` (debug profile): 2502 passed, 6 skipped. Per crate: flare 707, flare-term 551, flare-agent 314 (new in M6: the indicator, hook state, poll, resume, plugin and scan tests), flare-theme 235, flare-settings 135, flare-win 136, flare-tabs 157, flare-render 127, flare-shells 103, flare-diag 24 (M6 adds `agent_diag`), material-spike 13. The M6 ledger's own columns give about 223 ported and 60 new tests; that is the planned split, not a per-test count.
- **Hub drain.** 10,000 `Output` events for a hooked session through `AgentHub::on_host_events`: 80 microseconds, best of five, release.
- **Process snapshot.** `flare_win::procs::snapshot_processes` with one root on this machine (609 processes): 9.0 milliseconds, best of five, release. The plan's 10 shells and 300 processes were not reproduced.
- **First frame.** `flare --scene marks-ring --size 1000x640 --screenshot`, whole process from start to exit, five runs each: software median 274 ms (first run 526 ms), GPU median 652 ms. This includes process start, so it is not an in-process first-frame timer.
- **Not measured in this run.** The idle CPU of the windowed app over 10 s (with a finished tab, and with a working tab), and the cold-restore timings of Task 32. Both need an interactive window and an agent tab, so they belong to the Task 36 review.

## Settings, command help, About, import (M7), 2026-10-18

Settings is a tab that fills the pane, opened from the cog. The page you chose stays while the tab is open and
resets to Appearance on reopen; the memory lives in the window only. The rail lists six pages in Prism Terminal's
order (Appearance, Terminal, Agents, Dictation, Diagnostics, About, with About after a spacer). The look is
Prism Terminal's, measured from its Tailwind classes and its layout (Appendix B of the M7 plan): the grouped
cards, a section heading, one panel of rows, 32 px icon tiles, one line of subtext, hairlines that start after
the icon. Every size is held to a layout test (`layout::rows_match_prism_terminals_measured_boxes`).

- **Every row applies at once, Settings still open (owner bug 2026-10-21).** As in Prism Terminal, a change
  reaches the live window without leaving Settings: Tab position moves the tabs to the side or the top, the
  look rows (theme, colours, see through, Panel edges) are worn, the terminal font and size reach the grid, the
  marks and the taskbar count follow their switches, Command help goes, the key rows hold for the next key and
  Detailed logging switches. The cause was the tab view reading its own rows (the tab position, the taskbar
  count) only on its next refresh, which closing Settings caused; the controller now listens to every row key
  the look, the help panel and dictation do not own and refreshes once per turn (`AppCtl::listen_to_rows`).
  Kept as Prism Terminal has them: the default shell and Exact status from Claude Code apply to terminals
  opened after the change, the folder for new tabs to the next new tab. Held by `ui_tests::settings_live::*`
  (a test per row class at 100 to 300 percent, plus `every_other_row_key_reaches_the_main_window`, which
  makes a row added later live too), `the_mark_rows_and_the_taskbar_count_apply_while_settings_is_open`, and
  the e2e `settingsLive` (Tab position both ways and Panel edges, read from the main window with Settings in
  front, at 100 percent and again after a move to a 225 percent monitor).
- **Find a setting.** The index is built from the same row tables the pages draw, so a row that is not drawn on
  this PC is not indexed (the GPU row needs NVIDIA; the dictation rows wait for M8). Words match by prefix with
  accents folded, ranked label first. Choosing a result switches page, scrolls the row to the middle, flashes it
  and focuses its first control. Tests: `settings_find::typing_each_label_opens_its_page_flashes_and_focuses_its_first_control`,
  `find_state::*`.
- **Neutral controls (rule 78).** Only Save changes and an ON switch wear the accent. The chosen rail page is a
  grey fill. Rendered pixels prove it (`settings_ink::no_other_settings_file_uses_the_accent_except_the_wall_and_help_marks`).
- **Plain words (rule 79).** Every label is plain, every subtext is at most eight words and names no key. The
  See-through row keeps Prism Terminal's nine-word subtext (decision D5, O8), the one named exception
  (`copy_rules::every_subtext_of_the_tables_is_at_most_eight_words_except_the_see_through_one`).
- **One colour picker with alpha everywhere.** HEX, RGBA and HSLA with the format toggle (remembered), the three
  named sliders, before and after, Escape reverts exactly the opening value. A colour field commits only on
  Enter or on losing focus, and only with a parseable change (rule 77). Tests: `picker::*`, `settings_colour::*`.
- **Eyedropper (O7, ported in M7).** A Win32 screen pick (`flare-win/src/eyedrop.rs`): a snapshot of the virtual
  screen, a cross cursor and a 9 by 9 magnifier, Escape or a right click cancels. Under `--e2e` the window never
  appears: `FLARE_E2E_PICK` answers at once. The by-hand window test is not yet run (see the numbers below).
- **Command help (ported, not extended).** The side panel, copy only. Seven catalogue files are data generated by
  `tools/oracle-help` from Prism Terminal's own TypeScript (D2); the search is ported to Rust and checked against
  2468 golden queries made by Prism Terminal's `searchHelp` (ids and scores equal, zero known ULP differences). The
  panel never inserts, runs or types into a shell: a test counts writes across search, copy and Enter and finds
  none (`settings_help_flows::no_write_reaches_a_shell_across_search_copy_and_enter`).
- **About.** Names Flare and its version, and carries Slint's own "Made with Slint" badge with its licence
  (O9: the About description line is the owner's wording, still open).
- **First-run import from Prism Terminal.** Once, before the first paint, on a worker with a 1.5 s ceiling. The
  Stable profile first, then the main profile. Chromium's Local Storage is read from a COPY made in a temp folder of
  Flare's own, with `rusty-leveldb`. `prism.*` keys map to `flare.*`, a value that cannot be read reliably is
  skipped and named in the report, never guessed. Nothing is ever written into Prism Terminal's folders (a hash test
  holds that). Tabs are never imported. The import records itself in `flare.import.done`, so it runs once.
  Tests: `import::run::*`, `import::map::*`, `import::legacy::*`, `import_e2e::*`.

### Decisions the M7 plan records from the owner

- Settings is a tab that fills the pane, not a window (M7 plan, Goal).
- The rail, the grouped cards and Find a setting follow Prism Terminal's layout (M7 plan, Goal and Appendix B).
- The colour picker has alpha on every colour, and the eyedropper is ported (O7).
- Command help copies and never types (rule 82). The Prism Terminal copy rules hold (rule 79), with the one
  exception of the See-through subtext (O8).
- About names Slint (O9 for the About description line, still open).
- Setting keys are `flare.*` (O10), with the import table as the only place that knows `prism.*`.
- Every display scaling works, 100 to 300 percent, both renderers (the roadmap; owner, 2026-10-17).

### Planner decisions D1 to D20

- **D1** `flare-help` is its own crate (the catalogue is about 1.2 MB of data).
- **D2** The catalogue and the golden vectors are generated, never typed.
- **D3** `flare-settings` owns the Settings logic without a window: page models, layout, actions.
- **D4** The row tables use `flare.*` keys. The `explorer-verb` row stores nothing (the registry).
- **D5** The See-through subtext keeps its nine words (O8). The About card's two lines are About content, not a row subtext.
- **D6** Rows not drawn on this PC are not indexed (the GPU row, the dictation rows before M8).
- **D7** The Dictation page is a provider seam: a placeholder in M7, the real page in M8.
- **D8** The Explorer verb and the diagnostics writer are seams: `PendingVerb` answers busy until M9, and the verbose flag file plus the folder path are M7's.
- **D9** No backdrop blur in popups. The Select menu casts no shadow; the colour popover and the help panel keep a soft shadow.
- **D10** The Select menu is an overlay at the window root, placed by `layout::place_menu`.
- **D11** The colour popover is an overlay inside the window, placed by `picker::place_popover`; a left-open picker comes back when Settings is shown again.
- **D12** The eyedropper freezes the screen with a Win32 snapshot; the seam answers under `--e2e`.
- **D13** The theme wall's cards are laid out by Rust (columns = floor((width + 12) / 184), cards stretched to fill the row and never under 172 px less one device pixel, edges on whole device pixels relative to the wall, PT PR #251; the editor's preview card stays 196 px, see parity.md); the expand animation is 240 ms with Prism Terminal's curve, none under reduced motion.
- **D14** Import runs once, before the first paint, with a 1.5 s ceiling. A failed copy is retried on the next three launches, then recorded as given up. A profile that is absent or empty is done. There is no "import again" button.
- **D15** `mic` is not imported (Chromium device ids mean nothing to Flare). Retired keys and the registry-backed verb state are never imported.
- **D16** The F8 and F9 developer chords of M4 are removed; the look flags stay for headless scenes.
- **D17** The help panel sits inside the page area, top to bottom, right edge; its width is `min(clamp(stored, 420, 1100), page_width - 120)`.
- **D18** About sets the palette's colour scheme so the Slint badge reads on both themes; the fallback is a copy of the two SVGs if the pixels fail.
- **D19** Help category headings are not sticky (a Slint `Flickable` has no sticky children). A rendering difference, recorded in the M7 image review.
- **D20** The LevelDB reader is a vetted crate (`rusty-leveldb` 4.0.1). If a later real profile defeats it, the importer skips; no builder writes its own reader.

### Seams until later milestones

- The Explorer row reads "Checking with Windows." (disabled) until M9 implements the registry verb.
- The diagnostics writer and its quiet level are M9's; M7 ships the verbose flag file and the folder path.
- The Dictation page's rows are M8's; M7 ships the placeholder page and the rail entry.
- The GPU row is indexed only with NVIDIA, which M8 sets.

### Open questions

1. About description line (O9): the owner's wording.
2. The persisted-keys line of the parity list still names the `prism.*` keys. That contradicts D4 and O10 (the keys are `flare.*`); the owner should confirm the wording before the line is ticked.

### Numbers for the M7 PR (measured 2026-10-18 on this PC, debug profile unless said)

- **Tests.** `cargo nextest run --workspace`: 3176 passed, 11 skipped (the hand-run tests). Per crate: flare 985,
  flare-term 551, flare-settings 406, flare-agent 314, flare-theme 235, flare-tabs 157, flare-win 144,
  flare-render 127, flare-help 109, flare-shells 103, flare-diag 32, material-spike 13.
- **Golden vectors.** 2468 queries in `crates/flare-help/tests/fixtures/golden_search.json`; `KNOWN_ULP` is empty.
- **Oracle.** `node --test oracle.test.mjs` in `tools/oracle-help`: 5 of 5 pass, the copy check included when
  `PT_HELP_SRC` names Prism Terminal's help source.
- **Gates.** `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`
  and `cargo deny check` all pass. The scan tests (`no_prism_name_outside_the_import`, `no_em_dash_*`) are in the nextest run.
- **Not measured in this run (honest gaps).**
  - The three hand-run tests: `real_profile_copy_reads_the_theme` and `the_real_stable_profile_copy_imports_in_under_300_ms`
    need a copy of the owner's real Prism Terminal Stable profile, and the copy was refused by the session's
    permission check, so they were not run. The import timing on the real profile is therefore open.
  - `the_window_picks_a_pixel` shows a window and waits for a click at the desktop, so it was not run.
  - The help index build time and the median keystroke on the release build: the speed test passes its bounds
    in debug only (10 times looser there). Release figures are open.
  - The metrics comparison of Task 45 (the largest difference), the idle CPU of the Settings tab over 10 s, and
    the ported-case count against Appendix A (about 200) are not recounted in this run.

## Updates, installer, Windows integration, diagnostics (M9), 2026-10-18

Updates are a one-click, per-user install through Velopack, switched off until the repository is public. The
Explorer folder menu says "Open in Flare", the log is a local file with no typed text in it, and the icon is
the owner's own artwork. The owner decided these on 2026-10-17 (O15, O17, O4, the update line in the brief);
the planner's decisions D1 to D16 are in the M9 plan and summarised as O28 to O43 in `docs/parity.md`.

- **Updates are off until public (the owner, 2026-10-17).** The check, the feed and the install exist and
  are tested every day, but the GitHub source is compiled in only with the `live-updates` cargo feature,
  which is off by default. A build without it has no check, no request and no chip. The flip is one reviewed
  line, a `PRIVACY.md` edit and a release (D1). Test: `a_default_build_has_no_live_mode`, `velopack_feed`'s
  `no_network_source_exists_without_the_feature`, and `release_files::live_updates_need_a_public_repository`.
- **One click, per user.** No licence page and no folder choice; a Start menu shortcut and no desktop shortcut;
  unsigned; win-x64 only; the uninstaller keeps the user data (D8, D14, O15). The install runs only the
  package Velopack offered, named by an opaque token the UI cannot forge (D2). Test: `installs_only_the_current_offer`,
  `refuses_any_other_token_a_none_and_everything_with_no_offer`.
- **Install, cancel and restart.** The close question is answered before the download, so a running agent is
  asked about once, at the start (`installs_the_offer_it_made_handing_the_installer_the_close_pre_answer`). A
  cancel wins even after the last byte and leaves nothing (`cancel_wins_even_after_the_last_byte`). A failed
  download is said once in the window, and the chip offers again (`updateGuard_a_failed_download_shows_could_not_be_downloaded_once_and_the_chip_offers_again`).
  The app quits 400 ms after a successful hand-off; Velopack applies and restarts (`the_app_quits_400_ms_after_a_successful_handoff`).
- **The installed version decides.** An installed app compares an offer with the version Velopack installed
  (its manifest), and the update window shows that version as the current one; a dev build uses its own
  (rule 1144, `an_offer_is_measured_against_the_installed_version_not_the_builds`).
- **Preview.** `--preview-update` previews the window in the running app or in an unpackaged build, with the next
  minor version and the sample notes, and touches no network, file or process (`preview_version_is_the_next_minor`,
  `updateQuiet` in `tools/e2e`).
- **Update window and chip.** The chip is `Update <version>` in `--p-sel-bg` with on-accent text at 4.5:1 or
  more, leftmost in the right group, and its width never changes between phases. The window is a 460 px box with
  the release notes capped at 20 entries and 160 characters each, grouped as New features, Improvements, Bug
  fixes and Under the hood, and it is plain text only: a hostile body becomes no element and runs no handler
  (`updateNotes_a_hostile_body_becomes_no_element_and_runs_no_handler`). Tests: `ui_tests::update_chip`,
  `ui_tests::update_window` (eight scales), `ui_tests::update_flows`.
- **The Explorer verb.** Two HKCU keys, `Directory\shell\Flare` and `Directory\Background\shell\Flare`, label
  "Open in Flare", pointing at `<install root>\current\Flare.exe` (D4). Written through the Win32 registry API,
  never `reg.exe` (D3). Written only when packaged and not under `--e2e` (D5). Windows 11 shows the classic verb
  under "Show more options", not on the first menu. Test: `verb_keys`, `verb`, `verb_switch`, `registry` in flare-win.
- **Two apps side by side.** Different app id, verb key, single-instance mutex and pipe, profile, plugin name,
  hook signal and tab-id variable. Test: `every_flare_name_differs_from_prism_terminals` (`crates/flare/tests/names.rs`,
  against `names.json`). The side-by-side install test (both apps installed on a CI runner) was removed on 2026-11-11 by
  owner decision: no CI step downloads the reference app or needs a token for it (rule 1183, `docs/shipping.md`).
- **The log.** `%APPDATA%\Flare\logs\diag.jsonl`, one JSON object per line, rotated at 2 MB into four old files,
  written by a thread that never blocks the UI (a queue of 2000 lines, dropped and counted when full). It holds
  no typed text, clipboard, audio or conversation, and `mark` carries no note (D12). Read it with `cargo diag`
  (D13). The stall kinds and the crumbs are in [docs/diagnostics.md](diagnostics.md). Tests: `writer`, `stall_flow`,
  `summary`, `gate`, `reader`, `control` in flare-diag; the privacy scan in `crates/flare/tests/privacy.rs`.
- **The icon.** The owner's artwork (`assets/icon/flare-color.svg`) is the app icon, the taskbar and the Explorer
  icon, rendered as it is with no badge and no outline (O17, 2026-10-17). `tools/make-icon.py` builds the
  frames; `tools/test_make_icon.py` checks them. 2026-10-21: the owner's final icon replaced the first
  artwork: an orange and a gold flame on a dark rounded square (the three SVGs in `assets/icon/`). The
  title bar's and the About card's mark keep the flame's outline, tinted by the theme. 2026-10-22 (owner: "the
  flame icon on this screen should be the one from the icon which has that two toned design with orange and
  yellow"): the start screen draws the flame in the icon's own two tones, the orange flame with the golden one
  exactly over it (`AppMarkColor` in `mark.slint`, the artwork's two paths and their 135 degree gradients, 84 px),
  never in the accent. Held by `ui_tests::start::the_mark_is_the_two_toned_flame_at_84_px` (orange and gold
  pixels, the ink size, the gold inside the orange, at 100 and 225 percent) and
  `the_color_mark_is_the_artworks_two_paths_and_gradients` (the paths, stops and direction are the SVG's), the e2e
  `lifecycle` (`start.state` reports the two tones).
  2026-11-12: the app icon got its own sources, one per platform (rule 1182): Windows' `.ico` from
  `assets/icon/app-windows.svg` (a gold flame on a dark rounded square), the Mac's from the Icon Composer icon
  `assets/icon/app-macos.icon` (Liquid Glass, compiled by `actool` into `Assets.car` plus a fallback `Flare.icns`).
  `flare-color.svg` stays the in-app mark.
  2026-11-13 (owner: "the app icon should stay the same as default, no new default, just a picker so the user can
  change their look"): the original icon is the shipped app icon on both systems again (rule 1230): `build/icon.ico`
  and the Mac's `Flare.icns` both come from `assets/icon/flare-color.svg`. The new designs are choices instead:
  **Settings > Appearance > App icon**, a line of twelve icon tiles under the row's words (Original first and picked
  until another is, then Liquid Glass, High Voltage, Daybreak, Hazard Reverse, Safety Orange, Blue Enamel, Raspberry
  Lacquer, Lemon Ink, Jade Glass, Citrus Punch, Cobalt Electric; sources in `assets/icon/app-icons/`). A tile has no
  words: its name is its tooltip and accessible label ("Original, the default" for the first), the chosen one wears
  the accent ring, and the picked name sits at the row's right. Stored as `flare.window.appIcon` in `settings.json`.
  How a pick is worn (rule 1231): on the Mac, `NSApplication.applicationIconImage` (the Dock and the app switcher),
  at start and on every pick; Finder keeps the original, since the signed bundle is never written to. On Windows,
  the window's big and small icons (`WM_SETICON`, which the taskbar button shows), at start, on a DPI change and on
  every pick; shortcuts, the exe and the installer keep the original.
- **Taskbar identity per profile.** 2026-10-22 (owner: the stable copy showed the installed Flare's old icon on
  the taskbar): a run with its own `--user-data-dir` has its own taskbar identity,
  `com.flare.terminal.p<16 hex>`, the FNV-1a 64 hash of the lowercased profile path (`/` as `\`, trailing
  separator dropped, a `\?\` prefix dropped, a relative path read against the cwd), so it never groups under the installed Flare's Start menu shortcut and its old icon. The
  default profile keeps `com.flare.terminal`, which the installer stamps on its shortcut. `flare.exe
  --print-aumid [--user-data-dir=<path>]` prints the identity and exits, so an install script can stamp it on
  the shortcut of a second copy (the last `--user-data-dir=` wins; a space form or an empty one exits 2). The install script must stamp exactly the value printed for the path the shortcut launches with. Toasts must use the same value (`ship_ctl::effective_aumid`). Held by
  `flare_win::names::tests::aumid_is_the_plain_one_for_the_default_profile_and_per_profile_otherwise` and
  `ship_ctl::tests::profile_arg_reads_the_equals_form_only`.
- **Vertical tabs at start.** 2026-10-22 (owner: "there was a startup bug where it was on vertical tabs but it
  was showing the horizontal style, clicking horizontal then vertical in settings fixed it"): cause OPEN, owner
  decision pending. A likely cause: `side-mode` in `app.slint` is `layout-vertical && has-tabs`, so a vertical
  profile with ZERO tabs draws the top bar on the start screen; Settings is itself a tab, so opening it sets
  `has-tabs` and the sidebar appears. Today that is by design (no look change without the owner's yes: should a
  vertical profile with no tabs show an empty sidebar with its + button?). A profile with saved tabs was checked
  (with and without a saved tab, GPU and software, 100 and 225 percent) and starts vertical. Held, for those
  cases only, by `appctl::tests::a_profile_stored_as_vertical_starts_vertical_on_the_first_frame` and the e2e
  `verticalStart`; `appctl::tests::a_vertical_profile_with_no_tabs_keeps_vertical_but_has_no_tabs_for_the_sidebar`
  pins the zero-tab inputs as they are.
- **Vertical tabs are the default.** 2026-11-11 (Max: "the tab bar should be vertical by default"): a profile
  that has never chosen a Tab position opens with the tabs down the side, the sidebar open at 220 px, and
  Settings shows Vertical. Nothing is written until somebody picks: a stored `horizontal` (picked, or imported
  from Prism Terminal) is kept, and an unknown word reads vertical. Prism Terminal's default stays horizontal,
  so this is a DIFF in `docs/parity.md` and rule 1130. The zero-tab start screen still shows the top bar (the
  entry above). Held by `tab_layout_is_vertical_until_chosen`, `a_stored_horizontal_choice_is_kept`,
  `a_fresh_profile_starts_vertical_with_the_sidebar_open_at_220`, `a_profile_stored_as_horizontal_keeps_the_strip`,
  `tab_position_shows_vertical_on_a_fresh_profile_and_a_stored_horizontal` and the e2e `verticalStart`. Tests and
  e2e scenarios written for the strip now choose Horizontal themselves (the test fixtures, the `--scene` setups,
  `flare_e2e::world::tabs_along_the_top`).
- **Privacy.** `PRIVACY.md` states one automatic request, the update check, and that it is off until the
  repository is public. A new network call without a line in `PRIVACY.md` fails the build
  (`a_new_file_with_a_network_call_fails_the_scan`).

### Decisions the M9 plan records (planner D1 to D16, summarised in `docs/parity.md` as O28 to O43)

The planner decided the mechanics: a compile-time feature for the update switch (D1), an opaque token (D2),
the registry API (D3), the verb path (D4), the write gate (D5), cooperative cancel (D6), the notes from the feed
(D7), the vpk flags (D8), "packaged" by the Velopack layout (D9), the hooks (D10), the preview mode (D11), the
native diagnostics kinds (D12), the `cargo diag` alias (D13), one-click with no licence page (D14), the release
tag `v<version>` with no in-place replacement (D15), and the placeholder icon (D16, superseded by O17).

### Not in this milestone (open)

- The out-of-process `diagLog` e2e belongs to M10. The dictation rows and the dictation engine files are M8,
  which this branch does not contain (the owner moved dictation to the end).
- The installer is not yet run on a clean machine: `update-e2e` installs Flare in CI. (The real install of both
  apps side by side was removed on 2026-11-11, rule 1183.)
- The e2e scenarios updateWindow, updateGuard and updateNotes are in-process here (Task 34), not run out of process.

### Numbers for the M9 PR (measured 2026-10-18 on this PC; release build unless said)

- **Tests.** `cargo nextest run --workspace` on the final tree: 3751 run, 3750 passed, 17 skipped (the hand-run tests). One test failed on the first full pass: `flare::agent_real quit_waits_for_every_agent_shell` (two real shells still dying 10 s after a kill, a timing check on real processes). It passed on two reruns (6 of 6 in its binary), and the file is unchanged on this branch, so it is a load-sensitive test, not a regression from M9. It is open, not retried away.
  Per crate for the M9 areas: flare-update 188, flare-win 226, flare-diag 142 (`cargo nextest list -p <crate>`).
- **Gates.** `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`
  pass. The names test found one unlisted e2e switch (`FLARE_E2E_INSTALL`, a test-harness flag); it is now listed.
- **Cold start.** Ten launches of the release exe with a fresh profile, to the first window handle: median 416 ms
  (range 402 to 975 ms; the first run was the slowest). This is the window, not a usable shell prompt; the
  prompt time is not measured here. M8 has no recorded number on this branch to compare with, so the cost of
  `VelopackApp::run` and the log start is not isolated.
- **Idle memory.** Private bytes after 10 s: 321 MB; after 70 s: 320.8 MB. M8 comparison not available (above).
- **Idle CPU of the watchdog.** The whole process over 60 s with no input: 0.234 percent of one core.
- **`DiagLog::write` on the calling thread.** 100,000 calls in a tight loop: mean 110 ns, median 100 ns, p99
  1.3 us, max 42 us. The queue holds 2000 lines, so 98,000 of the 100,000 were dropped (and counted) in that burst;
  the cost measured is the enqueue, not the disk.
- **Installer (Task 29, recorded on 2026-10-17).** `Flare-win-Setup.exe` 24,800,965 bytes; the full package
  17,230,541 bytes; the delta 59,060 bytes (0.34 percent of the full package, a floor because the build was
  unchanged); the portable zip 17,174,711 bytes. Not re-packed for this PR.
- **Release build time.** `cargo build --release -p flare` took 1 min 52 s here, rebuilding `slint` from source.
- **Not measured in this run (honest gaps).** The usable-prompt time and the M8 comparisons (M8 is not on this
  branch); the sampler and the stall watchdog on a real hang; the installer on a clean machine (CI).

## Parity sweep, the e2e and performance (M10), 2026-10-19

- **Parity is ticked, with the test that holds each line.** Every open line of M1 to M7 and of the harness
  rules is ticked with the test (or the recorded difference) that holds it; the E2E scenario lines name their
  scenario and file. The lines tagged `[M8]` (dictation) stay open on purpose. `docs/parity-ratchet.txt` is 0 and
  `parity_lines.rs` runs strict. The recorded differences the sweep added (DIFF-13 to DIFF-15: the parking spot, the title bar's drag, the tab mark motions) are PROPOSED in the register until the owner approves them.
- **The e2e is ported.** All 70 Prism Terminal scenarios that M10 owns, plus `hookSmoke`, `dpiMatrix` and
  `monitorMove`, run in `tools/e2e` as `flare-e2e --exe <flare.exe>`. The full run on 2026-10-19 (release build,
  nothing else running): every scenario passed, no FAIL, 39 skips (each a recorded skip with its reason, never a
  pass), the clipboard and foreground guards silent, no process left, no new temp folder. The port map is in
  `docs/test-port.md` ("E2E map"), generated by `tools/e2e/src/bin/ledger.rs`.
- **The test hook exists only under `--e2e`.** A named pipe for the current user, first-instance only. Keys go
  through the same `AppCtl::on_key` the real key path ends in.
- **Display scaling.** The eight scales (100 to 300 percent) hold on both renderers in the headless matrix
  (`dpiMatrix`, 587 checks) and in the live window (`monitorMove`, 81 checks, a real `WM_DPICHANGED` and moves
  between monitors of different scalings). The scaling images were read and every scene at every scaling is
  recorded `ok` in `docs/screenshots/m10/dpi/README.md`.
- **The follow-ups are fixed.** Selection survives a redraw; find and count run in slices of at most 8 ms; big
  copies are read on a worker; stroked icons draw at full size outside Settings; the first-run import shares one
  1.5 s budget; the theme change reaches running programs as one report and a one-column nudge; the first frame
  waits only for what it draws; only the faces the terminal needs are loaded; a clean quit on the GPU renderer
  exits 0.
- **The performance bar is measured, and it is not met everywhere.** The verdict and the method are in
  `docs/performance.md`. Missed: the GPU renderer's cold start to a usable prompt (814 ms against 300 ms), its idle
  private working set (76.6 MB against 42.2 MB), and heavy-output throughput: the GPU renderer at 2.48 MB/s against twice Prism Terminal's 2.47 MB/s (4.94 MB/s), and the software renderer at 2.39 MB/s against Prism Terminal's 2.47 MB/s. Met: the software
  renderer's cold start (170 ms), its idle memory, key-to-frame latency, scroll, idle CPU, and the 225 percent
  frame time. The misses are recorded, not waived; the owner decides what happens next.
- **Measured on this PC** (AMD Ryzen 9 9950X3D, RTX 5090, Windows 11 Pro 10.0.26300, 2026-10-19; Prism Terminal
  is the installed 0.58.0): the numbers are in `docs/performance.md` and `docs/perf/`.

## Dictation (M8), 2026-10-20

- **The parity target is Prism Terminal at `e5848cb`** (the owner's decision, 2026-10-18), not the frozen v0.55.6.
  Four later commits changed dictation (#244 the orchestrator, #246 dictate from any app, #248 every app's sessions,
  #249 a link fix). Their dictation behaviour is N1 to N34 in `docs/parity.md`. There is no orchestrator in Flare
  (Q1, DIFF-16): the picker always shows.
- **Off by default.** Switching it on and choosing a model downloads the engine and the model from pinned sources,
  each checked by size and SHA-256 before use. The NVIDIA GPU pack is a separate download you choose. A download
  starts only on a press of Download or Enable, never in a test. On each switch-on Flare links the models Prism already has from
  `%LOCALAPPDATA%\PrismDictation` when one is missing and matches by size and SHA-256 (checked the same way; O14, Q9).
  The page refreshes when one was linked. An overridden store root (`FLARE_DICTATION_ROOT`) links nothing, so an isolated
  profile never takes the user's models. Held by `the_first_switch_on_reuses_the_neighbours_models_and_never_while_off`.
  An uninstalled model is tombstoned (`removed/<id>` in the store root) so the next switch-on does not link it back; asking
  to download it again clears the tombstone. Uninstall waits up to 5 s for a stopping engine to let go of its files and answers
  `Err(Disk)` if one stays held (audit 2026-10-20, L28, L29). Held by
  `a_removed_model_is_not_reused_again_until_it_is_asked_for_again` and `remove_waits_for_a_held_file_and_reports_one_that_stays_held`.
- **The key is Ctrl+Win by default**, hold or toggle, with the solo rules: a chord of modifiers works, and a stored key
  (Right Alt included) is kept (N1 to N3). "Dictate from any app" is on by default once dictation is on, so Ctrl+Win
  also works with another app in front, as in Prism Terminal. Both apps on Ctrl+Win means both widgets open (Q2): turn
  one of them off, or bind another key.
- **The pill and the tab mark in the window; the widget outside.** Outside, the widget shows what you say and lets you
  pick a Claude session by a click or a number from 1 to 9; Esc keeps it unsent. The checked send types into that tab
  and presses Enter once (N22 to N24). Inside the window dictation pastes at the cursor and never presses Enter.
  The press's first key decides which window was in front, so a Flare tab in front gets the words with no picker
  (rule 1149).
- **"Sent to" means sent** (2026-11-11, rule 1149). The send waits until the words show in Claude's prompt (longer
  for a long text, at most 3 s), presses Enter, and waits for the prompt to empty. Only then does the card say "Sent
  to X". Words that never showed say "Not sent: the words did not show in X, copied"; words the Enter did not send say
  "Typed into X, press Enter there". A text so long that the prompt box grows past the rows read still counts as
  shown and is sent (rule 1150). After a refused start or the 5-minute cap the next press starts a recording.
- **Words that cannot be sent are kept** (owner, 2026-10-23, "Yes, build it"). A finished transcript that ends unsent
  (no Claude session, Esc or the 30 s timeout on the picker, a refused send, a copy of the app that has closed) is
  copied to the clipboard and the note says so: "No Claude session, copied", "Not sent, copied", "Not sent: <why>,
  copied". A failed copy keeps the old note. "Cancelled", "Heard nothing", pastes and successful sends copy nothing.
  Rule 186 lists every path and the tests (`SvcEffect::CopyText`).
- **One dictation for every Flare copy.** Copies talk over a local pipe that proves a shared key both ways. The pipe,
  lock and key are Flare's own; Prism and Prism Terminal are not on them (DIFF-20).
- **The engine.** The bundled whisper.cpp engine (pinned, Authenticode-checked runtime DLLs, MIT notice), and Parakeet
  through `flare-parakeet-host.exe` (DIFF-19), a resident host that keeps its model loaded. While dictation is on, the
  engine never stands down by itself (N5, D11; rule 95). Quitting ends every child through a Job Object.
- **Media.** Playback pauses through Windows' media session (never the play/pause key) and resumes only what it paused
  (rule 97). The microphone is the only device Flare opens, only on a press or the Test button; opening Settings never
  opens it. Audio never touches disk and never leaves the machine (rule 91).
- **Voice processing** is Windows' communications processing, not Chromium's echo cancellation (DIFF-18, Q4).
- **Differences, PROPOSED until the owner approves them:** DIFF-16 (no orchestrator), DIFF-17 (the models card wording,
  Q3), DIFF-18 (voice processing), DIFF-19 (native Parakeet host and hook), DIFF-20 (Flare's own link), and DIFF-21 (a
  tab's Claude session across a prompt; the owner decides whether to keep it).
- **Planner decisions D1 to D13** are in `docs/superpowers/plans/2026-10-19-flare-m8.md`: a pure controller with its
  effects run on the UI thread (D1), the key arriving where every key does (D2), the native host and hook (D3, D6),
  ureq for downloads (D4), the zip crate at a release without the advisories (D5), copy names on the link (D8), no
  orchestrator (D9), the fixture cache filled by a command and never by a test (D10), the engine's stand-down
  amendment (D11), and the models card wording (D13).
- **Not machine-tested** is in `docs/dictation-by-hand.md`: the cues, AltGr on a Norwegian keyboard, the Start menu after
  Ctrl+Win, pause against a real player, the widget over a full-screen game and on each monitor, an elevated window in
  front, and a real dictation at 225%.
- **Numbers** (engine start, a Base pass, a Parakeet host pass) are measured by Task 37 on the full e2e run. They are
  not measured at this commit, so this entry gives none yet.

## The close question wears the accent, and a right-click never reaches a program (2026-10-22)

- **The close question's confirm is the solid accent, never red** (owner, 2026-10-22: "this button is miscolored it
  should be just orange on my theme but its orange and red for some reason"). Flare drew "Close tab" with the danger
  fill (`#b4353f`) and an accent ring; Prism Terminal (0.58.3, main 60b6a4a) gives its choices no `danger`, so the
  confirm is `bg-[var(--p-accent)]` with `--p-on-accent` text. Text, sizes and button order already matched. Held by
  `a_tab_hosting_an_agent_asks_naming_it_and_working_for` (appctl: no danger fill) and
  `the_primary_wears_the_accent_or_the_danger_fill` (the component keeps the unused `danger` for a future host).
- **The focus ring is `:focus-visible`: it shows only after the keyboard.** Every PT dialog button carries
  `focus-visible:ring-2 ring-accent-hi`; Chromium rings a programmatically focused button only when the last input was
  a key. So a question opened by a click rests with no ring, one opened by Ctrl+W has it, and Tab or Enter inside
  brings it. Flare's close question, update window and theme-switch question follow it (`by-key` in `dialog.slint`,
  `update.slint`, `settings/wall.slint`). Held by `the_ring_shows_only_after_the_keyboard` and
  `a_dialog_opened_by_a_chord_shows_the_ring_from_the_start`.
- **A right-click always opens Flare's menu and sends nothing to the program** (owner, 2026-10-22: "right click pastes
  in the terminal, it shouldn't ... right click should just open the menu not paste in the input"). Flare never pasted
  by itself: in a mouse-reporting program (Claude Code) it forwarded the right press and release as `CSI < 2 ; x ; y`
  while also opening the menu, and the program pasted. Now the press is kept back in every mode. The old behaviour is
  the opt-in Settings > Terminal > Keys row "Send right clicks to programs" (`flare.window.rightClickReports`, off by
  default). Held by `a_right_click_in_mouse_mode_opens_the_menu_and_sends_nothing`,
  `with_the_setting_on_a_right_press_is_reported_unless_a_selection_stands` (term_ui.rs) and
  `right_clicks_to_programs_is_off_by_default_beside_the_agent_keys` (flare-settings).

## Panels (Surfaces stage 1), 2026-10-21

Plan: `docs/superpowers/plans/2026-10-21-surfaces.md`, stage 1. Decisions and holes: `docs/surfaces/decisions.md` and
`docs/surfaces/lessons.md` (the notes for the Prism Terminal port).

- **A tab can show a panel beside its terminal.** One panel per tab, right of the grid, behind a draggable splitter
  and under a 32 px header (file name, size, close). The panel holds a picture or a text file. The terminal refits
  once, when the splitter is let go (rule 24); the slot's width is a share of the page kept per tab and stored in
  `panels.json` (rules 36, 40, 41). Layout numbers: 40 percent by default, at least 240 css px, the terminal keeps 320,
  at most 70 percent, none under 120 css px of room. Held by `the_slot_matches_the_layout_maths_at_every_scale_width_and_share`
  (all eight scales, five widths, four shares), `the_splitter_follows_the_pointer_clamps_and_the_grid_waits_for_the_release`
  (the grid does not fit while dragging and fits exactly once after), `the_slot_edges_sit_on_whole_device_pixels`, the
  unit tests of `flare-surfaces` `layout.rs` at 100, 150, 225 and 300 percent, and the e2e `panelImage`.
- **A left click on a path link to a picture or a text file opens it in the tab's panel.** The backend resolves the path
  again (rule 13); the file must be on the click allowlist as named and as the file system names it, and both names must be
  of the same previewable kind. Pictures: png, jpg, jpeg, gif, bmp, webp, ico. Text: the types in `preview::TEXT_EXTS`.
  Folders, documents, svg, and every script keep today's behaviour (a script is revealed, never opened or shown; rule 14).
  Held by `a_click_on_a_text_file_shows_it_in_the_panel_and_hands_nothing_to_windows`,
  `a_click_on_a_picture_shows_it_with_its_size`, `a_script_a_document_and_a_folder_never_open_a_panel`,
  `a_picture_named_like_one_that_is_a_script_is_not_shown`, `a_click_on_a_text_file_or_a_picture_asks_for_a_preview_and_a_script_is_still_revealed`,
  `a_link_that_changes_kind_shows_nothing` and `only_a_local_file_of_the_right_kind_is_loadable`.
- **Reviewed by file type (2026-10-24).** Every type tried through the tools and the browser panel (the throwaway e2e
  `panelTypes` on branch `review/panel-types`, files made by hand). Native panel: png, jpg, webp, bmp, ico, gif (first frame only),
  UTF-8 and UTF-16 text, csv and md show; an empty or broken picture says "Flare cannot read this picture." and a binary
  `.txt` "This file is not text."; svg, tiff, pdf, docx, xlsx and sound are refused, now with a hint that a PDF, sound or
  video plays in the browser panel from a local web server. Browser panel: pdf (Edge's viewer), mp3, wav, flac and ogg (the
  player), svg, gif and text show; tiff, csv, docx and xlsx are downloads and get the Save question. Fixed the same day: a
  photo is drawn the way its camera held it (EXIF orientation, `a_photo_is_drawn_the_way_its_camera_held_it`), a picture too
  big for the decoder's memory says it is too large, a sound file no longer keeps the last page's title in the header
  (`a_new_document_with_no_title_does_not_wear_the_last_pages`; its load still reports "did not finish", as WebView2 hands
  the stream to the player), and a file on a mapped network drive is `not_found` for both tools instead of each sending
  the caller to the other (`a_file_on_a_network_drive_is_not_found_by_either_tool`).
- **Setting "Open previews in Flare"** (Settings > Terminal > Links, `flare.window.openPreviews`, default on). Off: the click goes
  to Windows as before. The right-click menu's Open always does. Held by
  `with_previews_off_a_text_file_goes_to_windows_and_turning_them_on_again_shows_it`, `previews_are_on_until_a_stored_zero` and
  `the_previews_row_is_a_switch_that_is_on_until_turned_off`.
- **The picture viewer** fits the slot without enlarging, keeps its shape, and a click shows one image pixel per device
  pixel with scrolling (a second click fits it again); a picture with any transparency sits on a checkerboard. Decoded on a
  worker: 64 MB, 100 megapixels, a side over 8192 px scaled down. **The text viewer** is monospace and read-only with a
  line-number gutter and only the rows in view built; the first 4 MB; an unreadable or binary file shows its reason, never a
  blank. Held by `a_big_picture_fits_the_slot_and_a_click_shows_it_at_one_pixel_to_one_pixel`,
  `a_small_picture_is_shown_as_it_is_and_a_click_changes_nothing`,
  `a_picture_with_alpha_sits_on_a_checkerboard_and_an_opaque_one_does_not`, `a_big_file_draws_only_the_rows_in_view`,
  `the_arrow_keys_the_page_keys_and_home_and_end_scroll_the_text`, `a_picture_that_is_not_one_answers_with_a_reason_in_the_panel`,
  `panel_close_glyph_draws_at_full_size_at_every_scale`, and the `preview.rs` unit tests (caps, NUL, bad UTF-8, long lines).
- **Two chords, in the one table (rule 7):** Ctrl+Shift+P shows or hides the panel, Ctrl+Shift+O moves the keys between the
  terminal and the panel (Esc also returns them). Both are always the app's (with no panel they do nothing and the shell does not
  hear them), and a panel never takes the keys by itself. Held by `the_panel_chords_are_host_chords_and_plain_ctrl_p_and_o_stay_the_shells`,
  `the_close_button_and_the_toggle_chord_close_and_bring_the_panel_back`,
  `the_focus_chord_moves_the_keys_both_ways_and_escape_gives_them_back`,
  `the_keys_return_to_the_terminal_when_the_panel_they_were_in_closes` and `the_panel_chords_are_always_taken_so_the_shell_never_hears_them`.
- **Per tab, stored, and free while closed.** Each tab has its own panel and share; only the front tab's is loaded; a closed
  panel holds no picture and no rows; closing a tab forgets its panel; a session that never opens a panel writes no file and
  starts no thread. A restored tab takes its panel by folder and place. A stored panel whose target is a network path, a
  script, or of another kind is dropped; a kind this build cannot draw yet is kept in the file and not shown. Held by
  `each_tab_has_its_own_panel_and_the_front_tab_decides_what_shows`, `closing_a_tab_forgets_its_panel`,
  `the_splitter_release_stores_the_share_for_that_tab_only`, `a_panel_comes_back_with_its_tab_and_a_closed_one_stays_closed`,
  `a_stored_panel_whose_target_is_not_a_local_file_of_its_kind_is_dropped`,
  `a_panel_of_a_kind_this_stage_cannot_draw_is_kept_in_the_file_and_not_shown`,
  `a_session_that_never_opens_a_panel_writes_no_file_and_starts_no_writer`,
  `an_answer_for_a_file_no_longer_on_show_is_dropped` and the `store.rs` unit tests (damaged files, unknown kinds, widths).
- **One property says an overlay is open.** `overlay-open` is true while any of the 16 overlays is (menu, tab list, dialog,
  update window and notice, Settings page, its dropdown, theme editor and questions, colour picker, Command help, find bar,
  the F12 card, a tab being dragged, and the collapsed sidebar out on a hover when it reaches over the panel's slot, added
  2026-10-24).
  Native panels ignore it; stage 3's web panel hides while it is true. Held by
  `every_overlay_turns_overlay_open_on_and_off_again` (no catch-all arm: a new overlay fails to compile until counted) and
  `overlay_open_is_read_only_for_rust_and_computed_in_the_window_file`.
- **Reviewed and tightened (2026-10-21).** `panels.json` is flushed at logoff, shutdown and quit with the other stores; a click on
  the file already on show reads it again (a rewritten log, a file that failed and now exists); one picture decodes at a time and a
  decode the user has left stops; every load, a restored one included, is vetted like a click; UTF-16 files with a BOM are text; the
  foot names the cap that cut a file and the header shows its real size; the chord leaves a panel of a kind not drawn yet alone.
  Held by `a_session_end_flushes_the_panels_file_too`, `a_click_on_the_file_already_shown_reads_it_again`,
  `a_file_that_was_missing_shows_when_it_exists_and_is_clicked_again`, `a_restored_target_that_fails_the_clicks_checks_is_not_read`,
  `a_utf16_file_with_a_bom_is_text`, `a_file_with_many_lines_is_cut_by_lines_not_bytes`, `a_stale_decode_is_abandoned`,
  `the_keys_return_to_the_terminal_when_the_tab_changes_or_the_close_button_is_pressed`,
  `a_display_scaling_change_with_the_panel_open_keeps_the_layout_and_the_one_to_one_zoom`,
  `every_previewable_type_is_on_the_click_allowlist` and `the_panel_chord_does_nothing_for_a_kind_this_build_cannot_draw`.
- **End to end:** `panelImage` (100 and 225 percent) prints a picture and a text path, clicks them, and checks the slot against
  the layout maths, the painted pixels, the 1:1 zoom, both chords, the splitter, an overlay, the close button and that the
  terminal refits and answers. The `panel.state` probe and the `ui.rect` names `panel`, `splitter`, `panel-body`, `panel-image`
  and `panel-close` are for it.
- **Not built in this stage:** web, changes, tasks and mod panels (later stages); selecting or copying text in the text
  viewer; wrapping long lines; animated GIFs; svg; more than one panel per tab.

## Surfaces: the control channel and the MCP server (stage 2, library part), 2026-10-21

Two new crates and no UI. Nothing in `crates/flare` uses them yet (stage 2 proper wires them after the
panels land). Contract: `docs/surfaces/protocol.md`. Choices: `docs/surfaces/decisions.md`.

- **Decision (stage 2 library, 2026-10-21): one channel for everything that drives a panel.** The UI,
  the agent tools and mods use the same versioned JSON-lines protocol over a per-user named pipe; no
  private back door (design spec, goal 2).
- **S1, owner-only pipe, first instance.** `\\.\pipe\flare-ctl-<hash of SID>`, descriptor
  `D:P(A;;GA;;;<SID>)`, remote clients rejected, created first-instance; a client refuses a server of
  another user. Held by `the_pipe_is_closed_to_other_users`, `a_second_server_on_the_same_name_fails_to_start`,
  `the_pipe_name_is_per_user_and_per_scope` and `this_process_has_a_sid_and_it_matches_by_pid`.
- **S3, the agent cannot type into a shell, change settings or enable mods.** Held by
  `rule_s3_the_agent_cannot_type_into_shells_change_settings_or_enable_mods`,
  `nothing_over_the_pipe_reaches_the_ui_namespace` and `the_agent_cannot_reach_ui_methods_or_eval`.
- **S4 and S5 at the channel.** `browser.eval` needs the user's switch (agent) or `browser` and `eval`
  (mod); a disabled mod's token kills its open connection on the next request. Held by
  `browser_eval_needs_both_browser_and_eval` and `disabling_a_mod_kills_its_open_connection`.
- **Limits.** 16 MB per line, 64 clients (the 65th is told `too_many_clients`), a 10 s hello deadline,
  15 s and 60 s request timeouts, 100/s rate with a burst of 200, an event outbox of 256. Held by
  `the_65th_client_is_refused_and_the_first_64_keep_working`, `a_line_over_the_cap_is_too_long_with_or_without_newline`,
  `an_oversize_line_closes_only_that_client`, `a_slow_call_times_out_and_the_connection_survives`,
  `too_many_stuck_calls_refuse_new_ones`, `the_rate_limit_answers_without_closing`,
  `silence_before_hello_closes_the_client` and `the_outbox_drops_the_oldest_past_its_cap`.
- **A broken client closes only itself.** Not JSON, not a request, or too large closes that connection
  after one error line. Held by `a_malformed_line_closes_only_that_client` (real pipe) and
  `malformed_json_closes_only_this_client`.
- **flare-mcp.** A stdio MCP server (`initialize`, `ping`, `tools/list`, `tools/call`) with the tools
  `show_image`, `show_file`, `close_panel`; outside Flare every tool answers `isError`. Held by the unit
  tests in `crates/flare-mcp/src/lib.rs` and the end-to-end `crates/flare-mcp/tests/stdio.rs`, which runs
  the built exe against a real in-process server (`a_tool_call_reaches_the_server_with_the_tab`,
  `outside_flare_every_tool_says_so_and_the_handshake_still_works`).
- **A cancelled tool call stops (channel-9, 2026-11-09).** `flare-mcp` reads stdin on its own thread and acts on
  `notifications/cancelled` while a call runs: the call gives up, its connection drops and Flare ends the job (a browser job at
  the next timer turn, a mod's open call at once, whose late `tools.respond` gets `not_found`); a queued call never runs. Replies
  keep their order and stdin closing ends the server. Held by `a_cancel_during_a_running_call_ends_it_and_it_gets_no_response`,
  `a_cancel_for_an_unknown_or_finished_request_does_nothing`, `a_cancel_for_a_request_still_queued_means_it_never_runs`,
  `stdin_closing_during_a_call_lets_it_finish_and_ends_the_server_cleanly`,
  `stdin_closed_past_the_grace_cancels_the_call_so_no_thread_hangs` (`flare-mcp`),
  `a_cancel_during_a_call_ends_the_job_on_flares_side_and_the_next_call_still_works` (the exe over a real pipe),
  `a_client_that_goes_away_mid_call_raises_that_calls_cancel_flag` (`flare-ctl`),
  `a_job_whose_caller_went_away_ends_and_the_page_is_free_for_the_next_call` and
  `a_mod_tool_call_ends_when_the_caller_goes_away_and_the_late_answer_is_refused` (`flare`). Regression rule 229.
- **Review hardening (2026-10-21).** The wire layer refuses a `ui` grant from the app and any `ui`
  method (`a_ui_token_is_refused_at_hello`, `a_ui_grant_for_the_agent_is_refused_at_hello`,
  `a_grant_that_turns_into_ui_mid_connection_reaches_nothing`); only timed-out calls count as stuck
  (`slow_calls_inside_their_timeout_are_not_stuck`, `a_stuck_call_stops_counting_when_it_finishes`);
  stop frees a worker blocked writing and waiting calls give way
  (`stop_frees_a_worker_blocked_writing_to_a_client_that_never_reads`,
  `a_waiting_call_gives_way_when_the_server_stops`); a 16 MB line in small pieces is read once
  (`a_huge_line_in_small_pieces_is_not_rescanned`); events never interleave a half-read request
  (`idle_runs_between_messages_but_not_inside_one`); every request takes a rate token
  (`forbidden_unknown_and_hello_requests_spend_rate_tokens_too`); the live pipe carries the owner-only
  descriptor (`the_live_pipe_carries_the_owner_only_descriptor`); the pipe name has known answers for
  the port (`the_pipe_name_has_known_answers`); the hello permissions match the grant
  (`permission_names_agree_with_what_each_grant_can_call`); `flare-mcp` waits the method's timeout plus
  5 s (`the_client_waits_longer_than_the_server_lets_a_call_run`).

## Surfaces: the control channel in Flare and the agent tools (stage 2), 2026-10-21

Wires the library part into the app. Contract: `docs/surfaces/protocol.md`; choices: `docs/surfaces/decisions.md` (Stage 2);
the notes for the port: `docs/surfaces/lessons.md` (Stage 2). Issues #18 and #19.

- **A tab is named by its secret, never by an id (S1, regression rule 186).** Flare makes 64 hex digits from two random UUIDs for
  every tab when it starts its shell and puts them in that shell's environment as `FLARE_TAB_TOKEN`, with `FLARE_CTL_PIPE` and
  `FLARE_MCP_EXE` (rule 12). `hello` takes `tab_token` and refuses a `tab`; an unknown token is `unauthorized`; a closed tab's token
  stops working at once; a restored tab gets a new one every launch. Held by `every_shell_gets_its_own_unguessable_control_token_and_the_pipe`,
  `a_channel_that_did_not_start_leaves_no_control_variable_in_a_new_shell`, `an_unknown_empty_or_partial_token_names_no_tab`, `a_closed_tab_s_token_stops_working_and_a_new_shell_gets_a_fresh_one`,
  `a_warm_spare_belongs_to_no_tab_until_one_adopts_it`, `with_no_channel_no_control_variable_is_set`, `the_secret_compare_looks_at_every_byte`
  (`flare-term`), `the_control_channel_variables_reach_the_shell_and_replace_the_users_own`, `without_a_channel_an_inherited_token_never_passes`
  (`flare-shells`), `an_unknown_tab_token_is_refused_and_closes_the_client`, `a_tab_name_is_refused_because_only_the_secret_names_a_tab`,
  `a_closed_tab_loses_its_connection_s_tab_at_once` (`flare-ctl`) and the e2e `ctlChannel`.
- **One pipe per Flare instance.** The default profile uses the user's plain pipe name; any other profile folder is hashed into it, so
  an e2e run never meets the installed Flare. Every shell is told its pipe. Held by `the_pipe_name_is_per_user_and_per_scope`,
  `the_pipe_name_has_known_answers` and the e2e `ctlChannel` (the scoped name, and the shell's own variables equal the probe's).
- **The server runs in Flare** on worker threads (the UI thread only makes the pipe at start and answers messages), stops first at
  quit inside the quit cap, and every call that needs the window waits at most 10 s on a one-shot reply (rules 28 and 34). Held by
  `an_unanswered_call_times_out_and_a_stop_ends_the_wait_within_a_slice`, `a_message_dropped_by_the_ui_thread_answers_unavailable_at_once`,
  `a_message_after_the_end_of_life_is_dropped_so_its_waiter_lets_go` and the e2e `ctlChannel` (the app quits by itself with a client
  connected).
- **`tabs.list`, `panel.close`, `panel.state`, `view.image`, `view.file`, `view.text`.** `tabs.list` gives the agent and a mod its own tab only.
  `view.image|file` resolve the path again in the backend, require the click allowlist on both names and the right kind, and answer
  `not_found` for anything a probe could learn from (S2, rule 187). `view.text` takes a string (4 MB), shown in the text viewer with
  a title, kept in memory, never stored. A call for a tab that is behind stores its panel and switches nothing. Held by
  `view_image_resolves_vets_and_then_shows_on_the_callers_tab`, `a_path_nobody_may_open_is_not_found_and_never_reaches_the_ui_thread`,
  `the_kind_must_match_and_a_link_that_changes_kind_shows_nothing`, `bad_params_are_named_before_the_ui_is_asked`,
  `view_text_is_capped_and_a_title_is_one_short_line`, `tabs_list_gives_the_agent_its_own_tab_only` (`ctl_app.rs`);
  `view_text_shows_in_the_front_tabs_panel_with_its_title`, `view_file_shows_a_file_under_a_title_and_a_picture_with_its_size`,
  `a_call_for_a_tab_that_is_behind_stores_its_panel_and_never_switches_tabs`, `panel_state_and_close_report_what_is_open`,
  `the_tab_list_has_every_shell_tab_or_only_the_callers`, `a_closed_or_invented_tab_is_no_tab`,
  `an_explicit_ask_shows_even_when_clicks_open_previews_in_windows`, `text_handed_over_is_not_written_to_the_panels_file_and_not_restored`,
  `panel_changes_are_published_to_the_tab_s_clients` (`appctl.rs`); `a_piece_of_text_is_shaped_like_a_file_and_cut_on_a_character_boundary`
  (`preview.rs`).
- **`flare-mcp` ships and Claude gets it with no setup.** Built by the workspace and by its own step in the three packing workflows,
  copied beside `Flare.exe` by `pack.ps1`, required in the portable zip by `verify-pack.ps1`, and named in the bundled `flare-status`
  plugin's `mcpServers` with the absolute path written in at install, or no block when the file is missing (plugin version 1.1.0, rule
  192). Tools: `show_image`, `show_file`, `show_text`, `close_panel`; it reads `FLARE_TAB_TOKEN`, `FLARE_CTL_PIPE` and `FLARE_CTL_PID`
  and connects only to the pipe that process serves. Held by `declares_the_flare_mcp_server_by_a_written_in_absolute_path_and_never_by_expansion`,
  `install_writes_the_mcp_server_only_when_the_program_exists`, `without_the_server_s_pid_the_real_channel_never_connects`,
  `a_pipe_served_by_another_process_is_never_given_the_secret`,
  `has_a_manifest_claude_code_accepts` (`flare-agent`), `show_text_routes_to_view_text_with_its_title`,
  `tools_list_has_the_five_tools_with_object_schemas`, `a_tool_call_reaches_the_server_with_the_tab`,
  `an_unknown_tab_token_is_reported_as_flare_refusing` (`flare-mcp`),
  `the_package_ships_the_mcp_server_beside_the_app_and_every_packing_workflow_builds_it` (`release_files.rs`) and the e2e `mcpRoundTrip`.
- **End to end (the release exe, its own profile, offscreen):** `ctlChannel` (45 checks) reads a tab's secret through the `ctl.info`
  probe and from inside a real shell, connects to the pipe, opens a picture, a text and a file in the panel, reads them back with
  `panel.state`, is refused for a wrong secret, the tab's id, a tab named in `hello`, a missing file, a program, a program named like a
  picture, a network path, a picture through `view.file`, over-long text, `browser.eval` and `shell.input`, stores a behind tab's panel
  without switching, closes, and quits with a client connected. `mcpRoundTrip` (22 checks) runs the built `flare-mcp.exe` as Claude Code
  would (`initialize`, `tools/list`, `tools/call show_text`, `show_image`, `show_file`, `close_panel`) and reads what the window shows.
- **Not built in this stage:** the web panel and `browser.*` (stage 3 and 4), `convo.state` (6), mods and tokens for them (8), `tabs.changed`,
  scrolling `view.file` to a line, syntax colouring for `view.text`, and the by-hand check that Claude Code starts the `flare` server
  from the installed manifest (`/mcp`, see `lessons.md`: a merge gate).
- **Review fixes (2026-10-21, same stage):** a click clears an agent's title and handed-over text; shells get the channel's variables
  only once the server runs, plus `FLARE_CTL_PID` so `flare-mcp` talks only to the app's own pipe (a squatter gets no secret) and a
  refusal's text is cleaned; one deadline per call with the UI dropping a late request; the manifest rendered at install; tests that
  prove what they say (a panels file that must exist, folders told apart by picture size). See `decisions.md` and `lessons.md`.

## Surfaces: the web panel (stage 3), 2026-10-21

A real browser beside the terminal, per tab. Issues #20 (the browser panel) and #21 (video). Choices: `docs/surfaces/decisions.md`
(Stage 3); the notes for the port: `docs/surfaces/lessons.md` (Stage 3); the contract: `docs/surfaces/protocol.md`
(`browser.open`, `browser.navigate`, `panel.open`, the address rules). Engine: Microsoft Edge WebView2 through `webview2-com`,
one controller per tab with an open web panel, parented to Flare's window.

- **Nothing exists until a web panel opens.** No browser object, no process, no handler; the environment is made by the first
  panel and goes with the last controller; a closed panel, a closed tab and Ctrl+Shift+P free the controller (the browser's own
  process ends). Held by `nothing_exists_before_a_web_panel_opens`,
  `a_web_panel_makes_its_controller_at_the_slot_and_frees_it_when_closed`, `closing_a_tab_frees_its_page`,
  `the_toggle_chord_brings_the_page_back_at_its_last_address` (`appctl/tests/web_tests.rs`) and the e2e `webPanel` (it watches
  the controller count, the browser object and the browser process).
- **Quitting closes the browser first (rule 28).** `finish()` closes every web view, answers every waiting `browser.*` call
  `unavailable` and drops the environment before the window hides; nothing opens again afterwards. Held by
  `quitting_closes_every_web_view_before_the_window_goes` (`web_tests.rs`) and the e2e `webPanel`, which watches the browser's
  process end when the window quits.
- **The page sits exactly on the slot.** The controller's rectangle is the box Slint lays out inside the panel body less the slot's
  hairline, in physical pixels, after every layout change, settled splitter drag, scale change and monitor move; at 100 and 225
  percent the controller's own bounds, the child window of Flare's window, Slint's box and `flare_surfaces::web::web_rect` agree.
  Held by `the_page_fills_the_body_less_the_slots_hairline_at_every_scale`,
  `the_page_rectangle_equals_the_layout_maths_at_every_scale_width_and_share`, `the_page_follows_the_splitter_while_it_moves`
  (`ui_tests/panel.rs`), `it_sits_on_the_slot_at_every_display_scale`, `the_controller_follows_the_slot_when_the_splitter_moves`
  (`web_tests.rs`), the unit tests of `web.rs` (`a_logical_rect_rounds_by_its_edges`, `the_web_rect_is_the_slot_less_the_header_and_the_hairline`)
  and the e2e `webPanel`.
- **Airspace.** While any of the 16 overlays is open the browser window is hidden and a still of its last frame (taken before it
  hides) is drawn in its place; it shows again and the still is let go when the overlay closes; a tab behind hides its window and
  keeps its page. Held by `an_overlay_takes_a_still_and_then_hides_the_window`, `a_still_that_cannot_be_taken_still_hides_the_window`,
  `a_late_still_after_the_overlay_closed_is_dropped`, `a_tab_behind_keeps_its_page_but_hides_the_window`,
  `the_still_is_drawn_in_the_page_box_and_only_while_it_is_set`, the phase tests of `web.rs`
  (`an_overlay_takes_a_still_before_it_hides_the_window`, `a_late_still_is_dropped`, `leaving_the_tab_hides_from_any_phase`),
  the existing `overlay-open` term scan, and the e2e `webPanel` (a terminal menu hides the window and the still is 401 by 540 px
  at 100 percent and 900 by 1215 at 225). A page that is busy or hung and never answers the capture is hidden by a 250 ms
  timer with no picture, so it can never stay over a menu or dialog (`a_page_that_never_answers_a_capture_is_hidden_by_the_timeout`,
  `a_capture_that_never_answers_is_ended_by_its_timeout` in `web.rs`).
- **The browser bar.** Back, forward, reload (stop while loading), an address field (Enter opens it; text with no scheme becomes
  https, `localhost` becomes http; `file:`, `javascript:`, `data:` and logins are refused with a dim line at the field's right
  end; Escape restores the page's address and gives the keys back; anything that is not an address, such as `youtube`,
  `cat videos` or `what is rust?`, is a Google search, `https://www.google.com/search?q=...`, and Enter on an empty field does
  nothing; a page whose site cannot be reached says "Could not open <host>" in that dim line until the next load, see
  `docs/superpowers/specs/2026-10-28-address-bar-search.md`) and the panel's close at the row's right end. A web page
  has no header (2026-10-27, Max: "I'd rather have more space for the browser"): the bar is the panel's top and the page starts
  right under it, 32 px taller than before. The page title still names the panel (`web.state`'s `title`) and the tab. A
  mini-app has no bar, so it keeps its header (title and close). Held by
  `a_web_page_has_no_header_and_the_address_row_holds_the_close` (100 to 300 percent), and by
  `the_address_field_opens_what_is_typed`, `the_address_field_refuses_what_a_panel_may_not_open`,
  `the_address_field_searches_google_for_what_is_not_an_address`,
  `a_site_that_cannot_be_reached_is_said_in_the_address_row_until_the_next_load`,
  `the_bar_buttons_drive_the_page_only_when_they_can`, `the_bar_follows_the_page`,
  `a_refused_navigation_is_said_in_the_header_and_leaves_the_page`, `the_address_field_shows_the_page_and_enter_and_escape_do_their_part`,
  `the_bar_buttons_call_back_and_a_dead_one_does_not`, `the_browser_bar_is_part_of_a_web_panels_head_and_of_nothing_else`, and the
  `normalize_url` tests (`text_with_no_scheme_becomes_https`, `a_local_host_with_no_scheme_becomes_http`,
  `every_other_scheme_is_refused`, `bad_text_is_refused`, `a_login_in_the_address_is_refused`), and the `address_or_search`
  tests (`the_address_field_opens_an_address_as_an_address`, `the_address_field_searches_for_anything_else`,
  `the_address_field_still_refuses_a_scheme_and_does_not_search_it`, `a_caller_and_a_stored_panel_never_get_a_search`,
  `a_site_that_cannot_be_reached_is_named_and_the_rest_say_nothing`).
- **Keys.** The page has its keys; the app's chords (the table less Ctrl+F and the agent guard) are taken from it by
  `AcceleratorKeyPressed`; a second Escape within 600 ms gives the keys back; Ctrl+Shift+O moves them both ways; a press on the
  window takes them back. Held by `the_app_takes_its_chords_from_the_page`, `the_page_keeps_every_other_key`,
  `the_hook_takes_only_the_second_escape_and_resets_on_another_key`, `a_held_escape_is_one_press_and_its_repeats_count_for_nothing`,
  `virtual_keys_become_the_keys_the_chord_table_knows`, `the_blur_decision_follows_where_the_keyboard_went`
  (`web_ctl.rs`), `a_chord_the_page_hands_back_is_the_apps`, `the_second_escape_gives_the_keys_back_to_the_terminal`,
  `the_focus_chord_hands_the_keys_to_the_page_and_back`, `a_press_on_the_window_takes_the_keys_back_from_the_page` (`web_tests.rs`).
  Closing a tab, a panel or a failed page that has the keys hands them back to Flare first and takes the window off the screen
  before closing it (`closing_the_tab_whose_page_has_the_keys_hands_them_back_first`). Losing the keyboard to another app
  while the window is still foreground is judged one message later, not at the instant of the kill-focus. By hand: real keys
  in a real page and a real switch to another app (below).
- **What a page may not do.** Permissions are denied (the e2e's geolocation request is answered with the denied code), a plain
  `target=_blank` link opens in the same panel, a pop-up after the person's click opens over the page in the panel (below), one no
  click asked for opens nothing (the e2e counts top-level windows of both processes before and after), only http, https and about:blank open whoever asks (a link to an app is asked about, below, and a page's own `blob:` opens), the profile is the run's own folder and never
  Edge's (`the_profile_lives_under_the_named_profile_or_local_appdata`, `the_profile_is_never_an_edge_folder`). Held by the e2e
  `webPanel` for the page behaviours and the unit tests above for the rest.
- **Pop-ups (2026-11-10).** A page's pop-up (a card's 3-D Secure check, a sign-in window) opens as a real second browser view
  inside the side panel, so it keeps its link back to the page (`window.opener`), its window name and a form's POST. Only after
  the person's own click or key, and only when the request looks like a pop-up: a size or position, a named window, or a blank
  page to write into. A plain `target=_blank` link still opens in the panel; a pop-up no click asked for opens nothing and the
  address row says "Pop-up blocked"; past 2 a tab or 4 in all the next one is refused. The pop-up covers the page in the panel
  under a strip in Flare's own look, "Pop-up from <site>" (the site, never the page's title) with a Close button; the page waits
  hidden behind it. It closes on `window.close()`, Close, a lost process, closing the panel or tab, and quitting, and the page
  shows again; a pop-up from the pop-up stacks on top. It has the panel's profile, saved passwords and autofill, the permission
  denial and the address rule; its downloads are cancelled. The browser tools never reach it: while one is open, click, type,
  press, scroll, navigate and eval answer "a pop-up is open, the person must finish it", a look still reads the page behind, and
  `browser_state` says a pop-up is open; `open_page` answers the same. The bar's Back, Forward, Reload and address leave the page
  behind a pop-up alone and say "Close the pop-up first". Mini-apps and plugin pages still open nothing. Held by regression rules 249 to 251
  (`popup_plan`'s tests in `web.rs`, the `popups.rs` tests, `a_popup_shows_under_flares_strip_and_closing_it_brings_the_page_back`,
  `the_agents_input_calls_wait_for_the_person_while_a_popup_is_open`, `closing_the_tab_takes_its_popups_with_it`, `the_bar_leaves_the_page_behind_a_popup_alone`,
  `a_popup_no_click_asked_for_is_said_in_the_bar`, `the_popup_strip_takes_the_pages_top_and_its_close_calls_back`) and the e2e
  `webPanel`. Spec: `docs/superpowers/specs/2026-11-10-web-popups-design.md`, section 1.
- **Links to apps (2026-11-10).** A link in a web page or a pop-up to an app (`bankid:`, `swish:`, `mailto:`, `tel:`) shows
  Flare's own question instead of being blocked: "Open an app?", "<site> wants to open <scheme> on this PC.", with Open and
  Cancel. The browser never launches the app; Flare does, after Open, with the Windows shell on the bare link and nothing else.
  Cancel is the default: Enter, Space, Escape and a press outside cancel, Open needs a click or Tab then Enter, and does nothing
  in its first 0.3 s. A question that follows another in the open dialog starts on Cancel again, so the Tab given to the
  first never opens the next. The shell opens the app on a worker thread, so a slow handler never freezes the window; if it
  fails, the bar says "The app could not be opened". No click by the person is needed (a bank page opens its app from a
  script). Never asked, cancelled with
  "Not opened here" as before: `javascript:`, `data:`, `file:`, `vbscript:`, `about:`, another site's `blob:`, browser pages and a
  list of Windows handlers that have been used to run code (`ms-msdt`, `search-ms`, `ms-officecmd`, the Office schemes and
  others; `NEVER_SCHEMES`), and anything too long, odd or path-like. An app nothing on the PC opens says "Nothing on this PC
  opens X:". One question per tab, two more waiting, one per site and app every 10 s, 5 a minute in all; a Cancel mutes that
  site and app in the tab until the page is on another site ("App link not opened: asked too often"). While an agent uses the
  page no question appears: the link is not opened, the bar says so, and a waiting call answers "only the person can allow
  opening an app". A page may open a `blob:` it made itself (a receipt) in the panel or its pop-up; the address bar and an agent
  still cannot. The probe of 2026-11-10 (a hidden WebView2 view, the installed Runtime): `NavigationStarting` fires for a link
  to any scheme, registered or not, with or without a click, and cancelling it means no launch at all;
  `LaunchingExternalUriScheme` fires only for a scheme Windows has a handler for and only after a click, and a link in a frame
  raises `FrameNavigationStarting` only. So Flare asks from the navigation and keeps the launch event as a backstop that is
  always cancelled. Held by regression rules 252 and 253 (`app_link_tests` in `web.rs`, the `app_ask.rs` tests,
  `an_app_link_is_asked_about_with_cancel_the_default_and_opened_only_on_open`,
  `while_an_agent_waits_on_the_page_an_app_link_is_never_asked_and_its_call_answered_once`,
  `with_cancel_the_default_enter_space_escape_and_a_press_outside_cancel`). Spec:
  `docs/superpowers/specs/2026-11-10-web-popups-design.md`, section 2.
- **A download is asked about (S6).** WebView2 saves straight into Downloads, so Flare holds every download (`DownloadStarting`
  deferral) behind its own question: "Save this file?", the file name and the host, Save or Cancel. A no, a closed panel, a
  closing window or no answer saves nothing; a second download waits behind the first. Held by
  `a_download_is_asked_about_and_saved_only_on_a_yes`, `a_no_cancels_the_download_and_a_second_one_waits_its_turn`,
  `a_download_of_a_page_that_is_closed_is_not_asked_about` (`web_tests.rs`), `the_question_names_the_file_and_where_it_comes_from`,
  `a_name_cannot_inject_markup_or_run_on`, `a_second_download_waits_behind_the_first` (`download_ask.rs`) and the e2e `webPanel`
  (an `<a download>` asks, nothing reaches Downloads while it is open, and a no saves nothing).
- **A failed page.** Only a lost renderer or a lost browser process ends a page (the GPU, a utility process or a frame restarting
  is only logged): the window comes off the screen, the note shows, and after a lost browser the environment is dropped so the next
  open starts fresh. Held by `only_the_fatal_process_failures_end_a_panel` (`web.rs`), `a_lost_renderer_takes_the_window_off_and_says_so`
  and `a_lost_browser_marks_the_page_and_closing_the_panel_lets_the_next_start_fresh` (`web_tests.rs`).
- **The channel and the tool.** `browser.open`, `browser.navigate` and `panel.open` (web) answer when the page has loaded, bound to
  the browser's own navigation id (the first navigation for the requested address, and a `#fragment` change of the same page is
  answered too); MCP `open_page`. Held by `browser_open_vets_the_address_before_the_ui_hears_of_it`,
  `browser_navigate_takes_a_url_or_an_action`, `panel_open_web_opens_a_page_and_anything_else_is_refused`,
  `what_this_build_can_do_is_listed_and_the_grants_still_apply` (`ctl_app.rs`), `browser_open_answers_when_the_page_has_loaded_and_not_before`,
  `a_load_that_finishes_before_the_calls_own_navigation_is_not_its_answer`, `a_redirect_rebinds_the_wait_to_the_last_navigation`,
  `a_refused_navigation_answers_a_waiting_call`, `a_second_call_replaces_the_first_and_the_first_is_told`,
  `an_answer_after_the_callers_deadline_is_not_sent`, `navigate_sends_the_page_on_and_back_needs_history`,
  `a_tab_behind_answers_at_once_with_what_is_stored`, `the_panel_reports_the_url_and_no_path`,
  `a_navigate_to_a_fragment_of_the_same_page_answers`, `a_stale_load_in_the_queue_does_not_become_the_calls_navigation`
  (`web_tests.rs`), `an_address_matches_itself_in_the_forms_a_browser_reports` (`web.rs`),
  `open_page_routes_to_browser_open_and_reports_what_loaded` and `open_page_needs_a_url_and_a_refusal_reaches_the_model` (`flare-mcp`),
  and the e2e `webPanel`, which does it through a real pipe.
- **Video.** A local clip plays in the panel at the clip's own rate, as a VP8 webm and as an H.264 mp4: the e2e `webVideo` reads
  the clock and frame counter of the page in the parked window (about 1.00 times real time and 30 frames a second). YouTube and the like need no code of ours and are
  by hand (below).
- **No runtime.** `GetAvailableCoreWebView2BrowserVersionString` is asked at every open; when it fails the panel shows why with
  Microsoft's download address and a button that opens it, the channel answers `unavailable` with the address, nothing else is
  touched and the next open asks again. Held by `a_missing_runtime_is_a_message_with_the_link_and_nothing_else_breaks`,
  `a_build_with_no_browser_at_all_says_so_without_a_crash`, `the_download_button_shows_only_for_a_missing_runtime` and the e2e
  `webNoRuntime` (the seam `WEBVIEW_RUNTIME=missing`).
- **Stored.** A web panel is stored with its current address and comes back with its tab; a stored address that is not exactly its
  own normal form is dropped. Held by `a_stored_web_address_comes_back_and_a_bad_stored_address_is_dropped` and
  `set_target_follows_a_page_without_reopening_it`.
- **A question takes the keys from the page (2026-10-24).** The close question and the download question take the Windows
  keyboard focus back from a page that held it (Ctrl+W or a download link pressed in the page), so Enter and Escape answer the
  question. Held by `the_download_question_takes_the_keys_back_from_the_page`. Measured the same day: a menu over the terminal
  hides the page about 25 ms after the right click (the still is taken first), so the part of a menu over the panel can sit
  under the page for one or two frames.
- **End to end (the release exe, its own profile, offscreen):** `webPanel` (108 checks, at 100 and 225 percent, including a change of display scale with the page open), `webVideo` (13, a webm and an mp4) and
  `webNoRuntime` (7).
- **Not built in this stage:** the agent's hands on the page (look, click, type, press, scroll, wait, eval; the ribbon) are
  stage 4; mini-apps and diagrams stage 5; "open this localhost address" offered from the terminal stage 7. By hand, not
  machine-tested: the list at the end of `docs/surfaces/lessons.md` (Stage 3).

## Surfaces: Claude can see and use the browser panel (stage 4), 2026-10-22

Issue #22. The owner's wish: Claude looks around and clicks around in Flare's own browser instead of opening Chrome, like the iOS
simulator MCP on a Mac and Playwright MCP's snapshot of refs. Choices: `docs/surfaces/decisions.md` (Stage 4); the notes for the
port: `docs/surfaces/lessons.md` (Stage 4); the contract: `docs/surfaces/protocol.md` (`browser.*`, the `interrupted` error, the
MCP tools and how a look is written for the model).

- **Look.** `browser.look` / `browser_look` answers with a PNG of the visible area (at the display's own resolution, scaled to at
  most 1600 wide, the real scale reported) and a numbered list of the page: every control and the text worth reading, each with a
  ref (`e12`), role, name, value, state and its place in CSS pixels. Refs last until the next look or page load. Held by the unit
  tests of `flare_surfaces::agent_web` / `agent_job` (the tree trim on synthetic recorded-shape JSON, the cap, hostile trees, the
  rectangles, the screenshot scale at 100 and 225 percent, the PNG header) and the e2e `agentBrowser` (a real PNG of the parked
  window that decodes and has colours, the viewport equal to the browser window, the refs).
- **Click, type, press, scroll, wait, navigate.** By ref or by point (CSS pixels, so any display scale), double and right clicks,
  typing into a field, named keys with modifiers, the wheel over an element or the page, waits for text, a selector or a pause (at
  most 30 s), and navigation (`open_page` / `browser_navigate` open the panel when none is open). Held by `a_click_by_ref_scrolls_finds_the_middle_and_presses`,
  `typing_into_a_ref_focuses_it_then_inserts_the_text`, `a_key_press_sends_down_then_up_in_order`,
  `scrolling_a_ref_wheels_over_it`, `a_wait_for_text_polls_until_it_is_there` (`agent_job.rs`), the parameter
  tests in `ctl_app.rs` (`click_takes_a_ref_or_a_point_and_nothing_else`, `type_press_and_scroll_check_their_params`,
  `a_wait_is_one_thing_for_at_most_thirty_seconds`) and the e2e (the button changes the page's text by ref and by point at 100 and
  225 percent, the double click presses twice, Enter submits a form, Control+a then typing replaces, the list scrolls, a wait for
  text that shows after 700 ms, a selector, a timeout with `met` false, a link click that navigates).
- **A wait cannot run the agent's code.** The only script a wait sends is a fixed template around one JSON string literal. Held by
  `the_wait_checker_is_a_fixed_template_around_one_string_literal`, `a_wait_asks_the_page_with_the_fixed_checker_only`
  (through the controller) and the e2e (a wait text with quotes and code leaves `window.hacked` unset).
- **Stale refs do nothing.** A ref after a page load, a takeover, a new look that dropped it, or for an element that is gone is
  `not_found` ("look again") and nothing reaches the page. Held by `a_ref_is_forgotten_when_the_page_loads_something_new`,
  `a_stale_ref_is_not_found_and_nothing_is_sent`, `an_element_that_vanished_is_not_found` and the e2e.
- **The page in use (no ribbon).** While a call runs and for 1.5 s after (the hold, `AGENT_HOLD_MS`) the agent counts as
  using the page: the edge glow and the cursor with its tag show it, and nothing else is drawn. The orange ribbon "Claude is
  using this page" over the panel's head went with the header (2026-10-27, Max); it had no button of its own (its X was the
  panel's close, now in the address row). `web.state` still says who is using the page (`using`: `shown`, `text`, "Claude is
  using this page"). Held by `the_page_is_in_use_while_the_call_runs_and_the_person_taking_over_ends_the_call`,
  `the_hold_lasts_after_a_call_that_finished_until_the_person_acts`, `the_words_name_who_is_using_the_page` and the e2e (in use
  during a 2.5 s call, still right after, over within 6 s, the browser window's rectangle unchanged throughout).
- **The agent's cursor.** While an agent clicks, types into a ref or scrolls over a ref on the page in front, an orange arrow
  (`#fe8f34`, white rim) with a tag naming the agent ("Claude", "Codex", else "Agent") glides there before the events go
  (350 ms plus 0.5 ms a CSS pixel, at most 700 ms, ease-in-out; the events wait exactly that long), ripples on each press, stays
  while the hold lasts and fades in 200 ms after. It never jumps: after its fade the next action on the same page shows it
  where it rested and glides on. Only the first action on a page, one after a navigation or a change of the page's size, and one
  after a takeover or the page going out of sight show it at the target (held 180 ms so it is seen before the press). It is a
  window of Flare's own above the page, never in it: the page cannot see or hide it and it is in no look. A takeover, a closed
  panel, a tab behind, a menu over the page or a minimised window takes it away at once; it follows the window and the
  splitter. A page that is not live in front has no cursor and no glide. With Windows' animation effects off nothing glides or
  fades: the arrow is placed at once. Held by the `agent_cursor` tests (`flare-surfaces`, `flare-win`),
  `a_click_finds_its_point_then_waits_for_the_glide_then_presses`,
  `the_events_wait_for_the_glide_the_window_says_not_the_longest`,
  `the_cursor_shows_where_a_click_lands_before_it_presses_and_ripples_on_the_press`,
  `after_the_fade_the_next_action_glides_on_from_where_the_arrow_rested`,
  `a_new_page_or_a_takeover_forgets_where_the_arrow_rested`, `with_animation_effects_off_nothing_glides_or_fades`,
  `the_person_taking_over_takes_the_cursor_away_at_once`,
  `a_menu_over_the_page_or_a_tab_in_front_hides_the_cursor_and_a_hidden_page_does_not_wait` and the e2e `agentBrowser` (at the
  button's middle at 100 and 225 percent, gone after the hold, its spot remembered).
- **The agent's glow.** While an agent uses the page in front, from its first call until the hold ends, a soft
  orange glow comes in from the page's edges (2026-10-27, Max: it was a 2 px ring): the cursor's orange at 50 % right at
  each edge, fading to clear 32 CSS px in (eased, scaled with the DPI), corners blended with no seam. It fades with the
  arrow in 200 ms and comes back with the next call. It is four windows of Flare's own above the page, one per edge band
  (the browser is a child window Slint cannot draw over), transparent to every click; the bands are rebuilt only when the
  page's size or scale changes (about 1.2 ms for a 4K page). A takeover, a menu over the page, a tab behind or a closed
  panel takes it away at once; while the person holds the panel's splitter it is off the page and comes back at the new
  size when they let go. Held by `a_light_orange_border_rings_the_page_while_the_agent_uses_it_and_fades_with_the_arrow`,
  `a_menu_over_the_page_takes_the_border_away_and_it_comes_back_after`,
  `a_held_splitter_takes_the_glow_away_while_the_page_holds_its_width_and_it_comes_back_after`, the `glow` tests in
  `flare-win` and the e2e `agentBrowser` (the page's own rectangle while the hold lasts, gone after).
- **The panel drag follows the pointer and the page does not reflow.** Dragging the splitter of a web panel moves the page's
  left edge once per step (one callback for its whole rectangle, not one per side), never rebuilds the agent's glow, and is
  not a takeover of a page the agent is using (2026-10-27, Max: it was laggy and went in steps). The page keeps its width while
  the splitter is held, so it does not lay itself out again at each step and flicker: narrowing, its right part runs past the
  window's edge; widening, the panel's ground shows at its right. It takes the slot's exact size after the width has held
  still 120 ms and when the splitter is let go or the grab goes away (rule 230, 2026-11-09). A window resize still moves the
  page at every step. Held by `a_held_splitter_moves_the_pages_edge_but_never_its_width_until_it_is_let_go`,
  `a_pause_in_a_splitter_drag_fits_the_page_once_and_the_next_step_holds_again`,
  `a_splitter_grab_that_goes_away_leaves_the_page_on_the_slot`,
  `a_panel_closed_during_a_splitter_drag_comes_back_at_the_slots_size`,
  `a_window_resize_moves_the_page_at_every_step_with_no_hold` (`web_tests.rs`) and the `DragHold` tests in `flare-surfaces`.
- **The person keeps control.** A key of theirs in the page, or a mouse button pressed over it, ends the running call with
  `interrupted` and ends the hold (the glow and the cursor go at once); the agent's own key presses are never the app's chords. Held by
  `a_key_of_the_persons_posts_a_takeover_only_while_an_agent_works`, `the_agents_own_key_press_is_neither_the_persons_nor_a_chord`,
  `a_point_is_inside_a_rectangle_by_its_near_edges` and the e2e (a stand-in for the person's input ends a 30 s wait `interrupted`,
  the page is no longer in use, the refs are forgotten; pressing Ctrl+Shift+P as the agent closes nothing). The mouse is read every 16 ms and also
  for a press that came and went between two reads; the page taking the window's focus is a takeover too; a click cut short lets go
  of its button (`a_click_cut_short_lets_go_of_the_button`, `the_person_taking_over_a_click_lets_go_of_the_button`,
  `the_person_focusing_the_page_while_a_call_runs_takes_over`). The real mouse path is by hand.
- **Scripts are off.** `browser.eval` / `browser_eval` need the new setting "Let agents run scripts in pages" (Settings > Terminal
  > Links, default OFF); the grant follows it at every request. Held by `the_agent_grant_follows_the_users_switch_for_scripts`,
  `scripts_for_agents_are_off_until_a_stored_one`, `the_scripts_row_is_a_switch_that_is_off_until_turned_on` and the e2e (refused
  `forbidden` and, through MCP, with the setting's name; a second launch with the switch seeded returns values, awaits promises
  and reports an exception as `bad_params`).
- **Page text is data.** A page cannot end the page-content block, pose as another element or hide words from the model with
  control, direction-override or zero-width characters. Held by `hostile_page_text_cannot_end_the_block_pose_as_an_element_or_hide`,
  `clean_page_keeps_words_and_drops_tricks` (`flare-mcp`), `page_text_is_cleaned_before_it_goes_anywhere` (`agent_web.rs`) and the
  e2e `mcpBrowser` (the page carries a fake end marker and an element name with a line break and a forged `[e99]` line, and a
  title with a quote, a fake end marker and hidden tag characters). One cleaning list serves both layers (`flare_ctl::text`, Unicode
  tag characters, variation selectors and fillers included). The title and address of a navigation answer sit between the markers
  too (`a_hostile_title_or_address_after_a_navigation_stays_inside_the_markers`).
- **A tab that is behind.** An agent call in a tab behind makes that tab's browser hidden and answers "the page is not ready yet"
  until it has loaded, so `open_page` followed by `browser_look` works there (list, no picture) and never loops. Held by
  `an_agent_in_a_tab_behind_gets_a_hidden_page_and_does_not_loop_on_open_page`.
- **Browser zoom.** Clicks land under the panel's own zoom (150 percent in the e2e, by ref and by point).
- **Parked and unfocused.** The agent drives a page the person is not looking at: the e2e window is parked off the screen and never
  foregrounded and a look there returns the real picture (`agentBrowser` decodes it at 100 and 225 percent). A tab that is behind
  keeps its page hidden; a look of it gives the list and no picture (a hidden page paints nothing), answered at once
  (`a_look_of_a_tab_that_is_behind_has_the_list_but_no_picture`, and the e2e).
- **Nothing costs anything while unused.** No browser, no DevTools call, no timer until a call is made; the 16 ms timer runs only
  while a call or its hold lasts, and the window's properties are synced only when a call starts or ends or a hold ends. Held by `nothing_exists_before_a_web_panel_opens` and the timer names in
  `timer_registry`.
- **MCP.** `flare-mcp` lists 13 tools (the five that show things and `browser_look`, `browser_click`, `browser_type`,
  `browser_press`, `browser_scroll`, `browser_wait`, `browser_navigate`, `browser_eval`), each described for an agent. Held by
  `tools_list_has_the_show_tools_then_the_browser_tools_with_object_schemas`, `the_tools_are_listed_with_descriptions_written_for_an_agent`
  (`browser.rs`) and the e2e `mcpBrowser` (initialize, tools/list, a look whose result has an image content item that is a PNG and
  the list as text, then click, type, press, scroll, wait, navigate and eval through stdio).
- **End to end (the release exe, its own profile, offscreen):** `agentBrowser` (141 checks, at 100 and 225 percent, then a launch
  with scripts on) and `mcpBrowser` (26 checks, `flare-mcp.exe` over stdio). See `docs/e2e.md`.
- **By hand, not machine-tested:** a real mouse click (a quick tap too) in the page while a call runs (the polling of the real buttons), a real key,
  the ribbon's look at the 18 themes, a monitor change during a look. The list is at the end of `docs/surfaces/lessons.md` (Stage 4).

## Surfaces: mini-apps and diagrams (stage 5), 2026-10-22

Issues #23 and #24. The owner's wish: when Claude has something to show that is not a picture or a text (a chart of numbers it just
worked out, a calculator, an explainer, a 3D viewer, a diagram of a flow), it writes a small page and Flare shows it beside the
terminal, working, instead of describing it in words. Choices: `docs/surfaces/decisions.md` (Stage 5); the notes for the port:
`docs/surfaces/lessons.md` (Stage 5); the contract: `docs/surfaces/protocol.md` (`view.html`, `view.diagram`, the `app` panel kind,
the MCP tools `show_html` and `show_diagram`); the rule: `docs/regression-rules.md` 199 to 201.

- **A page Claude writes.** `show_html` / `view.html` takes the whole page (`html`, at most 4 MB) or the path of an `.html` file and
  shows it in the tab's panel, answering once the page has loaded. It is a web panel with a different view: the panel has no address
  bar, is named by the title the agent gave (the page cannot rename it), cannot be navigated and is not stored. Held by
  `view_html_writes_the_page_policy_first_and_asks_the_ui_to_show_it`, `view_html_checks_its_params_before_anything_is_written`
  (`ctl_app.rs`), `a_mini_app_gets_a_view_of_its_own_kind_and_answers_when_it_has_loaded`,
  `the_page_cannot_rename_the_panel_or_move_its_address`, `the_address_field_and_the_channel_cannot_send_a_mini_app_anywhere`,
  `a_mini_app_is_never_stored_and_a_stored_app_address_is_dropped_on_restore` (`web_tests.rs`) and the e2e `miniApp` (the page's
  script runs, the header is the agent's title, no bar, the browser window sits exactly on the slot at 100 and 225 percent).
- **A sandbox of its own, with no network.** The page is served from a host of its own (`flare-app-<n>.local`, a site each, so two apps never
  share storage or a `Domain=` cookie), in a private browser profile of its own (`flare-app`, apart from the web panel's, nothing on disk), and the first
  thing in it is a Content-Security-Policy (`default-src 'self' 'unsafe-inline' data: blob:; connect-src 'self' ...`). A fetch, an
  image and a script from outside are each a policy violation (the page's own `securitypolicyviolation` events are the proof, not a
  refused connection), and a script that removes the policy tag lifts nothing. Behind the policy a second wall answers every request
  that is not the app's own with a refusal (the e2e switches the policy off with a seam that exists only under `--e2e` and a
  `no-cors` fetch to outside still gets an answer from Flare, not a connection attempt). Held by
  `the_policy_has_no_network_unless_asked`, `the_policy_comes_before_anything_the_page_says`,
  `a_request_leaves_only_as_far_as_the_network_switch_allows`, `hosts_are_one_per_app_and_never_the_web_panels` (`app.rs`) and the
  e2e `miniApp` (profile name and private flag read back from the browser, the three violations, the tag removed, the wall, the next
  app finding no storage or cookie of the first).
- **The network is two keys.** `allow_network` from the agent AND the new setting "Let mini apps use the network" (Settings >
  Terminal > Links, default OFF, read at every request); the answer says what the page got, `show_html` tells the agent when it asked
  and did not get it, and plain `http:` is never allowed. Held by `the_network_needs_the_agent_to_ask_and_the_users_switch_to_be_on`
  (`ctl_app.rs`), `mini_apps_have_no_network_until_a_stored_one`, `the_mini_app_network_row_is_a_switch_that_is_off_until_turned_on`
  (`flare-settings`), `the_network_the_agent_asked_for_is_reported_as_given_or_not` (`flare-mcp`) and the e2e `miniApp` (a second
  launch with the switch seeded: the policy allows `https:` and still blocks `http:`).
- **It goes nowhere.** A navigation to anything but its own page is cancelled and the header says "Not opened here"; a link and
  `window.open` open nothing, not even in the browser panel; developer tools and the status bar are off; permissions are denied.
  Held by `a_view_stays_on_its_own_page` (`app.rs`), `an_app_view_and_a_page_view_replace_each_other` (`flare-web`) and the e2e `miniApp`
  (`location.href` to outside and to a sibling file, and `window.open`, leave one view at its own address).
- **A page named by path is vetted.** Resolved again by the backend (S2): only `html` and `htm`, as typed and as the file system
  names it; a script, a text file, a missing file, a folder, a share or a `..` escape is the same `not_found`; at most 4 MB; UTF-8.
  A click on a path still never opens a page (`html` is not on the click allowlist), and files next to the page are not served. Held by
  `a_page_for_a_mini_app_is_html_or_htm_and_nothing_else`, `a_page_resolves_in_the_backend_and_a_script_a_folder_or_a_share_does_not`
  (`path_open.rs`), `a_page_named_by_path_is_resolved_again_vetted_and_read` (`ctl_app.rs`) and the e2e `miniApp`.
- **Folders live and die with the panel.** `<profile>\apps\a<n>\index.html`, written off the UI thread before the request; removed on
  a worker when the panel closes, another panel replaces the app, the tab closes, the request is too late or the tab is gone;
  renamed aside at start and swept on a worker, emptied at quit; only a folder directly inside the apps root is ever removed. Held by
  `an_app_is_written_to_a_folder_of_its_own_and_removed_with_it`, `only_a_folder_directly_inside_the_apps_root_is_ever_removed`,
  `startup_empties_the_apps_folder_and_leaves_the_profile_alone`, `retiring_the_root_renames_it_aside_and_the_sweep_deletes_every_old_one`, `the_page_loses_peer_connections_and_prefetch_before_its_own_code`, `a_file_name_with_a_separator_is_refused_and_leaves_nothing_outside`
  (`app.rs`), `closing_a_mini_app_ends_it_and_removes_its_folder`, `a_second_mini_app_replaces_the_first_and_its_folder`,
  `a_mini_app_for_a_closed_tab_is_refused_and_its_folder_removed`, `a_mini_app_asked_for_too_late_is_never_shown` (`web_tests.rs`) and
  the e2e (a stale folder gone at start, one folder at a time, none after `panel.close`).
- **Updating in place.** A second `show_html`, a diagram, a `view.*` or `open_page` swaps the page in the same browser (the process
  id does not change), and the other way round. Held by `a_page_replaces_a_mini_app_and_a_mini_app_replaces_a_page`,
  `a_second_mini_app_replaces_the_first_and_its_folder`, `a_mini_app_for_a_tab_behind_gets_its_view_hidden` and the e2e (the browser
  process id before and after).
- **Diagrams.** `show_diagram` / `view.diagram` draws Mermaid text with the bundled build (11.17.2, MIT, compiled into the exe; never
  fetched), `securityLevel: 'strict'`, no network, and answers once it has drawn. A diagram that does not parse fails the call with
  Mermaid's own error text (up to 800 characters and its line breaks) and the panel shows it in red; the MCP tool says what to do
  next. Held by `the_diagram_text_cannot_end_its_element`, `a_drawn_diagram_is_ok_and_an_error_carries_mermaids_words`,
  `an_error_is_plain_short_text` (`app.rs`), `view_diagram_writes_the_page_and_the_library_beside_it`, `view_diagram_checks_its_params`,
  `the_bundled_mermaid_is_the_pinned_build` (`ctl_app.rs`), `a_diagram_is_answered_by_the_pages_word_and_a_drawing_that_failed_says_why`,
  `a_diagram_whose_page_failed_closed_or_moved_on_answers_unavailable` (`web_tests.rs`),
  `a_diagram_that_did_not_draw_returns_mermaids_own_words` (`flare-mcp`) and the e2e `diagram` (a flowchart with its four nodes
  and labels, a sequence diagram with its actors, a bad diagram refused with the parser's text that the panel shows too) and
  `mcpDiagram` (through `flare-mcp.exe`).
- **The agent can look at its own page.** The `browser_*` tools work on a mini-app as on any web panel of the tab (it sees its chart
  and clicks in it); `browser_navigate` with an address is `unavailable`. Held by `the_agents_browser_calls_work_on_a_mini_app`
  (`web_tests.rs`) and the e2e (`browser.look` finds the page's button, `browser.click` changes its text; through MCP the same).
- **A click by ref does not press what is covered.** Right before the press the page is asked what is at the point; when something else
  is on top nothing is pressed, and the agent is told what covers the element instead of "Clicked". The target's own child, a label of
  it and what wraps it count as the target; a click by point has no target and is not asked about. Held by
  `a_covered_click_presses_nothing_and_names_what_is_on_top`, `a_click_on_the_target_or_a_child_of_it_is_pressed`,
  `a_page_that_cannot_answer_the_check_is_clicked_as_before` (`agent_job.rs`), `a_covered_click_is_refused_and_presses_nothing`
  (`web_tests.rs`), `a_hostile_covering_element_cannot_end_the_block_or_pose_as_an_element` (`browser.rs`) and the e2e `agentCoveredClick`.
- **MCP.** `flare-mcp` lists 15 tools (the seven that show things and the eight browser tools); `show_html` and `show_diagram` are
  described for an agent: what they are for, the 4 MB and 200 KB caps, no network by default and what `allow_network` needs, that
  Mermaid errors come back. Held by `tools_list_has_the_show_tools_then_the_browser_tools_with_object_schemas`,
  `show_html_tells_the_agent_what_the_page_can_and_cannot_do`, `show_html_routes_html_or_a_path_and_never_both`,
  `show_diagram_routes_to_view_diagram` and the e2e `mcpDiagram` and `mcpRoundTrip` (the tool list).
- **Nothing costs anything while unused.** No folder, no browser view and no profile until the first mini-app; the profile is
  created with its first view and keeps nothing on disk. The mini-apps' own browser (apart from the web panel's, one kind for apps
  with no network and one for apps with it) is made with the first app of its kind and closed with the last; while one is open it is
  a second set of browser processes, with the memory and the extra start-up time that means.
- **No peer connections, three ways.** The page loses `RTCPeerConnection` and the other RTC classes before its own code in every
  frame (the script is awaited before the page loads; a failure fails the view), the mini-apps' browser is started with the WebRTC
  policy `disable_non_proxied_udp` (and, with no network, a proxy that is not there; with it, Flare's filtering proxy, issue #97), and the page policy shuts child frames
  (`frame-src 'none'; child-src 'none'; worker-src 'self' blob:`). `show_html` tells the agent exactly what is and is not blocked.
  Held by the e2e `miniApp` "webrtc" checks (script and policy both off by seams: a loopback UDP socket as STUN server hears nothing
  from the window or from a srcdoc or about:blank frame, for an app with no network and for one with it, and does hear the web panel
  as the control; a WebSocket to a loopback listener is stopped for a sealed app and, since issue #97, for a networked one too), the frame
  probe in `app_on` (no usable `RTCPeerConnection` in a srcdoc or about:blank frame, at once and later) and the unit tests named in
  rule 199. Decision: `docs/surfaces/decisions.md` (stage 5 second review).
- **End to end (the release exe, its own profile, offscreen):** `miniApp` (92 checks, at 100 and 225 percent, then a launch with the
  policy off by the seam and one with the network switch on), `diagram` (18) and `mcpDiagram` (10). The existing `webPanel`,
  `webVideo`, `webNoRuntime`, `agentBrowser`, `mcpBrowser`, `ctlChannel` and `mcpRoundTrip` still pass. See `docs/e2e.md`.
- **By hand, not machine-tested:** the list at the end of `docs/surfaces/lessons.md` (Stage 5): a mini-app in a tab behind, an
  overlay over it, the light and dark system themes, a real Claude session, the network setting against a real https address, a
  4 MB page.

- Mini-apps (2026-10-22): the sandbox also refuses to start when a Windows policy overrides the browser's arguments, launches no external program, and no longer wipes the shared cookie jar; the tool text says storage lasts until the last mini-app panel closes.
## Surfaces: the conversation reader library (stage 6, library part), 2026-10-21

The pure-Rust part of the reader: `crates/flare-convo`, no UI. Format notes for Prism Terminal: `docs/surfaces/convo.md`; choices: `docs/surfaces/decisions.md` (Stage 6 library); the notes for the port: `docs/surfaces/lessons.md` (Stage 6 library). Issues #25, #26 and #27 (library parts; the Changes panel is the next part).

- **The transcript path comes from the session id and the tab folder.** `<home>\.claude\projects\<slug>\<session-id>.jsonl`, with the slug from `flare_agent::resume::encode_cwd`. An id with a separator, a dot or a space is refused. A fallback finds `<session-id>.jsonl` in any project folder. Held by `transcript_path_uses_the_slug_and_refuses_an_unsafe_id` and `the_fallback_finds_the_session_under_any_project_folder` (`flare-convo`).
- **The tail reads what was appended, bounded and never whole.** One poll reads at most 4 MB from its offset and returns only complete lines; a partial last line waits for its newline; a line longer than 8 MB is dropped and counted (`skipped_long`). Held by `reads_only_what_was_appended_since_the_last_poll`, `a_partial_last_line_waits_for_its_newline`, `a_bounded_read_says_there_is_more_and_the_rest_comes_next`, `a_line_longer_than_the_cap_is_skipped_and_counted_and_the_next_line_reads`.
- **A truncated or replaced transcript starts again from its first line.** Shorter than the offset, or a new file (volume serial and file index from the open handle; NTFS keeps the creation time across a quick delete, so that is not used). The consumer clears its model when a batch says `restarted`. Held by `a_truncated_file_restarts_from_zero` and `a_replaced_file_restarts_even_when_it_is_longer`.
- **The tail worker polls every 500 ms only while someone listens.** Dropping the subscription stops it. Held by `the_worker_delivers_growth_and_stops_when_the_subscription_drops` and `a_subscription_with_nothing_new_sends_nothing`.
- **The fold reads edits and the named files, and tolerates anything it does not know.** Edit, MultiEdit, Write and NotebookEdit make edits (with the time and whether the result failed); absolute Windows paths in the assistant's words are the named files, images flagged. A subagent's lines are tagged. A bad line is counted and skipped. TodoWrite, TaskCreate and TaskUpdate were read into a task list for the Tasks panel; since that panel was removed (2026-10-26) they are tools the fold ignores (`the_task_tools_fold_into_nothing`). Held by the `flare-convo` fold tests (`an_edit_is_listed_with_its_path_time_and_applied_result`, `multiedit_keeps_each_entry_as_its_own_change`, `write_is_a_whole_file_and_diffs_as_all_added`, `a_failed_edit_is_kept_and_marked_failed`, `named_files_are_absolute_windows_paths_with_an_extension_once_each`, `counts_every_line_and_the_bad_ones`, `an_unknown_type_or_a_non_object_never_panics_or_changes_the_model`).
- **An edit can be shown as a diff of its own text (library only).** A line diff of old to new, three lines of context, capped at 2000 lines and marked when cut; a whole new file shows as all added. No git is run. Held by the `diff` tests (`one_replaced_line_with_context_on_both_sides`, `far_apart_changes_make_two_hunks`, `output_is_capped_and_says_so`, `a_pair_too_big_for_the_table_is_all_removed_then_added`).
- **A subagent's transcript is read from its own file.** Its lines are in `<slug>\<session-id>\subagents\agent-<id>.jsonl`, so `tail::subscribe_session` tails the session file and every `agent-*.jsonl` beside it, each batch naming its file, and every line of it is a sidechain line. Held by `a_session_worker_reads_the_session_file_and_every_subagent_file_by_name`, `a_subagent_file_folds_as_sidechain` and `subagent_edits_arrive_from_their_own_file` (review fix, 2026-10-21).
- **The reader is bounded.** The worker's queue holds two batches (a slow consumer parks the reads); the model keeps 2000 edits and 32 MB of edit text, with 64 KB per stored text. A rewrite in place that ends longer restarts (the last 64 bytes are compared). Held by `a_worker_blocked_on_a_full_queue_still_stops_when_the_subscription_drops`, `a_file_rewritten_in_place_that_ends_longer_restarts`, `the_oldest_edits_go_once_there_are_too_many_and_a_late_result_is_ignored` and `the_kept_edit_text_is_bounded_in_bytes_too` (review fix, 2026-10-21).
- **In the app now:** the Changes panel and `convo.state` are the next section (stage 6).

## Surfaces: the Changes panel (stage 6; the Tasks panel was removed), 2026-10-22

What Claude did, shown beside its terminal: the files it edited with each edit as a diff. (A second panel, Tasks, showed its task list until it was removed on 2026-10-26: see below.) Issues #25, #26 and #27. The library is above; choices: `docs/surfaces/decisions.md` (Stage 6); the holes and the notes for Prism Terminal: `docs/surfaces/lessons.md` (Stage 6); the method: `docs/surfaces/protocol.md` (`convo.state`, `panel.open`).

- **Nothing runs unless a panel is open.** A tab gets one transcript tracker only while its Changes panel is open AND the hooks have named its Claude session; closing the panel, switching it to a file, losing the session or closing the tab stops it. A tab with a session and no panel has no thread, no timer and no read. Held by `a_tab_with_a_session_and_no_panel_runs_nothing`, `closing_the_panel_stops_the_tracker_and_reopening_starts_it_again`, `closing_the_tab_stops_its_tracker` (`ui_tests/agent_flows/convo_flows.rs`) and by the e2e `convoPanels`.
- **The folding and the diffing run on a worker.** The tracker follows the session file and its subagents' files, folds them and makes the rows; the window only wraps finished rows (rule 34). A new session (a `/clear`, a resume) switches the file and forgets the pick; a replaced or cut transcript is read again from its start; a transcript that is not at the tab's folder is found in any project folder. Claude's folder is `CLAUDE_CONFIG_DIR` (absolute only), else `%USERPROFILE%\.claude`. Held by `a_new_session_switches_the_file_and_forgets_the_pick`, `the_changes_panel_lists_the_edits_and_follows_the_file_as_it_grows`, and in `flare-convo` by `a_replaced_transcript_starts_the_model_again`, `a_transcript_in_another_project_folder_is_found`, `a_transcript_that_appears_later_is_picked_up`, `dropping_the_tracker_stops_the_worker` and the `claude_dir` tests.
- **Changes lists the edited files, newest first, and one click shows a file's edits as a diff.** Each row has the name, the folder, "3 edits" and a red ", 1 failed". The diff has a header per edit, context rows, removed rows and added rows tinted with the theme's terminal red and green, two line-number columns and monospace text; "Files" (or Backspace) goes back and the path opens the file in the panel (the stage 1 preview, re-resolved and allowlisted: a program does nothing). Only the rows in view are built, so 100 000 rows build about 40. Held by `ui_tests/convo.rs` (`a_big_file_list_builds_only_the_rows_in_view`, `a_diff_of_a_hundred_thousand_rows_builds_only_the_rows_in_view_and_scrolls`, `added_and_removed_rows_wear_the_themes_inks_and_context_rows_wear_none`, `a_click_on_a_file_picks_it_and_the_bar_goes_back_or_opens_the_path`, `backspace_goes_back_from_a_diff_and_the_arrows_do_not_scroll_an_empty_one`) at 100 and 225 percent, by `a_failed_edit_is_marked_in_the_list`, `a_click_on_the_path_in_a_diff_shows_the_file_and_a_program_shows_nothing`, and in `flare-convo` by the `view` tests (`files_list_the_newest_first_and_count_each_edit_once`, `a_diff_marks_each_edit_and_numbers_its_lines`, `a_huge_diff_is_cut_at_the_row_cap_with_a_note`).
- **The Tasks panel: REMOVED (owner, 2026-10-26: sessions never used it).** It showed the agent's TodoWrite and TaskCreate list from the transcript (states, the one in progress highlighted, subagents' lists as groups that folded), opened from the terminal menu ("Show task list") or `panel.open` kind `tasks`. Gone entirely: the panel kind, the `TasksView` in `convo.slint`, the menu row and its `menu-tasks` glyph, the task list in `flare-convo` (the fold, the view's groups and counts), the task counts in `convo.state`, the Tasks part of the `convoPanels` end to end and their tests. A `panels.json` an older build wrote with a `tasks` panel reads it as an unknown kind and drops that one panel, the tab and its neighbours stay (rule 40; `a_stored_tasks_panel_is_dropped_on_load`, `a_stored_panel_of_the_removed_tasks_kind_is_dropped_and_the_tab_restores`); `panel.open` kind `tasks` is `bad_params`. The Changes panel is untouched.
- **Empty states say what is missing, in plain words.** "Claude is not running in this tab yet...", "Reading the conversation...", "Waiting for the conversation file.", "Claude has not changed a file yet." Held by `a_panel_with_no_session_says_so_and_starts_nothing`, `an_empty_session_says_nothing_has_changed_yet` and `the_empty_words_say_what_is_missing`.
- **How to open it.** The terminal's right-click menu on a tab that runs Claude gains "Show files Claude changed" ("Hide ..." while open; reworded 2026-10-22, see "Owner feedback, round 9"); Claude opens its own with `panel.open` kind `changes`. A plain shell's menu does not change. No chords were added (the shared chord table is the owner's call). Held by `the_terminal_menu_offers_the_changed_files_only_on_a_claude_tab` and `panel_open_takes_the_conversation_kinds_and_nothing_more`.
- **It is stored and restored like the other panels.** A stored `changes` panel reopens empty with the "not running" words and fills when the session is known; one with a wrong target is dropped. Held by `a_stored_conversation_panel_reopens_empty_until_the_transcript_loads` and `a_stored_panel_of_a_kind_with_the_wrong_target_is_dropped`.
- **`convo.state` gives the agent its own tab's summary.** Files, edits, failures and the 20 newest changed files; no contents (the task counts went with the Tasks panel, 2026-10-26). A tab Flare is not reading answers `tracking: false` and says how to have it read. Held by `convo_state_summarises_the_tabs_own_conversation`, `convo_state_asks_for_the_callers_own_tab_only` (`ctl_app.rs`) and the e2e `convoPanels`.
- **End to end (the release exe, its own profile and Claude folder, offscreen):** `convoPanels`, at 100 and 225 percent, with a synthetic transcript: no tracker before a panel, `panel.open` kind `changes`, the file listed, a real click on its row showing removed and added rows, an appended edit updating it, `panel.open` kind `tasks` refused, a new session switching the file, and `panel.close` stopping the tracker. See `docs/e2e.md`.
- **By hand, not machine-tested:** a real Claude Code session's `tool_result` shapes (the fixtures are synthetic), the look in all 18 themes, a transcript of hundreds of megabytes. The list is at the end of `docs/surfaces/lessons.md` (Stage 6).
- **Review fixes (2026-10-22).** A diff line is cut at 400 characters with an ellipsis on the worker; an update rebuilds only the list or the diff that changed (an edit of another file leaves the open diff's rows alone); a path answer that arrives after the panel changed is dropped; the search for a missing transcript backs off to every 30 s. Tests: `view::a_very_long_line_is_cut_to_the_row_width_with_an_ellipsis`, `convo_ctl::a_capped_diff_gives_five_thousand_rows_no_wider_than_the_viewport`, `convo_flows::an_edit_of_another_file_leaves_the_selected_diff_rows_alone`, `convo_flows::a_path_answer_that_arrives_after_the_panel_changed_is_dropped`, `convo_flows::closing_the_tab_stops_its_tracker` (the worker really exits). (A fix for wrapped task rows went with the Tasks panel, 2026-10-26.)

## Surfaces: live preview and question notifications (stage 7; the agent board was removed), 2026-10-22

Two small things that make several agents and dev servers easy to live with (the third, the agent board, is gone: see below). Issues #28 and #30 (#29, the board, closed as not wanted). Choices: `docs/surfaces/decisions.md` (Stage 7); the holes and the notes for Prism Terminal: `docs/surfaces/lessons.md` (Stage 7); the rules: `docs/regression-rules.md` 202 to 204 and 206 (205 is retired).

- **A "Preview" chip for a local dev server.** When a tab prints `http://localhost:5173/`, `http://127.0.0.1:3000/app` or `http://[::1]:8080` (https too, any path), a small pill ("Preview localhost:5173") appears in the top right of the terminal once something listens on that port; a click opens the tab's web panel at that server's ORIGIN (`http://localhost:5173/`; whatever path or query the program printed is dropped, and the tooltip shows the origin). The chip needs the listener to belong to the tab's own process tree (the shell and what it started), so a port another program holds, or a server in WSL, gets no chip. A click looks at the port once more first and opens the page only if the same server is still there; a stranger taking the port drops the chip at once, and a tab that comes to the front shows its chip only after a fresh look. It goes when the listener is gone (two failed looks in a row), when the screen is cleared, when the tab closes, and while the panel already shows that page; one chip per tab, for the newest address. A server stopped and started again on the same port prints the same address and gets its chip back. A chip is looked at again only in the tab in front and not while the window is minimised. Only these three local hosts ever qualify (`localhost.evil.com`, `localhost@evil.com`, `0.0.0.0`, a LAN address and every remote name never do). The address is read off the terminal's own live screen (the history is never read, nor the row the cursor is on, which is what you are typing), in any tab, so a server started in a tab you left is found, and the port is looked up in the system's listener table, IPv4 and IPv6, never by connecting. Setting: **Offer previews of local servers** (Settings > Terminal > Links, on). Tests: `flare_term::dev_server` (11: the three hosts, ports, paths, every refusal, the clear watch across every chunk cut and on the alternate screen), the emulator's reading of the screen (`the_emulator_reads_a_local_server_address_off_its_screen`, `the_row_the_cursor_is_on_is_input_and_never_read`, `a_suggestion_painted_after_the_cursor_and_typed_over_is_not_read`, `an_address_that_scrolled_off_the_screen_is_still_the_one_held`, `a_cleared_screen_forgets_the_address_though_the_history_still_holds_it`, `a_long_address_wrapped_over_rows_is_read_whole`, `nothing_is_read_on_the_alternate_screen_and_no_remote_address_ever`, `a_reading_asked_for_too_soon_is_owed_not_lost`, `the_same_address_printed_again_moves_reprint_not_seq`, `a_full_scrollback_does_not_hide_a_newer_address`, `a_full_screen_program_clearing_its_own_screen_keeps_the_address`, `reading_a_huge_screen_full_of_addresses_stays_cheap`), `flare_surfaces::devpreview` (17, a scripted listening table; restart, give-up then reprint, reprint while live, front tab only), `a_server_started_again_on_the_same_port_gets_its_chip_back` and `another_tabs_steady_output_never_holds_back_a_chip` (flow), `flare_win::tcp_owner` (the table on both families), the flow tests in `ui_tests/agent_flows/board_flows.rs` (`a_local_address_offers_a_chip_once_its_port_listens`, `clicking_the_chip_opens_the_web_panel_at_that_address`, `the_chip_goes_when_the_process_that_printed_it_is_gone`, `clearing_the_screen_takes_the_chip_at_once`, `the_newest_address_is_the_only_chip_of_a_tab`, `an_address_that_is_not_local_never_gets_a_chip`, `the_chip_belongs_to_the_tab_in_front`, `the_setting_turns_the_chips_off_and_on`), `ui_tests::board` (the chip whole in the corner and clickable at 100 and 225 percent) and the e2e `devPreview`.
- **The agent board: REMOVED (owner, 2026-10-22: "this is basically useless remove it").** It was Ctrl+Shift+A, a menu row and `panel.open` kind `board`: a panel listing every tab with an agent and its state. Gone entirely: the panel kind, `board.slint`, the chord (Ctrl+Shift+A is the shell's again), the menu row, the `flare-surfaces` `board` module, the hub's state-start times, the `agentBoard` end to end and their tests. A `panels.json` an older build wrote with a `board` panel reads it as an unknown kind and drops that one panel (rule 40; `a_stored_agent_board_panel_is_dropped_on_load`); `panel.open` kind `board` is `bad_params`. The Preview chip and the question notifications are untouched. Issue #29 is closed as not wanted.
- **A Windows notification when an agent asks.** A question from an agent in a tab out of sight, or in a window without the keyboard, shows a toast with the tab's name and the question's text (or "Claude has a question for you."); clicking it brings the window up and that tab to the front. One per question; none while you are looking at that tab; a question you saw is never announced later. Setting: **Notify me when an agent asks** (Settings > Agents, off by default since 2026-10-24). The toast is shown under Flare's AppUserModelID (the installed app's shortcut carries it) and carries no button or action; when Windows would not take the request the taskbar button flashes instead. Tests: `flare_surfaces::notify` (15: hidden tab, unfocused window, looking, grace, de-dup, setting, clean text), `flare_win::toast` (the XML cannot become markup), `a_question_in_a_tab_out_of_sight_notifies_once_with_the_tabs_name_and_text`, `a_question_without_its_text_waits_out_the_grace_and_says_who_asks`, `nothing_while_the_person_is_looking_at_that_tab`, `an_unfocused_window_notifies_even_for_the_tab_in_front`, `coming_back_to_the_window_before_the_grace_ends_cancels_it`, `the_setting_turns_the_notification_off`, `by_default_a_question_shows_no_notification`, `a_second_question_in_the_same_tab_is_a_second_notification`, `a_click_on_the_notification_brings_the_tab_to_the_front`, `a_notification_windows_would_not_take_flashes_the_taskbar_instead` and the e2e `questionToast` (the recorder, `ToastClick`).
- **End to end (the release exe, its own profile, offscreen):** `devPreview` (at 100 and 225 percent) and `questionToast`. See `docs/e2e.md`.
- **By hand, not machine-tested:** a real toast on the installed build, real dev-server banners, the chip in all themes. The list is at the end of `docs/surfaces/lessons.md` (Stage 7).
## Mods, the library part (Surfaces stage 8), 2026-10-22

- A mod is a folder under `%APPDATA%\Flare\mods\<id>\` with a `mod.json` that declares its permissions (`panel`, `view`, `browser`, `convo`, `tools`) and, optionally, a panel (an HTML file in the folder) and the tools the agent can call. The library reads, checks and routes them; no screen shows them yet. The contract is `docs/surfaces/mods.md`.
- Held by the `crates/flare-mods` unit tests (run on every change): every manifest rule and its refusal message, the unknown-key and device-name refusals, the size caps, path escapes (`..`, drives, UNC, `:` streams), the folder-name and id rules, discovery bounds (64 folders, per-folder errors, a junction out of the folder), the enabled store round trip, a damaged file, an unknown entry and a newer version (read-only), a failed save that puts the state back, the token lifecycle (minted, replaced, revoked, never in `Debug`), the grant a token maps to, tool names and clashes, and the sample mod (`docs/surfaces/sample-mod/notes`) being valid and making no network reference.
- Not yet: Settings > Mods, the `window.flare` bridge in a mod's page, the `tool` events, the answer to `tools.respond` and the `flare-mcp` routing (the rest of stage 8). No e2e scenario yet, because nothing on screen changed.
- Review fixes, 2026-10-22, held by unit tests: a mod's page is served per request through `paths::serve` (a junction out of the folder and a file swapped in after the scan are refused on the request; the serving rule is written in `mods.md`); the agent name is at most 52 characters (a 40 and 40 name is `TooLong`, left out); `mint` refuses `eval` and `mod_grant` drops it; a damaged `mods.json` is moved to `.bad` and a newer version is read-only; `input_schema.type` must be `object`; nested missing fields name their place; hidden Unicode format characters are refused in text a person reads; junk folders do not use up the 64-folder cap and an entry past the bound is an error.

## Mods, the app part (Surfaces stage 8), 2026-10-22

Issue #31. Choices: `docs/surfaces/decisions.md` (Stage 8, the app part); the holes and the notes for Prism Terminal: `docs/surfaces/lessons.md` (Stage 8, the app part); the contract: `docs/surfaces/mods.md`; the methods: `docs/surfaces/protocol.md` (`tools.respond`, `mods.tools`, `mods.call`, `panel.open` kind `mod:<id>`, the `mods.changed` event).

- **Settings > Mods.** A seventh Settings page lists each mod found in `%APPDATA%\Flare\mods` with its name and version, its description, a switch (off until you turn it on), what it may do in plain words ("Show its panel in a tab, Answer the agent's tool calls", shown BEFORE you turn it on) and a "Show panel" button; a folder that is not a valid mod is a warning with the reason (`mod.json is not valid JSON: ...`), tools left out for a name clash or length are listed, and "Open mods folder" opens the folder. The folder is scanned at start and each time the page opens, on a worker. Turning a mod on makes its token (memory only) at once and saves the switch on a worker; turning it off kills the token at once (a page's next call is refused, a waiting tool call fails, its panel closes, a connection with its token is refused on its next request). Tests: `flare_settings::page::mods` (11), `ui_tests::settings_mods` (8, at 100 and 225 percent: the rail, the words before turning on, the switch and buttons reaching the port, warnings, whole device pixels whatever the length of the words), `appctl::tests::mods_tests::a_mod_is_off_until_turned_on_and_the_switch_mints_and_kills_its_token_and_tools`, `a_switch_that_cannot_be_saved_is_said_on_the_page_and_still_takes_effect`, the hub tests in `flare_mods::hub` (19).
- **A mod's panel** is a web panel at `https://flare-mod-<id>.local/`, in a browser profile of its own (`flare-mod-<id>`, kept between runs) inside the sealed browser the mini-apps use (no network, no WebRTC, no name resolves). Its files are served one request at a time (checked against the folder every time, so a junction in it serves nothing outside it; a header policy on every answer; a navigation anywhere but its own address and every new window are refused). The `window.flare` bridge is injected before the page's own scripts: `on('tool', fn)`, `respond(callId, result, error?)`, `call(method, params)`, frozen, with no token in the page. Tests: `flare_mods::serve` (6), `flare_mods::bridge` (5), `flare_surfaces::modview` (5), `a_mods_panel_is_a_web_panel_at_its_own_address_with_its_own_kind_title_and_no_address_bar`, `the_page_cannot_rename_its_panel_or_move_it_and_the_mod_is_not_stored`.
- **Grants are enforced on the page's calls.** A call from a page is checked against the mod's live permissions and runs through the same dispatcher as the pipe: a mod without `browser` is refused every `browser.*` (`forbidden`, before anything runs), one without `view` refused `view.*`, a turned-off mod is `unauthorized`, `tools.respond` and `hello` are not sent from a page, 16 calls at once at most. A mod's own browser panel is not built (a mod with `browser` is answered `unavailable`; recorded in decisions.md). Tests: `mods_ctl` (14), `a_page_call_is_held_to_the_mods_permissions_and_a_mod_without_browser_is_refused_browser`, `an_allowed_call_goes_through_the_dispatcher_with_the_mods_grant_and_the_answer_reaches_the_page`, `a_call_made_after_the_mod_is_turned_off_is_refused_unauthorized`, `flare_ctl::grant` (`a_mod_without_browser_is_refused_every_browser_method_with_forbidden`).
- **The agent calls a mod's tools.** `flare-mcp`'s `tools/list` ends with the enabled mods' tools as `mod_<id>_<tool>` (`mod_notes_append_note`), asked of Flare at listing time; a call opens the mod's panel in the calling tab if it is not showing, hands the call to the page and returns what the page answers (30 s at most, then `timeout` naming the mod and the tool). A mod's own error comes back as the tool's error text with its reason. When the set of tools changes `flare-mcp` sends `notifications/tools/list_changed`. Tests: `flare-mcp` (6 new unit tests and the real binary over a real pipe, `a_mods_tools_are_listed_called_and_announced_when_they_change`), `a_tool_call_opens_the_mods_panel_in_the_callers_tab_waits_for_the_page_and_returns_its_answer`, `a_call_to_a_page_that_is_already_up_is_handed_over_at_once`, `a_mod_cannot_answer_a_call_that_was_routed_to_another`, `closing_the_panel_the_page_dying_or_the_mod_going_off_fails_the_waiting_call`, `a_tool_call_for_a_tool_that_is_not_offered_is_not_found_and_reaches_no_page`, `the_browser_tools_do_not_drive_a_mods_page`, `panel_open_mod_opens_the_panel_only_for_a_mod_that_is_on_and_has_one`.
- **The sample mod works end to end.** `docs/surfaces/sample-mod/notes`: turn it on, the agent calls `mod_notes_append_note`, the Notes panel shows the line. End to end (the release exe, its own profile with the sample copied into its mods folder, offscreen): `modsSample` (63 checks): the page lists the sample off with its words and the broken folder with its reason; the switch turns it on (tool offered, `mods.json` holds the switch and no token); `flare-mcp` lists the tool; two calls put two lines in the panel; the page is at its own address in its own non-private profile, cannot reach an outside address or replace `window.flare`, is refused `browser.open`, `view.text`, `convo.state` and `settings.set`, loads a second page and an image of its own folder; turning it off removes the tool, closes the panel and frees the browser view, and a call is refused; "Show panel" opens the panel in the shell tab; a restart keeps the switch and the page's notes; a manifest edited on disk to ask for `browser` turns the mod off at the next look (tool gone, the page says why) until it is switched on again, which approves the new list (rule 212). See `docs/e2e.md`.
- **By hand, not machine-tested:** a real Claude Code session using a mod's tool and re-listing, a junction in an installed mod's folder, the page in every theme. The list is at the end of `docs/surfaces/lessons.md` (Stage 8, the app part).

## Surfaces: owner feedback, round 9 (2026-10-22)

The owner used the build and said: "whats the agent board i dont get the new right click menu options ... and why cant i view in sidebar as an option. also the sidebar with the preview of a file isnt super great its a bit buggy sometimes when moving the sidepanel to grow it or shrink it will bug out, and im not sure if text wraps or disappears". Then, of the board: "this is basically useless remove it". Choices: `docs/surfaces/decisions.md` (Round 9); what the splitter did wrong and what fixed it: `docs/surfaces/lessons.md` (Round 9); the rules: `docs/regression-rules.md` 215 to 219.

- **The agent board is removed** (see the stage 7 section above for the list and the test that holds a stored one is dropped).
- **The terminal menu says what its Surfaces row does.** On a tab that runs Claude, after a rule (its own group, a wider gap with a hairline), the row is "Show files Claude changed" ("Hide files Claude changed" while open; the row shows only on a tab that is Claude, by session or detection, so the wording never varies), with its own glyph in the menu glyph set (`menu-changes`, a page with a plus and a minus). A tab with no agent does not get it (it would be empty). A second row, "Show task list" with a checklist glyph, went with the Tasks panel (2026-10-26). Tests: `the_terminal_menu_offers_the_changed_files_only_on_a_claude_tab`, `glyph_paths_match_the_slint_file`.
- **"Show in side panel" on a path's menu.** A right-click on a path that is a picture or a text file (the kinds a left click would preview, vetted the same way: the backend resolves the path again and both names must be of the same previewable kind) adds "Show in side panel" right after "Open". It is an explicit ask, so it works whatever "Open previews in Flare" says about plain clicks. A script, a program, an svg, a pdf, a folder and a page do not get the row. Tests: `show_in_side_panel_follows_open_for_a_file_the_panel_draws`, `show_in_side_panel_is_offered_only_where_a_left_click_would_preview`, `show_in_side_panel_on_a_text_file_menu_opens_the_preview_and_opens_nothing_in_windows`, `show_in_side_panel_works_even_when_plain_clicks_open_files_in_windows`, `show_in_side_panel_on_a_picture_menu_and_never_for_a_script_or_a_folder`.
- **Long lines wrap in the text preview.** Before, a long line ran off the right edge and looked as if it disappeared. Now the rows are wrapped to the columns that fit (after a space or a dash in the last quarter of a row, else a hard cut), the line-number gutter shows a number on a line's first row only, the header says "N lines" (lines, not rows), and only the rows in view are built. The wrap is LIVE (2026-10-23, owner: "make that dynamic so it wraps as you move it, thats how other programs work like notepad, even flare in general if you make the window smaller things wrap"): the rows wrap to the current width at every step of a splitter drag and of a window resize, the line at the top of the view stays at the top, and the exact final wrap follows the last step. A file under 200 KB of shaped text is wrapped where the step lands (a refresh during the move wraps live as well, never the exact way) (about 4 ms measured in a debug build for a 1000-line, 100 KB file, less in release); a bigger one on a worker, one wrap in flight, whose answer wraps again to the newest width, so a drag is never stalled. The terminal grid still fits once, at the release (rule 24). A button in the panel header (a bent-line glyph, left of the close button) and the setting **Wrap long lines in previews** (Settings > Terminal > Links, `flare.window.wrapPreviews`, default on) flip it; off, the rows are the lines and the view scrolls sideways as before. A file over 1 MB is wrapped on a worker while the panel says "Opening". Tests: `flare_surfaces::wrap` (5), `ui_tests::panel_wrap` (8, at 100 and 225 percent: a 500 character line takes several rows, the gutter numbers first rows only, nothing past the right edge, 300 000 rows stay virtual, the button), `a_long_line_wraps_by_default_and_the_header_button_turns_it_off_and_on`, `a_splitter_drag_wraps_live_at_every_step_and_the_release_changes_nothing_more`, `a_live_drag_keeps_the_line_at_the_top_in_place`, `a_big_file_drag_never_stalls_the_ui_and_ends_exact`, `a_small_file_wrap_per_step_is_cheap`, `narrowing_the_slot_mid_drag_rewraps_at_once_so_the_text_never_overflows_or_is_cut`, the e2e `panelSplitter` (rows re-wrap mid-drag), `a_big_file_wraps_on_a_worker_and_a_stale_answer_changes_nothing`, `the_wrap_row_is_a_switch_that_is_on_until_turned_off`.
- **The splitter, tested the way a hand drags it.** `appctl::tests::splitter_tests` (headless, through the real controller, 100 and 225 percent, a picture and a text file) and the end to end `panelSplitter` (the release exe, GPU renderer, real pointer events and window pictures) drag in many steps, quick and slow, past both limits and back, and check after every step that the slot and the splitter stay inside the limits and on the slot's edge, the slot follows the pointer to the device pixel and never against it, the grid is held and not fitted during the drag and fitted exactly once on the release, the panel's content is on the frame, nothing odd shows in the gap, and the grab is let go when the pointer leaves the window. What was wrong and what fixed it: `docs/surfaces/lessons.md` (Round 9).

- **Review fixes (round 9, 2026-10-22).** A re-wrap keeps the line at the top of the view (and a view scrolled sideways returns to the left edge when wrap turns on); wrapping counts display width (CJK and emoji take two columns); while the splitter is held a narrowing slot lets the wrapped rows overflow sideways instead of cutting them; a tab switched by chord during a drag lets go of the grab; a page in front (Settings) no longer unwraps and re-wraps the file; "Show in side panel" on a file that went away or is no longer showable says "Cannot show this file" in the panel and still opens nothing in Windows; PRIVACY.md no longer describes the removed board; the e2e scenario file `board.rs` is `live_preview.rs` (group `preview`). The owner's "bug out when growing or shrinking" was not reproduced: a short screen recording would settle it.

## Plugins phase 1: badges and progress on Claude's own tab, 2026-10-25

The first phase of `docs/superpowers/plans/2026-10-25-plugin-system.md` (design: `docs/superpowers/specs/2026-10-25-plugin-system-design.md`,
sections 5.1, 5.2, 8 and 13). No plugins yet: Claude puts a badge or a progress on the tab it runs in, and the drawing
every later phase uses is built. The methods: `docs/surfaces/protocol.md` (`tabs.badge.*`, `tabs.progress.*`).

- **What shows.** An outlined pill just before the tab's end in the strip (left of the ring on a sidebar row) holds a
  progress meter (a static fill from 0 to 1, or four dots when the amount is unknown) and badge glyphs (phone, globe,
  bell, mic, eye, moon, lock, bolt, star, plus, pin, 14 px, in the tab's ink, an accent or a muted tone). At most 2
  chips: 2 on a named tab of 150 css px or more, 1 from 110, none below or on an icon tab; the name narrows by what the
  pill takes. Never at the tab's foot (the agent's bar mark stays whole) and nothing animates. Every piece is a line in
  the tab's tooltip (`Claude: Review 50%`, `Claude: Call-me is on`); the pill has no pointer area, so a drag still
  starts on it. Tests: `ui_tests::decor` (5: the pill's place at 100, 150 and 200 percent, the name giving way, no
  room and icon tabs, the bar under it untouched, the sidebar row), `tabs_ui` (3), `menus_ui` (the glyph pin test and
  `every_built_in_badge_has_a_glyph_and_nothing_else_does`).
- **The model.** `flare_tabs::decor`: badges keyed by who set them and a key (replace by key, clear by key, clear by
  source, 4 keys a tab and 200 in all per source), one progress a tab, which chips a tab draws for its width, and the
  tooltip lines. Flat fields in the Slint row (`chip1-cmd`, `chip1-tone`, `chip2-cmd`, `chip2-tone`, `chip-count`,
  `prog`, `chip-room`), so the row diff stays one compare. Tests: `flare_tabs::decor` (11).
- **The channel.** A `Decor` namespace with `tabs.badge.set|clear` and `tabs.progress.set|clear`, a `decorate`
  permission (the agent holds it; a `mod.json` cannot ask for it yet), for the caller's own tab only. Key, icon, tone,
  tip (the manifest's clean-text rule, 80 characters) and value are checked on the channel's worker, and each change is
  spent from 10 a second per tab and caller there, before the window is asked. Tests: `flare_ctl::grant` (incl.
  `a_grant_with_no_permission_may_call_only_hello_and_tabs_list`), `flare_ctl::decor`, `flare_ctl::method`,
  `ctl_app` (4: own tab only, bad params named, what each op carries, the rate), `flare_mods::manifest`.
- **Cleared with the session.** The agent hub reports a tab whose Claude session changed from one id to another or
  whose agent left; the window clears the agent's pieces on those and on close. Tests:
  `agent_hub::the_agent_decor_goes_when_the_session_changes_or_the_agent_leaves`,
  `appctl::tests::a_badge_and_a_progress_from_the_channel_reach_the_drawn_row`,
  `appctl::tests::the_agent_decor_goes_with_its_session_its_agent_and_its_tab`.
- **The agent's tools.** `flare-mcp` lists 19 tools: the 15 before, then `set_tab_badge {key, icon, tip?}`,
  `clear_tab_badge {key}`, `set_tab_progress {value?, label?}` and `clear_tab_progress`. Tests: `flare-mcp` (3 new,
  and the counts in `tests/stdio.rs`), the e2e tool list in `mcpRoundTrip`.
- **By hand (step 9 of the plan, the owner):** in a NEW Claude session after the install (a running session keeps its
  tool list), `set_tab_badge phone` and `set_tab_progress 0.5` show in the strip and the sidebar.

## Plugins phase 2: the protocol 2 manifest, the plugin permissions, Settings > Plugins, 2026-10-25

The second phase of `docs/superpowers/plans/2026-10-25-plugin-system.md` (design sections 3, 4, 9 and 13). A plugin is
a folder with a `plugin.json` that says protocol 2; a protocol 1 mod (`mod.json`) reads and behaves exactly as before.
The keys of later phases are checked now and run later. The contract: `docs/surfaces/plugins.md`, "The manifest".

- **The manifest.** `plugin.json` adds `author`, `homepage` (`https://` only), `background`, `bar {entry, height 24 to
  160}`, `contributes {sidebar (2), actions (8, places tab-menu, tab-hover, top-bar, session-bar), icons (32 SVG paths
  of path characters, 2 KB)}` and `helper {command (.exe in the folder), args (16 of 200)}`; each key goes with its
  permission both ways, and unknown keys are refused at every level. `mod.json` must say protocol 1 and `plugin.json`
  protocol 2; a protocol 2 key in a `mod.json` is refused with a pointer to `plugin.json`. Tests: `flare_mods::manifest`
  (the plugin vectors, one per key), `flare_mods::paths` (the helper's `.exe` rule).
- **Two folders, one id space.** `<profile>\plugins\` is read after `<profile>\mods\` (either file in either); an id in
  both is refused in `plugins\`; every file a manifest names must be a real file in the folder. Tests:
  `flare_mods::discover` (incl. `an_id_in_both_folders_is_refused_in_the_plugins_folder` and
  `the_sample_plugins_are_valid`, which loads `sample-plugins/`). Replaced in phase 2c: one folder, `plugins\`.
- **The permissions.** `tabs`, `decorate`, `sidebar`, `actions`, `bar`, `sessions`, `input`, `network` and `process`,
  askable only from protocol 2 (`ASKABLE_V1` is the old five). In `flare-ctl` each new method family has its own
  namespace (`Sidebar`, `Actions`, `Bar`, `Session`, `Input`) opened by exactly its permission, nothing rides on the open
  `Tabs` one, `network`, `process` and `tabs` open no method, and the agent enters none of them. `tabs` is to widen the
  tab a call may name (`Grant::may_act_on_any_tab`, defined and tested); nothing calls it yet, so every call still acts
  on the caller's own tab (it is wired with the background pages). Their methods come with their phases. Tests:
  `flare_ctl::grant` (`each_namespace_is_opened_by_exactly_its_own_permission`,
  `a_grant_with_no_permission_may_call_only_hello_and_tabs_list` and four more).
- **The approval.** The switch store stays `mods.json` (version 1): a plugin's entry adds `author`, `homepage`, `source`
  (once install exists) and `hash` (`sha256:` over the folder's files, made on the save worker); protocol 1 entries are
  written byte for byte as before. A plugin whose author or homepage changes is turned off until approved again, as one
  that asks for more is. A plugin whose tools only a background page could answer offers none yet (a note says so).
  Tests: `flare_mods::enabled`, `flare_mods::hash`, `flare_mods::hub` (the plugin tests).
- **Settings > Plugins.** The page keeps its key `mods` and is titled Plugins, with an Open plugins folder row above the
  mods folder row (phase 2c: the plugins folder row only). A protocol 1 mod's card is unchanged; a plugin's adds "Made by" (author, homepage), its permissions in
  plain words, and a warnings row: "Full control" for `process` and `input`, the combined notes for `input`+`sessions`,
  `network`+`sessions`, `network`+`tabs`, and `network`'s own. Tests: `flare_settings::page::mods`,
  `ui_tests::settings_mods` (at 100 and 225 percent), `mods_ctl::the_page_info_of_a_plugin_has_its_maker_and_warnings_and_a_mod_has_none`.
- **Not built in this phase** (the plan's later phases): background pages, a call naming another tab under `tabs`, the
  sidebar sections and actions, the bar, the helper and install from a zip or a URL. Their manifest keys are parsed and validated only.

## Plugins phase 2b: views keyed by slot, 2026-10-25

Phase 2b of `docs/superpowers/plans/2026-10-25-plugin-system.md` (design 13): a refactor that the background pages and
the bar build on. Nothing the person sees changes, except that a mod's page can no longer show an `alert`, `confirm` or
`prompt` dialog (design 13: no script dialogs in any plugin view).

- **Slots.** `flare_surfaces::slot::Slot {owner, kind}`, kind `Panel` (owner a tab), `Bar` (owner a tab) or
  `Background` (owner a plugin id). `flare_web::WebHost` keys its views, held downloads, file reads and callbacks by
  slot, with a generation per slot (kept after a close, so a late callback of a slot's earlier view is let go).
  `WebBackend` gained slot methods (`open_view`, `close_view`, `set_view_bounds`, `set_view_visible`, `post_to_view`,
  `probe_view`); the tab methods act on the tab's panel and behave as before. An event of a bar or background view
  comes as `WebEvent::View {slot, event}`, so code that knows only panels never mistakes it for a tab's. A bar or
  background slot takes only a mod's or plugin's page. `ModCtl` keys its pages by slot too (a bare tab id means its
  panel), a reply names its slot, and a background page's call has no tab. The app makes only panels so far. Tests:
  `flare_surfaces::slot`, `flare_web::backend` (the three slot tests with the fake), `mods_ctl::pages_are_kept_by_slot_and_a_background_page_calls_with_no_tab`.
- **One seal.** `WebHost::seal_plugin_view` puts up every wall of a mod's or plugin's view from `flare_web::seal::plugin_seal(kind)`:
  no script dialog (dismissed at once), no window of its own, no permission, no external program, no developer tools
  or status bar, frames of its own host only; a bar and a background page also have no context menu, no browser keys
  and no download (cancelled at once), while a panel keeps its menu, keys and the download question as before. Tests:
  `flare_web::seal` (one per kind) and `every_kind_of_plugin_view_is_sealed_as_its_kind_says`.
- **The plugins' own browser.** Mods and plugins run in a WebView2 environment of their own (`EnvKind::Plugin`), apart
  from the web panel's and the mini-apps'. It keeps the folder the mods' profiles were always in
  (`<profile>\WebView2-app-sealed`, `modview::env_folder`), so every `flare-mod-<id>` profile and its storage stay where
  they were; the sealed mini-apps, which keep nothing on disk, moved to `WebView2-mini-sealed`. Test:
  `the_plugins_browser_keeps_the_mods_folder_and_shares_it_with_nobody`.

## Plugins phase 2c: one plugins folder, 2026-10-25

Asked for by the owner after phases 2 and 2b: every plugin, protocol 1 (`mod.json`) and protocol 2 (`plugin.json`)
alike, lives in one folder, `<profile>\plugins\`. The contract: `docs/surfaces/mods.md`, "Where mods live and when they
run".

- **One folder.** Flare reads only `<profile>\plugins\` (`flare_mods::discover(dir)`; the `Root` of a found mod and the
  second root are gone). Its refusals name "the plugins folder" ("not a plugin id", "a link to somewhere outside the
  plugins folder"). Tests: `flare_mods::discover` (incl.
  `mods_and_plugins_share_the_one_folder_and_the_old_mods_folder_is_not_read`).
- **The move.** The first scan (on the scan worker, before it reads) moves every folder of `<profile>\mods\` whose name
  is a legal id and that `plugins\` does not hold into `plugins\` by a rename. A name `plugins\` already holds stays in
  `mods\` and is a "Not loaded" row in Settings > Plugins naming it in one sentence; a folder whose name is not an id
  is left alone; an emptied `mods\` is removed, else it keeps what could not move. It runs once per start, so nothing
  is moved twice. The switch, approval (`mods.json`, name unchanged) and storage (`flare-mod-<id>`) are by id, so a
  moved mod keeps them. Tests: `flare_mods::migrate` (5: moved and the empty folder removed, a clash kept and named, a
  non-id folder and loose files left alone, nothing moved twice, the plugins folder made only when something moves),
  `flare_mods::hub::the_first_scan_moves_the_old_mods_in_and_a_moved_mod_keeps_its_switch`,
  `mods_ctl::the_page_info_of_a_plugin_has_its_maker_and_warnings_and_a_mod_has_none`.
- **Settings > Plugins.** One folder row, `plugins-folder`, "Open plugins folder"; the mods folder row and every
  "mods folder" word are gone. The scan worker makes the folder at start and each time the page opens (the old mods
  folder is never made). Tests: `flare_settings::page::mods::the_one_folder_row_comes_first_opens_the_plugins_folder_and_shows_its_path`,
  `ui_tests::settings_mods::the_plugins_folder_button_opens_the_plugins_folder`,
  `mods_ctl::a_scan_runs_on_a_worker_and_a_second_ask_runs_after_it`.
- **e2e.** `modsSample` copies the sample into the profile's `plugins\` and checks the `plugins-folder` row; its other
  expectations are unchanged.

## Plugins phase 3: background pages, events, any tab, decor for plugins, 2026-10-25

The third phase of `docs/superpowers/plans/2026-10-25-plugin-system.md` (design sections 5.1, 6, 10 and 13). The
contract: `docs/surfaces/plugins.md`, "Background pages" and "Events", and `docs/surfaces/protocol.md` (the tab scope, the
wide `tabs.list`, decor for plugins).

- **Background pages.** A protocol 2 plugin that is on and has `background` gets a hidden view of it, tied to no tab,
  in the plugins' browser and its own profile, sealed as a background view and never focused (`WebCtl::open_background`,
  `AppCtl::sync_backgrounds`); at most 6 on at once (the seventh switch is refused with the reason); 3 restarts in 5
  minutes, then "Stopped" under its switch in Settings; a 10 s ping with no answer is an end; safe mode after two
  unclean starts in a row with plugins on, or `--no-plugins`, or Shift at launch (`plugin_safe`). Tests:
  `appctl::tests::mods_tests` (`a_plugin_with_a_background_page_gets_one_hidden_sealed_view_tied_to_no_tab_while_it_is_on`,
  `a_background_page_that_ends_is_started_again_three_times_in_five_minutes_then_stays_stopped`,
  `a_background_page_that_does_not_answer_the_ping_in_ten_seconds_is_closed_and_counts_as_ended`),
  `flare_mods::hub` (the cap, the generation, safe mode, stopped), `plugin_safe` (2), `flare_settings::page::mods`
  (`a_stopped_background_page_and_safe_mode_are_said_in_warning_rows`), `config` (`--no-plugins`).
- **Tools in the background page.** A plugin with a background page answers its tools there; no panel opens. Protocol 1
  and plugins without one keep the panel routing (the e2e `modsSample` is unchanged). Tests:
  `a_tool_of_a_plugin_with_a_background_page_is_answered_there_and_no_panel_opens`,
  `flare_mods::hub::tools_of_a_plugin_with_a_background_page_are_answered_by_it`.
- **Events.** The bridge's `on()` takes `tool`, `tabs`, `agent` and `plugin`, and answers the host's ping itself; after
  each refresh the window compares the tabs with what the plugins last heard (`tab_events`: opened, closed, focused,
  renamed, moved, panel, and agent kind, state and session) and hands each change to the pages that listen and may see
  the tab, at most 32 a page a round. Tests: `flare_mods::bridge` (2 new), `tab_events_say_what_changed_and_only_that`,
  `events_reach_every_page_that_listens_and_may_see_the_tab`.
- **Any tab, decor and the wide list.** With `tabs` a call names any tab by its public id (never a secret); a background
  page needs `tabs` to name one. Plugins decorate as `plugin:<id>`, with their own `contributes.icons` (at most 256
  commands, numbers within +-1000) drawn in the chip, one badge drawn per plugin per tab, 30 changes a second per plugin;
  everything a plugin drew goes when it is turned off or its background page ends. Tests: `ctl_app` (4 new: the scope,
  the 30 a second, the wide list, the own panel on a named tab),
  `a_plugin_with_tabs_lists_and_decorates_any_tab_by_its_public_id_and_its_decor_goes_when_it_stops`,
  `tabs_ui::a_plugins_badge_draws_its_own_icon_and_says_its_name`, `flare_tabs::decor` (2 new),
  `flare_mods::manifest` (the icon limits).
- **The budget.** `flare_mods::limit`: one limiter per plugin shared by its pages, 50 messages a second (burst 100), 2 MB a
  second, 256 KB a message; the browser glue drops a page's message over it before copying it, the window spends it, a
  pipe connection spends it in the dispatcher. Tests: `flare_mods::limit` (4),
  `a_plugins_pages_share_one_budget_and_a_message_over_it_is_dropped_unread`.
- **Sample.** `sample-plugins/browser-badge` (a background page: a globe on a tab while its browser panel is open);
  `manifest-tour`'s background page now answers `add_item`. Both load as valid in
  `flare_mods::discover::tests::the_sample_plugins_are_valid`.

## Plugins phase 4: sidebar sections and actions, 2026-10-25

The fourth phase of `docs/superpowers/plans/2026-10-25-plugin-system.md` (design sections 5.3, 5.4 and 13). The
contract: `docs/surfaces/plugins.md`, "Sidebar sections and actions", and `docs/surfaces/protocol.md` (`sidebar.set`,
`actions.set`, click scope).

- **Sections.** Each `contributes.sidebar` entry of a plugin that is on is a section under the tab list in the vertical
  layout, drawn by Flare in the sidebar's look under the plugin's name (`ui/plugin_sections.slint`): a header that
  folds on a click and has a menu (Move up, Move down, Fold), items with a checkbox, an icon, a second line and a jump
  arrow, and a note when empty; at most 6 in all, in the person's order (`plugins.sidebar.order` and
  `plugins.sidebar.folded` in the preferences), at most 45 percent of the sidebar. In the horizontal layout a Plugins
  button in the title bar opens the same rows as a dropdown (an overlay: `Overlay::PluginsDropdown`). The model:
  `plugin_ui` (pure); the window: `appctl::plugins_glue`. Tests: `plugin_ui` (9), `ui_tests::plugin_surfaces` (the
  room, the ticked box, the clicks, the cap of the height, the dropdown), `a_section_is_drawn_under_its_plugins_name_set_by_sidebar_set_and_its_ticks_and_clicks_are_events`,
  `sections_and_buttons_are_capped_and_the_persons_order_and_folds_are_kept`,
  `the_plugins_dropdown_opens_from_the_top_bar_only_while_a_section_is_drawn`.
- **`sidebar.set` and `actions.set`.** New methods in the `Sidebar` and `Actions` namespaces, checked on the worker
  (declared key, fields, 50 items, 120-character clean lines, icons, a tab the plugin may see; 5 a second per section,
  20 a second per plugin) and applied only for the generation they were checked for. Tests:
  `sidebar_set_is_five_a_second_for_one_section_and_only_a_plugin_calls_it`, the refusals in the two appctl tests above
  and below.
- **Actions.** `tab-menu`: one flyout per plugin under its name in a shell tab's menu (`MenuRowData` gains `flyout`,
  `parent` and an own icon path). `tab-hover`: Flare's outlined chip (`PluginChip`) on a hovered tab of 110 px or more
  and on a hovered sidebar row, left of the badges and never over the ring, two on a tab at most. `top-bar`: up to 3
  chips left of the update chip and the window buttons, with a 10 px gap of their own. State per tab or for every tab,
  ticked and faint. A click is the page event `action` `{key, tab}` (to the background page when it listens), an item's
  click or tick `sidebar.item` `{key, item, checked?}`; a background page's tool call says the agent's `tab`. Tests:
  `actions_show_in_the_tab_menu_on_hover_and_in_the_top_bar_and_a_click_is_an_event_with_click_scope`,
  `ui_tests::plugin_surfaces` (hover buttons: place, never over the ring, none on narrow or icon tabs, the click; the
  top bar's gap in both bars), `flare_mods::bridge` (2 new), `ui_tests::overlay_order`, `ui_tests::panel` (the new
  overlay term).
- **Click scope.** A click on a plugin's action or item on tab X lets that plugin act on X until X closes or the plugin
  is turned off (`Hub::grant_click_scope`, read by `AppDispatcher::scoped_tab`). Tests:
  `flare_mods::hub::a_plugin_on_shows_what_it_contributes_and_a_click_scope_lasts_until_it_stops_or_the_tab_closes`, and
  the actions test above.
- **Sample.** `sample-plugins/todo`: to-dos per session tab in a sidebar section, `add_todo` and `todo_done` for Claude,
  ticked off after a moment, kept in the page's `localStorage`, "Add to-do" in a tab's menu. It loads as valid in
  `flare_mods::discover::tests::the_sample_plugins_are_valid`.

## Plugins phase 5: sessions, typing and storage, 2026-10-26

The fifth phase of `docs/superpowers/plans/2026-10-25-plugin-system.md` (design sections 6, 7 and 13). The contract:
`docs/surfaces/plugins.md`, "Sessions, typing and storage", and `docs/surfaces/protocol.md` (`session.read`,
`tabs.input`, `storage.*`).

- **Session events.** `window.flare.on('session', fn)` hears `{tab, session, role, text, tool?, at?, dropped?}` for
  each prompt, reply and tool call of a tab's Claude session, for a plugin that holds `sessions` and may see the tab;
  text cleaned and at most 2000 characters; 50 a second per plugin, the rest counted in `dropped`. The entries:
  `flare_convo::said`; the follower: `flare_convo::follow` (the transcript tail started at its end,
  `Tail::skip_to_end`); which tabs are followed and each plugin's share: `plugin_sessions` and
  `appctl::sessions_glue`. A tab is followed only while a page listens and may see it and its session is known; off,
  nobody listening or the tab closed stops it. Tests: `flare-convo/tests/said.rs` (7), `plugin_sessions` (3),
  `a_tab_is_followed_only_while_a_plugin_with_sessions_listens_and_may_see_it`,
  `each_plugin_hears_fifty_entries_a_second_and_the_next_event_says_what_it_missed`.
- **`session.read`.** The last 200 entries, read from the transcript's last 4 MB on the call's worker; once a second per
  tab. Test: `session_read_gives_the_last_two_hundred_entries_to_a_plugin_that_holds_sessions_only`.
- **`tabs.input`.** Typing for a plugin that holds `input`, on a tab it may see: one paste the way the person's goes
  (bracketed when asked), the Enter 300 ms later only while the tab is open and the plugin on; 4 KB, control characters
  refused but tab and line breaks, 2 a second per tab. The typed intent is `flare_term::handle::PluginInput` (regression
  rules 2 and 188 amended: the plugin exception, never the agent). Tests:
  `tabs_input_types_as_a_paste_with_its_checks_and_the_enter_comes_after`,
  `the_enter_is_not_pressed_once_the_plugin_is_off_and_a_closed_tab_takes_nothing`,
  `rule_s3_the_agent_cannot_type_into_shells_change_settings_or_enable_mods` (now with `tabs.input`).
- **Storage.** `storage.get {key?}` and `storage.set {key?, value}`: each protocol 2 plugin's own JSON object in
  `<profile>\plugins-storage\<id>.json`, at most 1 MB (`too_large`), read on the call's worker, written atomically by
  a writer thread at most twice a second (`flare_mods::storage`). A new `Storage` namespace that needs no permission and
  is closed to the agent; a protocol 1 mod is refused. Tests: `flare_mods::storage` (7),
  `storage_is_each_plugins_own_and_never_a_protocol_1_mods_or_the_agents`, the grant table
  (`a_grant_with_no_permission_may_call_only_hello_tabs_list_and_its_own_store`).
- **Settings.** `input` already reads "Type and run commands in your terminals (full control)" and `input` with
  `sessions` adds "With both, it can read what Claude says and steer Claude by typing to it." (unchanged, tested in
  `flare_mods::hub` and `ui_tests::settings_mods`).

## Plugins phase 6: session bar, network and audio, 2026-10-26

The sixth phase of `docs/superpowers/plans/2026-10-25-plugin-system.md` (design sections 5.5, 5.6, 8 and 13). The
contract: `docs/surfaces/plugins.md`, "Session bar, network and audio", and `docs/surfaces/protocol.md` (`bar.show`,
`bar.hide`, `view.audio`, `show_audio`).

- **The session bar.** `bar.show {tab?}` and `bar.hide {tab?}` (namespace `Bar`, permission `bar`; once a second per
  plugin per tab) put the plugin's strip (`bar.entry`, `bar.height` 24 to 160) under a tab's terminal. One bar a tab: a
  second plugin gets `busy`. The terminal is laid out shorter by the bar's height and the shell is resized once (the
  window-resize path, no animation); the bar shows only while its tab is in front with its terminal. Its page is a
  plugin view in the bar slot, sealed as a bar, made lazily for the tab in front, hidden behind or under a Flare
  overlay, at most two kept a plugin and 12 plugin views in all (`plugin_bar::MAX_VIEWS`: the panels of mods and
  plugins and the background pages stay, an older bar behind goes; the front one always shows), and never the keyboard. Every bar of a plugin goes when it stops, a tab's with the
  tab, and a bar page that ends takes its bar. The pure part: `plugin_bar` (one bar a tab, the height limits, what a
  plugin stops, which views go past two or past the total); the window: `appctl::bar_glue` and `WebCtl::sync_bars`; the layout:
  `bar-h`, `bar-row` and `bar-web-*` in `app.slint`.
- **`session-bar` actions.** Drawn as Flare's chips at the right end of the bar row (at most 4), with `actions.set`
  state; a click is `action {key, tab}` with click scope.
- **`network`.** A plugin that holds it runs its pages in a networked plugins' browser (`EnvKind::PluginNet`,
  `WebView2-plugin-net`: the WebRTC policy, Flare's filtering proxy since issue #97 (below), and a resolver that knows
  no local name and no local address range, the wall for a WebSocket, which `https:` in the policy matches as `wss:`
  and which never meets the request wall) with the policy
  `flare_mods::serve::NET_CSP` (https `connect-src`, `img-src`, `media-src`; `frame-src 'none'`, `form-action 'self'`,
  `base-uri 'none'`); the request wall lets out only https to a public host (`flare_surfaces::net`: loopback, private,
  shared and link-local ranges, dotless hosts and local names refused). No navigation away and no window, as before.
  Without it the policy is the sealed one. The combined Settings warnings for `network` with `sessions` and with
  `tabs` were already there (phase 2).
- **Audio.** `view.audio {path | url, title?, autoplay?}` (namespace `View`) and the agent's `show_audio` in
  `flare-mcp`: Flare's own player page (`flare_surfaces::audio`) in the panel, a mini-app whose policy lets in its own
  copied track (at most 100 MB, the sound types by both names, rules 13 and 14) or one public https origin; play,
  pause, seek, no loop, autoplay only when asked and once. A plugin plays an address only with `network`.
- **Tests.** `plugin_bar` (5), `appctl::tests::mods_tests::plugin_bar` (3: the bar and its one fit, `busy`, the rate,
  lazy views, hiding behind a tab and an overlay, clearing on hide, off and tab close; the chips and their clicks; the
  bar page's calls; networked pages), `ui_tests::plugin_bar` (3: the grid gives up the height at every scale, the row
  under it, the chips' room and clicks, nothing while the grid is not in front), `flare_surfaces::net` (2),
  `flare_surfaces::audio` (4), `modview` (4 new), `flare_mods::serve` (the policy per permission), `flare_mods::hub`
  (`a_plugins_bar_and_network_count_only_while_it_is_on_with_them_approved`), the grant table
  (`bar_opens_only_the_bar_and_view_opens_the_audio_player`), `ctl_app` (`view_audio_*`, 2) and `flare-mcp`
  (`show_audio_routes_a_path_made_absolute_or_a_url_to_view_audio`, the pinned tools list).

Builder's calls (the owner may overrule):
- "Unless the user pinned it" (design 5.5) is not built: there is no pin yet, so a second plugin is always `busy`.
- The bar's page hides under any Flare overlay instead of showing a still (it is a thin strip; the panel's still
  machinery is for the panel).
- A click that gives the bar's page focus hands the keyboard back to the terminal at once ("never take focus").
- A networked plugin's profile lives in its own browser folder, so a plugin that gains or loses `network` in an update
  starts with empty page storage (two browsers with different arguments cannot share a folder).
- The networked plugins' browser walls the local addresses in its resolver (`MAP 127.* ~NOTFOUND` and the like), not
  with a proxy script: Chromium sends loopback and link-local direct whatever a script says, and `<-loopback>` works
  only with a fixed proxy (`net/docs/proxy.md`). That the rules apply to an address written as numbers is read from
  Chromium's `HostMappingRules`, not yet measured on this runtime; a public name whose first label is such a number
  (`10.example.com`) is not found either.
- `show_audio` with an address uses the networked mini-app browser whatever the mini-app network switch says (that
  switch is for pages the agent writes); the page is Flare's and its policy lets in one origin's media only.

## Plugins phase 7: helper programs, install, update and remove, 2026-10-26

The seventh phase of `docs/superpowers/plans/2026-10-25-plugin-system.md` (design sections 4, 9, 10 and 13). The
contract: `docs/surfaces/plugins.md`, "Helper programs" and "Install, update, remove", and `docs/surfaces/protocol.md` ("Events", a
plugin's helper).

- **Helper programs.** `flare_mods::helper::Host` starts a plugin's `helper.command` (an `.exe` checked again inside
  the folder by `paths::program_inside`) for a plugin that is on and holds `process` in its live grant
  (`Hub::helper_of`), suspended, in a kill-on-close job object of its own, then resumed; `CREATE_NO_WINDOW`, the plugin
  folder as its current folder, stdin closed, `FLARE_TAB_TOKEN` and `FLARE_RESUME_ID` removed, `FLARE_PLUGIN_TOKEN`
  and `FLARE_CTL_PIPE` set on the child. stdout and stderr go to `<profile>\plugins-logs\<id>.log` (moved to `.log.1`
  past 1 MB). An end by itself restarts it at once, 3 times in 5 minutes, then it stays stopped
  (`helper::STOPPED_HELPER` in Settings) until the plugin is switched. Turning the plugin off terminates its job before
  Settings hears it is off; the quit stops every helper. Starting, waiting and logging run on the helper's own threads.
  The window: `appctl::install_glue` (`sync_helpers` after every change of the plugins, beside `sync_backgrounds`).
- **Helper events.** `flare_ctl::session::Hub::publish_to_owner` routes an event to the connections that said hello
  with one plugin's token; the window sends a helper the `tabs` and `agent` events of each refresh when its plugin
  holds `tabs`, and `session` entries with `sessions` (and `tabs` or click scope).
- **Install.** `flare_mods::install`: the zip rules (create-new writes, links, encryption, traversal, drives, `:`, `\`,
  device names, case duplicates, 200 characters, 8 folders, 16 MB an entry, 100 MB, 50 MB, 2000 entries), staging in
  `plugins\.staging\<random>` checked by `discover::check_folder` and hashed before anything lands, the commit by
  rename with the old copy moved aside and put back on a failure, and the Recycle Bin (`SHFileOperationW`, silent).
  `flare_mods::fetch`: the download (https on every hop, at most 5 redirects followed one at a time, the phase 6 address
  wall and a resolver that drops local addresses, 50 MB counted, 60 seconds, no cookies, no proxy). The hub
  (`hub::lifecycle`): `stage_install`, `pause`, `commit_install`, `remove_plugin`, one disk lock with the scan; the
  install record (`source`, `hash`) in `mods.json` under `installed`; a change of source is a new approval like a change
  of author or homepage, and wipes the plugin's storage and browser profile. The workers: `crate::plugin_install`.
- **Settings > Plugins.** "Install a plugin" (Install from file, a file picker) and "From an address" (a text field,
  the new `Control::Text` and `SetKind.text`, `SetTextField`; Install or Enter) under the folder row, and Update and
  Remove on each card's switch row. One install, update or removal at a time; the install row's subtext is what the
  last one said, a warning when it failed.
- **Tests.** `flare_mods::install` (7: the name rules, traversal, case, links, the size and count limits, the layouts,
  the commit and the old copy, leftovers), `flare_mods::fetch` (4: the limit, redirects and their rule, the resolver,
  a server error), `flare_mods::helper` (7: the environment and the log, the job ends a stopped helper and its child,
  the restart policy with real processes and as a counter, only an exe inside the folder, log rotation, no token in
  `Debug`), `hub::lifecycle` (6: installed off with its record, an update asking for more turns off, a downgrade
  refused, another source wipes storage and profile, remove forgets everything, `helper_of` only on and with
  `process`), `enabled` (the install record), `flare_ctl::session` (owner events), `flare_surfaces::net`
  (`ip_refusal`), `crate::plugin_install` (3: the rules, refused addresses, the id an update holds),
  `appctl::tests::mods_tests::plugin_install` (7: from a file, a refused zip, update and its stop, remove, the
  picker, a helper on and off with its events, a helper stopped after its restarts), `ui_tests::settings_mods` (the
  install rows, the field and the buttons) and the Plugins page model.

Builder's calls (the owner may overrule):
- A downgrade is refused with a sentence instead of asked about (design 13 "a downgrade asks"); removing first installs
  the older one.
- "Installing over an existing id shows the differences" is the existing card: an update that asks for more turns the
  plugin off and its card lists what it asks for now, with "Asks for new permissions"; there is no side-by-side view.
- Remove asks nothing: the folder goes to the Recycle Bin, so it can be brought back by hand (its approval cannot).
- Update downloads again from the address the plugin was installed from, or opens the file picker for one installed
  from a file or by hand; the zip must hold the same id.
- A plugin's browser profile is deleted as the folder `EBWebView\flare-mod-<id>` of both plugin browsers, after its
  views closed; that this is WebView2's profile folder is read from WebView2's layout, not measured.
- The design's "at most 4 pipe connections per plugin token" is not built in this phase.
- A helper hears `tabs`, `agent` and `session` events without asking for them (a page asks with `window.flare.on`);
  there is no `plugin` event for a helper, since it is stopped when the plugin turns off.

## Plugins phase 8: samples, call-me and the plugin docs, 2026-10-26

The eighth phase of `docs/superpowers/plans/2026-10-25-plugin-system.md` (design sections 1, 11 and 13). No Rust
behaviour changes.

- **Samples.** `sample-plugins/` gains four, each small and commented, with a line in its README: `what-next` (a "Next
  up" sidebar section Claude fills with `set_next`, kept in the plugin's store; adding `network` and a repository lets
  the page fetch the open issues from `api.github.com` itself), `sonara-bar` (a bar with play and pause, a seek slider and
  a title, back and skip as `session-bar` actions, driven by a fake source Claude sets with `sonara_now`; no real Sonara
  integration), `session-buttons` ("Compact" and "Tell it to carry on" on the hover button, the tab menu and the bar,
  typed with `tabs.input` and Enter into tabs where Claude runs; `input` is full control) and `look` (a star on a tab
  whose agent finished while you were elsewhere, and a weighted progress chip from `set_steps`: the weights done over
  all the weights, so 9 of 10 steps done reads 50 % when the last is half the work). Test:
  `flare_mods::discover::tests::the_sample_plugins_are_valid` (every sample loads as a valid plugin with the
  permissions it should hold and uses the methods it says).
- **Docs.** `docs/surfaces/plugins.md` is the plugin contract (the manifest, the permissions in plain words, the methods
  per namespace, the events, which tab a call acts on, the limits, the bridge, pages and the serving rule, helpers,
  install, update and remove, the safety rules); `mods.md` keeps the protocol 1 notes and a pointer. The README's
  plugin line and `sample-plugins/README.md` point to it.
- **Call-me.** The call-me skill (outside the repo, `~/.claude/skills/call-me/SKILL.md`) calls
  `set_tab_badge {key: "call-me", icon: "phone", tip: "Call-me is on"}` when it turns on inside Flare and
  `clear_tab_badge {key: "call-me"}` when it turns off, and skips both when the tools are absent.

Builder's calls (the owner may overrule):
- The weighted progress is a plugin tool (`look`'s `set_steps`), in the page's JavaScript; there is no Rust helper to
  unit test. A workflow without the plugin reports the same value with `set_tab_progress`.
- The "Tell it" button is titled "Tell it to carry on" and types its preset line (set at the top of its page): a
  plugin has no prompt to ask for the text.
- `session-buttons` asks for `tabs` too, only to type nowhere but in Claude tabs (in a plain shell `/compact` and Enter
  would run as a command).
- `sonara-bar` and `session-buttons` show their bar from a tab-menu toggle (click scope), so they need no `tabs` for it.
- `look` has no icon override or effect: `tabs.icon.*` and `tabs.effect.*` are not built. (An agent's icon motion is
  built separately, see "Agent icons that move".)

## General review, 2026-10-23

A review of the whole app by eight reviewers found these, all fixed on `fix/general-review` with a test each (rules 8,
220 to 223 and the dictation send rule carry the notable ones):

- **Terminal.** Ctrl chords follow the layout letter (term-1); the default mouse report is UTF-8 past column 94 (term-5);
  an OSC 52 write at its cap survives the scanner (term-7); OSC 8 links are percent-escaped before the browser sees them
  (term-3); DECRQM 2031 is answered once, even inside a synchronized update (term-6); a cmd resume never searches the tab's
  folder (term-8); a resize waits for neither the pump nor the console host (term-4); closing a tab spares processes in a
  console window of their own (term-2); the missing-folder test tests the absolute path (term-9).
- **Rendering.** A lazily found fallback font repaints an idle window (render-1); a zoom reflows one tab (render-2, since replaced by the shared zoom of rule 1154); a
  family that will not load is asked for once (render-3, the repeat only); the cell underline is the one hairline
  (render-4); the see-through texture is freed when the ground is solid (render-5).
- **App and platform.** Hand-offs (app-1, app-5, platform-5), restore stats with a deadline (app-2), unreadable stores and
  a quit that waits for a save (app-3, app-4), an absolute `--user-data-dir` (platform-6), the log rotation and the verbose
  switch (platform-3, platform-4), the clipboard block (platform-8), the dictation Enter (platform-1).
- **Agents.** Detection by program or package (agent-1), a change of kind (agent-2), `CLAUDE_CONFIG_DIR` (agent-3),
  recycled pids (agent-4, platform-2), a title cut across two reads (agent-5), and two tests that now test what they say
  (agent-6, agent-7).
- **UI.** Ctrl+C copies Flare's own selection over an idle agent (ui-1); Settings lays out again when only the scale
  changes (ui-2); the chrome is named for a screen reader and Space presses a dialog button (ui-3,
  `crates/flare/tests/chrome_access.rs`); the theme editor's code fields follow the popover's alpha rules (ui-4); Open
  folder uses Explorer by absolute path on a worker (ui-5); the flash poll timer stops with its watch (ui-6).
- **Not fixed here.** render-3's one-time font database build on the UI thread when a new family is picked (the font
  system is read synchronously by `set_font`; it needs the build moved to a worker with a swap, a design of its own), and
  term-6's second half (scanner replies such as XTVERSION and OSC 10/11 can still jump ahead of DA replies that alacritty
  holds inside a synchronized update).

## Antigravity (agy) as an agent, 2026-10-26

Spec: `docs/superpowers/specs/2026-10-26-agy-and-agent-motion.md`, Part 1. Facts about agy 1.4.7 and the map of every place
Flare names an agent: `C:\Users\Admin\Documents\Claude\research\flare\2026-10-26-agy-cli-and-flare-agent-map.md`.

- **The third agent.** `AgentKind::Agy` and `DetectedAgent::Agy`, the word `"agy"` in `tabs.json`, `tabs.list` and the
  `agent` event, shown as "Antigravity" (close question, cursor tag, decor tooltip, notifications). The one list is
  still `RESUMABLE_AGENTS`; the guard (`no_source_file_lists_the_resumable_agents_by_hand`) now fails on any line that names
  two of the three. Detection: the program `agy` / `agy.exe` as the first token or a program-file argument (one `agy.exe`
  process, no node); `agy` is in the prefilter, so `legacy` and `strategy` pass it and the strict match drops them. Under one
  shell Claude wins, then Codex, then agy, then any other agent.
- **State from OSC 7501** (`flare-agent` `osc7501.rs`), a third source under the hooks and over titles and output scoring.
  `OSC 7501 ; state=<idle|working|done|blocked|error|clear>[:kind=permission|question|auth][:app=..][:msg=base64] ST`, BEL
  or ESC \, key order free, unknown keys ignored, the grammar and limits of the program-status spec
  (https://www.superlogical.com/rex/docs/build/program-status, read 2026-10-26). working is Working, blocked a Question,
  done Finished, error Failed, idle nothing; `clear` hands the tab back to the titles and the scoring. `msg` is decoded
  only into clean text (no control or bidi character) and is not shown anywhere yet. Every shell Flare starts gets
  `AGY_CLI_OSC7501=on` unless the user set the variable; Flare does not answer agy's `OSC 7501 ; ?` query.
- **Chosen where the spec left it open.** Only the root record counts: a report with an `id` (a part of the work) is
  dropped, so a subtask's "done" never marks the tab. A report from a tab with no agent found yet lights nothing; it asks
  the poll to look and is applied when the poll names an agent (so a plain program's 7501 never makes a shell an agent).
  Claude's hooks take a tab over from 7501 (Claude Code may send both). A question raised by 7501 is cleared by the
  program's next report, never by Flare's screen reads. A report goes with its program: a prompt (the shell reported its
  folder) or a poll answer naming another agent drops it and the marks it set, so an agent that quit without
  `state=clear` never freezes the next one's marks.
- **Help.** The command help has `agy` (start) and `agy -c` (carry on the last conversation) under "Claude Code, Codex and
  Antigravity"; Flare's own entries sit in `tools/oracle-help/flare.mjs` over Prism Terminal's copied catalogue, and the
  golden searches are regenerated from Prism Terminal's own search.
- **Fallback.** With no 7501 (an older agy) the poll still names the agent and the output scoring gives working and idle.
- **Resume.** A saved agy tab comes back with `agy:last`, started as `agy -c` (the most recent conversation in that folder);
  `valid_resume` accepts the marker, and no id travels.
- **Icon.** `IconKind::Agy`, row int 3 (Settings moved to 4): the pixel arch of agy's banner, 32 cells on a 10 x 8 grid
  (the picked mock `docs/surfaces/agy-icon-mock.html` has 32, not the 46 the spec guessed), one Slint `Path` per colour
  (`Glyphs.agy`, `Theme.agy`), pinned to `flare_agent::icon::{AGY_PIXELS, AGY_COLOURS, agy_paths}` by
  `ui_text_marks.rs`. It stands still unless a plugin picks a motion (Part 2, "Agent icons that move").
- **Keys.** agy takes Codex's image paste (^V) and newline (backslash, CR) until they are measured. Ctrl+Backspace sends ^W (0x17) while agy runs: agy deletes a word on ^W or Alt+Backspace and ignores ^H and every encoded Ctrl+Backspace (measured on 1.4.8), and Flare keeps Ctrl+W for closing a tab. Every other program still gets ^H.
- **Not in this PR.** Hooks for agy, Flare's MCP server in agy's config (`agy mcp add` writes the user's global file),
  dictation routing, answering the `OSC 7501 ; ?` query (the spec's reply is
  `OSC 7501 ; ? : states=idle,working,done,blocked,error : kinds=permission,question,auth ST`; agy's own check is not
  captured yet).
- Held by `agy_is_found_by_its_program_and_never_by_a_word`, `claude_then_codex_win_over_agy_under_one_shell`, the
  `osc7501` parser tests, `each_status_drives_its_mark_on_an_agy_tab_away` and the other OSC 7501 indicator tests,
  `an_agy_tab_is_marked_from_its_osc_7501_reports`, `osc_7501_is_passed_up_raw_with_either_terminator_and_across_chunks`,
  `an_agent_that_follows_agy_without_a_clear_drives_its_own_marks`, `a_prompt_drops_the_report_of_the_program_that_ended`,
  `agy_gets_its_own_marker_and_its_word_round_trips`, `agy_resumes_its_last_conversation_with_no_id`,
  `an_agy_tab_comes_back_as_agy_c_wearing_its_icon_from_the_first_frame`, `turns_agys_status_reports_on_unless_the_user_chose`,
  `the_agy_icon_is_the_banners_rainbow_arch`, `the_agy_glyph_is_the_agent_crates_paths` and `agent_icons_are_eighteen_pixels`.

## Agent icons that move, picked by a plugin, 2026-10-27

Spec: `docs/superpowers/specs/2026-10-26-agy-and-agent-motion.md`, Part 2 (owner decisions 3 and 4); the mocks Max picked:
`docs/surfaces/agent-motion-mock.html` (Claude H1, Codex O6) and `docs/surfaces/agy-motion-mock.html` (agy A1).

- **What moves.** An agent's tab icon, in the strip and the sidebar, while its tab works (the Working mark is on), in the
  motion a plugin picked for that agent: `hop-blink` (H1: a slow hop with a squash, 1.8 s, Clawd's eyes shut every other
  landing), `spin-fade` (O6: a turn every 3.5 s, fading 0.55 to 1 over 1.8 s), `rainbow-flow` (A1, agy only: the arch's
  colours run along it every 3 s), and for any agent `hop`, `spin` (4 s a turn) and `breathe` (0.82 to 1.05 of its size).
  Without a pick, and for a tab that does not work, the icon stands still as before; with Windows' animation effects off
  every icon stands still.
- **How.** `flare_agent::icon_motion` computes every frame from the mark clock's time (`icon_phase`, no clock read
  there); `push_frame` puts it on `MarkMotion` with the marks; `AgentIcon` (`tabmark.slint`) moves and scales the icon's
  box for the hop and the breath, fades it, turns it (`transform-rotation`, left out by the software renderer), fills
  Clawd's eyes (swaps in `Glyphs.clawd-shut`, pinned to `flare_agent::icon::clawd_shut_path`: one shape, no lid path laid
  over the body) and gives each path of agy's arch its flowing colour. A row gets its motion from `tabs_ui::wear_motions`.
  No Slint `animate`, and nothing new ticks: the clock runs while a tab works anyway.
- **Whole pixels (2026-11-13).** The moving box keeps the squash and stretch but lands every edge, its size and the hop's
  rise on whole device pixels of the window at any scale (`flare_agent::icon_motion::icon_box` is the rule), and a hop
  squashes about its art's feet (`icon::CLAWD_FEET`, `AGY_FEET`, `OPENAI_FEET`), which stay on the rest line, so the icon
  no longer shrinks, wobbles or smears. Windows and macOS alike (rule 1202). On the GPU renderer (the Mac at 200 %, and
  Windows) the same box now draws the same pixels frame after frame too: femtovg's concave cover quad left stencil
  behind that ate the next frames' edge (rule 1203, `crates/flare-render/tests/femtovg_clawd_hop.rs`).
- **The plugin API.** `agents.motion.set {agent, motion}` and `agents.motion.clear {agent}`, namespace `Agents`, new
  permission `agent-looks` ("Change how agent icons move while they work"), protocol 2 only; the newest pick wins, and a
  plugin that stops takes its picks with it (`flare_agent::icon_motion::MotionPicks`, kept by the window, applied for the
  plugin generation that was checked). Contract: `docs/surfaces/plugins.md` ("Agent icon motions"), `protocol.md`.
- **Sample.** `sample-plugins/agent-moves/`: the owner's three picks from its background page, and nothing in a tab's
  menu (owner, 2026-11-13: the "Agent motions" item and its picker panel are gone; picks saved by 0.1 still apply).
- Held by the `icon_motion` tests (each motion's frames at fixed phases: hop height and squash, the blink's frames, the
  turns, the fade, the flow's colours and offsets, the still frame, the picks), `only_a_working_agents_tab_wears_its_agents_motion`,
  `a_picked_motion_moves_a_working_agents_icon_from_the_mark_clock_and_stills_with_effects_off`,
  `agent_icons_move_by_the_pushed_frame_and_stand_still_when_stilled`, the `plugin_motion` tests (permission, params,
  rate, the newest pick, a plugin turned off), `agent_looks_opens_only_the_agents_motions`,
  `clawds_lids_are_the_agent_crates_path_in_clawds_colour`, `the_agent_icon_moves_by_the_mark_clock_only` and
  `the_sample_plugins_are_valid`.

## New tab: a folder arrow on the sidebar button, and no ask mode, 2026-10-27

Spec: `docs/superpowers/specs/2026-10-27-new-tab-folder-arrow.md`; the picked look is N1 in
`docs/surfaces/new-tab-arrow-mock.html`.

- **A split New tab button in the sidebar.** The main part opens a tab in the default folder at once (Ctrl+T does the
  same); a 40 px arrow on the right, behind a full-height hairline, with a chevron and its own hover, opens the folder
  menu as a dropdown from the arrow (Max 2026-10-27, spec decision 5): its right edge on the button's right edge, opening
  left and up 4 px above the button, below it only when there is no room above, held 8 px inside the window (it may
  cover the terminal past a narrow sidebar). While it is open the arrow stays lit and its chevron points up. The main
  part's tooltip is "New tab (Ctrl+T)", the arrow's "Open in a folder" (also its accessible name). A right-click on the
  button does nothing any more.
- **The strip's + is unchanged.** Same look, a click opens a tab, a right-click opens the same folder menu at the
  pointer (two small buttons side by side there would make New tab hard to hit).
- **The folder menu.** "Choose a folder…" is always the first row: the folder picker (starting in the new-tab folder),
  then a tab there, and the folder joins the recents (every spawn remembers its folder). Under a rule: the pins in pin
  order, then the 5 newest unpinned recents. While fewer recents than that are known, Documents, Downloads, Desktop and
  the user folder fill the empty spots, in that order, each once, never one already pinned or in the history; they say
  where they are on the right (`~\Documents`, "user folder"), and a real recent pushes one out. Each folder row keeps its
  pin. Nothing to list at all: a disabled "No recent folders" under the first row.
- **The starter folders** are Windows' known folders (`SHGetKnownFolderPath`, so a Documents redirected to OneDrive is the
  one Explorer shows), checked for existence on a worker once at start (`flare_win::paths::starter_folders`, rule 34);
  until it answers the menu simply has none. Under `--e2e` there are none.
- **No ask mode.** Settings > Terminal > "Folder for new tabs" has no segment any more: it says where tabs open, with
  "Use my user folder" (when a folder is chosen) and "Choose folder…". A stored `flare.newtab.mode = "ask"` reads as
  folder mode (the key is no longer read; the row's reset still clears it); the import writes `prism.newtab.mode` as
  `folder` whatever it was.
- Held by `the_new_tab_button_opens_a_tab_and_its_arrow_opens_the_folder_menu`,
  `sidebar_new_tab_arrow_draws_at_full_size_at_every_scale`, `the_plus_menu_is_choose_then_pins_then_five_recents_or_a_disabled_note`,
  `starters_fill_the_menu_until_real_recents_push_them_out`, `starters_fill_the_empty_spots_and_recents_push_them_out`,
  `a_starter_already_pinned_or_recent_is_never_listed_twice`, `choose_a_folder_in_the_folder_menu_asks_opens_there_and_remembers_it`,
  `the_starter_folders_fill_the_folder_menu_once_the_worker_answers`, `a_stored_ask_mode_opens_in_the_folder_at_once`,
  `the_new_tab_row_has_no_mode_to_pick`, `the_new_tab_mode_imports_as_folder_whatever_it_was`,
  `plus_opens_at_once_in_the_chosen_folder_and_does_not_ask_again` and `the_starter_folders_exist_and_end_with_the_user_folder`.

## Networked pages checked after the name lookup (issue #97), 2026-10-28

Spec: `docs/superpowers/specs/2026-10-28-dns-rebinding-proxy.md`. A public name whose DNS answer is this PC or the local
network (DNS rebinding) passed the text checks of a networked page; now every connection is checked after the lookup.

- **The filtering proxy.** `flare_surfaces::netproxy`: a std-only HTTP `CONNECT` proxy on `127.0.0.1` with a port the
  system picks, started once a process and kept, a thread a connection (at most 256), the request head within 8 KB and
  10 s, `CONNECT` only (another method 403, a bad target 400), the address check, a 10 s connect to one checked address
  (pinned: the name is not looked up again), then a copy both ways that closes after 5 minutes with no byte.
- **The check.** `flare_surfaces::net::parse_authority` (host and port, IPv6 in brackets, port required) and
  `allowed_addresses` (the host's text by `refusal`, a numeric address as it is, a name looked up and only the
  addresses `ip_refusal` passes kept; none left is a refusal).
- **Who uses it.** The networked plugins' browser (`EnvKind::PluginNet`) and the networked mini-apps' (`EnvKind::AppNet`,
  the audio page too): `--proxy-server=http://127.0.0.1:<port> --proxy-bypass-list=<-loopback>`, so loopback and
  link-local go to the proxy too and are refused. The plugins' resolver rules stay, after `EXCLUDE 127.0.0.1` so the
  browser reaches its proxy. The web panel is unchanged; the sealed kinds keep the dead proxy. No proxy, no networked
  browser: the views fail with the reason (`flare_web::host::env_arguments`).
- **Tests.** `flare_surfaces::net` (3 new: `a_connect_target_is_a_host_and_a_port_and_nothing_else`,
  `a_public_name_pointed_at_this_pc_is_refused_after_the_lookup`,
  `an_address_written_as_numbers_or_a_local_name_is_refused_without_a_lookup`), `flare_surfaces::netproxy` (2),
  `crates/flare-surfaces/tests/netproxy.rs` (8, a loopback echo server: relay both ways, early bytes, GET 403 and a
  rebinding name or numeric local target 403 with nothing dialled, a bad target 400, an oversized head and a slow head
  closed, an idle tunnel closed), `modview` (`a_networked_plugins_browser_sends_everything_through_the_filtering_proxy`),
  `app` (`a_sealed_app_browser_has_the_webrtc_policy_and_a_dead_proxy_a_networked_one_the_filtering_proxy`),
  `flare_web::host` (`every_browser_kind_gets_the_same_arguments_each_time_it_is_made`,
  `the_networked_browsers_go_through_the_filtering_proxy_and_the_web_panel_does_not`) and the e2e `miniApp` "webrtc"
  (a WebSocket to loopback is now stopped with the network on too; not yet run live).

## /browser and the page the agent is told about, 2026-10-28

Spec: `docs/superpowers/specs/2026-10-28-browser-command.md`.

- **`/browser [url or search words]`.** The `flare-status` plugin ships `commands/browser.md` (plugin version 1.2.0). Claude
  calls `open_page` with the address (empty: `https://www.google.com/`; words that are not an address:
  `https://www.google.com/search?q=...`), calls `browser_look` once, says in one line that the page is open and the user can
  browse it, and then waits: no clicking, typing or navigating unless asked. Only `open_page` and `browser_look` are
  pre-allowed (`allowed-tools`). Held by `the_browser_command_opens_a_page_looks_once_and_then_waits` and
  `install_writes_the_prompt_hook_only_when_the_program_exists_and_ships_the_browser_command` (`plugin.rs`).
- **The agent hears which page is open.** A second `UserPromptSubmit` hook runs `flare-mcp --hook-context` (the absolute
  `flare-mcp.exe` written in at install like the MCP server's; the hook is left out when there is no program). It asks
  `panel.state` with a 1 s timeout per step (2.5 s for the whole run) and adds context only when the tab's web page changed since it last told this tab and
  Claude session (a state file in `%TEMP%\flare-mcp`, named by a hash of the tab token and the session id), or once when
  the page has gone. The title and address are cleaned, quoted and between the page-content markers, as in a look. Outside
  Flare or when Flare does not answer it prints nothing and exits 0; the state hook (`hook.cmd`) is unchanged. Held by
  `the_prompt_hook_runs_flare_mcp_by_its_placeholder_with_one_flag`, `the_prompt_hook_gets_the_absolute_program_or_is_left_out`,
  `install_writes_the_mcp_server_only_when_the_program_exists` (`plugin.rs`), the five `hook::tests` (`flare-mcp`) and
  `the_prompt_hook_tells_the_agent_of_a_page_once_and_says_nothing_outside_flare` (the real binary over a real pipe, `stdio.rs`).
- **`browser_state`.** A ninth browser tool wraps `panel.state`: no panel, a web page (title and address as page data), a
  mini-app (the browser tools work on it) or another kind. 21 built-in tools now. Held by
  `browser_state_says_what_the_panel_shows_with_the_page_as_quoted_data`, `the_tools_are_listed_with_descriptions_written_for_an_agent`
  (`browser.rs`), `tools_list_has_the_show_tools_then_the_browser_tools_with_object_schemas` (`lib.rs`) and the e2e `mcpRoundTrip` list.

## The web panel saves passwords and fills in forms, 2026-10-28

Cookies already kept a site signed in (the panel's own profile folder, `WebView2` under the profile). Now the panel also offers to
save a password when you sign in and fills saved passwords and form entries in (`ICoreWebView2Settings4`:
`IsPasswordAutosaveEnabled` and `IsGeneralAutofillEnabled` on for the web panel, both off for a mini-app's and a plugin's
page), in `configure` (`crates/flare-web/src/host.rs`). The controller probe reports it as `autofill`; held by the e2e
`webPanel` (true), `miniApp` and `modsSample` (false). Zen's logins cannot be shared (another engine); sign in once in Flare.

## Plugins fill logins, with your approval each time, 2026-10-28

Part A of the Bitwarden design (`Maxaubert/flare-plugins`, `docs/2026-10-28-bitwarden-design.md`); Flare's record:
`docs/superpowers/specs/2026-10-28-plugin-fill.md`. The contract: `docs/surfaces/plugins.md`, "Filling logins".

- **The `fill` permission.** "Fill logins into your web panel", with the warning "Can type passwords into pages, after
  you approve each one." (and, with `network`, that it can send the passwords you type for it to the internet). Its
  namespace holds `browser.fill`, `browser.fill.page` and `secret.ask`; the agent never has it and `browser` does not open
  it. Held by `fill_opens_only_the_fill_methods_and_never_for_the_agent` (`flare-ctl`) and the hub's words and warnings
  tests.
- **`browser.fill`.** Only into the tab's web page, only when its site is the login's (host or subdomain, `https` unless
  the login says `http`; no public suffix list), and only after Allow in Flare's own dialog ("Fill <login> into <host>?",
  Deny and Fill and nothing under them; Fill by the pointer only and not in its first 0.7 s, Enter presses Deny). The
  values reach the page as JSON data through `ExecuteScript`, read only after the script checked the document's
  `location` (a tab that moved to another site gets nothing), the fields are found by `autocomplete` first, and `input`
  and `change` fire. One dialog at a time, 6 asks a minute per plugin, a minute to answer. Held by `flare_mods::fill`
  (14 tests) and
  `appctl::tests::mods_tests::plugin_fill` (the fake browser), and the e2e `pluginFill`. "Deny for this session" and
  the line "<Plugin> fills it into the page; the agent never sees it." were taken out at the owner's ask (2026-11-10):
  each Deny answers one fill.
- **`secret.ask`.** Flare's masked field; the text goes to the plugin that asked, and no log or store of Flare's has it.
- **Tool scope and helpers.** While Claude's call to a plugin waits, the plugin may fill into the tab it came from. A
  plugin with a helper and neither a background page nor a panel has its tools listed, sent to the helper as the `tool`
  event `{call_id, tool, tab, input}`, and answered with `tools.respond`; a manifest may now have tools with only a
  helper.
- **The e2e `pluginFill`.** A test plugin whose helper is the e2e exe itself (`--fake-fill-helper`), a local sign-in page
  on `https://flare-test.local`, the dialog pressed by the e2e pointer like every dialog, Enter that does not allow, an
  early press and the agent's own key and click that do not allow, a
  login of another site refused, and the profile's logs, the plugins' logs and the helper's record scanned for the values.

## Build ready (staged builds), 2026-10-29, one click since 2026-11-10

Specs: `docs/superpowers/specs/2026-10-29-idle-install.md`, then `docs/superpowers/specs/2026-11-10-build-ready-one-click.md`
(the menu, the idle install, Not today and Discard are gone). The agent builds; Flare only installs what was staged, and
only when the person clicks "Build ready". How to stage: `docs/shipping.md`, "Installing a local build with Build ready".

- **Staging.** `tools/stage-install.ps1 [-Stable]` copies the release files into `<install>\staged\` under their names in
  `current\` (`FlareStable.exe` or `Flare.exe`, `flare-mcp.exe`, `flare-parakeet-host.exe`, `LICENSE`,
  `THIRD-PARTY-NOTICES.md`, the pinned `conpty.dll` and `OpenConsole.exe` as `pack.ps1` ships them, `claude-plugin\`)
  and writes `staged.json` last: `{commit, built_at, files: {name: sha256}}`. Two names for one file (case, slashes) are
  refused. (`whisper\` is not staged; it changes only with a new engine and stays as installed.)
  It never touches the running app; a second stage replaces the first. Held by `crates/flare-update/tests/stage_script.rs`
  (runs the real script against a temp install).
- **Only an installed copy.** An exe in a folder named `current` (`flare_update::staged::Layout::from_exe`); a build run
  from `target\release` never looks. The install root is watched (`FindFirstChangeNotificationW`) and every change is one
  check of `staged\` on a worker: every name a plain relative path, the copy's own exe among them, every hash right.
  Anything else shows no chip.
- **The chip.** "Build ready" in the title bar, left of the update chip (both tab layouts). No tooltip: it has a label
  (rule 1201, since 2026-11-13). It has no menu: a click is install now (rule 258). Nothing installs on its own, however long the
  app is idle. A build stays on offer until it is installed or a new stage replaces it (there is no Not today or Discard).
- **The install.** A click asks the close question first when an agent is working ("Stop the agent and install the
  update?", "Install and restart"); Cancel there keeps the build on offer. With no agent working it goes on at once. Then
  a toast under the chip, "Installing the new build and restarting", with Cancel, for 10 s (Cancel leaves the build for
  the next click). After a yes the countdown goes on regardless. A click with no agent at work asks that question as soon
  as one starts (in the countdown or during the swap). Then a worker
  swaps, holding `<install>\install.lock` (two profiles on one install never swap at once): each file in `current\` that
  is not already the staged one is first journaled in `<install>\install.json` (state `swapping`), then renamed to
  `<name>.old` (a running exe can be renamed), the staged file copied in and its hash checked; any failure puts every file
  back and says "The build could not be installed; nothing changed". A swap cut off by a crash or power cut is put back by
  the next start (`Start::Interrupted`; the old exe is started when this one was among the files). A swap is refused while
  the last one is not confirmed (its `.old` files are the last good build); a run that is itself unconfirmed confirms
  itself first. A good swap leaves `install.json` pending and `staged.json` in place. If an agent started working while the swap ran
  (and the close question was not answered yes), the swap is taken back, the build stays on offer and the question asks;
  otherwise the app ends through the normal end of life (tabs.json with the
  agents, so the sessions resume) and starts the new exe with `handoff::restart_args_keeping` (the same
  `--user-data-dir`, and `--e2e` / `--renderer` / `--no-plugins` / `--no-import` when given).
  If the new exe will not start, the old files go back and the stage is dropped (so it is not offered again).
- **After the restart.** The first start says "Installed <commit>" once (8 s) and removes `staged.json`, then `staged\`;
  after 20 s up, or a clean quit, the build is good and the `.old` files go (one still in use is retried at the next
  start; a stage made since is kept). The last-good
  guard runs in `main` after the single-instance check: a new build that died before it was good is counted per start, and
  the third start puts the `.old` files back, starts the old exe on the same profile and exits; the old build then says
  "Build <commit> did not start, so the last build is back".
- **Tests.** `flare_update::staged` (temp install: swap, a failed copy, a bad hash, a file held open, rollback after two
  early deaths, confirm, leftovers), `staged_flow` (pure), `ui_tests::staged_flows` (the real `AppCtl`: chip, a
  click that installs with no menu, nothing installed after minutes idle, a click under a working agent that asks and
  whose Cancel keeps the build, the toast's Cancel, a failed swap, the swap taken back, the start notices, confirm at
  20 s and at a clean quit, the chip and toast boxes), and the offscreen e2e `clickInstall` (a temp install in the world;
  a click under a stand-in Claude asks and Escape keeps the build; idle, nothing installs; a click installs; the new
  process answers on the same pipe, parked, its tab back, confirmed after 20 s). The chip's box is `ui.rect staged-chip`.
- **macOS (2026-11-11, rule 1132).** The unit is the whole bundle: `tools/release/mac/stage-install.sh` stages
  `~/Library/Application Support/Flare/staged/Flare.app` and `staged.json` (names relative to the bundle); a `.app` in an
  `Applications` folder is an installed copy (`Layout::from_bundle_exe`), ad hoc or not; the swap renames the running
  bundle to `Flare.app.old`, renames the stage in (same volume, else `NotReady::OtherVolume`) and checks the hashes in
  place; the restart is `open -n <bundle>`; the watch polls the manifest every 2 s. How to stage: `docs/shipping.md`.
  Tests: `staged::mac_tests`, `stage_script::mac`, and the `a_mac_bundle_...` and `the_mac_restart_...` cases in
  `ui_tests::staged_flows`.
- **Not covered.** A new exe that crashes before `main` reaches the guard is still counted only when started again by
  hand; nothing restarts it on its own (the same holds for a swap cut off while the exe itself was half copied: the next
  start by hand of that exe may not run, and `<name>.old` is then the way back). Files removed from `claude-plugin\` in a
  new build stay in `current\` (the swap is per file, not a mirror). The restarted exe waits up to `HANDOFF_WAIT` (6 s)
  for the quitting one's pipe before it takes the lock, so the window comes back about 6 s after the old one goes.

## 3D model viewer (issue #36), 2026-11-09

The agent's `show_model {path, title?}` and a plugin's `view.model` (namespace `View`) open a `.glb`, `.gltf` or `.stl` file in the
tab's panel. It is a mini-app Flare writes itself, like the audio player, so there is no new panel kind.

- **Flow.** The path is resolved again (rules 13 and 14) through `path_open::resolve_file_of` with the viewer's extensions; the
  extension must agree on both names and the file must be on a local drive. A bounded read (64 MB), then `index.html`, `viewer.js` and
  `model.<ext>` go into the app's folder. No network.
- **Page.** `flare_surfaces::model::page`, policy first: `default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; img-src 'self' data: blob:;
  connect-src 'self'; worker-src 'none'`, with frames, objects and forms none. The viewer fetches the model itself.
- **Viewer.** `tools/model-viewer/src/viewer.js`, three.js 0.170.0 built once to `assets/three/viewer.js`: orbit, zoom, pan, double-click reset,
  Wireframe and Grid buttons, a neutral studio light, the camera fitted to the model, the background following light or dark, drawn only
  when something changes. A message instead of a blank panel for no WebGL, over 5 million triangles, no geometry, a damaged file, or a
  `.gltf` that points at other files. `window.__flareModel` settles to `{ok, triangles, textures}` or `{ok: false, code, error}`.
- **Tests.** `flare_surfaces::model` (3), `path_open` (`a_file_for_a_viewer_is_one_of_its_extensions_under_the_same_name_rules`), `ctl_app` (`view_model_*`,
  `the_real_model_resolver_is_rules_13_and_14_with_the_viewers_extensions`, `the_bundled_model_viewer_is_the_pinned_build`), `flare-mcp`
  (`show_model_routes_a_path_made_absolute_to_view_model`, the pinned tools list), the method and grant tables, and the e2e `model`.
- **STL normals.** The viewer works out an STL's normals from its facets and ignores the ones in the file, which are commonly zero and would draw the part as one flat silhouette (rule 233).

Not in this version: click a path to preview, a `.gltf` with separate `.bin` or texture files, Draco or other WASM decoders, OBJ, FBX, animation.
## Ask the agent about a selection (#33), 2026-11-09

- **What it is.** A small "Ask agent" button in the panel's header (in a web page's address bar, which is its header) and a row "Ask agent about this
  selection" in the terminal's right-click menu over a grid selection. Both only on a tab with an agent, whichever agent. What is sent lands at the
  prompt and is never submitted: the person reads it and presses Enter, and the keys go to the terminal.
- **What is sent.** Web page: `window.getSelection().toString()`, framed. With no selection on a plain web page (2026-11-10), a reference to the
  page instead (below). A mini-app or plugin page with no selection is a note and nothing typed. Text panel with text selected (2026-11-10): the
  selection, framed as from `<file name> lines A-B` (below). Text or image panel with nothing selected: the file's path, quoted for the tab's shell.
  Handed-over text has no file and says so unless some of it is selected. Terminal grid: the selection, framed; the selection is then let go.
- **Ask about the page** (2026-11-10, rule 238). The agent can already read the page with `browser_look`, so only a reference is typed, and the
  person adds the question:
  `About the page open in this tab (example.com):` then `> title: Pricing`, `> address: https://example.com/pricing` and
  `> (use browser_look to read it)`. The address and title come from the document that answered, in the same answer as its (empty) selection, never
  from the tab. The title is the page's own words: cleaned like a selection, one line, at most 160 characters, left out when empty. The address
  loses any user name and password and is cut at 2 KB with `[cut]`. Only `http` and `https` pages; `about:blank` and the like keep the note.
- **Tooltip.** One fixed wording, since whether a page has a selection is only known by asking it: "Ask agent about the selection or this page" in a
  web page's address bar, "Ask agent about the selection" on a mini-app or plugin page, "Ask agent about the selection or this file" on a text
  file, "Ask agent about this file" on a picture. It sits left of the button, so a page's native window cannot cover it.
- **Select text in files** (2026-11-10, rules 242 to 244 and 246). The Text panel (and the Code view of a markdown file) selects like an editor: a drag,
  a double click for a word, a triple click for a line, Shift with a click to extend, Shift with the arrows, Home and End (Ctrl too for the whole
  file), Ctrl+A and Ctrl+C (the "Copied" badge shows). A drag held past an edge scrolls toward it; the wheel, the keys and the scrollbar scroll as
  before. Esc lets a selection go first and gives the keys back to the terminal only with nothing selected. The selection is kept as lines and
  characters, so it stays through scrolling, a splitter drag and the wrap button, and goes with a reload, another file or another tab. The text
  is what the panel shows: tabs as four spaces, a very long line as cut, rows a wrap made joined with no break. With text selected, Ask agent
  sends it framed as from `notes.md lines 12-40` (the name is cut first to fit the 64 character label); with none it still sends the path. The
  Compiled and Markdown views are pages and keep the browser's own selection. The Changes panel has none. An empty file (or empty text an agent
  shows) takes every click and key and simply selects nothing.
- **The frame.** `About this from <label>:` then `> ` per line, at most 4 KB with `> [cut]`. The label is the host or file name, cleaned and capped at
  64 characters; a page Flare made itself reads "this page", and a markdown view's page reads the file's name (rule 241). A web page's host is the one its document gave with the text (`location.href`), so a click
  while the next page loads still names the page that is showing.
- **Refusals.** A short note over the terminal for 4 s: no agent, a question box up, no selection (mini-app and plugin pages, or a page that is not
  `http` or `https`), a page that did not answer, answered in a shape that names no page, or answered with more than the script can give (about
  400 KB, checked natively), a path that cannot be typed.
- **Narrow panels.** Below 380 px the button is its glyph alone; below 200 px it is gone.
- **Not in this version.** A row in the page's own right-click menu, a right-click menu in the Text panel, selection in the Changes panel, any
  auto-submit, any ctl, MCP or plugin way in.
- **Tests.** `ask_agent::tests` (hostile text and titles, `frame_page`), `appctl::tests::ask_tests` (fake shell and fake browser), `ui_tests::panel`
  (`ask_agent_*`, the tooltip included, and the text panel's drag, wheel and highlight), `panel_sel::tests` (positions, wrapping, words, lines,
  wide characters, the label) and `appctl::tests::text_sel_tests` (the pointer, the keys, the copy, the drag's scroll, Ask agent).
- **Not covered.** A real WebView2 selection or title and a real agent's prompt: no e2e scenario. Where the tooltip lands is not checked in a real
  window.

## Image thumbnails on hover (#17), 2026-11-09

Point at a picture's path in the terminal for 350 ms and a small picture appears over the grid, above or below the line so it never covers it. Rows cannot be added under a line without changing the agent's text, so it is a floating bubble.

- **Look.** The tooltip's bubble: hairline border, shadow, 120 ms fade, picture up to 256 px on its longest side, caption with the file name and the picture's own size. It ignores the pointer. In a small window the picture shrinks to the side with more room; with no useful room it is not shown. It stays inside the grid, so a web panel (a native window) cannot cover it.
- **Hides** when the pointer leaves, the link under the cell changes (a scroll or output under a still pointer included, looked at again every tick), or on a key, press, tab switch, resize, DPI change, Find or a menu. After a key or a press that link stays quiet until the pointer has left it.
- **Loading.** A worker, one job at a time with only the newest waiting, sharing the panel's decode lock. Vetted like a click (rules 13, 14, 236). A relative path lit only through a folder the tab has shown keeps its picture and its click once the "no" for the tab's own folder has run out (rule 237). Limits 16 MB and 25 MP, below the panel's. Memory only, 32 pictures and about 8 MB, keyed by path, length and time, so an edited picture refreshes.
- **Setting.** "Show thumbnails when hovering image paths" (Settings > Terminal > Links, `flare.window.hoverThumbs`, default on). Off means no state and no worker.
- **Tests.** `thumbs` (the clock, the cache, the worker, placement), the `term_ui` rig (`resting_on_an_image_path_*`, every hide reason, the refusals), `preview` (`a_thumbnail_*`), `ui_tests::thumb` at several scales and the `appctl` wiring.

Not in this version: always-on thumbnails, web image URLs (network on hover), SVG (scripts), video, PDF, animation (first frame only), a disk cache.

## Markdown views, 2026-11-10

A `.md` or `.markdown` file opens **compiled** in the panel, every time (owner: "compiled by default"; no remembered preference). A control in
the panel's header switches between three views; it is Flare's own chrome, drawn outside the page.

| View | What you see | How |
|---|---|---|
| Compiled (default) | The formatted document: headings, lists, tables, code blocks, block quotes, GitHub callouts, task lists, footnotes | A page Flare writes |
| Markdown | The text with its markdown coloured (headings, emphasis, code, links, quotes, list and task markers, raw HTML) | A page Flare writes |
| Code | The text with line numbers and the wrap button, as before | The native Text panel |

- **Keeps its view.** A panel's pick stays with it across a restart (`"view"` in `panels.json`, written only once picked). Showing a file again,
  by a click or the agent's `show_file`, opens it compiled.
- **Safety (rule 239).** The file is untrusted. `flare_surfaces::mdview` writes every tag from pulldown-cmark's events (0.13.4, pinned with no HTML
  writer) and escapes all text: raw HTML shows as text, a picture shows its alt text, and a link has no address (its target is the tooltip) unless it
  jumps to a heading or footnote inside the page. No script; the policy `default-src 'none'; style-src 'unsafe-inline'; img-src data:` comes first.
  The page is a mini-app of its own in the sealed browser, with no network. Depth and size are capped and the render runs on a worker.
  Headings of one name are numbered with a counter per name (as GitHub's slugger does), so a file of millions of empty `#` lines names its
  anchors in linear time, and naming them stops too when the panel moves on.
- **Look.** The page follows the panel: light or dark, the panel's own ground and ink, Segoe UI for text and Cascadia Mono for code, at a width
  that reads well in a narrow panel. Narrow panels show the control as "Doc", "Md" and "Code".
- **Fallback (rule 240).** If rendering fails or the WebView2 runtime is missing, the file shows in Code. Picking a view tries again.
- **Ask agent (rule 241).** Works on selected text in the two page views, framed `About this from <file name>:`. With nothing selected the file's
  path is sent, as from Code.
- **Tests.** `mdview::tests` (escaping, links, anchors, pictures, caps, the stale stop, the look), `store` and `state` (the view and rule 40),
  `preview` (`is_markdown`, `read_source`), `appctl::tests::md_tests` (fake browser: the default, the control, the restart, both fallbacks, Ask agent).
- **Not covered.** A real WebView2 drawing the page: no e2e scenario yet.
- **Not in this version.** Links that leave the page and pictures (each needs its own safety rules: opening an address from an untrusted file,
  reading local image files), colouring for other code files, a `view` argument for agent tools. The Code view selects like any text file
  (2026-11-10, "Select text in files" above).

## Browser problems in the log, 2026-11-10

The diagnostics log now says what went wrong in the browser panel, so a failed 2FA hand-off (a shop sending you to your bank) can be
read afterwards. Spec: `docs/superpowers/specs/2026-11-10-web-popups-design.md`, section 3.

| Kind | When | A problem in `cargo diag` |
|---|---|---|
| `web-refused` | A navigation (`why`: `scheme`, `not-its-page`, `not-its-host`), a mini-app's or plugin's frame (`why: frame`) or app link (`why: app-link`; the web panel's app links too, which the browser never opens and Flare asks about), or a pop-up (`why: popup`, `popup-blocked` when no click asked for it, `popup-limit` past the limits) was refused | no |
| `web-popup` | A pop-up opened over the page (`to: popup`) or a new-tab link went to the panel (`to: panel`), and whether the person's click opened it (`user`) | no |
| `web-close-ignored` | The page called `window.close()`, which does nothing here | no |
| `web-load-failed` | A page failed to load, with the error's name (`cert-expired`, `name-not-resolved` and so on) | yes |
| `web-process` | A browser process failed: which one, and the reason and exit code when the Runtime says them | yes |
| `web-cert` | A certificate error, logged only: the browser shows its own error page as before | yes |
| `web-permission` | A permission the page asked for was denied (`camera`, `location` and so on) | no |
| `web-nav` | The chain of sites a page went through. Verbose only | no |

- **Site only (rules 110, 247).** Each line has the tab (`id`), the view (`web`, `app` for a mini-app, `mod` for a plugin's page), the
  `site` and the site of the page it came `from`. A site is the host in lower case, never the address: no port, login, path, query or
  fragment (`flare_surfaces::web::site_of`; an address with no host is its scheme, anything else `odd` or `unparsed`). The sites are made
  in `flare-web` (`WebNote::new`), so an address never leaves it for the log.
- **Rate-limited (rule 248).** One line per kind, site and word per 10 s, the copies counted as `repeats` (the events a line stands for
  besides itself); at most 10 lines per kind per 10 s, and what is held waits for its kind's next 10 s. A process failure and a certificate
  error always write their first line. `web-nav` steps are never folded, so a hop back (shop, bank, shop) stays in order.
- **Nothing changes in the browser.** The certificate handler reads the error and the address and never sets an action. A Runtime too old
  for the certificate event or the process reason simply logs less.
- **Reading it.** `cargo diag` lists the failed loads, process failures and certificate errors among the problems; `cargo diag -- --web`
  shows every web line, and `--web --tab <id>` one tab's.
- **Tests.** `web::tests` (`the_logs_site_is_the_host_alone_and_never_the_address`, the fixed words), `backend::tests`
  (`a_note_keeps_the_sites_and_never_the_addresses`), `web_diag::tests` (the gate), `reader::tests`
  (`the_web_panels_failures_are_problems_and_its_steps_show_under_web`), `web_ctl::web_line_tests` and the text-key scan in `neutral.rs`.
- **Not covered.** A real WebView2 raising these events: no e2e scenario yet.

## Arrows over the page, 2026-11-10

Spec `docs/superpowers/specs/2026-11-10-ask-agent-additions-design.md`, section 2; rules 254 to 256; the probe and the choices in
`docs/surfaces/decisions.md` ("Arrows over the page").

- **The agent points (`browser_point`).** `browser.point` `{ref | x, y, label?, clear?}` in the Browser namespace, under the same grant
  as the other browser tools. One orange arrow per tab, with an optional label on an orange pill (gated like Ask agent's text, one line,
  at most 48 characters). It never clicks and cannot type, and it is not a job: it scrolls its element into view and answers
  `{pointed, offscreen}` once it has measured it, then measures again about every 100 ms (`Page.getLayoutMetrics`,
  `DOM.getContentQuads`) so it follows a scroll or a resize, a moment late. It goes on `clear`, a new arrow, the person clicking or
  typing in the page, a navigation, the panel or tab closing, or after 60 s. It is drawn only where the agent's cursor may be (the page
  in front and live: not under a Flare menu, the still, another tab or a minimised window) and never over a pop-up; with a pop-up open
  the call answers "a pop-up is open, the person must finish it". The tool text says it is for steps the person must do themselves.
- **The person points ("Draw arrow").** A button beside Ask agent in the web bar (and in a mini-app's header), only on a tab with an
  agent, on a web page or a mini-app (never a plugin's page), and not while a pop-up is open. It puts a pad over the page: a layered
  window, every pixel at alpha 1, that catches the mouse and never takes the keyboard from the page. Press, drag and release are read on
  the 16 ms agent tick; a blue arrow follows live. On release Flare asks the browser, with no page script, what is under the head
  (`DOM.getNodeForLocation`, then `Accessibility.getPartialAXTree` for its role and name, the nearest element a look would list) and types
  this at the prompt, unsent: `About this arrow on <site>:`, `> points at: <role> "<name>"`, `> at: x, y in a W x H page`,
  `> (use browser_look to see it)`. A click with no drag points at the target from up and left. Cancel: the button again, a right click,
  Escape (in the page or in Flare's window), a menu, a tab switch, a pop-up or a navigation.
- **The agent sees the person's arrow.** It is kept for 5 minutes or until the page moves on. The next `browser_look` gets
  `user_arrow` `{from, to, ref, role, name}` (a ref is made for the element when the look did not list it) and its screenshot has the
  arrow drawn on it at the look's scale; after that look it is gone. The arrow is kept on the document, not the screen: while its page is
  in front the scroll is measured again, so the blue mark moves with what it points at and hides while part of it is scrolled out of
  sight, and the look gives its ends where they are in the look's viewport, `offscreen` and undrawn when the head is outside it
  (rule 257). `browser_look`'s text says where the person pointed, with the
  element's words inside the page-content markers.
- **Windows.** `flare_win::agent_cursor`: `compose_arrow` (only the pixels near the shape are worked out), `point_tail` (the agent's
  tail turns to stay on the page), `blend_onto_rgba`, `ArrowWindow`, `PadWindow`, and the stand-ins `RecordArrow`, `RecordMark`,
  `RecordPad` that the tests and `--e2e` keep. `web_ctl/arrows.rs` holds the rest.
- **Tests.** `agent_cursor::tests` (the arrow's pixels, labels, the tail, the screenshot blend, the stand-ins, the ignored `pad_probe`),
  `agent_web::tests` (`pointed_at`), `agent_job::tests` (`ref_for`), `ask_agent::tests` (`frame_arrow`, `point_label`), `ctl_app::tests`
  (`point_req`, the post), `web_ctl::arrows::tests`, `appctl::tests::arrow_tests` (fake browser, recording windows, a stand-in mouse:
  the arrow follows, every clear, hidden under a menu, a tab and a pop-up, the draw mode and every cancel, the frame, `user_arrow` once),
  `flare-mcp` `browser.rs` (the tool, the look's arrow lines).
- **Not covered.** A real mouse on the real pad over a real WebView2 (the probe used a plain child window and posted messages on a
  hidden desktop), a real agent's prompt, the arrow's look at every theme: no e2e scenario yet.

## Logins in the web panel last, 2026-11-10

- **Where the panel keeps a site's login.** Cookies, `localStorage` and saved passwords are in `<profile>\WebView2`: `%APPDATA%\Flare\WebView2` for
  the plain copy (`Flare.exe`, no `--user-data-dir`), `%APPDATA%\FlareStable\WebView2` for Stable (its shortcut's `--user-data-dir`). Before this,
  the plain copy kept them in `%LOCALAPPDATA%\Flare\WebView2`, inside its install folder, which a reinstall with Setup or an uninstall deletes;
  the first start of this build moves that folder and its `WebView2-<kind>` siblings (mini-apps, plugins) under the profile, once, and never
  over a folder already there. Rule 261.
- **A folder still in use waits, logins and all.** When that move cannot happen (the Flare that just quit still has a browser
  holding the folder), the folders stay together in the old place and the panel keeps using them there, never an empty new folder; the move
  is tried again when the first browser starts and at each start. Rule 262.
- **Dev and Stable have separate logins.** They are two profiles, each with its own panel profile, so a site signed in in Stable is not signed in
  in the plain copy, and the other way round. This is on purpose and is not merged.
- **What lasts.** A site's cookie with an expiry and its `localStorage` survive closing the panel, quitting Flare and installing a staged build
  (Build ready). A session-only cookie (a sign-in without "stay signed in") ends when the last web panel closes or Flare quits, since the panel's
  browser ends then. Held by the hidden e2e `webLogins` (a site of its own on `127.0.0.1`; the page reads `document.cookie` itself) and the unit
  tests in `crates/flare-surfaces/src/web.rs`.

## File links by marker, 2026-11-11

Spec and plan: `docs/superpowers/specs/2026-11-11-file-link-markers.md`. Owner, 2026-11-11: "the link system we have is
poor. many links dont work."

- **A file link is a path between `⟦` and `⟧`.** A session writes the path (relative to its folder, or absolute)
  between U+27E6 and U+27E7, optionally with `:line` or `:line:col`: `⟦C:\repo\src\main.rs:42⟧`. The whole span, marks
  included, is one link. Nothing is statted before it lights: the session wrote it on purpose. A relative path is a link
  only while the agent's or the tab's folder is known. Rule 266.
- **Read exactly, over rows.** `flare_term::file_marks` reads to the end mark in the same row and up to 7 rows below
  (at most 1,024 characters): a row the terminal wrapped joins with nothing dropped; a row Claude Code broke by hand
  joins with its leading spaces (Claude Code's indent) and the last row's trailing blanks dropped. A second start mark
  starts again; a blank row or no end mark is no link. A web address stops at a mark. Rule 267.
- **No more path guessing.** A plain shell's output (`dir`, `git status`, a compiler error) has no file links, even for
  names that exist. Web addresses (http and https) are still found in the text, also across a row broken by hand, and
  OSC 8 links are as before. `paths.rs`, `path_links.rs` (the cache, the shown folders, the questions to a worker)
  and the retry of a path that named nothing are gone; `path_open` (the allowlist, `open_term_path`, `StatGate`)
  stays. macOS: the `file://` and escaped-space readers went with the guesser.
- **The click.** A worker takes the first spelling that names something (as written, then with one space where a row
  broke by hand: each break alone, then all of them) and goes the way a path click always went: a picture or text file
  to the side panel with "Open previews in Flare" on, else the default app (allowlist), else Explorer. When nothing
  is there the panel says "Flare found no such file" (a note in a closed panel, the header or the page's bar in an open
  one). A text preview opens at the link's line with that line marked in the selection colour, a few lines below the
  top; a markdown file with a line opens in Code for it. The right-click menu keeps Open, Show in side panel, Show in
  Explorer (Finder on macOS) and Copy path; hover thumbnails work for a marked picture. Rule 268.
- **The menu and the thumbnail try the same spellings.** Open, Show in side panel, Show in Explorer, Copy path and
  the hover thumbnail act, on their worker, on the first spelling that names something, as the click does. A wide
  character the terminal moved to the next row leaves the last column empty: that cell is no space of the path (a space
  there is tried as a second spelling). Rule 270.
- **A path in a table cell (2026-11-11, review).** Claude Code draws a markdown table with box borders and breaks a long
  cell inside its column. A span whose start mark has a vertical border (`│`, `┃` or `║`) on each side is read in its
  cell: each row stops at the cell's right border (blanks before it dropped) and goes on in the next row right of the
  cell's left border, which that row must have at the same column (the table's bottom edge has a corner there: no
  link). The link covers only the path's cells. Rule 271.
- **Short paths, read from the session's folder (2026-11-11, review).** The note now asks for a path relative to the
  session's working folder (`⟦app\src\favorites\favorites.ts:17⟧`), an absolute path only for a file outside it, and a
  short marked path in a table. Every `UserPromptSubmit` hook sends the `cwd` of its input along with `panel.state`, and a
  new `PostToolUse` hook of `EnterWorktree` and `ExitWorktree` (`flare-mcp --hook-context --folder`) does only that.
  Flare keeps it per tab as the agent's folder (the latest wins; it goes when the agent leaves the tab, at a shell
  prompt or when the tab closes) and reads a relative file link from it, else from the tab's own folder: the click, the
  menu rows, the hover thumbnail and the lighting of a relative mark. A Flare that does not know the field ignores it.
  Rule 272.
- **A link that names nothing goes to no app (2026-11-11, owner report).** The menu's Open, Show in Explorer (Finder)
  and Show in side panel now say "Flare found no such file" when no spelling of the link names anything, as the click
  does; before, Open and Show in Explorer did nothing at all. No route hands such a path to an app. Rule 273.
- **Copy path copies a path that exists, or exactly what was written (2026-11-11, owner report).** Copy path on every
  file link copies, from a worker, the first spelling that names something as a whole path; when none does, the text
  as written. A path in a table cell is copied without the table's borders or the cells beside it. Rule 274.
- **The agent's folder reads relative links only (2026-11-11, review).** A path rooted with no drive (`\x`) and `~`
  are read from the tab's own folder, as before. An agent folder no relative path can be read from (a network or
  `\\?\` path) is not used: the tab's own folder is. Rule 275.
- **`show_file` takes `line`.** The same scroll and mark as a click (`view.file`'s `line`, from 1).
- **Every Claude session is told.** The Flare plugin's `SessionStart` hook (start, resume, clear and compact) runs
  `flare-mcp --hook-context --session-start`, which prints, only inside a Flare tab, the note "You are running inside
  Flare. When you mention a file in your reply, write its path between ⟦ and ⟧, ..." as
  `additionalContext`. The MCP server's `instructions` end with the same note, so a session that skips the hook still
  sees it. Codex and Antigravity are not told (no Flare plugin), but any program that prints marks gets links. Rule 269.
- **Tests.** `file_marks::tests`, `link_scan::tests` (`a_plain_listing_has_no_file_links`,
  `a_marked_path_is_a_file_link_with_its_line`), `emulator::tests` (`a_marked_path_broken_by_hand_and_indented_is_one_link`),
  `term_ui::tests` (`a_click_on_a_marked_line_asks_for_the_preview_at_that_line`,
  `a_click_tries_a_space_where_the_row_broke_when_the_joined_path_names_nothing`,
  `a_click_on_a_marked_path_that_names_nothing_says_so_and_opens_nothing`), `appctl::tests`
  (`a_preview_at_a_line_marks_that_line_and_a_later_one_without_marks_nothing`), `flare-mcp` (`hook::tests`, the
  stdio test `the_session_start_hook_tells_a_flare_session_about_file_links_once`), `plugin::tests`
  (`the_session_start_hook_tells_claude_about_file_links_on_every_start`,
  `entering_or_leaving_a_worktree_tells_flare_the_sessions_folder`), `ctl_app::tests`
  (`panel_state_carries_the_agents_folder_only_when_it_is_a_plain_absolute_path`), `appctl::tests`
  (`a_relative_file_link_opens_under_the_agents_folder_not_the_shells`) and the hidden e2e `markerLinks` (a table cell,
  and a relative mark read from the folder the real hook names), which replaces `pathLinks`.

## Remote sessions, PR 1: the stdio wire, the remote grant and remote.hello, 2026-11-12

Library and docs only: nothing a person sees changes yet. Spec `docs/superpowers/specs/2026-11-12-remote-sessions-design.md`,
plan `docs/superpowers/plans/2026-11-12-remote-sessions.md` (PR 1). Contract: `docs/surfaces/protocol.md` (section 1
"Remote link", section 4, `remote.hello`).

- **A wire over stdio.** `flare_ctl::StdioWire` speaks the control protocol over two byte streams: the ssh child's
  (`from_child`, the viewer) or the serve process's own stdin and stdout (`from_process`, binary, the host). A reader
  thread fills a 1 MB queue and the probe never waits. Held by the `stdio::tests`.
- **The remote grant.** A new `remote` grant, given only to `flare --serve-remote`'s own stdio after `remote.hello`,
  reaches the `remote.*` methods and nothing else; the pipe never gives it. Rule 276.
- **remote.hello.** Both ends say their remote protocol, machine and Flare version; a mismatch is refused with one line
  naming both (`remote protocol 1 here, 2 there`). A login banner before the first message is skipped. Rules 277, 278.
- **For other crates' tests.** The `testing` feature of `flare-ctl` opens `framing::testwire` (an in-memory wire pair).

## Remote sessions, PR 2: serve mode and the host bridge, 2026-11-12

A host Flare now serves remote sessions; nothing a person sees in the window changes yet (the viewer tab is PR 3).
Plan `docs/superpowers/plans/2026-11-12-remote-sessions.md` (PR 2). Contract: `docs/surfaces/protocol.md` (section 1
"The serve process", section 4 `bridge`, section 5 `remote.*`).

- **`flare --serve-remote`.** The host end of `ssh <host> flare --serve-remote`: a headless bridge with no window and
  no single-instance lock (rule 283). It answers the link's `remote.hello` itself and relays every other `remote.*` call
  to the Flare running on that machine, over its pipe, with the bridge key the running Flare writes into its profile at
  every start (`remote-bridge.key`, the `bridge` grant, rule 284). With no Flare running the hello answers and every
  call is `unavailable`. Held by `serve_remote::tests` and `tests/serve_remote.rs`.
- **Run, list, follow, type, size and close a host tab.** `remote.open` (a tab in a host folder, optionally resuming a
  Claude session), `remote.ls` (one host folder, 500 names at most), `remote.tabs`, `remote.attach` and
  `remote.detach`, `remote.input` (typed intents: keys, committed text, sanitised pastes; rule 279), `remote.resize`
  (the remote viewer owns the size until a key is typed on the host; rule 282), `remote.output` (numbered bytes from a
  4 MiB ring, polled; rule 281), `remote.snapshot` (the screen and history as text, for a viewer that fell out of the
  ring) and `remote.close` (asks first like any close, rule 48). A detach or a dropped link never ends a session
  (rule 280). Held by the `ctl_app::tests` `remote_*` tests, `tab_host::tests`, `flare_term::tail::tests` and the
  `handle::tests` `remote_*` tests.
- **A snapshot holds a half-drawn frame; a slow call holds up nothing.** A snapshot taken inside an open synchronized
  update (DEC 2026) lets the update go first, so the bytes it kept back are in the text (rule 285). The bridge sends
  attach, detach, typing, resize and close in turn on one connection and every other call on a pooled one, so a slow
  folder read or open never holds up typing or the polls; a call whose turn comes after its time is answered `timeout`
  and never sent, and a timeout no longer drops the connection that follows the tabs (rule 286).
- **Not yet.** The viewer tab, reconnect and the Mac/PC tag (PR 3), status and conversation (PR 4), the hidden start of a
  host Flare that is not running (PR 5). (A snapshot keeps its colours since 2026-11-13, rule 1213: see "Remote tab
  extras" below.)

## Remote sessions, PR 3: the viewer tab, reconnect and restore, 2026-11-12

A Flare can now show a tab that runs on another machine. There is no menu or command to open one yet (PR 5 and PR 7):
the machine is hand-written in the profile's `machines.json`, and only the e2e opens a tab. Plan
`docs/superpowers/plans/2026-11-12-remote-sessions.md` (PR 3). Contract: `docs/surfaces/protocol.md` (section 1 "The
viewer").

- **A remote tab.** A shell tab whose session is the host's (`Tab.remote`, rule 287). It draws the host's output like a
  local shell (the same batcher, emulator, scrollback and OSC parsing) through a `RemotePty`, and what is typed in it
  goes to the host as keys, text and pastes, never raw bytes; the emulator's answers to the program stay here. Its
  file links are not opened and a file dropped on it is not typed (its `localhost` Preview chip and its folder in the
  recent folders came 2026-11-13, rule 1213). Held by `remote_pty::tests`, `session::remote_tests` and
  `ui_tests::term::flow::a_remote_tab_shows_the_hosts_output_and_takes_typing`.
- **The link.** One `ssh -o BatchMode=yes -T -- <host> '<flare>' --serve-remote` per remote tab, no console window, no
  token and no environment on the line (rule 290); `machines.json` names each machine's id, name, kind (`mac` or
  `pc`), ssh host, user and Flare path. Output is polled every 30 ms while it flows and up to 250 ms apart when idle;
  typing is sent at once, batched; sizes are coalesced. Held by `remote_link::tests`.
- **Reconnect and restore.** A broken link is tried again after 1 s, then 2, 4 and up to 30 s, and the tab goes on from
  the output it read, no line twice; a tab that fell out of the host's ring starts again from the host's screen. Closing
  the tab closes the session on the host (asking first there, as any close); quitting Flare only lets go, and the
  session goes on. `tabs.json` keeps the host's tab, so after a restart the tab attaches to the same session and shows
  its screen (rule 288). Held by `remote_link::tests`, `restore::remote_tests` and the e2e `remoteViewer`.
- **The host's screen, cursor and pastes.** A tab that starts from the host's screen puts its cursor where the host's
  is, so what is typed next lands in the same place on both (rule 293). A paste in a remote tab always goes to the
  host as one paste, and the host brackets it as its shell asked, so a pasted line never runs before Enter (rule 292).
  Held by `handle::tests::a_snapshot_puts_the_viewers_cursor_where_the_hosts_is` and
  `handle::tests::a_paste_on_a_remote_tab_crosses_as_one_paste_whatever_the_local_mode`.
- **Replays raise nothing.** The host's screen and what a reconnect catches up on never raise Finished, Question or a
  working mark here; live output still does (rule 289). Held by
  `agent_hub::tests::a_remote_tabs_replay_raises_no_mark_and_live_output_does`.
- **Not yet.** Opening a remote tab from the + menu, a command or an agent (PR 5, PR 7), the host's status words and the
  conversation view (PR 4), file links and file drops over the link (PR 6), the Mac/PC logo on the tab (PR 7). (A
  remote tab keeps its colours after a reconnect since 2026-11-13, rule 1213. Typing while the link is down is kept since
  2026-11-13, rule 1210: see "Remote link edge cases" below.)

## Remote sessions, PR 4: status, conversation and keep-awake, 2026-11-12

A remote tab now shows what its agent does on the other machine, the host stays awake while such a session works, and
the host can send what is said in the session. No new screen (the fitted conversation view is PR 7). Plan
`docs/superpowers/plans/2026-11-12-remote-sessions.md` (PR 4). Contract: `docs/surfaces/protocol.md` (section 1 "The
viewer", section 5 `remote.status` and `remote.convo`).

- **Marks from the host's words.** The tab's link asks the host for its status every 1.5 s (`remote.status`): one word
  a tab (working, question, done, failed, idle), the agent, its session and the question's words. A remote tab shows
  work and a question as the host says them, with the host's words in its question notification. Finished and Failed
  are this machine's own: raised only when it showed the tab working and nobody here looked, cleared by looking here,
  and never raised again by a status read after a reconnect (rule 294). A host whose window looks at the tab goes from
  working straight to idle, and that idle is still an end of the work here; a passing refusal of `remote.status` (the
  host busy for a moment) never stops the polls, and a question taken down here by an answer key comes back with the
  host's next question (rule 298). An older host without `remote.status` keeps working, without the words. Held by `remote_status::tests`, `remote_link::tests`, `agent_hub::tests`,
  `indicator::tests` and `ui_tests::agent_flows::remote_flows`.
- **One notification per question.** A question in a tab followed from the other machine is announced there, never
  also on the host (rule 295). Held by `remote_flows::a_question_in_a_followed_tab_is_the_viewers_to_notify`.
- **Keep-awake.** The host holds off system sleep while a tab followed from the other machine has an agent at work,
  and lets go when the work stops, the viewer goes or Flare ends; the display still sleeps and the screen still locks
  (rule 296, owner decision). Held by `keep_awake::tests` and
  `remote_flows::a_followed_tab_holds_the_host_awake_while_it_works_and_lets_go_after`.
- **The conversation over the link.** `remote.convo` sends what is said in the tab's Claude session as numbered
  entries; the host finds and reads the transcript itself, no path crosses, and a viewer takes each entry once (rule
  297). Held by `remote_convo::tests` and `ctl_app::tests::remote_convo_sends_the_hosts_transcript_as_numbered_entries_and_no_path`.
- **Not yet.** The fitted conversation view that shows those entries (PR 7). When the viewer's link drops, the host
  takes its questions back and announces an open one itself.

## Remote sessions, PR 5: starting sessions (command, agent tool, hidden start), 2026-11-12

A remote session can now be started without the + menu (PR 7): by a command, by an agent, and on a host whose Flare is
not running. Plan `docs/superpowers/plans/2026-11-12-remote-sessions.md` (PR 5). Contract: `docs/surfaces/protocol.md`
(section 1 "The hidden start", section 4, section 5 `open.remote`).

- **`flare open --on <machine> <folder> [--resume <id>]`.** Asks the Flare running here for a tab on that machine (an
  id in `machines.json`) in that folder, in front, resuming that Claude session when asked; prints one line and exits.
  The folder is the other machine's and is never looked at here; `--on`'s value never reaches path parsing (rule 300).
  When no Flare runs here it starts one hidden first. Held by `config::tests`, `cli_open::tests` and the e2e
  `remoteOpen`.
- **The agent's `open_remote_session` tool.** An agent may open a session on a linked machine with no prompt (owner
  decision): `open.remote`. The tab opens behind the one in front, and the agent cannot type into it or read it (rule
  299). A machine `machines.json` does not name is refused with the names it does. Held by
  `grant::tests::the_agent_may_open_a_remote_session_but_never_type_into_one`, `ctl_app::tests`, the `flare-mcp`
  tests and `ui_tests::term::flow::an_agents_remote_open_stays_behind_the_tab_in_front`.
- **Hidden start.** When the other machine's Flare is not running, the link's `flare --serve-remote` starts it with
  `--start-hidden` before it answers: a normal Flare whose window stays parked off every screen and never takes the
  focus (the e2e's park code); a later normal launch brings it forward. A Windows login over sshd (session 0) cannot
  start one: the tab shows one line (`host_not_ready`) and connects once Flare runs there (rule 301). Held by
  `hidden_start::tests`, `serve_remote::tests`, `window::tests` and the e2e `remoteOpen`.
  On a Mac the hidden window is clear and click-through from its first order-in, as the Mac e2e's is, and its first
  raise makes Flare a regular app again with an opaque window that takes clicks (rule 302). Held by
  `window::tests::a_background_start_on_a_mac_is_born_clear_and_its_raise_undoes_the_mac_park` and the macOS
  `window_park::tests`.
- **`machines.json` registry.** `crates/flare/src/machines.rs` reads and writes the hand-edited list (whole-file
  writes, unknown kinds left unknown). Held by `machines::tests`.
- **Not yet.** The past-sessions list (`remote.sessions`) is not built: the owner's PR 7 picks drop it from the +
  menu, and `--resume` takes an id. Whether a Mac's sshd login can start the app hidden, and a raise of a hidden Mac
  window on a real screen, are on the by-hand list.

## Remote sessions, PR 6: file links, image paste and file drop across the link, 2026-11-12

A remote tab's files now cross the link both ways, and both directions (a PC viewing a Mac, a Mac viewing a PC) use
the same code. Plan `docs/superpowers/plans/2026-11-12-remote-sessions.md` (PR 6). Contract:
`docs/surfaces/protocol.md` (section 1 "The viewer", section 5 `remote.fetch` and `remote.put`). Checks only a real PC
and Mac can make: `docs/remote-sessions-manual-checks.md`.

- **File links from the other machine.** A click on a file link in a remote tab asks the tab's link for the host's
  file (`remote.fetch`), never a file of this machine. The host reads it only from the tab's folder or its own temp
  folder, as the file system names it (no `..` or link out, no device, no share), at most 32 MB (rule 303). A copy is
  kept in this machine's temp folder: a picture or a text file shows in the side panel; anything else opens the way a
  clicked file opens (a program is shown in Explorer or Finder, never run). A link the host has no file for says "No
  such file" as a local one does. The copies go when the tab closes. Held by `remote_files::tests`,
  `remote_link::tests::a_file_link_is_fetched_by_the_first_spelling_the_host_has_in_chunks`,
  `term_ui::tests::a_click_on_a_remote_tabs_file_link_asks_the_link_and_opens_nothing_here`,
  `ui_tests::term::flow::a_remote_file_link_is_fetched_by_the_link_and_shown_from_a_copy_in_the_panel` and the e2e
  `remoteFiles`.
- **Drop and paste of files and pictures.** A file dropped on a remote tab, copied files pasted there, and a picture
  pasted there (as a PNG) are copied to the tab's own folder in the host's temp folder first (`remote.put`, at most
  32 MB a file), under a clean name; the host's path, quoted for the host's shell, is then pasted as one paste. This
  machine's path is never typed into the host's shell, and no image key is sent (the host's Claude cannot read this
  machine's clipboard) (rule 304). A dropped folder is copied whole since 2026-11-13 (rule 1213). Held by `remote_files::tests`,
  `remote_link::tests::a_put_copies_the_files_then_pastes_the_hosts_quoted_paths_as_one_paste`,
  `term_ui::tests::a_drop_on_a_remote_tab_is_never_typed_and_goes_to_the_app_to_copy`,
  `ui_tests::term::flow::an_image_pasted_on_a_remote_tab_goes_to_the_host_as_a_png` and the e2e `remoteFiles`.
- **A remote paste chooses as a local one.** Ctrl+Shift+V (plain) on a remote tab pastes the text only and copies
  nothing to the host. An Excel or Word copy (text plus a picture) pasted at a plain prompt pastes its text; the picture
  goes to the host only when an agent runs in the tab or there is no text (rule 1190). Held by
  `clip_flow::tests::a_remote_paste_copies_files_or_an_image_only_where_a_local_paste_would_use_them` and
  `term_ui::tests::a_remote_tab_pastes_the_text_of_an_excel_copy_and_a_plain_paste_never_copies_to_the_host`.
- **A live run's copied files stay.** Each run locks a file in its own `flare-remote` and `flare-remote-copies` folder
  while it lives, so another Flare starting on that machine sweeps only the folders of runs that are gone and a day
  old, never a running one's (rule 1191). Held by
  `remote_files::tests::a_live_runs_folders_are_never_swept_however_old_and_a_gone_runs_are`.
- **The ssh command for each kind of host.** A Mac's command is quoted for sh, a PC's for cmd (no `&&`, nothing cmd
  expands), from separate argv items with no secret (rule 305). Held by
  `remote_link::tests::the_remote_command_table_quotes_each_hosts_shell`.
- **Not yet.** Real ssh between the PC and the Mac is on the by-hand list. (A dropped folder is copied since 2026-11-13,
  rule 1213. A Mac's
  TIFF-only clipboard picture is copied as a PNG since 2026-11-13, rule 1198.)
## Remote sessions, PR 7: Settings > Machines, the + menu's machines, tab logos and the fitted conversation, 2026-11-12

The remote sessions get their screens, as the owner picked them in the 2026-11-12 mockups
(`docs/superpowers/specs/remote-sessions-mockups/`). Plan `docs/superpowers/plans/2026-11-12-remote-sessions.md` (PR 7).
Contract: `docs/surfaces/protocol.md` (`remote.hello`'s `os`, `remote.status`'s `sized`, `remote.open`'s `~`).

- **Settings > Machines.** A page after Plugins with no subtext (a Not linked machine's reason line aside). Linked
  machines are a table: the machine's logo (Apple for a Mac, Windows for a PC) and name, its SSH host, the Flare
  version it said, its status ("Linked" with a green dot, or "Not linked"), and a ⋯ menu with Test and Remove. A
  machine that is Not linked shows why as one small grey line under its row, from the name's left edge, elided on that
  one line; the status has no tooltip (Max's rule 2026-11-13: a control with words gets none). Rule 1205. "+ Add a
  machine"
  opens a form in its place: an SSH host drop-down (the plain hosts of `~/.ssh/config` and its `Include`s), a Name
  field, Cancel and Add. Add checks the link first (Flare on the host's PATH, then the Mac app's) and saves only a
  machine that answered; a failed check is one red line under the form (for example "Flare there speaks remote
  protocol 0, here 1. Update it there."). Every machine is checked once a run when the page opens. Find a setting finds
  it ("ssh", "add a machine"). Rule 1194. Held by `ui_tests::settings_machines::*`, `machines_page::tests`,
  `ssh_config::tests`, `machines::tests` and the `flare-settings` page tests.
- **The + menu.** One row per linked machine after the folders, its logo and its name; a pick opens a shell tab on
  that machine in its home folder (`remote.open {folder: "~"}`), where Claude is started or resumed by hand. No
  submenu. Held by `menus_ui::tests::each_linked_machine_is_one_row_after_the_folders_with_its_logo_and_name`,
  `ui_tests::term::flow::a_machines_row_in_the_plus_menu_opens_a_shell_there_wearing_its_logo` and the e2e `remoteUi`.
- **Tab logos.** A remote tab wears its machine's logo: in vertical tabs before the folder path on the card's second
  line (Clawd stays in the pane), in horizontal tabs at the right after the status ring and before the ×. Its tooltip
  names the machine. A restored tab takes the logo once the machines are read. Held by
  `tabs_ui::tests::a_remote_tabs_row_carries_its_machines_logo_and_name`.
- **Hide here only.** A remote tab's right-click menu has "Hide here only" after "Close tab": the tab goes from this
  window and the session goes on on its machine. Close still ends it everywhere and asks first while an agent works
  (rule 1192). Held by `ui_tests::term::flow::hide_here_only_detaches_and_close_ends_the_session_everywhere` and the
  e2e `remoteUi`.
- **The fitted conversation view.** While the other machine sizes a remote tab (someone typed there last) and the tab
  has a Claude session, this screen shows the conversation wrapped to its own width instead of a terminal at the other
  window's size, with "Live on <machine>. Click or type to take the terminal here." above it. A click or a key brings
  the live terminal back at this window's size; the key goes on to the tab (rule 1193). Held by
  `ui_tests::term::flow::a_remote_tab_the_host_sizes_shows_its_conversation_until_a_key_takes_it_back` and
  `ui_tests::remote_convo::*`.
- **The host's own fitted view, 2026-11-13.** While a remote viewer sizes an agent tab of the host (someone typed on the
  viewer last) and that tab is in front, the host's window shows the same fitted conversation view of the tab's own
  Claude session, with "Live on <viewer's machine>. Click or type to take the terminal here." above it, instead of the
  terminal at the viewer's size. A key typed there or a click takes the size back: the terminal returns at the host
  window's size, the key goes on to the shell, and the viewer's window shows its own fitted view. A plain shell keeps
  its terminal at the viewer's size. The viewer's machine is the name its link said in `remote.hello`; an older
  host-side bridge names none, and the line then says only how to take the terminal back (rule 1211). Held by
  `ui_tests::agent_flows::remote_flows::the_hosts_own_window_shows_the_fitted_view_while_a_viewer_sizes_an_agent_tab`
  (every shipped scale) and `appctl::remote_glue::tests`.
- **Only after the viewer types, 2026-11-13.** The host's own fitted view now waits until the viewer really typed in
  the tab while it holds the size. A viewer that only attaches, reconnects, refits or switches to the tab still sets
  the size, but the host keeps its live terminal; a key typed on the host ends it until the viewer types again
  (rule 1212). Held by `tab_host::tests::a_viewer_typed_last_only_after_typing_while_it_sizes_the_tab` and
  `ui_tests::agent_flows::remote_flows::a_viewers_size_alone_never_puts_the_hosts_fitted_view_up`.
- **Not yet.** Adding a machine reached
  as a user other than the one in `~/.ssh/config`, or with Flare elsewhere than on the PATH, a PC's two installs or
  `/Applications`, still needs a hand edit of `machines.json`. Real ssh between the PC and the Mac is on the by-hand
  list.
- **A PC as host, 2026-11-13.** A PC whose ssh login shell is PowerShell (sshd's `DefaultShell`) works as a host: the
  viewer runs `cmd /c "<path>" --serve-remote` there, which PowerShell and cmd both run and wait for. Settings >
  Machines finds the PC's Flare at `%LOCALAPPDATA%\StableFlare\current\FlareStable.exe` or
  `%LOCALAPPDATA%\Flare\current\Flare.exe` when it is not on the PATH, and `FlareStable.exe` started by ssh with no
  `--user-data-dir` serves the Stable profile (`%APPDATA%\FlareStable`), not the Dev one (rule 1207). Held by
  `the_remote_command_table_quotes_each_hosts_shell`, `adding_a_pc_finds_its_stable_install_and_saves_it_as_a_pc`,
  `each_candidate_is_probed_with_the_quoting_its_path_implies` and
  `a_renamed_exe_with_no_flag_takes_the_profile_of_its_name`.
- **A PC with a space in its profile folder, 2026-11-13.** Under a PowerShell login shell, Settings > Machines also
  finds the PC's installs at `$env:LOCALAPPDATA\...` when `%LOCALAPPDATA%\...` does not answer (a profile folder such
  as `C:\Users\John Doe`). A PC path with a curly quote is refused. A Windows host whose login shell is Git Bash keeps
  the sh quoting that answered, with no PC logo (rule 1208). Held by
  `a_pc_whose_profile_folder_has_a_space_is_found_through_powershells_variable`,
  `a_pc_that_answered_in_sh_quoting_keeps_that_quoting` and `the_saved_kind_keeps_the_quoting_that_answered`.

## Remote link edge cases, 2026-11-13

Three fixes to a remote tab's link when it is slow or drops, owner approved 2026-11-13 (rule 1210). Contract:
`docs/surfaces/protocol.md` (section 1 "The viewer", section 5 `remote.open` and `remote.attach`).

- **Attach right after open.** A new remote tab's first attach can come before the host's new shell runs; the host
  answers "that tab's shell is not running". The viewer asks again on the same link every 250 ms for up to 10 s, so a
  new tab no longer costs a second ssh connect. Held by
  `remote_link::tests::an_attach_the_host_cannot_take_yet_is_tried_again_on_the_same_link` and
  `an_attach_still_unavailable_after_the_patience_drops_the_link_and_the_next_one_attaches`.
- **No second tab on a lost open.** Each remote tab sends an open id with `remote.open`, the same on every reconnect.
  When the link breaks after the host opened the tab but before its answer came back, the open sent again gets the
  same tab, not a second one. The host remembers 64 ids for 10 minutes. An older host without the id still opens the
  tab (the viewer opens again without it). Held by `remote_link::tests::an_open_whose_answer_was_lost_opens_no_second_tab`,
  `an_older_host_that_refuses_the_open_id_still_opens_the_tab`, `remote_host::tests::an_open_id_is_remembered_ten_minutes_and_at_most_64_of_them`,
  `ctl_app::tests::remote_open_checks_the_host_folder_and_the_resume_id_first` and
  `remote_flows::a_remote_open_sent_again_with_its_open_id_opens_no_second_tab`.
- **Typing while the link is down is kept.** Keys, text and pastes typed in a remote tab while its link is down are
  kept and sent in order right after the next attach. Typing older than 30 s, or past 64 KB in all (and the rest of
  that outage's typing after it), is dropped, and the tab shows one red line: "Some typing while the link to <machine>
  was down was not sent". When the host's tab is gone (`no_tab`), the kept typing is dropped and never typed into
  another tab. Held by `typing_while_the_link_is_down_is_sent_in_order_after_the_next_attach`,
  `typing_older_than_the_kept_age_is_dropped_and_the_tab_says_so`,
  `typing_over_the_cap_while_the_link_is_down_is_dropped_and_the_tab_says_so`,
  `typing_kept_for_a_host_tab_that_is_gone_is_typed_nowhere` and
  `the_kept_typing_keeps_order_and_drops_old_and_over_the_cap_with_no_hole` (all `remote_link::tests`).
- **Not yet.** Typing in a `remote.input` call that the link broke under is not sent again (the host may have typed it
  already). Against a host older than this change, a lost open can still leave a second tab.

## Remote tab extras, 2026-11-13

Four gaps of a remote tab closed, owner approved 2026-11-13 (rule 1213). Contract: `docs/surfaces/protocol.md`
(section 1 "The viewer", section 5 `remote.snapshot` and `remote.put`). Checks only a real PC and Mac can make:
`docs/remote-sessions-manual-checks.md`.

- **Preview chip over an ssh tunnel.** A remote tab that prints a local server address (`http://localhost:5173`) now
  shows the Preview chip as soon as its screen names it (the server is the host's, so no port of this machine is
  looked at). A click opens an ssh tunnel to the tab's machine, from the same machine entry the link uses (`ssh -N -o
  BatchMode=yes -o ExitOnForwardFailure=yes -L 127.0.0.1:<free port>:localhost:<port> -- <host>`, every part its own
  argv item, never a shell string), on a worker, and the side panel opens at `http://127.0.0.1:<free port>/`. One
  tunnel per tab and host port, used again by the next click; it closes when the chip goes (the screen cleared, another
  address) or the tab closes. A tunnel that does not come up in 20 s is closed and said in one line in the tab's
  panel; nothing waits on the UI thread. Under `--e2e` a loopback forwarder stands in for ssh. Held by
  `remote_tunnel::tests` and `ui_tests::term::flow::a_remote_tabs_preview_chip_opens_the_page_through_a_tunnel_to_its_host`.
- **Remote folders in the recent folders.** A remote tab's folder joins the recent folders with its machine (a tab
  opened in the host's home folder: the first folder the host's shell names). The + menu lists up to 3 of them after
  this machine's folders, each with its machine's logo before its name and the machine's name on its right, and no pin;
  a pick opens a remote tab on that machine in that folder. A machine removed from Settings > Machines takes its folders
  off the list. They are kept apart from this machine's list (`flare.recentRemoteRoots`). Held by
  `recent_roots::tests::remote_folders_are_kept_with_their_machine_newest_first`,
  `menus_ui::tests::remote_folders_follow_the_local_ones_with_their_machines_logo` and
  `ui_tests::term::flow::a_remote_tabs_folder_joins_the_recent_folders_and_opens_there_again`.
- **Colours after a reconnect.** A remote tab that starts from the host's screen (a new tab, a restored one, one that
  fell out of the host's ring) now shows the host's colours, bold, italic, underline and inverse at once, not only on
  new output: the snapshot is asked styled. A title, a mark or a mode still never rides on it (rule 289). Against an
  older host the snapshot is plain, as before. Held by `emulator::tests::a_styled_row_carries_its_colours_and_draws_the_same_cells_again`,
  `handle::tests::a_styled_snapshot_shows_the_hosts_colours_and_keeps_the_screen_under_its_cap`,
  `remote_pty::tests::a_styled_snapshot_keeps_its_colours_and_nothing_else` and
  `remote_link::tests::a_snapshot_is_asked_styled_and_an_older_hosts_refusal_falls_back_to_plain`.
- **Folder drop.** A folder dropped on a remote tab (or copied and pasted there) is copied whole into the tab's upload
  folder on the host, its folders and files with their names (dot files keep theirs), links and junctions skipped;
  then the host's path of the folder, quoted for its shell, is pasted as for a file. Over 5000 files, 200 MB in all, or
  a file over 32 MB: nothing is copied and the panel says so in one line. A host older than this change takes single
  files only and says so the same way. Held by `remote_files::tests`, `appctl::remote_files_glue::tests`,
  `remote_link::tests::a_dropped_folder_is_copied_whole_and_only_its_path_is_pasted`,
  `remote_link::tests::a_folder_over_the_caps_or_to_an_older_host_copies_nothing` and
  `ctl_app::tests::remote_put_makes_a_dropped_folder_and_fills_it_and_a_snapshot_takes_styled`.
- **Not yet.** A "resume a past session there" list in the + menu (left out on purpose: open a shell there and resume
  Claude by hand; `flare open --on <machine> --resume <id>` still exists for scripts and agents). The tunnel and the
  folder copy over a real ssh between the PC and the Mac are on the by-hand list. The e2e has no tunnel scenario yet:
  the loopback stand-in is held by a unit test.

## Tooltips only on icons, 2026-11-13

- **A control with a text label gets no tooltip; an icon-only control does.** Max, 2026-11-13. The "Build ready" and
  "Update <version>" chips in the title bar no longer show one (the Build ready tip showed far from the chip); their
  accessible names are unchanged. The settings cog, help, window buttons and plugin icons keep theirs, and so does the
  Preview chip, whose tooltip shows the server's origin that its label leaves out (rule 1201). Held by
  `hovering_the_build_ready_chip_shows_no_tooltip` and `hovering_the_update_chip_shows_no_tooltip`.
