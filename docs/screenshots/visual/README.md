# Visual comparison: Flare and Prism Terminal (audit 2026-10-20)

Captured 2026-10-20 and re-captured whole 2026-10-21 (branch head `5aa8c7f`, a release build, run at BelowNormal priority) on the owner's PC (Windows 11, GPU and software renderers; the monitor refreshes faster than
60 Hz). Flare is the audit-fixes build (0.1.0 + the tour's probes); Prism Terminal is the installed 0.58.0, run from a
COPY of its folder with its own profile (see "Safety"). Every number is in `numbers.md` (made by
`tools/visual/summarize.py`) and in each cell's `numbers.json`; the pictures are in each cell's `frames/` (not
committed, 21 MB; `sheet.png` and `compare-<scale>.png` are).

    docs/screenshots/visual/<surface>/<scale>-<gpu|software>/    Flare      sheet.png, numbers.json
    docs/screenshots/visual/pt/<surface>/<scale>-pt/             Prism Terminal, the same
    docs/screenshots/visual/<surface>/compare-<scale>.png        Flare gpu, Flare software and PT, stacked
    docs/screenshots/visual/numbers.md                           every table

Surfaces: `startup`, `resume`, `sidebar`, `general` (tab strip, tab menu, command help, Settings pages, three theme
switches, see-through 40), `dictation` (Flare only: the pill listening, with live text, transcribing).

## How to run it again

    cargo build --release -p flare -p flare-e2e --target-dir <T>
    <T>\release\flare-e2e.exe --exe <T>\release\flare.exe visualTour      # Flare, about 6 minutes
    node tools/pt-shot/pt-visual.mjs --out docs/screenshots/visual/pt     # Prism Terminal, about 8 minutes
    python tools/visual/summarize.py                                       # sheets and numbers.md

`visualTour` is opt-in (`OPT_IN` in `tools/e2e/src/scenarios/mod.rs`): a plain e2e run never picks it. It reads
`FLARE_VISUAL_OUT`, `FLARE_VISUAL_CELLS` (`225-gpu,100-software`), `FLARE_VISUAL_SURFACES` (`sidebar,general`) and
`FLARE_VISUAL_RUNS` (timed start-up launches, 5).

## How it captures

- Flare: the window stays parked offscreen and unfocused (the harness guards hold; every launch re-checks
  `window.park`). Pictures come from the hook's `Snapshot`. The new probes `frames.series` (every presented frame: age,
  gap, render time, the sidebar's slot width and panel x at that frame) and `sidebar.anim` give the per-frame numbers.
  The GPU renderer's frames are its rendering notifier's; the software renderer has none, so its frames are the window's
  redraw requests. Start-up and resume pictures are copied from OUTSIDE with `PrintWindow` from process creation
  (`tools/e2e/src/winshot.rs`, before the pipe exists), each changed picture kept with its time.
  The harness wakes the monitor with a zero-distance mouse move before the timing checks.
- Prism Terminal: Playwright's Electron driver on the copy; the sidebar numbers come from a `requestAnimationFrame`
  recorder of the slot's width and the panel's left; the pictures of the slide are taken with every CSS transition
  paused at the offset (`document.getAnimations()`), exact where Flare's are within a few ms.
- Prism Terminal's page must lay out at exactly 1000 x 640 css px (audit V8, 2026-10-21). The old sizing step left it at
  1000, 1001.3 and 1002.2 css px wide at 100, 150 and 225 percent (the frame adds a few DIPs, and parking the window
  again re-applied them), so a picture cut at 1000 css px showed PT's right-anchored chrome 0, 2 and 5 device px to the
  right of where the same page puts it: that was the "Flare is left of PT" finding, and Flare reserves no inset or
  resize band (its chrome sits at the window width less 6 css px, as PT's `pr-1.5`). `size()` in `pt-visual.mjs` now
  compares the fractional `visualViewport` and `shot()` re-sizes before a picture. Measured on the re-taken general
  frames (06-settings-dictation): the buttons' ink right edge is 2207 for both at 225 percent (was 2207 / 2212), 1471
  / 1471 at 150 (1471 / 1473), 980 / 981 at 100 (one pixel of glyph rounding); the settings card's right edge is
  2150 for both at 225 (was 2150 / 2155). The sidebar PT cells were not re-taken (their numbers are in css px).
  Dictation page pickers (V12, 2026-10-21, general 225-gpu re-taken): the brightest pixel in the Microphone and Spoken
  language picker boxes is 116 for both, the dimmed labels 116 for both, the Test button 73 for both (Flare was 72 / 72
  against PT's 115 / 115 before: the picker dimmed twice, once with the row and once on its own).
- The slide numbers use one set of definitions in both tools (`tools/e2e/src/slide.rs`, mirrored in `pt-visual.mjs`):
  frames from the one before the first move to arrival, the longest gap between presented frames, the time to 50, 90
  and 99 percent, and the largest distance of the curve from `cubic-bezier(0.16, 1, 0.3, 1)` over 240 ms (the curve may
  slide by up to 30 ms to fit, so the start is not counted against it).

## What the numbers say

Sidebar (slot width, median of three; both apps declare 240 ms and `cubic-bezier(0.16, 1, 0.3, 1)`):

| | frames | longest gap | t50 / t90 / t99 | curve error |
|---|---|---|---|---|
| Prism Terminal, 100 / 150 / 225 % (2026-10-21 re-capture) | 11 to 38 | 18.7 to 19.1 ms | 7 to 8 / 62 / 131 to 132 ms | 0.002 to 0.008 |
| Prism Terminal, all scales (2026-10-20 capture, machine not throttled) | 27 | 7.0 ms | 18 / 72 / 142 ms | 0.000 to 0.003 |
| Flare GPU, 100 / 150 / 225 % (2026-10-21, re-taken after V3-gpu-snapshots) | 7 to 8 | 67.9 to 71.4 ms | 9 to 11 / 67 to 76 / 140 to 149 ms | 0.004 to 0.007 |
| Flare software, 100 / 150 / 225 % (2026-10-21, re-taken after V3-gpu-snapshots) | 13 | 17.0 to 17.8 ms | 9 to 10 / 63 to 64 / 133 to 134 ms | 0.001 to 0.008 |
| Flare GPU before V3 (2026-10-20, same machine state) | 7 | 67 to 71 ms | 8 to 27 / 66 to 78 / 139 to 153 ms | 0.116 to 0.206 |
| Flare software before V3 (2026-10-20) | 11 | 17.7 to 20.9 ms | 7 to 8 / 61 to 63 / 127 to 128 ms | 0.018 to 0.019 |

- Audit V3 (2026-10-21): the slide now starts at the first frame that draws it, as a CSS transition does, and every
  frame reads the curve at its own moment (`crates/flare/src/side_slide.rs`; the GPU path at its `BeforeRendering`, after
  the vsync wait). The curve error is now 0.002 to 0.008 (it was 0.018 to 0.206), near Prism Terminal's 0.000 to 0.009 on the re-capture.
- t50/t90/t99 count from the first frame that MOVED. Flare's first frame shows the start (progress 0, as CSS) and its
  next frame comes one compositor frame later (16.7 ms here); t50 reads 9 to 11 ms for Flare against 7 to 8 for Prism
  Terminal, t90 63 to 79 against 62, t99 133 to 150 against 131 to 132: the same curve, a few ms apart, and Flare's GPU
  numbers are carried by its 15 frames a second (below).
- Prism Terminal's old row (27 frames, 7.0 ms gap, t50 18 ms) was an artefact of the recorder in `pt-visual.mjs`: each run's
  `requestAnimationFrame` loop kept running into the next run's fresh object, so run k pushed 2k-1 rows per frame and the
  frame counts and gaps were wrong. Fixed 2026-10-21 (a loop stops when it is no longer the current recorder); the
  sidebar cells were re-taken. Prism Terminal presents 11 distinct frames in the slide, 18.5 to 19.0 ms apart (about
  54 Hz on this run), the same on all three scales.
- THE MACHINE WAS THROTTLED on this capture: the monitor was woken with a zero-distance mouse move before the
  Flare tour and before Prism Terminal's run, both ran at BelowNormal priority, and the Flare GPU path still presented
  about 15 frames a second (66 to 71 ms gaps, 7 to 8 frames; every GPU cell, all three scales), while the software path
  gave a frame every compositor frame (16.9 to 18.1 ms, 13 frames) and Prism Terminal one every 18.5 to 19 ms. The GPU
  frame rate is the machine, not the slide, and the GPU slide numbers (frames, longest gap, input to first frame, the
  sidebar snapshot offsets are now exact, see the next point) are NOT a baseline; do not compare them with Prism
  Terminal's frame count. The software cells are the trustworthy Flare numbers on this machine state.
- The 15 frames a second are not Flare (audit V3-gpu-baseline, 2026-10-21, checked and nothing changed): a stand-alone
  wgpu 30 DX12 program with none of Flare's code (an 800 x 500 window, clear and present in a loop, Fifo present mode,
  BelowNormal, 3 s per run) gave a 77 ms median gap, about 13 to 20 frames a second, with the window parked at -32000 and
  with it on screen at 1/255 opacity, click-through and unfocused, on the HWND and on the DirectComposition swapchain
  alike (gaps 76.9 to 77.7 ms, max 80). The same program in Mailbox mode ran at 12,700 frames a second, so the
  program and the adapter are fine; it is the compositor's cadence. DwmFlush() called 60 times from PowerShell took
  4,663 ms (78 ms each), and Flare with FLARE_NO_GLASS=1 (no DirectComposition) gave the same 7 to 8 frames and 68 to
  78 ms gaps. The machine has virtual monitors (Virtual Desktop, Meta) next to the 143 Hz RTX 5090 output; DWM is
  pacing at about 13 Hz here whatever the app does. So the Flare GPU slide cannot be judged for smoothness on this
  PC in this state; its curve (error 0.004 to 0.007) and the exact snapshot offsets are, and the software path's 13
  frames at 17 ms stand. To get a GPU baseline, run the tour when DwmFlush returns in about 7 ms
  ([W.D]::DwmFlush() in a loop, 60 calls under 0.6 s). The "one frame behind Prism Terminal" snapshot offsets (collapse
  142/84/54/33 against 125/69/38/21) were the old pictures, read when a slow GPU snapshot ended; since
  V3-gpu-snapshots the pictures sit on the curve at the offset (125, 69, 38, 21) and match.- Sidebar pictures (V3-gpu-snapshots, 2026-10-21): the GPU sheets used to read slot 154, 160, 16, 164, 18, 159 for offsets 0 to
  100 ms and the slide seemed to open and close between pictures. Cause, measured with a frame log per picture: the
  harness asked for a picture at an offset, but a GPU snapshot blocks the UI thread for 30 to 90 ms (29 to 87 ms measured)
  while the slide runs on, and the slot is read when the snapshot ENDS. The frames underneath were monotonic (220 at 73 ms,
  159 at 85 ms, 22 at 154 ms: the 15 frames a second above, in pairs 12 ms apart with a 70 ms gap); only the label was
  wrong, and it varied with the snapshot time. It was not the slide starting late and not a stale read. Fix: the e2e
  request `SlideHold` puts the slide exactly the offset into its time (`SideSlide::place`) and holds it while the
  picture is taken, as `pt-visual.mjs` pauses CSS transitions at the offset; the files are named by the offset, and a
  new check fails any picture more than 0.6 px off the curve. Every sidebar cell (GPU and software) was re-taken and every
  picture now sits on `cubic-bezier(0.16, 1, 0.3, 1)`: collapse 220, 125, 69, 38, 21, 12, 4, 1, 0, 0 and expand
  0, 95, 151, 182, 199, 208, 216, 219, 220, 220 at 100 % and 225 %. Each cell's `numbers.json` keeps `snapshot_ms` and the
  `frames_since_key` of each picture. The GPU frame cadence itself (15 a second, 68 to 70 ms gaps) is unchanged and is
  still the machine state noted above, not something this fix claims to change.
- The terminal keeps its width after a round trip (102 columns before and after), and goes 102 to 131 collapsed. The
  snapshots show the terminal text moving with the slot edge in both apps; no jump in the tab strip or the rows.
- Input to the first frame that moved: Flare GPU 21.6 to 23.5 ms, Flare software 16.1 to 17.1 ms, Prism Terminal 23 to 33
  ms (its key goes through Playwright and the renderer's event loop). Both apps open the sidebar with the middle tab
  (beta) selected, so the frames show the same tab; Flare's cell checks it.

Start-up (from process start; Flare's tabs and prompt times are wall clock from `CreateProcess`, Prism Terminal's are
from `process.uptime()`, so a few ms apart in kind):

| | first frame / paint | tabs there | prompt |
|---|---|---|---|
| Prism Terminal | 228 to 247 ms (resume 282 to 300) | 200 to 215 ms | 723 to 742 ms (resume 761 to 780) |
| Flare software | 47 ms (resume 46) | 90 ms | 156 ms (resume 159) |
| Flare GPU | 746 ms (resume 752) | 756 ms | 835 ms (resume 872) |

- Flare GPU's first frame is late: `startup.trace` shows `backend` at 478 ms and the first frame at 746 ms, against 46 ms
  on the software path. Until then the window is not shown at all (see below), so nothing blank is on the screen.
- The pictures from outside (`startup/`, `resume/`): the window first comes on the screen (shown and not cloaked) at
  about 788 to 795 ms on the GPU path and 124 to 125 ms on the software path, and its first picture there is already the UI in the
  theme's colours, frameless and at its size (2655 x 1755). The black, light-framed and 2880 x 1497 pictures before
  that are copies of a window that is not shown yet (named `-hidden`; DWM never composes them). See "Audit V1" below.
  Prism Terminal's first picture is already its UI, because Playwright attaches after the page has loaded (180 ms), so
  its pre-paint window is not seen by this method; its first-paint time above is.
- The LAST software picture of start-up and resume is the app's own snapshot (`-app`, taken once the launch has been
  still for 700 ms), not a copy of the window: a copy of the shown software window carries the classic caption strip
  ("Flare", a close box) and a thick light frame that `PrintWindow` draws from the style bits and the screen never shows
  (those copies are named `-copyframe`, shown in the sheet, and left out of the judging), `PW_RENDERFULLCONTENT` of the
  parked GDI window is black and a `BitBlt` from its window DC is black (tried 2026-10-21, `tools/e2e/src/winshot.rs`
  `Method::Gdi`). Check on one real frame (`startup/225-software`): frame 003 (t 164 ms) is caught as `copyframe` and
  frame 005 (t 920 ms, `-app`) is the clean UI. The `last change` column of a software cell is therefore the time of the
  snapshot, not of a change of the window.
- Resume: Flare shows all three tabs in one step with the saved middle tab in front (timeline in `numbers.json`: no
  tab popping in, no front change, no folder change afterwards). Prism Terminal likewise. Scrollback is restored by
  neither app (a new shell starts in each tab).
- Flare's start-up scale is the window's own (225 % on this PC for the software and the GPU path alike, so the
  folders are named `225-software` and `225-gpu`); the app cannot start at a chosen scale (the harness changes DPI
  only after connecting), so only the native one is captured for start-up. Prism Terminal's start-up is captured at 100,
  150 and 225 %.

## Like for like (2026-10-21)

- Tabs: every `general` frame of both apps shows alpha, beta and gamma with gamma in front (the three folders as
  arguments), including `see-through-40` (Flare's second launch used one tab before and now opens the same three; PT's
  frame is taken in its three-tab session). Every `sidebar` frame of both apps is a restored session of the same three
  tabs with the middle one (beta) selected and the vertical sidebar open (Flare's cell seeds `tabs.json` with
  `active: 1`, as PT's `prepResume` does, and checks the front tab). Start-up is one tab (alpha), resume three with
  beta in front, as before.
- Run: a release build, the monitor woken by a zero-distance mouse move, both tools at BelowNormal priority, one app
  at a time; Flare `visualTour` 233 checks, 0 failures, 342 s.

## Not captured, and why

- The dictation widget (the second window of "Dictate from any app"): it needs the speech engine and the routing of a
  front window of another app (`dictateAnywhere`'s setup); the tour records `dict.widget` as absent. The pill is
  captured (`dictation/`). Prism Terminal's pill and widget are not captured: they need its speech engine and a real
  microphone path, and the copy must never open the microphone. Its CSS (`overlay.css`: `rise 160ms
  cubic-bezier(0.16, 1, 0.3, 1)`, level bar `90ms linear`) matches Flare's `dictation_widget.slint`
  (160 ms, same curve; 90 ms linear), checked from source.
- Theme switches: Flare's by the Settings wall (a pick), Prism Terminal's by a stored value and a reload, so the
  "at once" pictures are not the same event; the Flare "at once" picture is taken as the pick returns, which is after
  its own settle wait.
- Prism Terminal's start-up pictures before its page loads (see above).
- A real display at 100 to 300 %: the matrix is 100, 150 and 225 % (300 % is held by `dpiMatrix`).

## Safety

Prism Terminal is only ever run from `%TEMP%\pt-visual-*\app`, a copy of the installed folder, with
`--user-data-dir` inside that temp folder and `--e2e` (parked, unfocusable); the installed copy and `%APPDATA%` are not
read or written, and what the script reaps is matched by the temp folder's path. Flare ran from its own worlds with
the harness's guards. Neither the real microphone, the keyboard hook nor the registry was touched.

## Audit V1: the black and light-framed start-up pictures are copies of a window not yet shown (2026-10-20)

The GPU start-up and resume cells showed an all-black picture, then a light Windows-classic caption band (191, 206,
220) and border (215, 228, 242) around a black client area for about 100 ms, then the UI; the resume cell's first
picture was 2880 x 1497 before 2655 x 1755. The tour now reads the window manager's facts with every copy
(`winshot::state`: `IsWindowVisible`, `WS_VISIBLE`, `DWMWA_CLOAKED`, the window and client rectangles, the styles) and
records them in each frame of `numbers.json` (`on_screen`, `framed`, `client`, `style`, `ex_style`, `tag`).

- Every one of those pictures was copied while the window was NOT shown (`style` 06cf0000, no `WS_VISIBLE`). The
  winit backend creates the window hidden, Slint sizes it and presents the first frame, and only then shows it
  (`set_visibility`: `draw()` before `set_visible(true)`). `PrintWindow` copies a hidden window anyway, and paints the
  classic frame that its style bits describe: winit keeps `WS_CAPTION | WS_THICKFRAME` on a frameless window and
  removes the frame in `WM_NCCALCSIZE`. The 2880 x 1497 picture is the window during `CreateWindowEx`, before winit
  applies the size (style 06cc0000), also hidden.
- The first copy taken once `WS_VISIBLE` is set (style 16cf0000) is the UI in the theme's colours, with the client
  rectangle equal to the window rectangle (no frame) and the window at 2655 x 1755, on both renderers, start-up and
  resume. No app change was needed, and start-up time is unchanged.
- The software path's later copy with the classic caption (`-copyframe`) is the same `PrintWindow` painting on a
  window that is on the screen with no frame (`GetClientRect` = the window rectangle): the screen does not show it.
- visualTour now checks, per start-up and resume cell, that the window came on the screen, and that from then on no
  picture is blank, framed or of another size, and the first one is painted. `tools/visual/summarize.py` judges only
  the pictures the screen can show and counts `not shown` and `copier frame` in their own columns.

## Audit V2: the black block in the software start-up and resume pictures is not the app (2026-10-20)

The earlier software pictures from outside showed a black rectangle from about (13, 65) to the right and bottom edge
over a painted window. It is not in the window. Two faults of the copier, both fixed in `tools/e2e/src/winshot.rs`:

- The harness is DPI-unaware, so `GetWindowRect` gave the window's size divided by the scale (1180 x 780 for a
  2655 x 1755 window) and the copy was a crop. The copy now runs with per-monitor-v2 awareness for the call and has the
  window's own size (a visualTour check compares it with the app's `window.park` rectangle).
- `PrintWindow` with `PW_RENDERFULLCONTENT` of the software renderer's window (a GDI surface) places the client area
  by the classic frame the window's style carries (winit's undecorated-shadow `WS_CAPTION`), ignoring the app's own
  `WM_NCCALCSIZE`, and answers black for it. Sampled from inside the process with `GetPixel` on the window's own
  device context, the surface was painted everywhere (a 6 x 6 lattice, no black) from the first paint to ready, and
  stayed so. A new visualTour check fails a last picture whose mean colour is near black. Software windows are now
  copied with `PrintWindow` and `PW_CLIENTONLY` (`Method::Gdi`); GPU windows keep `PW_RENDERFULLCONTENT`.
- A shown software window's later copies still carry the system caption strip ("Flare" and a close box) and a thick
  light frame, drawn by `PrintWindow` itself from that same style (the app never shows it: client area and window
  rectangle are equal, `GetClientRect` 2655 x 1755). Re-checked 2026-10-21: `PW_CLIENTONLY` and no flag draw it,
  `PW_RENDERFULLCONTENT`, `BitBlt` from the window DC and a direct `WM_PRINT` answer black, so no copier can show the
  screen for this window while it is parked. The tour keeps tagging those copies `copyframe` and takes the last
  picture from the app's own snapshot instead (see the start-up section above).
- Audit V6: `hello.scale` read 1.0 for the software window while its window was 2655 x 1755 at the monitor's 225 %, so the
  start-up and resume cells were wrongly named `100-software`; the software path never starts at 100 %. Cause: the
  renderer word and scale are noted before the window has its monitor's scale. Fix: `e2e_hook::set_hello_scale` runs on
  every move, resize and scale change, so Hello reports the window's own scale; the cells are now `225-software` (the old
  `100-software` start-up and resume folders were removed).

## Ink weight, pill width and preview baselines (audit V13, 2026-10-21)

- Weight 600 is NOT lost on the GPU path. The segmented label 'Horizontal' at 100 percent on the GPU has 26.5k of ink at
  weight 600 and 20.6k at weight 400 (a capture of each, same page), and GPU ink is at or above the software
  renderer's at 100, 150 and 225 percent. Prism Terminal's labels are about 19 percent heavier still (31.7k): that is
  Chromium's own rasteriser (ClearType at 100 percent), not a weight. Flare does not copy it.
- The Panel edges pill was 233 css px wide against PT's 230 / 231 / 231.2 (100 / 150 / 225): Slint rounds a Text's
  preferred width up to a whole px, four times. The segment now takes a sixteenth of the label repeated sixteen times.
  After: the pill's left edge is 711.0 at 100 percent on both apps.
- The theme preview's text sat 1.3 to 1.8 css px under PT's baselines; line 2's baseline is now at 473.0 on both at
  100 percent (472.3 at 300 percent).
- FLARE_VISUAL_CELLS=300-gpu runs the 300 percent cell (opt-in; ll keeps the three PT also has).
