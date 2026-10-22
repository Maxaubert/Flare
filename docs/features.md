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
  drawn again on `TaskbarButtonCreated` (L14). The stack sampler's abandon wait is bounded (L15).
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
  base 13 px, size steps 50 to 200 percent. Ctrl+wheel zooms the one tab between 7 and 64 px and is
  never saved. Held by `a_theme_leaves_the_font_and_its_size_alone`, `zoom_is_never_saved` and `zoom_stops_at_7_and_64`.
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
- **Paths.** A path is a link only when it exists; the backend resolves and checks it again; a click
  opens only allowlisted types and reveals everything else in Explorer (O22, rule 134). A folder opens
  in Explorer. The user's chosen folder handler is M9. Held by `a_path_is_a_link_only_if_it_exists_and_opens_by_the_allowlist`
  and `a_path_menu_open_and_show_go_through_the_allowlist`. A path a program breaks at the window's
  edge by hand (Claude Code writes each row on its own, so the next row is not marked wrapped) is
  read across the break and is ONE link on both rows, for the paint, the click and the menu; a web
  link never is, and an OSC 8 label cut that way stays one mark per row (owner 2026-10-21, PT #250,
  core 0.44.2; `cells::by_hand`). Held by
  `a_path_broken_by_hand_at_the_edge_is_one_link`, `a_web_link_is_never_carried_across_a_row_broken_by_hand`,
  `an_osc8_label_cut_by_hand_is_a_mark_per_row` and e2e `pathLinks` (`path_cut_at_the_edge`).
- **Terminal menu.** Right click opens the menu for what was clicked: path rows, link rows, Copy with a
  selection, Paste, Find in scrollback, and Command help last while it is on (M7). No Close tab. It is
  bound to the tab it opened on. Held by `the_menu_fits_what_was_clicked_and_copies_exactly` and the
  flare `term_menu.rs` tests.
- **Mouse reporting.** A program that owns the mouse gets press, release, motion and wheel as bytes;
  a right click still opens the menu, and a report never marks the tab as typed. Shift gives the
  selection back. Held by `a_mouse_program_gets_reports_and_shift_selects` and `write_report_is_not_typing`.
- **IME caret hold.** A key opens a 250 ms window that holds the caret on the input line for IME and
  screen readers; it is released on the alternate screen, a resize, a buffer change or scrollback.
  Held by `the_hold_is_released_on_the_alternate_screen_a_resize_and_scrollback` and the flare-term `caret_hold.rs` tests.
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
- **Finished is news and Question is state (2026-10-09).** Finished and Failed are raised once and
  cleared by the next turn or by looking; a question stays until it is answered or the agent leaves.
  The front tab never gets a finished or failed mark while it is being looked at, but a question is
  raised there. Held by `finished_is_not_raised_on_the_tab_in_front_but_a_question_is`,
  `each_attention_mark_is_behind_its_own_switch` and `working_outranks_question_outranks_failed_outranks_done`.
- **Keep agent sessions open (2026-10-16).** With an agent in the front tab Ctrl+D never reaches it, and
  Ctrl+C is taken unless the agent works or a selection is held. Never in a plain shell. Held by
  `keep_agent_sessions_open`.
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
  hook signal and tab-id variable. `side_by_side::flare_runs_beside_the_neighbour_and_shares_nothing` runs on this
  PC (Layer A); the real install of both runs on a clean CI runner (Layer B).
- **The log.** `%APPDATA%\Flare\logs\diag.jsonl`, one JSON object per line, rotated at 2 MB into four old files,
  written by a thread that never blocks the UI (a queue of 2000 lines, dropped and counted when full). It holds
  no typed text, clipboard, audio or conversation, and `mark` carries no note (D12). Read it with `cargo diag`
  (D13). The stall kinds and the crumbs are in [docs/diagnostics.md](diagnostics.md). Tests: `writer`, `stall_flow`,
  `summary`, `gate`, `reader`, `control` in flare-diag; the privacy scan in `crates/flare/tests/privacy.rs`.
- **The icon.** The owner's artwork (`assets/icon/flare-color.svg`) is the app icon, the taskbar and the Explorer
  icon, rendered as it is with no badge and no outline (O17, 2026-10-17). `tools/make-icon.py` builds the
  frames; `tools/test_make_icon.py` checks them. 2026-10-21: the owner's final icon replaced the first
  artwork: an orange and a gold flame on a dark rounded square (the three SVGs in `assets/icon/`). The
  in-app mark keeps the flame's outline, tinted by the theme.
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
- The installer is not yet run on a clean machine: Layer B and the real install of both apps run in CI.
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
  branch); the sampler and the stall watchdog on a real hang; the installer on a clean machine (CI, Layer B).

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
- **One property says an overlay is open.** `overlay-open` is true while any of the 15 overlays is (menu, tab list, dialog,
  update window and notice, Settings page, its dropdown, theme editor and questions, colour picker, Command help, find bar,
  the F12 card, a tab being dragged).
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
- **Not built in this stage:** web, changes, tasks, board and mod panels (later stages); selecting or copying text in the text
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
  hairline, in physical pixels, after every layout change, splitter step, scale change and monitor move; at 100 and 225
  percent the controller's own bounds, the child window of Flare's window, Slint's box and `flare_surfaces::web::web_rect` agree.
  Held by `the_page_fills_the_body_less_the_slots_hairline_at_every_scale`,
  `the_page_rectangle_equals_the_layout_maths_at_every_scale_width_and_share`, `the_page_follows_the_splitter_while_it_moves`
  (`ui_tests/panel.rs`), `it_sits_on_the_slot_at_every_display_scale`, `the_controller_follows_the_slot_when_the_splitter_moves`
  (`web_tests.rs`), the unit tests of `web.rs` (`a_logical_rect_rounds_by_its_edges`, `the_web_rect_is_the_slot_less_the_header_and_the_hairline`)
  and the e2e `webPanel`.
- **Airspace.** While any of the 15 overlays is open the browser window is hidden and a still of its last frame (taken before it
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
  https, `localhost` becomes http; `file:`, `javascript:`, `data:` and logins are refused with a line in the header; Escape
  restores the page's address and gives the keys back) and the page title as the header title. Held by
  `the_address_field_opens_what_is_typed`, `the_address_field_refuses_what_a_panel_may_not_open`,
  `the_bar_buttons_drive_the_page_only_when_they_can`, `the_bar_follows_the_page`,
  `a_refused_navigation_is_said_in_the_header_and_leaves_the_page`, `the_address_field_shows_the_page_and_enter_and_escape_do_their_part`,
  `the_bar_buttons_call_back_and_a_dead_one_does_not`, `the_browser_bar_is_part_of_a_web_panels_head_and_of_nothing_else`, and the
  `normalize_url` tests (`text_with_no_scheme_becomes_https`, `a_local_host_with_no_scheme_becomes_http`,
  `every_other_scheme_is_refused`, `bad_text_is_refused`, `a_login_in_the_address_is_refused`).
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
- **What a page may not do.** Permissions are denied (the e2e's geolocation request is answered with the denied code), new
  windows go to the same panel (`target=_blank` and `window.open` navigate the panel; the e2e counts top-level windows of both
  processes before and after), only http, https and about:blank open whoever asks, the profile is the run's own folder and never
  Edge's (`the_profile_lives_under_the_named_profile_or_local_appdata`, `the_profile_is_never_an_edge_folder`). Held by the e2e
  `webPanel` for the page behaviours and the unit tests above for the rest.
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
- **The ribbon.** "Claude is using this page" (the tab's detected agent, else "An agent") over the head of the panel while a call
  runs and for 1.5 s after; the page does not move or resize for it. Held by
  `the_ribbon_is_up_while_the_call_runs_and_the_person_taking_over_ends_the_call`,
  `the_ribbon_holds_after_a_call_that_finished_until_the_person_acts`, `the_ribbon_names_who_is_using_the_page` and the e2e (up
  during a 2.5 s call, still up right after, gone within 6 s, the browser window's rectangle unchanged with the ribbon up and after
  it goes; the ribbon's background is opaque so the page's title does not show through).
- **The person keeps control.** A key of theirs in the page, or a mouse button pressed over it, ends the running call with
  `interrupted` and drops the ribbon; the agent's own key presses are never the app's chords. Held by
  `a_key_of_the_persons_posts_a_takeover_only_while_an_agent_works`, `the_agents_own_key_press_is_neither_the_persons_nor_a_chord`,
  `a_point_is_inside_a_rectangle_by_its_near_edges` and the e2e (a stand-in for the person's input ends a 30 s wait `interrupted`,
  the ribbon goes, the refs are forgotten; pressing Ctrl+Shift+P as the agent closes nothing). The mouse is read every 16 ms and also
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
  while a call or the ribbon's hold lasts, and the window's properties are synced only when a call starts or ends or the ribbon goes. Held by `nothing_exists_before_a_web_panel_opens` and the timer names in
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
  policy `disable_non_proxied_udp` (and, with no network, a proxy that is not there), and the page policy shuts child frames
  (`frame-src 'none'; child-src 'none'; worker-src 'self' blob:`). `show_html` tells the agent exactly what is and is not blocked.
  Held by the e2e `miniApp` "webrtc" checks (script and policy both off by seams: a loopback UDP socket as STUN server hears nothing
  from the window or from a srcdoc or about:blank frame, for an app with no network and for one with it, and does hear the web panel
  as the control; a WebSocket to a loopback listener is stopped for a sealed app and reaches it for a networked one), the frame
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

The pure-Rust part of the reader: `crates/flare-convo`, no UI. Format notes for Prism Terminal: `docs/surfaces/convo.md`; choices: `docs/surfaces/decisions.md` (Stage 6 library); the notes for the port: `docs/surfaces/lessons.md` (Stage 6 library). Issues #25, #26 and #27 (library parts; the Changes and Tasks panels are the next part).

- **The transcript path comes from the session id and the tab folder.** `<home>\.claude\projects\<slug>\<session-id>.jsonl`, with the slug from `flare_agent::resume::encode_cwd`. An id with a separator, a dot or a space is refused. A fallback finds `<session-id>.jsonl` in any project folder. Held by `transcript_path_uses_the_slug_and_refuses_an_unsafe_id` and `the_fallback_finds_the_session_under_any_project_folder` (`flare-convo`).
- **The tail reads what was appended, bounded and never whole.** One poll reads at most 4 MB from its offset and returns only complete lines; a partial last line waits for its newline; a line longer than 8 MB is dropped and counted (`skipped_long`). Held by `reads_only_what_was_appended_since_the_last_poll`, `a_partial_last_line_waits_for_its_newline`, `a_bounded_read_says_there_is_more_and_the_rest_comes_next`, `a_line_longer_than_the_cap_is_skipped_and_counted_and_the_next_line_reads`.
- **A truncated or replaced transcript starts again from its first line.** Shorter than the offset, or a new file (volume serial and file index from the open handle; NTFS keeps the creation time across a quick delete, so that is not used). The consumer clears its model when a batch says `restarted`. Held by `a_truncated_file_restarts_from_zero` and `a_replaced_file_restarts_even_when_it_is_longer`.
- **The tail worker polls every 500 ms only while someone listens.** Dropping the subscription stops it. Held by `the_worker_delivers_growth_and_stops_when_the_subscription_drops` and `a_subscription_with_nothing_new_sends_nothing`.
- **The fold reads edits, the task list and the named files, and tolerates anything it does not know.** Edit, MultiEdit, Write and NotebookEdit make edits (with the time and whether the result failed); TodoWrite replaces the task list; TaskCreate and TaskUpdate change it (shape guessed); absolute Windows paths in the assistant's words are the named files, images flagged. A subagent's lines are tagged and its task list is kept apart. A bad line is counted and skipped. Held by the `flare-convo` fold tests (`an_edit_is_listed_with_its_path_time_and_applied_result`, `multiedit_keeps_each_entry_as_its_own_change`, `write_is_a_whole_file_and_diffs_as_all_added`, `a_failed_edit_is_kept_and_marked_failed`, `todowrite_replaces_the_whole_list_each_time`, `a_task_tool_create_then_update_by_its_id`, `a_subagent_todolist_never_replaces_the_main_one`, `named_files_are_absolute_windows_paths_with_an_extension_once_each`, `counts_every_line_and_the_bad_ones`, `an_unknown_type_or_a_non_object_never_panics_or_changes_the_model`).
- **An edit can be shown as a diff of its own text (library only).** A line diff of old to new, three lines of context, capped at 2000 lines and marked when cut; a whole new file shows as all added. No git is run. Held by the `diff` tests (`one_replaced_line_with_context_on_both_sides`, `far_apart_changes_make_two_hunks`, `output_is_capped_and_says_so`, `a_pair_too_big_for_the_table_is_all_removed_then_added`).
- **A subagent's transcript is read from its own file.** Its lines are in `<slug>\<session-id>\subagents\agent-<id>.jsonl`, so `tail::subscribe_session` tails the session file and every `agent-*.jsonl` beside it, each batch naming its file, and each subagent's task list is kept under its file name. Held by `a_session_worker_reads_the_session_file_and_every_subagent_file_by_name` and `a_subagent_file_folds_as_sidechain_and_each_list_keeps_its_own_name` (review fix, 2026-10-21).
- **The reader is bounded.** The worker's queue holds two batches (a slow consumer parks the reads); the model keeps 2000 edits and 32 MB of edit text, with 64 KB per stored text. A rewrite in place that ends longer restarts (the last 64 bytes are compared). Held by `a_worker_blocked_on_a_full_queue_still_stops_when_the_subscription_drops`, `a_file_rewritten_in_place_that_ends_longer_restarts`, `the_oldest_edits_go_once_there_are_too_many_and_a_late_result_is_ignored` and `the_kept_edit_text_is_bounded_in_bytes_too` (review fix, 2026-10-21).
- **In the app now:** the Changes and Tasks panels and `convo.state` are the next section (stage 6).

## Surfaces: the Changes and Tasks panels (stage 6), 2026-10-22

What Claude did, shown beside its terminal: the files it edited with each edit as a diff, and its task list. Issues #25, #26 and #27. The library is above; choices: `docs/surfaces/decisions.md` (Stage 6); the holes and the notes for Prism Terminal: `docs/surfaces/lessons.md` (Stage 6); the method: `docs/surfaces/protocol.md` (`convo.state`, `panel.open`).

- **Nothing runs unless a panel is open.** A tab gets one transcript tracker only while its Changes or Tasks panel is open AND the hooks have named its Claude session; closing the panel, switching it to a file, losing the session or closing the tab stops it. A tab with a session and no panel has no thread, no timer and no read. Held by `a_tab_with_a_session_and_no_panel_runs_nothing`, `closing_the_panel_stops_the_tracker_and_reopening_starts_it_again`, `closing_the_tab_stops_its_tracker` (`ui_tests/agent_flows/convo_flows.rs`) and by the e2e `convoPanels`.
- **The folding and the diffing run on a worker.** The tracker follows the session file and its subagents' files, folds them and makes the rows; the window only wraps finished rows (rule 34). A new session (a `/clear`, a resume) switches the file and forgets the pick; a replaced or cut transcript is read again from its start; a transcript that is not at the tab's folder is found in any project folder. Claude's folder is `CLAUDE_CONFIG_DIR` (absolute only), else `%USERPROFILE%\.claude`. Held by `a_new_session_switches_the_file_and_forgets_the_pick`, `the_changes_panel_lists_the_edits_and_follows_the_file_as_it_grows`, and in `flare-convo` by `a_replaced_transcript_starts_the_model_again`, `a_transcript_in_another_project_folder_is_found`, `a_transcript_that_appears_later_is_picked_up`, `dropping_the_tracker_stops_the_worker` and the `claude_dir` tests.
- **Changes lists the edited files, newest first, and one click shows a file's edits as a diff.** Each row has the name, the folder, "3 edits" and a red ", 1 failed". The diff has a header per edit, context rows, removed rows and added rows tinted with the theme's terminal red and green, two line-number columns and monospace text; "Files" (or Backspace) goes back and the path opens the file in the panel (the stage 1 preview, re-resolved and allowlisted: a program does nothing). Only the rows in view are built, so 100 000 rows build about 40. Held by `ui_tests/convo.rs` (`a_big_file_list_builds_only_the_rows_in_view`, `a_diff_of_a_hundred_thousand_rows_builds_only_the_rows_in_view_and_scrolls`, `added_and_removed_rows_wear_the_themes_inks_and_context_rows_wear_none`, `a_click_on_a_file_picks_it_and_the_bar_goes_back_or_opens_the_path`, `backspace_goes_back_from_a_diff_and_the_arrows_do_not_scroll_an_empty_one`) at 100 and 225 percent, by `a_failed_edit_is_marked_in_the_list`, `a_click_on_the_path_in_a_diff_shows_the_file_and_a_program_shows_nothing`, and in `flare-convo` by the `view` tests (`files_list_the_newest_first_and_count_each_edit_once`, `a_diff_marks_each_edit_and_numbers_its_lines`, `a_huge_diff_is_cut_at_the_row_cap_with_a_note`).
- **Tasks shows the agent's list with its states, the one in progress highlighted.** A ring for pending, the accent dot and a tinted row for the one in progress (with what it is doing under it), a grey dot with a tick for done. Subagents' lists are groups ("Subagent 1a2b3c4", "1 of 2 done") that fold with a click. A later TodoWrite appended to the transcript updates the list. Held by `a_task_wears_its_state_and_only_the_one_in_progress_has_the_accent`, `the_active_task_has_a_tinted_row_and_a_plain_one_does_not`, `a_group_header_calls_back_with_its_key_and_a_folded_group_has_fewer_rows`, `the_keys_scroll_a_long_task_list` (at 100 and 225 percent), `the_tasks_panel_shows_the_list_and_a_new_todowrite_updates_it`, `a_subagents_list_is_a_group_that_folds`, the glyph size tests `the_conversation_glyphs_draw_at_full_size_at_every_scale` (all eight scalings) and in `flare-convo` `tasks_group_the_main_list_then_each_subagent_and_mark_the_active_one`.
- **Empty states say what is missing, in plain words.** "Claude is not running in this tab yet...", "Reading the conversation...", "Waiting for the conversation file.", "Claude has not changed a file yet.", "Claude has no task list yet." Held by `a_panel_with_no_session_says_so_and_starts_nothing`, `an_empty_session_says_nothing_has_changed_yet` and `the_empty_words_say_what_is_missing`.
- **How to open them.** The terminal's right-click menu on a tab that runs Claude gains "Show changes" and "Show tasks" ("Hide ..." while open); Claude opens its own with `panel.open` kind `changes` or `tasks`. A plain shell's menu does not change. No chords were added (the shared chord table is the owner's call). Held by `the_terminal_menu_offers_the_two_panels_only_on_a_claude_tab` and `panel_open_takes_the_conversation_kinds_and_nothing_more`.
- **They are stored and restored like the other panels.** A stored `changes` or `tasks` panel reopens empty with the "not running" words and fills when the session is known; one with a wrong target is dropped. Held by `a_stored_conversation_panel_reopens_empty_until_the_transcript_loads` and `a_stored_panel_of_a_kind_with_the_wrong_target_is_dropped`.
- **`convo.state` gives the agent its own tab's summary.** Files, edits, failures, task counts by state, the task in progress and the 20 newest changed files; no contents. A tab Flare is not reading answers `tracking: false` and says how to have it read. Held by `convo_state_summarises_the_tabs_own_conversation`, `convo_state_asks_for_the_callers_own_tab_only` (`ctl_app.rs`) and the e2e `convoPanels`.
- **End to end (the release exe, its own profile and Claude folder, offscreen):** `convoPanels`, at 100 and 225 percent, with a synthetic transcript: no tracker before a panel, `panel.open` for each kind, the file listed, a real click on its row showing removed and added rows, an appended edit and an appended TodoWrite updating both, a new session switching the file, and `panel.close` stopping the tracker. See `docs/e2e.md`.
- **By hand, not machine-tested:** a real Claude Code session's `tool_result` and task-tool shapes (the fixtures are synthetic), the look in all 18 themes, a transcript of hundreds of megabytes. The list is at the end of `docs/surfaces/lessons.md` (Stage 6).
- **Review fixes (2026-10-22).** A diff line is cut at 400 characters with an ellipsis on the worker; an update rebuilds only the list, the diff or the tasks that changed (an edit of another file leaves the open diff's rows alone); a path answer that arrives after the panel changed is dropped; the search for a missing transcript backs off to every 30 s; wrapped task rows sit on whole device pixels. Tests: `view::a_very_long_line_is_cut_to_the_row_width_with_an_ellipsis`, `convo_ctl::a_capped_diff_gives_five_thousand_rows_no_wider_than_the_viewport`, `convo_flows::an_edit_of_another_file_leaves_the_selected_diff_rows_alone`, `convo_flows::a_path_answer_that_arrives_after_the_panel_changed_is_dropped`, `convo_flows::closing_the_tab_stops_its_tracker` (the worker really exits), `ui_tests::convo::wrapped_task_rows_sit_on_whole_device_pixels_at_fractional_scales`.

## Surfaces: live preview, the agent board and question notifications (stage 7), 2026-10-22

Three small things that make several agents and dev servers easy to live with. Issues #28, #29 and #30. Choices: `docs/surfaces/decisions.md` (Stage 7); the holes and the notes for Prism Terminal: `docs/surfaces/lessons.md` (Stage 7); the method: `docs/surfaces/protocol.md` (`panel.open` kind `board`); the rules: `docs/regression-rules.md` 202 to 206.

- **A "Preview" chip for a local dev server.** When a tab prints `http://localhost:5173/`, `http://127.0.0.1:3000/app` or `http://[::1]:8080` (https too, any path), a small pill ("Preview localhost:5173") appears in the top right of the terminal once something listens on that port; a click opens the tab's web panel at that server's ORIGIN (`http://localhost:5173/`; whatever path or query the program printed is dropped, and the tooltip shows the origin). The chip needs the listener to belong to the tab's own process tree (the shell and what it started), so a port another program holds, or a server in WSL, gets no chip. A click looks at the port once more first and opens the page only if the same server is still there; a stranger taking the port drops the chip at once, and a tab that comes to the front shows its chip only after a fresh look. It goes when the listener is gone (two failed looks in a row), when the screen is cleared, when the tab closes, and while the panel already shows that page; one chip per tab, for the newest address. A server stopped and started again on the same port prints the same address and gets its chip back. A chip is looked at again only in the tab in front and not while the window is minimised. Only these three local hosts ever qualify (`localhost.evil.com`, `localhost@evil.com`, `0.0.0.0`, a LAN address and every remote name never do). The address is read off the terminal's own live screen (the history is never read, nor the row the cursor is on, which is what you are typing), in any tab, so a server started in a tab you left is found, and the port is looked up in the system's listener table, IPv4 and IPv6, never by connecting. Setting: **Offer previews of local servers** (Settings > Terminal > Links, on). Tests: `flare_term::dev_server` (11: the three hosts, ports, paths, every refusal, the clear watch across every chunk cut and on the alternate screen), the emulator's reading of the screen (`the_emulator_reads_a_local_server_address_off_its_screen`, `the_row_the_cursor_is_on_is_input_and_never_read`, `a_suggestion_painted_after_the_cursor_and_typed_over_is_not_read`, `an_address_that_scrolled_off_the_screen_is_still_the_one_held`, `a_cleared_screen_forgets_the_address_though_the_history_still_holds_it`, `a_long_address_wrapped_over_rows_is_read_whole`, `nothing_is_read_on_the_alternate_screen_and_no_remote_address_ever`, `a_reading_asked_for_too_soon_is_owed_not_lost`, `the_same_address_printed_again_moves_reprint_not_seq`, `a_full_scrollback_does_not_hide_a_newer_address`, `a_full_screen_program_clearing_its_own_screen_keeps_the_address`, `reading_a_huge_screen_full_of_addresses_stays_cheap`), `flare_surfaces::devpreview` (17, a scripted listening table; restart, give-up then reprint, reprint while live, front tab only), `a_server_started_again_on_the_same_port_gets_its_chip_back` and `another_tabs_steady_output_never_holds_back_a_chip` (flow), `flare_win::tcp_owner` (the table on both families), the flow tests in `ui_tests/agent_flows/board_flows.rs` (`a_local_address_offers_a_chip_once_its_port_listens`, `clicking_the_chip_opens_the_web_panel_at_that_address`, `the_chip_goes_when_the_process_that_printed_it_is_gone`, `clearing_the_screen_takes_the_chip_at_once`, `the_newest_address_is_the_only_chip_of_a_tab`, `an_address_that_is_not_local_never_gets_a_chip`, `the_chip_belongs_to_the_tab_in_front`, `the_setting_turns_the_chips_off_and_on`), `ui_tests::board` (the chip whole in the corner and clickable at 100 and 225 percent) and the e2e `devPreview`.
- **The agent board.** Ctrl+Shift+A (or "Show agent board" in the terminal's right-click menu while some tab has an agent, or Claude with `panel.open` kind `board`) shows, in the front tab's panel, one row for every tab that has an agent: the tab's mark in its own colour, the tab's name, Claude or Codex, "Working for 4m 12s" / "Waiting 2m 00s" / "Finished 5m 00s ago" / "Failed 1m 01s ago" / "Idle", the folder and, for a question, what it asks (a failed turn shows why). Strip order, so a row does not move under the pointer; a click goes to that tab. It reads the agent hub (nothing on disk, nothing polled), rebuilds when the hub changes and once a second for the clocks while it is open, and costs no timer and no rows while closed. Tests: `flare_surfaces::board` (8), `the_board_lists_only_tabs_with_an_agent_in_the_strips_order`, `every_state_is_worded_and_the_time_is_the_states_own`, `a_question_row_shows_the_question_when_the_screen_has_it`, `a_click_on_a_row_goes_to_that_tab`, `the_board_costs_no_timer_and_no_rows_while_closed`, `the_chord_opens_and_closes_the_board_and_the_shell_never_hears_it`, `the_terminal_menu_gains_one_row_that_follows_the_board`, `the_board_panel_is_stored_like_another_and_comes_back_with_the_tab`, the keymap test `the_board_chord_is_ctrl_shift_a_and_plain_ctrl_a_stays_the_shells`, `ui_tests::board` at 100 and 225 percent (row heights, clicks on rows of both heights, the keys scrolling a long board, the tints, the empty note) and the e2e `agentBoard`.
- **A Windows notification when an agent asks.** A question from an agent in a tab out of sight, or in a window without the keyboard, shows a toast with the tab's name and the question's text (or "Claude has a question for you."); clicking it brings the window up and that tab to the front. One per question; none while you are looking at that tab; a question you saw is never announced later. Setting: **Notify me when an agent asks** (Settings > Agents, on). The toast is shown under Flare's AppUserModelID (the installed app's shortcut carries it) and carries no button or action; when Windows would not take the request the taskbar button flashes instead. Tests: `flare_surfaces::notify` (15: hidden tab, unfocused window, looking, grace, de-dup, setting, clean text), `flare_win::toast` (the XML cannot become markup), `a_question_in_a_tab_out_of_sight_notifies_once_with_the_tabs_name_and_text`, `a_question_without_its_text_waits_out_the_grace_and_says_who_asks`, `nothing_while_the_person_is_looking_at_that_tab`, `an_unfocused_window_notifies_even_for_the_tab_in_front`, `coming_back_to_the_window_before_the_grace_ends_cancels_it`, `the_setting_turns_the_notification_off`, `a_second_question_in_the_same_tab_is_a_second_notification`, `a_click_on_the_notification_brings_the_tab_to_the_front`, `a_notification_windows_would_not_take_flashes_the_taskbar_instead` and the e2e `questionToast` (the recorder, `ToastClick`).
- **End to end (the release exe, its own profile, offscreen):** `devPreview`, `agentBoard` (both at 100 and 225 percent) and `questionToast`. See `docs/e2e.md`.
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

