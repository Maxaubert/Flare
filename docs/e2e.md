# The end-to-end suite (`flare-e2e`)

The suite drives the built release exe through a test hook and checks what the window shows, the
clipboard, the registry-free profile and the processes it starts. It is the local PR gate for Flare: CI
runs the unit tests, clippy, fmt and `cargo deny`, and this suite needs the machine (a GPU, a real
desktop session, the installed Prism Terminal for the performance baseline). Milestone 10 ported Prism
Terminal's 74 scenarios; `docs/test-port.md` ("E2E map") has the per-scenario table.

## Run it

```powershell
cargo build --release -p flare --target-dir <TARGET>
cargo run -p flare-e2e --release --target-dir <TARGET> -- --exe <TARGET>\release\flare.exe
cargo run -p flare-e2e --release --target-dir <TARGET> -- --exe <TARGET>\release\flare.exe spawn findKey
```

- No names runs every scenario in `scenarios::ALL` order. Names are case-insensitive substrings of the
  scenario names (`verticalTabs`, `dpi` and `colour` each select what they contain).
- `--list` prints the scenarios; `--run-one <name>` runs one in a child process (the runner uses it so one
  scenario's crash is one FAIL); `--keep` leaves the throwaway worlds on disk for reading; `--shots <dir>`
  moves the screenshots (default `.e2e-shots/` in the working folder; the folder is not in `.gitignore`, so
  do not commit it).
- A missing exe exits 1 with the hint to build first. `--packaged` drives the installed layout instead.
- `perf` runs the performance bar (`docs/performance.md`, `docs/perf/README.md`).
- Gentle mode on a shared PC: one scenario at a time, never two `flare.exe` at once.

Every run prints one line per assertion (`pass` or `FAIL`), then a table of scenario, checks, failures and
seconds. The exit code is 1 when any assertion failed.

## What the suite holds (the guards)

- **Offscreen, unfocusable, never foregrounded.** The app starts with `--e2e`, which makes the window
  unfocusable and shows it without activation; the window is parked at (-32000, -32000) and is never
  hidden (`DIFF-13` in `docs/parity.md`: parking spot and no opacity 0 differ from Prism Terminal's
  (-4000, -4000)). `GetForegroundWindow` is read before and after each scenario: a window of the run that
  takes the foreground is flagged, and another app taking it is only noted
  (`the_foreground_guard_flags_a_window_of_the_run_and_only_notes_another_app`).
- **The clipboard is never touched.** The app's clipboard is an in-memory stand-in under the flag; the
  guard compares the OS clipboard sequence number before the first and after the last scenario
  (`the_clipboard_guard_flags_a_sequence_change`).
- **One world per scenario.** Each scenario gets a throwaway folder (`flare-e2e-profile-*` with `profile`,
  `alpha` and `beta` inside) and its own `--user-data-dir`. The world is removed afterwards with retries,
  and nothing is left in `%TEMP%` (`the_world_removes_itself_and_leaves_nothing_in_temp`).
- **Reaping by profile path only.** After every scenario and once before the first, every process whose
  command line contains the world's path is killed with its tree. A process without the path is never
  touched, so the owner's own Flare or Prism Terminal is safe (`the_reaper_spares_a_process_without_the_world_path`).
- **Time limits.** Each scenario has a limit (180 s by default; `verticalTabs` and `indicatorStyles` 360 s,
  `linkPaintStall`, `helpPanel`, `updateWindow`, `dpiMatrix` and `monitorMove` 300 s). A timed-out child is
  killed and reported as one failure (`a_timed_out_child_is_killed_and_reported`).
- **Close races a timer.** `closeApp` waits up to 15 s for the app to exit, then kills it
  (`close_races_a_15_s_timer_then_kills`).
- **One wait primitive.** `until(what, timeout, condition)` is the only wait with a timeout; there are no
  retry loops (`no_retry_loop_outside_until`).
- **Stall report.** After each scenario the diagnostic log is read for UI stalls of 1000 ms or more. It is a
  report, not a gate (`the_stall_report_lists_only_ui_stalls_of_1000_ms_or_more`).

## Stand-ins

Agents, Sonara and the flood sources are fakes that speak the real protocol: an `OSC 777;flare-agent` hook
line (parsed by the real agent parser, `hook_line_parses_with_the_real_agent_parser`), a fake `claude.cmd`
with a temp HOME and transcripts, and an HTTP Sonara stand-in with a `runtime.json` and a bearer key. A
stand-in waits for the first agent verdict of its tab before it stands in for an agent
(`an_agent_stand_in_waits_for_the_first_verdict`). Fixtures in `.e2e-cache/` are fetched once and checked
by SHA-256 on every run; a wrong hash is refused (`the_cache_refuses_a_wrong_hash`).

Machine-dependent features are faked, not read from the owner's machine: the clipboard, and the dictation
seams (the microphone, the speech engine's folder, the whisper-server fake) which arrive with M8, when the
three dictation scenarios are ported.

## The test hook

Under `--e2e` only, the app serves one named pipe, `\\.\pipe\flare-e2e-<profile hash>`, carrying JSON lines:
one request, one reply, in lockstep (`crates/flare-e2e-proto`, protocol version 1). The requests are
`hello`, key and chord events, pointer actions, drags, file drops, resizes, DPI changes and probe reads;
the probes are named and read-only. Keys go through the same `AppCtl::on_key` the real key path ends in.

- No `--e2e`, no pipe: `start_without_the_flag_is_not_enabled_and_creates_no_pipe`.
- The pipe is for the current user only and created with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a second
  `--e2e` launch on the same profile cannot take it over (`a_second_server_on_the_same_profile_cannot_take_the_pipe`,
  `the_pipe_is_refused_to_another_user`).
- The hook is started from one place behind the flag (`the_hook_is_only_started_from_one_place_behind_the_flag`).

## Env seams

Every `FLARE_E2E_*` read goes through `e2e::global().env(..)`, which returns nothing unless `--e2e` is on, so
a stray variable in the owner's environment cannot change the real app. No source file reads such a
variable directly (`no_source_file_reads_an_e2e_variable_directly`).

## When a scenario fails

1. Read its FAIL lines and the screenshot in `.e2e-shots/` (`--shots` moves it).
2. Run it alone (`flare-e2e ... <name>`) and once more with `--keep` to read the world's diagnostic log.
3. Decide: a defect in Flare is fixed with a failing test first; a defect in the harness or the probe is fixed
   in the harness and its probe test is extended; only a real difference from Prism Terminal becomes a
   recorded row in `docs/parity.md` ("Deliberate differences") and the assertion is rewritten with the reason.
4. A scenario that fails once in two runs is a flaky test: fix the race, never retry around it.
