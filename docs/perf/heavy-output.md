# Heavy output and frame time (M10 Task 67)

Taken 2026-10-19 on this PC, release build of `m10-parity`, GPU renderer, 225% (216 dpi), `cmd`,
window parked offscreen and unfocused as the e2e always does. Raw files are in the scratchpad of
the session; the numbers that matter are copied here.

## Step 1: the profile

`FLARE_TRACE_FRAME=1` makes every frame log one line at `trace` level (target `flare_render::stats`;
run with `RUST_LOG=info,flare_render::stats=trace`). Times are microseconds; `paint` includes
`glyph_upload`; `render` is the whole `BeforeRendering` to `AfterRendering` span, which holds
Slint's own draw of the window; `gap` is `AfterRendering` to the next `BeforeRendering`.

The windowed 100 MB flood (`perf flood --renderer gpu`, 163 frames after the first 10):

| Phase | median | p95 | max |
|---|---|---|---|
| lock_wait | 0 | 0 | 34 |
| snapshot (rows copied under the lock) | 48 | 95 | 167 |
| scene | 149 | 221 | 299 |
| glyph_upload | 87 | 171 | 284 |
| paint (glyph upload inside) | 330 | 447 | 829 |
| set_image | 1 | 2 | 2 |
| render (Slint's draw included) | 5838 | 7792 | 10352 |
| gap | 246365 | 259515 | 261568 |

What it says:

- Flare's own part of a frame (snapshot, scene, paint) is about 0.5 ms. The other 5 ms of `render`
  is Slint drawing the window (2250 x 1440 pixels at 225%). `render` p95 7.8 ms is
  under the 8.3 ms bar of B7.
- The UI thread waits for the lock for nothing (median 0), so step 2.2 is not what limits a flood.
- **The 250 ms `gap` is not Flare.** It is the same in an idle window, on the Vulkan and the DX12
  backends, with and without the glass plan, and with an alpha-1 window on the primary monitor.
  A 10 ms timer on the same event loop is held for the whole gap, so the UI thread is blocked
  inside wgpu's frame acquire (most likely waiting for the DX12 frame-latency object). With
  `WGPU_DX12_USE_FRAME_LATENCY_WAITABLE_OBJECT=none` one isolated frame is released at once (key to
  frame 22.7 ms median instead of 261 ms), but a continuous flood fills the present queue and
  runs at the same 4 frames a second again (256 ms mean). So the parked window's swapchain is
  released by DWM at about 4 Hz whatever the app does. A frame interval measured on a parked GPU
  window is that cadence, not the app's.
- **The 2.5 MB/s is not Flare either.** `cmd /c type flood.txt` runs at 2.5 to 2.8 MB/s on Flare
  GPU, Flare software and Prism Terminal 0.58.0 alike (2.47). With a writer that does not hit
  that limit (`flare_spew 100`, 120 byte lines, the M2 flood program) the same windowed Flare
  does **84.8 MB/s on the software renderer and 99.6 MB/s on the GPU renderer**, end to end,
  and the headless ConPTY test does 89.5. So 100 MB through the whole window takes about a
  second; the `type` command is what takes 37 s.

## Step 2: what was done, and what the trace did not justify

| Item | Verdict | Why |
|---|---|---|
| 1 damage rebuild (`Scene::update_rows`) | **not built** | scene build is 0.15 ms median, 0.22 ms p95; 5% damage could save at most 0.2 ms of a 6 ms frame. The plan's own rule is to fix what the trace shows. |
| 2 lock held while the UI copies | **done** | `Grid::reshape`, `Emulator::snapshot_into`, `TermHandle::snapshot_into`, `GridSource::grid_into` fill one buffer the host keeps: no allocation under the lock. At 480 x 135 the p90 hold went from 0.63 to 0.37 ms (`snapshot_holds_the_lock_under_1_ms`, release limit 1 ms). |
| 3 frame pacing | **done for the software path** | A wake-up now paints at once when the last paint is a frame old (`paint_on_wake`), else the 16 ms timer paints it, so output never waits for a wake and then the timer. The GPU path already coalesces wake-ups (`WakeGate`, `output_wakes_a_redraw_and_coalesces`) and paints in `BeforeRendering`. Software B4 median moved from 22.1 to 21.6 ms and p95 from 38.5 to 35.7 ms: the rest is the 8 ms batcher and ConPTY (Task 68). |
| 4 reads are small | **done** | `ReadSizer`: 64 KiB at rest, doubling to 1 MiB after two full reads in a row, back to 64 KiB at the first short read. The 8 ms window and the 1 MiB cap are unchanged (`the_flush_rule_still_holds`). |
| 5 glyph upload | **not built** | 0.09 ms median per frame, one submit per frame already; rasterising on a worker would save nothing visible. |

The M2 flood tests stay green: `a_100_mb_flood_keeps_snapshots_fast_and_arrives_complete` passes in
1.1 s in release, worst snapshot well under 1 ms.

## Acceptance (B3 and B7): not met in this harness, and why

| Bar | Result | Reading |
|---|---|---|
| B7 render p95 at 225% <= 8.3 ms | 5.9 ms (1000x640), 6.4 ms (1707x960): **pass** | measured inside the notifier |
| B7 presented-frame interval <= 33 ms | 245 to 267 ms | the parked window's DWM cadence above; **unmeasurable parked** |
| B3 no frame over 33 ms (GPU) | same 4 Hz cadence | same cause |
| B3 throughput >= 2 x Prism Terminal | 2.5 to 2.8 MB/s against 2.47 | all three terminals share the `type` limit; Flare is 84.8 / 99.6 MB/s with a fast writer, 34 to 40 times Prism Terminal's `type` figure |

Nothing in the harness or a bar was edited. What the owner can choose (D8, Q1):

1. Judge B3's throughput with a writer that is not the limit (a program that writes in large
   blocks, like `flare_spew`, instead of `cmd`'s `type`), and B7's interval on a window the
   compositor really draws, once, with the owner's say so (a parked window can never show it).
2. Keep the bars as written and take the two as "unmeasurable here".
3. For B4 only: the parked GPU number is 261 ms because of the same cadence; Task 70 can run it
   with `WGPU_DX12_USE_FRAME_LATENCY_WAITABLE_OBJECT=none` in the environment (22.7 ms), which
   the app already honours; it is a measuring condition, not a product setting (without the waitable
   a real window queues frames).

An idea that is not built because it changes the terminal's behaviour: ConPTY's pass-through mode
(`PSEUDOCONSOLE_PASSTHROUGH_MODE`) would stop the console host re-rendering `type`'s output and could
raise that ceiling; `portable-pty` does not expose the flag, and it changes cursor and resize
semantics, so it is the owner's call.
