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
  to 141 hold the ConPTY start-up, environment, query and synchronized-update lessons.
