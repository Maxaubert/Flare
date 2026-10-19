# Flare baseline against Prism Terminal 0.58.0 (M10 Task 61)

Taken 2026-10-19 on this PC (release build of `m10-parity`, `cmd` as the measuring shell, scale 216 dpi
for B7, 7 launches after 1 discarded, offscreen and unfocused). Prism Terminal: `docs/perf/pt-baseline-2026-10-19.json`
(installed 0.58.0, throwaway profile). Flare: `docs/perf/flare-baseline-2026-10-19.json`. Second shell for B1:
`docs/perf/flare-baseline-pwsh-2026-10-19.json`. The machine was not idle (the owner's own Prism Terminal was
running; no builds were running during the measuring).

## Verdicts

| Bar | Metric | Flare GPU | Flare software | Prism Terminal | Rule (`bar.json`) | GPU | Software |
|---|---|---|---|---|---|---|---|
| B1 | start to usable prompt (ms) | 1700 | 232 | 436 | gpu/software <= 300 and <= pt | FAIL | pass |
| B2 | idle private working set (MB) | 115.3 | 69.4 | 126.5 | gpu <= pt / 3 (42.2), software <= pt | FAIL | pass |
| B3 | flood throughput (MB/s) | 2.79 | 2.56 | 2.47 | gpu >= pt * 2 (4.94) and no frame over 33 ms; software >= pt | FAIL | pass |
| B4 | key to frame, median (ms) | 252.9 (p95 265.8) | 22.1 (p95 38.5) | 19.3 (p95 23.6) | median <= pt and p95 <= 25 | FAIL | FAIL |
| B5 | scroll frames per second | 0.00 | 56.8 | 131.0 | >= 55 and no frame over 33 ms | FAIL | pass |
| B6 | idle CPU (% of a core) | 0.16 | 0.16 | 3.47 | <= 0.5 and <= pt | pass | pass |
| B7 | render p95 at 225% (ms) | 5.90 / 4.20 (1000x640 flood / resize), 6.35 / 4.45 (1707x960) | n/a | n/a | render p95 <= 8.3 and interval <= 33 | FAIL (render passes, interval fails) | n/a |

B1 with PowerShell 7 (reported, no bar): Flare GPU 1965 ms, software 540 ms, Prism Terminal 735 ms.

## Read this before trusting the GPU rows

Every GPU frame interval in this run is a steady **252 ms** (B3 p50 252.3, B7 p50 252.7), the GPU
echo in B1 is 251 ms, B4's GPU median is 253 ms and B5's GPU run counted **zero** presented frames. The software
renderer, which has no surface to present, shows 20 ms frames in the same harness. The GPU window is parked at
(-32000, -32000) (D4), so its swapchain is occluded: Slint's femtovg-wgpu path skips the draw with
`DrawOutcome::Occluded` (`i-slint-renderer-femtovg-1.18.1/wgpu.rs:237`) and only a slow retry produces the
next frame. This is the likely cause, not a proved one. It means B3's frame rule, B4, B5, B7's interval and the
echo half of B1 on the GPU renderer measure the occlusion retry cadence, not Flare's render. Two things are
independent of it and are real: B1's phases (below) and B7's `render_p95` (5.9 ms and 6.35 ms in the flood,
inside the 8.3 ms bar; render time is measured inside the notifier). B2 is real.

Nothing was edited to make a verdict green. The planned remedies are in the "Decision" part below; the
occlusion question is sent to the orchestrator as a harness defect (Task 60 owns the frame statistics), not
fixed here.

## Start-up phases (median run, ms from process creation)

| Phase | GPU | Software |
|---|---|---|
| main | 15.7 | 15.3 |
| args | 15.8 | 15.3 |
| backend | 387.9 | 19.9 |
| component | 661.3 | 24.1 |
| profile | 705.6 | 66.4 |
| render_setup | 1212.7 | - |
| shell_spawn | 1243.6 | 153.8 |
| first_frame | 1224.0 | 124.8 |
| first_prompt / usable | 1435.8 | 231.8 |

What the table shows: on the GPU renderer `backend` (Slint's wgpu instance, adapter and device) costs about
370 ms, `component` another 270 ms, and `render_setup` (surface, femtovg pipelines, `attach_gpu`) about 510 ms;
the **shell is spawned only at 1244 ms, after the render set-up**, while on software it is spawned at 154 ms.
Prism Terminal's prompt arrives at 436 ms. Software meets B1 (232 ms) and the GPU path misses it by 5.7 times.

## First suspects per miss (Appendix C)

- **B1 GPU:** the shell spawn waits for render set-up (start the shell before the window finishes building:
  Task 63/64); `FontSystem::new()` loads every installed font (`raster.rs:31`, Task 62); Slint's wgpu creation
  on the main thread (Task 64); the grid painter's own build in `RenderingSetup` (Task 65, always runs); the echo
  half (251 ms) is the occlusion cadence above.
- **B2 GPU:** 115 MB against 42 MB allowed; software is 69 MB with the same fonts, so about 46 MB is the GPU path
  (wgpu heaps, atlas, grid texture sized by the 8192 limit, CPU frame buffer kept) on top of all-fonts (Tasks 62, 66).
- **B3 GPU:** throughput 2.79 MB/s against 4.94 needed. Prism Terminal itself only reaches 2.47 MB/s and Flare
  software 2.56 MB/s, so all three sit on the same ceiling (about 2.5 MB/s through `cmd /c type` into a pty in
  this setup), while Flare's headless ConPTY flood does 89.5 MB/s. The first suspect is what is common to the
  windowed runs (the flood file is `type`d by `cmd` into ConPTY; the app is not the limit on the software row),
  then the 252 ms occlusion cadence for the GPU row. Task 67 starts from the frame trace.
- **B4:** software 22.1 ms against 19.3 ms; p95 38.5 ms against 25 ms (batcher 8 ms coalescing, timer-paced wake:
  Task 68). GPU is the occlusion cadence.
- **B5 GPU:** zero frames counted; the occlusion cadence or a wheel path that does not redraw while occluded.
  Software 56.8 fps passes only just.
- **B6:** passes, 0.16% against 0.5%.
- **B7:** render p95 passes; the interval rule fails on the occlusion cadence (max 245 to 394 ms).

## Decision: which of Tasks 62 to 69 run

By the plan's own rules (misses: B1 GPU, B2 GPU, B3 GPU, B4 both, B5 GPU, B7 interval; passes: B6):

| Task | Runs? | Why |
|---|---|---|
| 62 fonts | YES | B1 and B2 miss |
| 63 deferrals | YES | follows 62; B1 phases show the shell spawn waiting on render set-up |
| 64 early GPU device | YES | follows 63; kept only if B1 GPU median improves by 5% |
| 65 first grid frames on the CPU | YES (always) | the owner's queued follow-up; kept only by its own 5% rule |
| 66 memory diet | YES | B2 misses |
| 67 heavy output and frame time | YES | B3 and B7 miss |
| 68 keystroke latency | YES | B4 misses |
| 69 idle CPU | SKIP | B6 passes (0.16% of a core, Prism Terminal 3.47%) |

Order from the wave table: 62, 63, 64, 65, then 67, 68, 66. Before 67 and 68 the orchestrator should settle
the occlusion question, or their GPU acceptance numbers will measure the retry cadence again.
