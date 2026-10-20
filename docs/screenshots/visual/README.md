# Visual comparison: Flare and Prism Terminal (audit 2026-10-20)

Captured 2026-10-20 on the owner's PC (Windows 11, GPU and software renderers; the monitor refreshes faster than
60 Hz). Flare is the audit-fixes build (0.1.5 + the tour's probes); Prism Terminal is the installed 0.58.0, run from a
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
| Flare GPU, 100 / 150 / 225 % | 21 to 22 | 8.9 to 12.0 ms | 14 to 15 / 68 to 70 / 135 to 137 ms | 0.016 to 0.024 |
| Flare software, 100 / 150 / 225 % | 22 to 26 (15 / 16 at 225) | 8.7 to 9.9 ms (14.5 ms at 225) | 13 to 18 / 68 to 72 / 135 to 140 ms | 0.009 to 0.020 |

- The easing and the duration match: same curve, same 240 ms, t50/t90/t99 within a few ms (Flare's are 2 to 4 ms
  earlier, which is the half-frame of where the first frame lands). No stutter: the medians of the longest gap are under 15 ms in both
  apps (Flare software at 225 % is the largest, 14.5 ms).
- Flare presents fewer frames in the same 240 ms than Prism Terminal (about 21 against 27): its frames come every 9
  to 12 ms, Prism Terminal's every 7 ms. Both are smooth; Flare is not tracking the display's full rate during the slide.
  At 225 % software it drops to 15 frames at 14.5 ms (about 70 fps), the one cell where the slide is visibly coarser.
- The terminal keeps its width after a round trip (102 columns before and after), and goes 102 to 131 collapsed. The
  snapshots show the terminal text moving with the slot edge in both apps; no jump in the tab strip or the rows.
- Input to first frame: Flare 19 to 27 ms, Prism Terminal 9 to 12 ms.

Start-up (from process start; Flare's tabs and prompt times are wall clock from `CreateProcess`, Prism Terminal's are
from `process.uptime()`, so a few ms apart in kind):

| | first frame / paint | tabs there | prompt |
|---|---|---|---|
| Prism Terminal | 303 to 308 ms (resume 325 to 342) | 198 to 212 ms | 715 to 786 ms |
| Flare software | 46 ms | 80 ms | 138 ms (resume 154) |
| Flare GPU | 738 ms (resume 749) | 748 ms | 816 ms (resume 824) |

- Flare GPU's first frame is late: `startup.trace` shows `backend` at 491 ms and the first frame at 772 ms, against 46 ms
  on the software path. Until then the window is blank (see below).
- The pictures from outside (`startup/`, `resume/`): Flare's window is created at 1280 x 665, shows a black picture,
  then is resized to 1180 x 780 and painted; on the GPU path a picture with a light strip along the top (the default
  window colour showing where the swapchain does not cover yet) comes between the black one and the first UI. Read this
  with care: the e2e window is shown at once (`showInactive`) and `PrintWindow` of a swapchain that has presented
  nothing is black, so a user may not see every one of these pictures; the resize from 1280 x 665 to 1180 x 780 and
  the black first picture are real states of the window. Prism Terminal's first picture is already its UI, because
  Playwright attaches after the page has loaded (180 ms), so its pre-paint window is not seen by this method; its
  first-paint time above is.
- Resume: Flare shows all three tabs in one step with the saved middle tab in front (timeline in `numbers.json`: no
  tab popping in, no front change, no folder change afterwards). Prism Terminal likewise. Scrollback is restored by
  neither app (a new shell starts in each tab).
- Flare's start-up scale is the window's own (100 % for the software path and 225 % for the GPU path on this PC, so the
  folders are named `100-software` and `225-gpu`); the app cannot start at a chosen scale (the harness changes DPI
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
