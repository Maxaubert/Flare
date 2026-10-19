# Performance: Flare against Prism Terminal (M10)

The verdict of M10 Task 70, taken 2026-10-19 on this PC (AMD Ryzen 9 9950X3D, RTX 5090, Windows 11
Pro 10.0.26300), release build of `m10-parity`. Prism Terminal is the installed 0.58.0, not 0.55.6
(plan Q5), measured the same day by `tools/pt-perf` (`docs/perf/pt-baseline-2026-10-19.json`).
Flare's kept run is `docs/perf/flare-2026-10-19.json` (the median of three `perf all` runs by the
B1 GPU median). How to run it again: `docs/perf/README.md`.

## What is counted (plan decision D5; the owner may overrule, question Q4)

- **Shell:** `cmd` for every gated number, on both apps. PowerShell 7 is reported for B1 only, with no bar.
- **Clock:** every gated number is measured inside the app (`startup.trace`, the hook's `Frames` and
  `Latency`), so pipe jitter never enters it; the harness clock is a cross-check.
- **B1 cold start to a usable prompt:** `usable_ms = prompt_ms + echo_ms` from the process creation time
  (`GetProcessTimes`): the end of the first frame that shows the `>` prompt, plus the time from one key
  (`x`) to the frame that shows it. Median of 7 launches after 1 discarded, each a fresh profile.
- **B2 idle memory:** private working set (`Win32_PerfFormattedData_PerfProc_Process.WorkingSetPrivate`,
  the same query on both apps) of the app tree minus the shells and what they start, one tab at the prompt,
  10 s after the first prompt, median of 3.
- **B3 heavy output:** `cmd /c type` of a 100 MB file of 100-byte lines ending in `FLOOD-END`; Enter to the
  first frame holding the marker; MB/s = 100 / seconds. Frames counted from 500 ms after the Enter.
- **B4 keystroke latency:** the key entering `AppCtl::on_key` to the frame that holds the echo, 200 keys
  20 ms apart. Prism Terminal: `keydown` capture to xterm's `onRender`.
- **B5 scroll:** 100,000 lines in the scrollback, then 5000 wheel notches 1 ms apart; presented frames per second.
- **B6 idle CPU:** kernel plus user time of the app tree minus shells over 10 s at a prompt. Windows reports
  process times in 15.625 ms ticks, so one tick in 10 s is 0.16% of a core: the number moves in those steps.
- **B7 frame time at 225%:** GPU, 1000 x 640 and 1707 x 960 css px at 216 dpi, during the flood and a 3 s
  resize drag: render time per frame (`BeforeRendering` to `AfterRendering`) p95, and the presented-frame
  interval.
- **Frame:** the app's own render, never the compositor's. Both apps are measured parked offscreen (the e2e
  rule), so what DWM does with the window afterwards is not in any number (a known limit).

## The verdict

| Bar | Flare GPU | Flare software | Prism Terminal | Rule (`docs/perf/bar.json`) | Verdict |
|---|---|---|---|---|---|
| B1 start to usable prompt | 814 ms | 170 ms | 436 ms | <= 300 ms and <= pt | **GPU MISS**, software PASS |
| B2 idle private working set | 76.6 MB | 66.5 MB | 126.5 MB | GPU <= pt / 3 (42.2 MB); software <= pt | **GPU MISS**, software PASS |
| B3 flood throughput | 2.48 MB/s, max frame 16.0 ms, 0 over 33 ms | 2.39 MB/s | 2.47 MB/s | GPU >= pt x 2 (4.94) and no frame over 33 ms; software >= pt | **MISS on throughput (both)**, frame rule PASS; see below |
| B4 key to frame | median 10.2 ms, p95 11.8 ms | median 3.2 ms, p95 5.2 ms | median 19.3 ms, p95 23.6 ms | median <= pt and p95 <= 25 ms | PASS |
| B5 scroll | 144 fps, max frame 9.6 ms | 59.9 fps, max frame 27.5 ms | 131 fps | >= 55 fps, no frame over 33 ms | PASS (after the fix below) |
| B6 idle CPU | 0.16% of a core | 0.31% of a core | 3.47% | <= 0.5% and <= pt | PASS |
| B7 frame at 225% | render p95 5.6 / 4.5 ms (1000 x 640 flood / resize), 6.2 / 4.7 ms (1707 x 960); interval max 20.5 ms | n/a | n/a | render p95 <= 8.3 ms, interval <= 33 ms | PASS |

B1 with PowerShell 7 (no bar, from the baseline run before Tasks 62 to 68): Flare GPU 1965 ms, software 540 ms,
Prism Terminal 735 ms.

The three runs agree: B1 GPU 812, 826, 814 ms; software 170, 171, 170 ms; B2 GPU 76.5, 76.5, 76.6 MB; B3
GPU 2.59, 2.48, 2.48 MB/s and software 2.55, 2.40, 2.39 MB/s; B4 GPU 10.1, 10.3, 10.2 ms; B6 software
0.47, 0.47, 0.31% (three ticks or fewer in 10 s; under the bar, but close to it in tick terms).

### The misses, written up as decision D8 asks

**B1 on the GPU renderer: 814 ms against 300 ms.** The floor is Slint's own GPU window set-up, not Flare's
work. In the kept run's median launch: `backend` 475 ms (the wgpu DX12 instance, adapter and device; Task 64
already builds them on a worker from the first line after `ship_ctl::bootstrap()`, so this is the device
creation itself), `component` 610 ms, `render_setup` 665 ms (surface and femtovg pipelines; Flare's own grid
painter build is 1.6 ms of it, Task 65 step 0), first frame 729 ms, prompt 796 ms, echo 14 ms. Tried: only
the fonts the terminal uses (Task 62), deferrals (Task 63, a fraction of a millisecond), the prewarmed device
(Task 64, about 9%), a DX12-only prewarm (Task 66, 1636 to 986 ms), the CPU-first grid (Task 65, measured,
nothing to win). The software renderer meets the bar at 170 ms and beats Prism Terminal's 436 ms. **Owner's
choice (Q1):** (a) accept the floor for the GPU renderer while software meets 300 ms; (b) start the window on
the software renderer (fast start, but see-through glass needs the GPU renderer, DIFF-5, and Slint cannot
change a window's renderer later); (c) a start-up splash painted by Flare's CPU painter while the GPU window
builds (a second window; not built unless chosen).

**B2 on the GPU renderer: 76.6 MB against 42.2 MB** (61% of Prism Terminal's 126.5 MB). Software is 66.5 MB,
so even a renderer with no GPU memory sits above the GPU bar; the GPU path holds a 17 MB grid texture
(resident because every pixel is drawn each frame) and window-sized buffers of Slint, the swapchain and the
driver. The breakdown and every candidate are in `docs/perf/memory.md`. **Owner's choice:** (a) accept the
miss with this floor; (b) change the bar to "at most half of Prism Terminal's" (63 MB, still missed by about
13 MB at this window size); (c) draw the grid straight into Slint's frame instead of an imported texture (a
rendering-architecture change).

**B3 throughput: 2.48 MB/s (GPU) and 2.39 MB/s (software) against 4.94 and 2.47.** The measure is held by the
writer, not by the terminal: `cmd /c type` into ConPTY runs at 2.4 to 2.8 MB/s in Flare (both renderers) and
in Prism Terminal (2.47) alike, so the software row's "miss" is run-to-run noise on that shared ceiling (2.55
in the first run, 2.39 in the kept one), and the GPU rule (twice Prism Terminal's) cannot be met by any
terminal fed by `type`. With a writer that is not the limit (`flare_spew`, the M2 flood program) the same
windowed Flare does 99.6 MB/s on the GPU renderer and 84.8 MB/s on software (`docs/perf/heavy-output.md`),
and the headless ConPTY flood does 89.5 MB/s. The frame half of B3 passes: no GPU frame over 33 ms (max 16.0
ms) during the flood. **Owner's choice:** (a) judge B3's throughput with a writer that is not the limit (both
apps fed the same way); (b) keep the bar as written and take it as a miss that no `type`-fed terminal can
pass; (c) try ConPTY's pass-through mode (not exposed by `portable-pty`, changes cursor and resize semantics).

## What changed the GPU numbers between the baseline and now

The baseline (Task 61) and the first three Task 70 runs of this day showed every GPU frame interval at about
250 ms: B4 GPU 253 to 259 ms, B5 GPU 0 frames, B7's interval 245 to 394 ms, and B1 GPU runs in two groups
250 ms apart. The same build, the same harness and the same command, about twenty minutes later, gave
frames at the display's 144 Hz (B4 GPU 10.1 ms, B5 GPU 144 fps, B7 interval max 20.5 ms, B1 GPU 812 to 826 ms
in one group). Nothing in the app changed in between except the wheel fix below, which touches only the wheel.
So the 4 Hz cadence that `heavy-output.md` read as "a parked window's DWM cadence" is a state of this machine's
display path, not of Flare and not of parking as such (the cause is not proved; the first runs had the PC's
virtual display drivers listed, the later ones one monitor). The verdict above is from the later runs, where
the GPU path presents normally. A rerun that shows 250 ms intervals on the GPU rows is in that state and is
not a measure of the app.

## The defect this task found and fixed

B5 GPU counted **zero** frames in every earlier run, even in the 4 Hz state, while the wheel loop got its
replies at once: no frame was ever drawn for a wheel scroll on the GPU renderer. A wheel scroll makes no
output, so nothing woke the GPU path (it paints only on a redraw request; the software path polls with its
16 ms timer, which is why software scrolled). `TermHandle::wheel` now answers whether the view moved, and the
window's wheel arm (and the e2e mirror of it) asks for the redraw. Tests:
`a_wheel_says_when_the_view_moved_so_the_window_can_redraw` (`flare-term` `handle.rs`),
`a_wheel_that_moves_the_view_asks_for_a_redraw` (`flare` `window.rs`). B5 GPU went from 0 to 144 fps.

## Start-up phases (the kept run's median launch, ms from process creation)

| Phase | GPU | Software |
|---|---|---|
| main | 15.3 | 14.6 |
| args | 15.4 | 14.6 |
| backend | 474.8 | 18.9 |
| component | 609.9 | 23.0 |
| profile | 618.5 | 31.9 |
| render_setup | 664.7 | - |
| first_frame | 728.7 | 43.0 |
| shell_spawn | 738.4 | 76.0 |
| first_prompt | 807.9 | 154.2 |

Baseline (Task 61) for comparison, GPU: backend 387.9, component 661.3, render_setup 1212.7, first frame
1224.0, usable 1435.8 (median launch); software usable 231.8.

## What each task changed and saved

| Task | What | Measured effect |
|---|---|---|
| 62 fonts | the font database holds the stack plus 14 fallback families (68 faces, not 630), from the registry | `FontSystem` 29 to 7 ms; B1 software 232 to 167 ms; B2 unchanged (fonts are mapped, not private) |
| 63 deferrals | the help warm-up and the Explorer verb reconcile start after the first frame | about 0.4 ms; within the noise |
| 64 prewarm | the wgpu instance, adapter and device are built on a worker before argument parsing | B1 GPU 1617 to 1478 ms (about 9%, kept) |
| 65 CPU-first grid | step 0 only: `GpuPainter::new` costs 1.6 ms (0.1% of B1) | nothing to win; no code changed |
| 66 memory | the prewarm starts DX12 alone (it had loaded every GPU's OpenGL and Vulkan stacks); the help index is built on first use and dropped a minute after the panel closes; the first CPU frame buffer is dropped at the first GPU frame; `mimalloc` tried and rejected | B2 GPU 135 to 75.8 MB and B1 GPU 1636 to 986 ms (the DX12-only prewarm); help index 2 to 2.6 MB; CPU buffer 2 MB |
| 67 heavy output | a frame trace (`FLARE_TRACE_FRAME=1`); the snapshot fills a kept buffer; software output is painted on its wake; the pty reader grows to 1 MiB reads under sustained output | lock hold p90 at 480 x 135: 0.63 to 0.37 ms; the damage rebuild and the glyph worker were not built (0.15 and 0.09 ms per frame) |
| 68 latency | an isolated pty chunk is flushed at once instead of waiting the 8 ms batch window (`FLARE_TRACE_KEY=1` logs each hop) | B4 software 22.1 to 3.0 ms |
| 69 idle CPU | **skipped**: B6 passed in the baseline (0.16% against 0.5% and Prism Terminal's 3.47%) | |
| 70 the wheel | a wheel scroll asks for a redraw on the GPU renderer | B5 GPU 0 to 144 fps |

## Against the baseline

| Bar | Baseline GPU | Now GPU | Baseline software | Now software |
|---|---|---|---|---|
| B1 | 1700 ms | 814 ms | 232 ms | 170 ms |
| B2 | 115.3 MB | 76.6 MB | 69.4 MB | 66.5 MB |
| B3 | 2.79 MB/s | 2.48 MB/s | 2.56 MB/s | 2.39 MB/s |
| B4 | 252.9 ms (4 Hz state) | 10.2 ms | 22.1 ms | 3.2 ms |
| B5 | 0 fps | 144 fps | 56.8 fps | 59.9 fps |
| B6 | 0.16% | 0.16% | 0.16% | 0.31% |
| B7 render p95 | 5.9 / 6.35 ms | 5.6 / 6.2 ms | n/a | n/a |
