# Visual comparison: Flare and Prism Terminal (audit 2026-10-20)

Captured 2026-10-20 on the owner's PC (Windows 11, GPU and software renderers; the monitor refreshes faster than
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
- The slide numbers use one set of definitions in both tools (`tools/e2e/src/slide.rs`, mirrored in `pt-visual.mjs`):
  frames from the one before the first move to arrival, the longest gap between presented frames, the time to 50, 90
  and 99 percent, and the largest distance of the curve from `cubic-bezier(0.16, 1, 0.3, 1)` over 240 ms (the curve may
  slide by up to 30 ms to fit, so the start is not counted against it).

## What the numbers say

Sidebar (slot width, median of three; both apps declare 240 ms and `cubic-bezier(0.16, 1, 0.3, 1)`):

| | frames | longest gap | t50 / t90 / t99 | curve error |
|---|---|---|---|---|
| Prism Terminal, all scales | 27 | 7.0 ms | 18 / 72 / 142 ms | 0.000 to 0.003 |
| Flare GPU, 100 / 150 / 225 % (V3, 2026-10-21) | 7 to 8 | 68.5 to 70.1 ms | 9 to 10 / 67 to 78 / 142 to 150 ms | 0.004 to 0.008 |
| Flare software, 100 / 150 / 225 % (V3, 2026-10-21) | 13 | 16.8 to 17.3 ms | 9 to 10 / 63 to 64 / 133 to 134 ms | 0.002 to 0.006 |
| Flare GPU before V3 (2026-10-20, same machine state) | 7 | 67 to 71 ms | 8 to 27 / 66 to 78 / 139 to 153 ms | 0.116 to 0.206 |
| Flare software before V3 (2026-10-20) | 11 | 17.7 to 20.9 ms | 7 to 8 / 61 to 63 / 127 to 128 ms | 0.018 to 0.019 |

- Audit V3 (2026-10-21): the slide now starts at the first frame that draws it, as a CSS transition does, and every
  frame reads the curve at its own moment (`crates/flare/src/side_slide.rs`; the GPU path at its `BeforeRendering`, after
  the vsync wait). The curve error is now 0.002 to 0.008 (it was 0.018 to 0.206), near Prism Terminal's 0.000 to 0.003.
- t50/t90/t99 count from the first frame that MOVED. Flare's first frame shows the start (progress 0, as CSS) and its
  next frame comes one compositor frame later (16.7 ms here), Prism Terminal's 7 ms later, so Flare's t50 reads 9 ms
  against 18: both reach 50 percent about 25 ms after the slide starts, which is the declared curve.
- The frame rate is the machine, not the slide, on these runs: the desktop compositor reported 60 Hz
  (`DwmGetCompositionTimingInfo`, 16.67 ms) although the panel is set to 143 Hz, and the GPU path presented only about
  15 frames a second (68 to 70 ms gaps, the monitor not awake to the harness's zero-distance mouse move), as the
  baseline build did too. The software path now gives a frame every compositor frame (16.8 to 17.3 ms, 13 frames).
  The slide asks the OS window for each next frame at once, so with the monitor awake it should follow the
  display's rate like Prism Terminal; that was not measurable on these runs.
- The terminal keeps its width after a round trip (102 columns before and after), and goes 102 to 131 collapsed. The
  snapshots show the terminal text moving with the slot edge in both apps; no jump in the tab strip or the rows.
- Input to the first frame that moved: Flare 16 to 22 ms (2026-10-21; 19 to 27 ms before V3), Prism Terminal 9 to
  12 ms.

Start-up (from process start; Flare's tabs and prompt times are wall clock from `CreateProcess`, Prism Terminal's are
from `process.uptime()`, so a few ms apart in kind):

| | first frame / paint | tabs there | prompt |
|---|---|---|---|
| Prism Terminal | 303 to 308 ms (resume 325 to 342) | 198 to 212 ms | 715 to 786 ms |
| Flare software | 46 ms | 89 ms | 147 ms (resume 156) |
| Flare GPU | 746 ms (resume 747) | 755 ms | 815 ms (resume 953) |

- Flare GPU's first frame is late: `startup.trace` shows `backend` at 491 ms and the first frame at 772 ms, against 46 ms
  on the software path. Until then the window is not shown at all (see below), so nothing blank is on the screen.
- The pictures from outside (`startup/`, `resume/`): the window first comes on the screen (shown and not cloaked) at
  about 795 ms on the GPU path and 125 ms on the software path, and its first picture there is already the UI in the
  theme's colours, frameless and at its size (2655 x 1755). The black, light-framed and 2880 x 1497 pictures before
  that are copies of a window that is not shown yet (named `-hidden`; DWM never composes them). See "Audit V1" below.
  Prism Terminal's first picture is already its UI, because Playwright attaches after the page has loaded (180 ms), so
  its pre-paint window is not seen by this method; its first-paint time above is.
- Resume: Flare shows all three tabs in one step with the saved middle tab in front (timeline in `numbers.json`: no
  tab popping in, no front change, no folder change afterwards). Prism Terminal likewise. Scrollback is restored by
  neither app (a new shell starts in each tab).
- Flare's start-up scale is the window's own (225 % on this PC for the software and the GPU path alike, so the
  folders are named `225-software` and `225-gpu`); the app cannot start at a chosen scale (the harness changes DPI
  only after connecting), so only the native one is captured for start-up. Prism Terminal's start-up is captured at 100,
  150 and 225 %.

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
- Left over: a late software picture can still carry the system caption strip ("Flare" and a close box) along the top
  and a one-pixel frame, drawn by `PrintWindow` itself from that same style. The app never shows it (client area and
  window rectangle are equal, `GetClientRect` 2655 x 1755).
- Audit V6: `hello.scale` read 1.0 for the software window while its window was 2655 x 1755 at the monitor's 225 %, so the
  start-up and resume cells were wrongly named `100-software`; the software path never starts at 100 %. Cause: the
  renderer word and scale are noted before the window has its monitor's scale. Fix: `e2e_hook::set_hello_scale` runs on
  every move, resize and scale change, so Hello reports the window's own scale; the cells are now `225-software` (the old
  `100-software` start-up and resume folders were removed).