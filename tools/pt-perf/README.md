# pt-perf

Measures the INSTALLED Prism Terminal the way Flare is measured (plan M10, Decisions D5 and D7), so the
performance bar can be derived from a baseline taken on this PC the same day.

    node tools/pt-perf/pt-perf.mjs --out docs/perf/pt-baseline-2026-10-18.json [--only start,memory,flood,latency,scroll,idle] [--runs 7]
    node tools/pt-perf/pt-perf.mjs --selftest           tiny sizes, 1 run each, same JSON shape, exit 1 on a missing field
    node tools/pt-perf/pt-perf.mjs --selftest-offline   starts no app: checks the shape checker, the statistics and the flood writer

It drives `C:\Users\Admin\AppData\Local\Programs\PrismTerminal\PrismTerminal.exe` through Playwright's Electron
driver (from the Prism repo's `node_modules`), each session in a throwaway `--user-data-dir` under `%TEMP%`, with
`--e2e` (window parked offscreen, unfocusable). Processes are reaped by profile path only. It never touches
`%APPDATA%\PrismTerminal*` and exits 1 if a real profile's timestamp changed or the temp profile is left behind.
`--exe <path>` overrides the installed copy.

## What each number is

| Key | Unit | How it is measured |
|---|---|---|
| `start_ms.<shell>.runs` | ms | per launch: `prompt_ms + echo_ms` |
| `start_ms.<shell>.prompt_median` | ms | zero is Electron's `process.getCreationTime()`; end is the page's `Date.now()` in the first animation frame whose xterm row ends with the prompt (`>` for cmd) |
| `start_ms.<shell>.echo_median` | ms | one `x`: `keydown` (capture) to the DOM mutation that shows it |
| `start_ms.<shell>.median` | ms | median of `prompt + echo` over `--runs` (default 7) after 1 discarded launch. The bar uses `cmd`; `pwsh` is reported beside it with no bar |
| `start_ms.<shell>.outside_median` | ms | harness wall clock, launch call to the echo; a cross-check only |
| `start_ms.<shell>.late_hook` | count | runs whose prompt was already there when the page hook first looked: that run's `prompt_ms` is an upper bound |
| `idle_private_ws_mb.cmd.processes[]` | MB | private working set (`WorkingSetPrivate`, CIM) of the Electron main and every descendant that is not `cmd`, `pwsh`, `powershell`, `OpenConsole`, `conhost` or `node`, 10 s after the prompt |
| `idle_private_ws_mb.cmd.total` | MB | the sum; the median session of 3 |
| `flood.mb`, `flood.seconds`, `flood.mb_per_s` | MB, s, MB/s | a 100 MB file of 100-byte lines ending `FLOOD-END`, `type "<file>"` + Enter, time to `FLOOD-END` in the buffer |
| `flood.frames.count`, `p95_ms`, `max_ms`, `over_33` | frames, ms, ms, frames | animation-frame intervals from 500 ms after Enter to the end; `over_33` counts gaps above 33 ms |
| `key_latency_ms.n`, `median`, `p95`, `max` | keys, ms | 200 keys 20 ms apart, `keydown` (capture) to the DOM mutation that shows the echoed character |
| `scroll.seconds`, `fps`, `max_frame_ms` | s, frames/s, ms | after a 100,000-line flood, 1000 wheel events over 5 s; animation frames per second and the longest gap |
| `idle_cpu_pct_of_core.cmd` | % of one core | kernel + user CPU time of the same process set over 10 s at the prompt |

Known limits, written down on purpose: the page hook is installed through Playwright, so it can start a few
milliseconds after the page does (`late_hook` shows when that mattered); the window is parked offscreen, so a
frame is the page's own render and animation frame, never the compositor's; xterm here is the DOM renderer,
so "shows the character" is read from `.xterm-rows`.
