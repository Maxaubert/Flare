# Features

What Flare does, decided by the owner, with the date and the test that holds each decision. Entries
are added as milestones land.

## Rendering (M1), 2026-10-17

- **GPU by default, software as the fallback.** Flare starts on Slint's `femtovg-wgpu` renderer
  and paints the terminal grid on the GPU through a shelf-packed glyph atlas. When the GPU cannot
  start (RDP, a VM, WARP only), Flare falls back to the CPU painter and logs
  `renderer: software (fallback: <reason>)`; the debug overlay shows the same reason.
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
