# Performance harness

`flare-e2e perf` measures Flare against the bar in `bar.json` (M10 Task 60, decisions D5 to D7 of
the plan). The bar file holds rules (`"<= pt / 3"`), never absolute Prism Terminal numbers; the
Prism Terminal side comes from `tools/pt-perf` (`pt-baseline-<date>.json`).

    flare-e2e perf [startup|memory|flood|latency|scroll|idle|frames|all]
        [--exe <path>] [--renderer gpu|software|both] [--runs N] [--scale 225]
        [--shell cmd|pwsh] [--out <json>] [--baseline <json>]

It prints `id metric renderer flare rule pt verdict` and writes `docs/perf/flare-<date>.json`
(or `--out`). Exit code 1 when a number breaks the bar. A rule that needs the baseline and has none
is `unknown`, never `pass`.

| Id | Measure | Unit | Notes |
|---|---|---|---|
| B1 | cold start to a usable prompt | ms | median of `--runs` (7) launches after one discarded; the app's own `usable_ms` from process creation; `--no-import`; `--shell pwsh` is reported with no bar |
| B2 | idle private working set | MB | app tree minus shells; 10 s after the prompt; median of 3 sessions; the CIM query `tools/pt-perf` uses |
| B3 | flood throughput | MB/s | `type` of a 100 MB file; frames counted from 500 ms after the Enter |
| B4 | key to frame (median) | ms | 200 keys, 20 ms apart; the first two keys of each line are typed but not timed |
| B5 | scroll frames per second | fps | 100,000 lines, then 5000 wheel events 1 ms apart |
| B6 | idle CPU | % of a core | kernel plus user time of the app tree over 10 s, after a 3 s warm-up |
| B7 | render p95 at 225% | ms | GPU only; 1000 x 640 and 1707 x 960 css px, during the flood and a 3 s resize drag |

Every gated number is measured inside the app (`startup.trace`, `Frames`, `Latency`); the harness
clock is only a cross-check. Known limits: the flood's 500 ms are the harness's (the frames window
starts there, and the seconds are those 500 ms plus the app's own wait); a flood that ends inside
them is flagged `short`. A GPU run on a PC without an adapter is reported and skipped. One Flare
app runs at a time, always `--e2e` (offscreen, unfocusable).

`FLARE_TRACE_FRAME=1` (with `RUST_LOG=info,flare_render::stats=trace`) logs the phases of every
frame; what it showed for the flood is in `heavy-output.md`. If every GPU frame interval reads about
250 ms, the PC's display path is in the 4 Hz state described in `../performance.md` (not the app, and
not parking as such): the GPU rows of that run measure the machine, not Flare.

The M10 verdict, every bar against Prism Terminal with the owner's choices for each miss: `../performance.md`.

The memory diet (M10 Task 66): what the private working set holds, what each candidate saved or did not, and where B2 stands is in `memory.md`.
