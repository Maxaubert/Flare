# Regression rules

Prism Terminal's rules in their native meaning for Flare. Each rule says what must hold, what it means in
Rust and Slint, and the Prism Terminal v0.55.6 source it came from (paths relative to the export;
`regression-rules.md` and `two-apps.md` are its `docs/` files). Electron-only rules are listed at the end
as dropped, with their replacement. Every rule that can be tested has a test named in the milestone that
builds it; `docs/parity.md` holds the matching checklist lines.

## What reaches a shell

1. **Input to a shell is inert until the user acts.** Strip every ESC and C0 control except TAB, LF, CR (plus DEL and C1 CSI) before any paste; quote each dropped path with a per-shell function (PowerShell single quotes with doubling, bash escape, cmd double quotes); start Windows tools by absolute System32 path. Native: pure functions `sanitize_paste` and `quote_path` in `flare-shells`, `Command::new` with absolute paths only, one unit test per quoting rule. Source: regression-rules.md inert-input (#1, #5); core/renderer/lib/termPaste.ts; core/main/sysTools.ts.
2. **The only commands written into a shell are the agent resume, dictated text, and (open decision O1) the idle-prompt cd line.** The resume is the shell's startup command, built from argv Flare owns, with the id validated and carried in `FLARE_RESUME_ID`; dictation is one bracketed paste with a trailing space, never Enter. Exception (M8, N30; rule 182): dictation from another app types into a RUNNING Claude prompt and presses Enter, through the checked send only; inside the Flare window dictation only pastes at the cursor and never presses Enter; words for a copy whose window was in front are only pasted. Exception (plugins phase 5, 2026-10-26): a plugin that holds `input`, which the person approved in Settings as full control, types into a tab it may see (its own, any with `tabs`, one a click gave it) through `tabs.input`: one paste the way the person's paste goes (bracketed when the program asked; a control character other than tab and line breaks is refused, not stripped; at most 4 KB; 2 a second per tab) and, when asked, one Enter 300 ms later; the tab, the plugin's generation and its `input` are checked again before each write. The agent never has it (rule 188). Native: no generic "send text to shell" path in the tab code; the pty write API takes typed intents (the plugin's is `TermHandle::plugin_input` with `PluginInput::Paste` or `PluginInput::Enter`, called only by `AppCtl::ctl_tab_input` and `AppCtl::plugin_enter`). Held for the plugin exception by `tabs_input_types_as_a_paste_with_its_checks_and_the_enter_comes_after`, `the_enter_is_not_pressed_once_the_plugin_is_off_and_a_closed_tab_takes_nothing` and `typing_is_refused_when_empty_too_long_or_holding_a_control_character`. Source: core/main/terminal.ts withResume; core/main/agentResume.ts validResume; core/renderer/lib/dictation.ts finishStop; core/shared/termCwd.ts cdCommand.
3. **The resume id travels in the environment, and every shell of a kind has the same command line.** Defender judges each new command line inside CreateProcess (700 ms each in Prism Terminal, 3 ms when repeated). Native: set `FLARE_RESUME_ID`, clear it in the startup line before the agent starts, keep each shell kind's command line constant, spawn on a worker, measure a cold restored Claude tab on the release build. Source: regression-rules.md same-command-line (#180); core/main/terminal.ts.
4. **A claude resume is the newest interactive session of that folder; nothing is guessed.** `--continue` once resumed the wrong conversation. Native: read each transcript's first 4096 bytes, drop a queue-operation first line or a non-cli entrypoint, no session means a plain shell; the lookup runs on a worker, 16 stats at a time. Source: core/main/agentResume.ts; two-apps.md IT RESUMES ONLY YOUR OWN CONVERSATIONS (#87).
5. **One Ctrl+V is one paste; one image key per agent.** The native paste and the app's paste both ran once. Native: the key handler consumes Ctrl+V and returns handled; the image key is a single lookup by agent kind (ESC v for Claude, ^V otherwise); a test asserts one pty write per key. Source: two-apps.md ONE CTRL+V IS ONE PASTE; core/renderer/lib/termPaste.ts imagePasteKey (#170).
6. **Shift+Enter is a newline only where an agent runs.** A backslash at a plain prompt ran a command with a stray backslash. Native: the newline key is sent only while the agent-armed state is true. Source: core/renderer/lib/termKeys.ts (#21, #175).
7. **Host chords never reach the pty, and one table decides both.** A chord missing from the "owns this key" check typed into a program while the app acted on it. Native: one keymap table in Rust used by the window key handler and the terminal input encoder; a test asserts the two agree. Source: src/renderer/src/lib/keymap.ts; two-apps.md (#198, table audit 2026-10-12).
8. **Chords and control bytes match by key name, with the physical key as fallback for letters.** Russian and Greek layouts send other letters. The key's ASCII name decides alone, so Dvorak, AZERTY and QWERTZ users get the control byte of the key they pressed (Ctrl+C is 0x03 where the key labelled C sits at physical KeyI); only a key named outside ASCII falls back to its position (general review 2026-10-23, term-1; `ctrl_letters_follow_the_layout_not_the_position`, `chords_follow_the_layout_letter`). Native (built in M5): Ctrl+C and Ctrl+V match the physical key, so Russian and Greek layouts still copy (open decision O2 answered; `ctrl_c_by_physical_key_copies`, flare-term `keys.rs`). Source: keymap.ts letter(); termKeys.ts isKey (rule 11).
9. **Typing is heard from keys, never from the terminal's own replies.** Focus reports and device attribute answers made untouched shells look typed-into. Native: mark "typed" only in the key-input path; bytes the terminal writes back never update typed, echo or activity state. Source: regression-rules.md typing-on-onkey (#99); core/renderer/lib/termActivity.ts.
10. **The app answers a program only what it asked.** OSC 52 is write-only (cap 1 MB, "c" or empty only, reads never answered); colour and version replies only on request; theme reports stop when the agent leaves. Native: every reply the grid writes back answers a query; no unsolicited bytes to the pty. Source: regression-rules.md replies-only-when-asked (#168 to #176); core/renderer/lib/termOsc52.ts.
11. **FORCE_HYPERLINK is never set.** It made supports-hyperlinks put raw OSC 8 into pipes. Native: the environment builder never sets it; a user's own value passes through. Source: core/main/terminal.ts ptyEnv (review 2026-10-11).
12. **The environment is clean: no session markers, no colour silencing, TERM is 256-colour.** Native: one pure function `pty_env` with the test list; the per-tab variable is `FLARE_TAB_ID`. Surfaces stage 2 adds three variables to the same function and the same test list: `FLARE_TAB_TOKEN` (the tab's control channel secret, see S1), `FLARE_CTL_PIPE` (this Flare's pipe) and `FLARE_MCP_EXE` (the MCP server beside the exe). A user's own value under any of the three names is replaced, and none is inherited, so a Flare started from a Flare tab never hands its shells the outer tab's secret. Held by `the_control_channel_variables_reach_the_shell_and_replace_the_users_own`, `without_a_channel_an_inherited_token_never_passes` and `no_exe_means_no_exe_variable` (`crates/flare-shells/src/env.rs`). Source: core/main/terminal.ts SESSION_MARKERS, ptyEnv.
13. **(REPLACED 2026-11-11 by rules 266 to 269: Flare guesses no file path; a file link is a path between marks.)** It said a path on screen is a link when it exists. What stays: the backend resolves every path again; the UI sends only text and folder, and the backend refuses UNC and NUL, caps length and stats it on the click's worker (`path_open`). Source: regression-rules.md path-links (#99); core/main/termPathOpen.ts.
14. **A runnable file is never run by a click.** Native: runnable extensions are revealed with `explorer.exe /select,` and never passed to ShellExecute; one test per listed extension. Source: core/main/termPathOpen.ts RUNNABLE.
15. **OSC 8 opens the shown host, http(s) only, no confirm.** Native: when label and uri hosts differ the click opens the address shown; only http and https reach the opener. Source: regression-rules.md links-painted (#169); core/renderer/lib/termOsc8.ts osc8Target.
16. **Links are painted and no pass holds the UI.** A full scrollback pass froze the window for 2 s. Native: scan and paint in slices of at most 8 ms per frame, live screen first, never 10,000 lines in one frame (budget 33 ms). Source: regression-rules.md links-painted (#167); core/renderer/lib/linkScanPlan.ts.
17. **A click places the caret only on a plain prompt; Backspace over a selection deletes only on the edited line.** Native: `caret_click_allowed` and `selection_delete_allowed` are pure functions with every refusal under test; otherwise the key goes to the shell untouched. Source: core/renderer/lib/termClickCaret.ts; termSelectionEdit.ts (two-apps.md #44).
18. **The caret follows typing, not a streaming agent.** Native: the IME and accessibility rectangle is held on the input line for 250 ms after a key while the program parks the cursor above, released on alternate screen, resize, scrollback or buffer switch. Source: regression-rules.md caret-follows-typing (#101); core/renderer/lib/termCaretHold.ts.
19. **Find is Ctrl+F except in a full-screen program; Ctrl+Shift+F always finds.** Native: the find chord is consumed unless the focused tab is on the alternate screen or reports the mouse. Built in M5; pinned by `ctrl_f_opens_the_bar_escape_closes_it_and_a_full_screen_program_keeps_ctrl_f`. Source: two-apps.md FIND IS CTRL+F (#50).
20. **The bell flashes only while unfocused, at most once a second, with no sound.** Native: a 1000 ms per-tab gate; a taskbar flash only when the window lacks focus. Source: regression-rules.md bell-flashes (#177); core/renderer/lib/termReplies.ts.

## Cells, resize, panel

21. **Cells are not characters.** Cursor and link edges were off by one after a trailing space, a wide character or an emoji. Native: all screen maths works on grid columns using the Cell flags WIDE_CHAR and WIDE_CHAR_SPACER; text indexes come from one cell-to-text function, never `len()` or char counts on screen positions. Source: regression-rules.md cells-not-characters (#20, #25); core/renderer/lib/termCells.ts.
22. **ConPTY sends nothing on a resize, so keep the prompt line.** Native: after every resize the cursor's logical line is re-laid out at the new width and the cursor put back on the same character; verify whether alacritty_terminal's reflow already does this (open decision O3) and keep the test either way. Source: regression-rules.md conpty-resize (2026-09-04); TerminalPanel.tsx fitKeepingCursorLine; e2e promptLayout. Resolved in M2: alacritty's reflow is bottom-anchored and ConPTY's is top-anchored; `resize_conpty` re-anchors; tests `o3_*` and the `resize.rs` unit tests.
23. **The panel paints the ground once.** Two translucent coats made a darker strip. Native: the grid is painted in one pass over the background, the glyph layer uses the ground as cursor-accent so a block cursor is never a hole; measure the strip under the last row. Source: regression-rules.md panel-paints-ground (#6).
24. **While the room animates the terminal keeps its columns and fits once at the end.** Native: hold the size during a sidebar slide, fit after it. Source: core/renderer/lib/fitHold.ts; two-apps.md.
25. **Tab switch keeps each grid alive.** Native: each tab keeps its grid and scrollback while hidden, only the visible grid is drawn, hiding never kills a shell, output keeps being parsed. Source: TerminalPanel.tsx detach comment.
26. **Ctrl+scroll zoom is never saved.** Native: 7 to 64 px, wheel handler non-passive, never sent to the program. It was per session (PT's zoomSession); since rule 1154 it is one zoom every tab shares. Source: TerminalPanel.tsx zoomSession.

## Processes, lifetime, quit

27. **Closing the window quits; closing the last tab does not.** Native: the window's close-request handler flushes tabs.json and window.json synchronously, kills every shell, waits, exits; no tray; the last tab removal shows the start screen. Source: regression-rules.md close-quits; src/main/index.ts; e2e lastTab.
28. **The quit waits for every shell to be gone (3 s cap), on the child's exit code.** A native callback landing during teardown aborted the process (0xc0000409). Native: every kill goes through one function that waits for exit with reader threads joined and a 3 s timeout; no detached thread outlives main (one recorded exception: the control channel's accept thread, a timed-out call and a handler still running at the end of its 2 s stop wait may outlive the stop; they touch only `post`, which fails once the loop has ended, the stop flag and their own pipe, and a call arriving after the stop is never posted, rule 186); a test kills ten shells and asserts none remain. Since Surfaces stage 3 the web views are closed in the same place, before the window hides (`WebCtl::close_all`, held by `quitting_closes_every_web_view_before_the_window_goes` and the e2e `webPanel`, which watches the browser's own process end with the window). Source: regression-rules.md quit-waits-for-shells (#127); core/main/terminal.ts shellsGone.
29. **Bundled ConPTY.** The inbox conhost fast-fails the process when a pty is killed mid-read. Native: ship `conpty.dll` and `OpenConsole.exe` beside the exe, load the bundled DLL first, a startup check fails loudly if missing. Source: regression-rules.md bundled-conpty; core/main/terminal.ts PTY_OPTS.
30. **Warm shells never leak, and a spawn killed while pending starts nothing.** Native: a warm entry is removed only by identity (compare the handle), every eviction kills its process; a spawn state holds a cancel flag set by kill. Source: core/main/terminal.sessions.test.ts (#2, #12).
31. **Output is coalesced every 8 ms and flushed before the exit event.** Native: batch per tab, flush before the exit message so the last words are painted. Source: core/main/terminal.ts OutputBatcher.
32. **A single instance hands its folders to the running window; every argv path is taken.** Native: a named mutex or pipe; the second process sends its absolute folder list and working directory then exits 0; the first raises its window (show, brief topmost, focus, restore). Source: src/main/index.ts; src/main/argv.ts.
33. **Relative command-line paths resolve against the folder they were typed in and are stored absolute.** Native: resolve each argument against the sender's working directory before the hand-off. Source: src/main/argv.ts; regression-rules.md.
34. **Main never waits on the disk or a process.** A stalled disk froze the page for seconds. Native: probes, stats, spawns, clipboard reads run on worker threads with timeouts; the UI thread only sends and receives messages; any blocking call is marked `sync-ok` with a reason and checked by a test; the quit flush and the startup read are the allowed exceptions. Source: CLAUDE.md "Main never waits on the disk"; src/main/syncCalls.test.ts.
35. **The process poll asks only after output, backs off 2.5 to 20 s, asks at most once per 15 s per tab from a title or hook, and reports changes only.** Native: the query runs on a worker; a native process snapshot may replace PowerShell but keeps these waits and the change-only send. Source: regression-rules.md process-poll (#73); core/main/agentPoll.ts.

## State and storage

36. **Save state atomically and flush synchronously on close.** tabs.json, window.json and settings are the restore; a lost last write loses an agent session. Native: write a temp file in `%APPDATA%\Flare`, rename over the target, debounce 400 ms, flush synchronously on close, session-end and quit. Source: regression-rules.md (tabs.json and window.json written atomically); core/main/atomicWrite.ts; src/main/tabsStore.ts.
37. **Tab reports are ignored until the first restore has answered.** An early empty list would overwrite tabs.json. Native: a `restored` flag in the tab model; the save path returns early until set. Source: src/main/index.ts awaitingRestore.
38. **A resumed conversation is not new work.** Native: during restore a tab accepts idle titles only until its own turn starts; no Finished, Question or Failed from a resume. Source: regression-rules.md #151; two-apps.md launch skeleton.
39. **The clear before the agent paints goes to the grid only, never the shell.** Native: the grid is cleared in renderer state; no bytes to the pty. Source: core/renderer/lib/resumeReveal.ts.
40. **Every stored value keeps its meaning; unknowns read as the default and are never written back.** Native: typed enums with `from_str` falling back to the default, serde defaults on each field, never re-serialise an unknown value; a bool default-on reads on unless '0'; never set is not zero (`Option<T>`, default before clamp). Source: core/renderer/lib/prefStore.ts; helpPrefs.test.
41. **Stores fail soft.** Native: load and save return `Result`; on failure keep defaults in memory and log to `flare-diag`; a crash never leaves a half-written file. Source: prefStore.test "a blocked store loses the choice".
42. **Settings keys keep their names and meanings across the import; legacy keys are read once and removed.** Native: a versioned schema with an import map from the PT key to a Flare field; the opacity key is read only by the importer. Source: core/renderer/lib/prefStore.ts; seeThroughMigration.ts; opacityMigration.ts.
43. **The window position is kept only if it overlaps an attached display.** Native: test the saved rectangle against each monitor's work area (`EnumDisplayMonitors`) before applying. Source: src/main/windowState.ts.
44. **Tab state is per session or per store, never shared across windows.** Native: the used list, walk, placeholders, recents and pins live in the window model or settings file; persist only tabs.json, window.json, recents, pins and stored choices. Source: tabs area notes.
45. **Every tab is one shell and one folder; Settings is a tab with no shell.** Native: a tab enum (Shell or Settings), only Shell owns a pty. Source: src/renderer/src/lib/tabs.ts.
46. **Tabs are not imported from Prism Terminal.** Native: Flare's tabs.json starts empty on a fresh `%APPDATA%\Flare`; no read of PT's profile for tabs. Source: design spec Migration.
47. **Settings page memory is in memory only.** Native: a field on the window state, not in the settings file. Source: src/renderer/src/components/settings/Settings.tsx.

## Tabs, close, keys, UI

48. **The close question is one rule, not a setting.** A plain shell closes unasked; a tab hosting an agent asks; the window close is held only while an agent is working. Native: pure `asks_before_closing_tab` and `holds_window_close` shared by the close and install paths, and a close-flow state object (`agent_busy`, `quit_wanted`, `close_agreed`) owned by the window, not loose booleans. Source: regression-rules.md close-question; core/renderer/lib/agentClose.ts; src/main/closeFlow.ts. M3 (amended): the pure rule lives in `flare-agent::close` (`a_plain_shell_closes_unasked_an_agent_asks`, `the_window_is_held_only_while_an_agent_works`) and the close-flow state in `flare-tabs::close_flow` (`holds_while_one_works_and_the_go_ahead_closes`, `an_install_answered_in_advance_goes_through_once`).
49. **One question at a time: tab chords swallow the key except close-tab.** An acting chord sent the Enter meant for the question into a shell. Native: a modal flag in the key handler; swallowed keys are consumed, not passed to the terminal. Source: two-apps.md ONE QUESTION AT A TIME; keymap.ts underModal.
50. **Keep agent sessions open.** Native: Ctrl+D and Ctrl+C are consumed in the front tab when an agent runs there; Ctrl+C passes while the agent works or a selection exists; never in a plain shell. Source: keymap.ts agent-guard (owner, 2026-10-16).
51. **Ctrl+Tab "Most recent" uses a snapshot and reorders only when the Ctrl hold ends.** Reordering each press made the second Tab flip back. Native: a `Walk` value (snapshot, index) created at the first press, committed on Ctrl release, focus loss or another Ctrl chord. Source: src/renderer/src/lib/tabMru.ts.
52. **Tab width is fixed; labels are the folder name, disambiguated by the parent only when two different folders share it.** Native: one width and floor constant in the Slint tab component, equal shrink. Source: two-apps.md EVERY TAB IS ONE WIDTH.
53. **Drag a tab only past a 4 px slop; a click under it is a pick; the drop slot uses the leading edge.** Native: a pointer-capture state machine in the tab component with lanes captured at drag start. Source: components/useTabReorder.ts; lib/tabDrop.ts.
54. **The strip is a window drag handle only while no drag is in flight.** Native: the empty strip is a drag area unless an external drag hovers it, reset on move, down, drop or end. Source: TabStrip.tsx dragInFlight.
55. **The title bar has a settings cog and no menu; window chrome keeps the DWM frame and suppresses the system menu.** A frameless window cannot composite acrylic. Native: keep the native frame (DWM, extended into the client area), handle `WM_NCRBUTTONUP` and `WM_INITMENU` so the system menu shows only through Alt+Space. Source: regression-rules.md settings-cog-no-menu, title-bar-style-hidden.
56. **Background throttling is off; agent clocks run while minimised.** Native: no throttled timer or frame pause for agent clocks; only mark animation pauses while minimised. Source: src/main/index.ts webPreferences.

## Agent indicator

57. **The indicator is the agent's own word.** Title first, Claude's hook signal above it, output scoring only for sessions with no title and no hook. Native: rules in `flare-agent` with no UI types so the timelines test runs without a window. Source: regression-rules.md indicator-agent-word, claude-hooks.
58. **Claude's hooks run in exec form with a static echo.** Native: `cmd.exe /d /c call <root>\hook.cmd <args>`, never a shell string, no stdin read, timeout 5 s; a test reads hooks.json and checks event, matcher and args against the table. Source: core/renderer/lib/agentHookSignal.ts; core/claude-plugin.
59. **A hooked session is never scored from output; the screen is read only to hold a question the hooks raised.** Native: the screen read is a function of the grid called only from the hook step path. Source: regression-rules.md claude-hooks.
60. **While the question box is on screen the question stands (#148); a spinner after a question is the approved work but never from the answer key; a Stop with the spinner up waits for the next title (#149, STOP_SETTLE_MS 1500).** Native: a deadline on the tab's own timer, cancelled by any state change; pin with the measured timelines. Source: agentHookState.ts; agentHookState.timelines.test.
61. **Failed and Finished are news; Question is state.** Native: done and failed clear on looking; question clears on answer, work again, agent gone or tab close; a look is focus plus the active tab. EXCEPT a done or failed mark raised on the tab in front while the window was unfocused: it is HELD, and neither focus returning nor the pointer moving takes it down (the 2026-10-14 pointer look is gone); only the front tab changing to a DIFFERENT tab (a click, a key, an overflow pick; clicking the tab already in front does nothing, and closing another tab keeps the mark), work again, agent gone or tab close does, and the badge counts it until then (owner, 2026-10-21, narrowed 2026-10-23; PT core 0.44.4, narrowed in PT #252). Source: regression-rules.md finished-and-question (#144).
62. **Every working tab has its own mark; the taskbar counts marked tabs, as an overlay only.** Native: one mark per tab from its own state; the only window-level mark is the taskbar overlay (ITaskbarList3), never a window icon. Source: regression-rules.md finished-and-question.
63. **Ring or Bar only, Ring by default, theme-coloured.** Native: stored off or full reads Ring; colours follow the theme until picked. Source: regression-rules.md indicator-minimal.
64. **A folder path is not the program.** Native: match the agent by exact basename or first argv token, never by a path segment alone. Source: two-apps.md; resumeReveal.ts namesAgent; agentDetect.ts.
65. **After a theme push, a resize nudge, never a key.** Native: a one-column resize and back; a test that the pty sees cols minus 1 then cols. Source: regression-rules.md redraw-nudge.
66. **The Sonara link is local only and quiet on failure.** Native: 127.0.0.1 only, runtime.json read, 1500 ms timeout, never throws into the UI, nothing sent without Sonara. Source: core/main/sonaraLink.ts.

## Theme, chrome, settings

67. **Contrast is measured, never named; floors are fixed numbers.** Light or dark comes from measured luminance (above 0.4); text 4.5:1, marks 3:1, sidebar names 7:1, name flip 2:1. Native: pure functions in `flare-theme` with the same constants, tests over every preset. Source: core/shared/ground.ts; chromeTheme.ts; termAnsi.ts. M4 tests: `every_presets_text_reads_on_its_ground`, `all_sixteen_of_every_preset_clear_the_floor_against_its_background`, `ground_mode_is_measured_never_named`, `no_source_compares_a_luminance_with_a_typed_0_4`, `light_is_measured_above_luminance_0_4`.
68. **Alpha never reduces legibility, and alpha 1 changes nothing.** Native: composite once when the theme resolves, hand the renderer opaque or 8-digit hex only; golden snapshot tests byte for byte. Source: termTheme.legible.test; termTheme.snapshot.test. M4 tests: `a_custom_at_every_alpha_reads`, `legible_on_at_alpha_1_returns_the_pick_unchanged`, `legible_on_is_never_less_legible_than_the_opaque_pick`, `oracle_alpha_sweep`, `oracle_opaque_custom_with_a_failing_cursor`.
69. **A theme id is a saved setting; retired ids map to their replacement at read time.** Native: the retired map is data in `flare-theme`, used wherever the id is read and by the import. Source: core/renderer/lib/termThemeRetired.ts. M4 tests: `each_retired_goes_to_a_listed_theme_and_none_is_listed`, `retired_map_equals_the_oracle`, `a_retired_theme_reads_as_its_replacement`, `a_retired_id_is_worn_as_its_replacement`.
70. **The theme drives the chrome, and a switch asks first.** A pick with Save changes lit shows Save as Custom, Discard, Cancel (Escape and outside press cancel); a clean pick lands at once and forgets picked Background, Secondary and Accent unless Custom. Native: a Slint popup with focus trapped; one function in `flare-settings` clears the window colours. Source: regression-rules.md theme-switch-colours.
71. **Save as Custom saves the whole setup; font, font size, indicator and edges belong to no theme.** Native: theme state and settings state are separate structs; only the theme struct carries palette and see-through level. Source: termLook.ts resetTermExtras.
72. **The editor's draft is worn but not saved.** Native: a draft slot in the settings model read each frame; Cancel drops it; the editor view re-mounts with the draft. Source: termLook.ts themeDraft.
73. **Material before colour, and acrylic needs a DWM-composited window on Windows 11.** Native: set the backdrop type first, then the ground colour (clear for glass); read both back in a test; build-number check once at start gates the slider; goes opaque before full screen. Source: regression-rules.md material-before-colour; src/main/material.ts. M4 tests: `going_glass_sets_the_frost_then_the_backdrop`, `going_glass_calls_the_frost_then_the_backdrop_and_the_ui_waits`, `order_only_material_first_when_glass_turns_on`, `leaving_glass_removes_the_backdrop`, `full_screen_goes_opaque_before_the_os_is_asked_and_back_on_settle`, `acrylic_ok_is_windows_11_and_later_by_build`.
74. **Material and DWM calls are cosmetic.** Native: a failed call is ignored and leaves the solid window; `Result` logged to `flare-diag`, never a panic. Source: dwmHelper.ts. M4 tests: `a_failed_backdrop_call_leaves_the_window_solid`, `the_dwm_wrappers_do_not_panic_on_a_bad_handle`, `an_unsupported_build_makes_no_dwm_call`.
75. **The DWM border is one pixel and only its contrast changes.** Native: `DwmSetWindowAttribute` `DWMWA_BORDER_COLOR` (34) as COLORREF 0x00BBGGRR, `DWMWA_COLOR_NONE` (-2) for none, from one debounced task (250 ms) after WM_SIZE, WM_SETTINGCHANGE or a theme change; removed when maximised or full screen. Source: regression-rules.md window-edge-hairline; windowEdge.ts. M4 tests: `apply_is_debounced_250_ms`, `a_new_apply_restarts_the_wait`, `maximised_or_full_screen_has_no_border`, `no_edges_is_no_border_whatever_the_ground`, `oracle_edge_vectors`, `colorref_has_red_in_the_low_byte`.
76. **Edges are a setting with four words and the default is the old look.** Native: closed enum with serde default Hairline; unknown maps to Hairline. Source: src/shared/windowEdges.ts.
77. **Settings ask nothing nobody changed.** Native: commit a colour or code field only on editing-finished with parseable text that differs from the stored value; focus traversal and Escape never write. Source: regression-rules.md settings-ask-nothing (#71); fields.test.
78. **Settings controls are neutral; only Save changes and ON switches wear the accent.** Native: Slint style tokens with neutral control brushes and one accent token used by two components; a test greps the style file for the accent token outside those. Source: regression-rules.md SETTINGS CONTROLS ARE NEUTRAL; neutralControls.test.
79. **Settings copy is plain words.** Native: a unit test over the row tables and Slint string tables: letters, digits, spaces, commas and full stops, no key names, subtext at most eight words (see open decision O8). Source: core/shared/settingsCopy.ts.
80. **Search reaches every row it indexes, and Off means off.** Native: the index is built from the same row table the pages draw from; a test types each label and checks the result opens its page; command help off removes the component, not only greys it. Source: settingsSearch e2e; coreIndex.ts.
81. **Layout is measured, not just present.** All Prism Terminal e2e checks once passed over a ruined page (#20). Native: Slint layout tests or screenshot diffs with measured geometry, and side-by-side screenshots at each UI milestone. Source: regression-rules.md page-looks-right.
82. **Command help never inserts, runs or types, and copies the text on screen.** Native: the view takes a `Fn(String)` for copy and nothing else; a test asserts a pty write count of zero across search, copy and Enter; a key-shaped entry has no copy action; help copies do not raise the Copied badge. Source: HelpPanel.tsx; PT #12 and the 2026-09-19 owner rule.
83. **Escape is handled once.** Native: the focus handler returns handled only when the find field has text or a colour popover reverts; the host closes Settings only on an unhandled Escape. Source: SettingsFrame.tsx.
84. **The Explorer verb state is read back from Windows.** Native: the registry is the source for the row; `flare-win` returns the state; disabled with "Checking with Windows." while busy. Source: appOptions.ts store "registry".
85. **The diagnostics verbose flag belongs to the backend.** Native: `flare-diag` owns the flag; the page calls its setter and shows the returned value. Source: core/renderer/settings/sections/DiagnosticsPage.tsx.

## Dictation

86. **Only the final text is pasted; partials are shown only; never Enter.** Native: the pill reads partials from the engine channel; only the final result reaches the pty write path, through the paste sanitiser with bracketed markers; a test asserts no CR, LF or ESC reaches the pty. Source: core/renderer/lib/dictation.ts; dictationClean.ts.
87. **Off means off.** Native: no key handler, process or download while off; switching off or quitting kills whisper-server, parakeet-cli and the media helper, via a Job Object with kill-on-close so a crashed Flare orphans nothing. Source: dictation.ts armDictation; dictationEngine.ts.
88. **The hotkey is a solo hold for bare modifiers so AltGr typing never records, and its Alt keyup is always swallowed.** Native: read physical key codes (winit `KeyCode` or Win32 scan codes), ControlLeft within 50 ms of AltRight is forgiven, the Alt key-up and key-down for the dictation key are consumed so no lone Alt reaches DefWindowProc; verify on a Norwegian layout. Source: core/renderer/lib/dictationKey.ts.
89. **A dictation key is never a key that types or a chord the terminal owns.** Native: `usable_hotkey` is checked on capture and on read from settings. Source: dictationKey.ts usableHotkey (#27).
90. **A recording is bound to the session in front when it started.** Native: capture the tab id at start; if it is gone say "That tab has closed" and paste nothing. Source: dictation.ts header.
91. **Audio never touches disk and never leaves the machine.** Native: the WAV is built in memory and goes to the engine's stdin or a loopback socket on 127.0.0.1; no temp file by Flare. Source: dictation design spec "Audio never leaves the machine".
92. **Downloads are pinned by commit or tag, byte size and SHA-256; an unverified file is deleted, never loaded or run.** Native: URLs are data from the catalogue; stream to `.part`, hash while streaming, rename only on a full match; a test that every URL carries a 40-hex commit or the b5130 tag. Source: core/main/dictationStore.ts; dictationCatalog.test.
93. **The GPU pack is optional and offered only with an NVIDIA adapter; a failed GPU engine falls back to CPU once per session, saying so.** Native: vendor 0x10DE from DXGI adapter enumeration, answered once; never spawn nvidia-smi. Source: dictationEngine.ts gpuFellBack.
94. **The engine is warm before the first press, and a partial never queues behind a cold start.** Native: warm at arm, at each press and on model or language change; a cold partial answers `warming` at once; the CUDA cache is capped at 4 GiB. Source: dictationEngine.ts.
95. **The engine stands down after 5 minutes idle, except while dictation is on in the app (amended at `e5848cb`, M8 D11, N5).** The app passes no idle limit, so while dictation is on the engine stays up until `stop()` (switching dictation off, quitting, or the model's engine changing). The 5-minute stand-down is the engine's default for a host that sets none. Native: a timer on the engine actor that does not keep the process alive; `Option<Duration>` idle, where `None` never stands down (`engine/mod.rs`). Held by `never_stands_down_with_no_idle` and `kills_the_server_after_idle_with_no_request_and_starts_a_fresh_one_on_the_next` (`crates/flare-dictation/tests/engine.rs`). Source: dictationEngine.ts DICTATION_IDLE_MS; src/main/index.ts idleMs (Prism Terminal passes `Infinity`).
96. **The microphone is the only permission, and opening Settings never opens it.** Native: capture starts only on a press or the Test button; a refused microphone shows "Microphone access was refused". Source: DictationPage.tsx header.
97. **Pause media uses the Windows media session API, never the play/pause key, and resumes only what it paused.** Native: GSMTC through the `windows` crate, 3 s per call, never throws. Source: core/main/mediaPause.ts.
98. **A limited-language model does not change the stored language; a recording is capped at 5 minutes and a clip under 0.35 s is dropped; the text cleaner removes only what is certainly not speech.** Native: picker shows Auto-detect disabled without writing; buffer bounded at 300 s of 16 kHz f32; lists ported verbatim. Source: dictationCatalog.ts languageFor; dictation.ts; dictationClean.ts.
99. **Engine binaries are official whisper.cpp builds, pinned, with the C++ runtime Authenticode-checked and the MIT notice shipped.** Native: the fetch script copies the four runtime DLLs only when the signature is Valid from Microsoft Corporation and fails loudly otherwise; notices go into THIRD-PARTY-NOTICES.md. Source: tools/fetch-whisper.mjs.
100. **The shared model folder survives an uninstall.** Native: the Velopack uninstaller does not delete the models folder. Source: dictation design spec.

## Updates, installer, Windows integration, diagnostics

101. **The updater installs only what it offered.** A page that could name any release asset could downgrade the app. Native: the update code takes the package from its own offer state, never from a UI message; the UI can only ask to install the current offer, named by an opaque token and never a URL (M9 D2); the feed refuses any version it did not find; with Velopack this is "only the package Velopack offered". Source: src/main/updateGate.ts; regression-rules.md MAIN INSTALLS ONLY WHAT MAIN OFFERED.
102. **The updater never runs in dev, test or an unpackaged build, and stays off until the repo is public.** Native: an `is_packaged` gate on the check thread and on verb writes. Source: src/main/update.ts; design spec.
103. **The close question is pre-answered at the quit; cancel stops the download only; a failed or cancelled download leaves nothing; the app does not quit if the handoff cannot start.** Native: the "may we quit" check runs when the installer is about to launch; a cancellation token in the download loop; delete the temp package on every failure path; quit only after a successful spawn. Source: update.ts installUpdate; updateGate.ts.
104. **Release notes are plain text, never HTML, markdown or a link; the chip is accent-filled with the selection token and its width never changes; the window stays up for the install.** Native: a Slint Text fed a plain `String`; contrast test on fill and label; fixed chip width; popup close-on-outside and Escape disabled while not idle. Source: core/renderer/components/UpdateDialog.tsx, UpdateChip.tsx.
105. **The installer ends the running app before touching files.** Native: the Velopack hook ends the process before replacement; tab state is already saved (within 0.5 s). Source: build/installer.nsh customInit.
106. **Explorer verbs are per user, via argument vectors, on folders and the folder background, never on files, never from a dev or test build.** A dev build once pointed the installed app's verb at a throwaway binary. Native: registry writes gated by `packaged && !test_flag` on every write including the Settings toggle, plus a writes counter the test asserts is zero. Source: src/main/shellVerb.ts; verbSwitch.ts.
107. **The one stored fact for the verb is the NO; the label is read by its REG_SZ type; the uninstaller removes every key.** Native: an off-marker file in the profile; parse `reg query` output for the REG_SZ column, not "(Default)"; one Rust constant of keys checked by a test that reads the installer script. Source: verbSwitch.ts; shellVerb.ts readLabel; shellVerbParity.test.
108. **The verb label (open decision O4).** Prism Terminal's label does not name the app ("Open terminal here"); the Flare brief says "Open in Flare". Native: the label is one constant in `flare-win`, chosen before the verb is written; relabel an installed verb at launch only when its key points at this exe and the label is readable REG_SZ text. Source: shellVerb.ts VERB_LABEL (#27).
109. **Both apps install side by side with Flare's own names.** Native: different app id, verb key, single-instance lock, profile, plugin name, hook signal and tab-id variable; `tools/e2e/tests/side_by_side.rs` proves no clash (Layer A on every run, Layer B on a clean CI runner). Source: design spec; roadmap. Retired 2026-11-11: the owner removed the side-by-side check. (Flare's own names stay held by `every_flare_name_differs_from_prism_terminals`, `crates/flare/tests/names.rs`.)
110. **The diagnostics log holds no typed text, clipboard, audio, screen text, title text or conversation.** Native: the writer accepts typed kinds and fields; opaque channels are summarised by size (`string:n`, `bytes:n`); a test that a marker string never reaches the file. A web page is kept as its site only (rule 247): never its address, path, query, title, cookies or form data, and `url`, `uri`, `query`, `cookie` and `path` are text keys the scan in `crates/flare-diag/tests/neutral.rs` refuses on any diagnostics line. Source: core/shared/diagSummary.ts; agentDiag.ts header; PRIVACY.md.
111. **Diagnostics writes are asynchronous; the quit flush is the one allowed synchronous write; a logger never throws.** Native: a background thread owns the file, the UI thread enqueues, errors are counted and the first becomes one `logger-error`. Source: core/main/diagLog.ts.
112. **Rotation at 2 MB keeping four files; errors are gated once per 10 s with a repeats count and 10 lines per kind per 10 s; observed-only panics.** Native: rename with retry, never drop lines because a rename failed; same gate for panics, render errors and frame spikes; a panic hook logs then lets the default behaviour run. Source: diagLog.ts; diagGate.ts; crashHooks.ts.
113. **The privacy statement lists exactly the network requests the app makes.** Native: a review item or CI check that compares Rust network calls with PRIVACY.md; a new request changes PRIVACY.md in the same PR. Source: PRIVACY.md.
114. **One .ico feeds the app, installer, uninstaller and taskbar; frames are rendered from the full-size artwork.** Native: embed the frames listed in tools/make-icon.py; the window icon is the exe's own. Source: tools/make-icon.py.

## The e2e

115. **Offscreen, never hidden, never focused.** Native: move the Slint window to (-4000,-4000) at alpha 0, create with `WS_EX_NOACTIVATE`, show with `SW_SHOWNOACTIVATE`; never hide. Source: tools/e2e/run.mjs 9-11, 38-43; CLAUDE.md THE E2E STAYS HEADLESS.
116. **Every scenario reaps what it leaves, matching only its own profile path.** One leaked process holds the single-instance lock and fails every later scenario. Native: enumerate processes whose command line contains `flare-e2e-profile`, terminate each tree (a job object with kill-on-close is stronger), release the mutex before the next scenario; never the owner's own Flare. Source: run.mjs 52-80.
117. **A throwaway profile per scenario, removed after; each scenario has its own time limit and fails alone.** Native: `%TEMP%\flare-e2e-profile-*` removed with retries, pointed at by the `--user-data-dir` equivalent, never the real `%APPDATA%\Flare`; a deadline on a worker thread; on timeout FAIL, reap, continue, silence late assertions; `closeApp` races 15 s then kills. Source: run.mjs 83-100, 135-148, 8214-8233.
118. **The owner's clipboard is not touched.** Native: the sequence number is compared before and after the run; under the test flag the app clipboard is an in-memory stand-in behind a trait; PSReadLine paste chords are muted before any scenario presses Ctrl+V. Source: run.mjs 215-229, 8189-8262.
119. **The e2e changes behaviour in exactly these ways:** in-memory clipboard, unfocusable window, test hooks registered, no Explorer verb write, no DWM edge helper, update watcher off, `--preview-update` as the only way to see a fake update, opened paths recorded instead of opened. Test hooks never reach a release build; a unit test asserts each registration sits behind the flag. Source: src/main/e2eOnly.test.ts; run.mjs.
120. **Stand-in agents speak the real protocol, owner data is never read, fixtures are verified every run, optional machine features are faked, a flaky check is a bug in the gate.** Native: stand-ins print `OSC 777;flare-agent`, a fake `claude.cmd` on PATH with a temp HOME, fixtures in `.e2e-cache/` checked with SHA-256 from Flare's own catalogue; no retry loops. Source: run.mjs 265-298, 3275-3310; CLAUDE.md.
121. **The e2e is the PR gate and nothing else; an install runs none.** Native: a separate `cargo test` target for the e2e; the install step runs unit tests only. Source: CLAUDE.md THE E2E IS THE PR GATE.

## Rendering (M1)

130. **Slint's wgpu device limits are raised to `Limits::default()`.** Slint's defaults are the WebGL2 set (2048 px textures), so a 4K window at 225% would fail. Native: `device_required_limits` is set in `crates/flare/src/backend.rs` before the renderer starts; pinned by `backend::tests::gpu_limits_fit_a_4k_window_at_225_percent`. Source: roadmap M1 Review Focus.
131. **The grid texture is the Image's physical size, `round(width / 1phx)`.** Anything else blurs or stretches a stale frame at 225% and when maximised. Native: `ui/app.slint` exports `grid-phys-width`/`grid-phys-height` as `round(grid-area / 1phx)` and the host paints exactly that size (`headless.rs` sizes its window to `round(size * scale)`); pinned by `host::tests::cpu_render_returns_image_of_size`, `scale_change_marks_dirty` and the 400x300 headless screenshot tests. Source: roadmap M1 Review Focus.
132. **No redraw without a dirty grid.** A resize or a scene change marks the host dirty; the software path's 16 ms timer repaints only when dirty, and the GPU path repaints from Slint's rendering notifier. Native: `host.rs` `is_dirty` and `repaint_due` in `window.rs`; pinned by the host tests (`render_clears_dirty`, the resize tests). Source: roadmap M1 Review Focus.

## Dropped Electron-only rules

122. **Dropped, with the native replacement:** renderer sandbox flags and the web-view CSP (no web view; no remote content loads), `titleBarStyle: hidden` (rule 55), the Electron `asarUnpack` and node-pty pin rules (portable-pty and the bundled conpty.dll pinned in Cargo.lock, plus a test for the bundled DLL), the DWM PowerShell helper (in-process DWM calls), the document-policy header for page stacks (native UI-thread watchdog, open decision O16), the IPC channel tables (typed Rust channels), and `process.resourcesPath` layout (a plain resources folder beside the exe). Each is recorded here so nobody re-adds the Electron form. Source: regression-rules.md renderer-sandboxed, title-bar-style-hidden, material-before-colour; electron-builder.yml.
123. **Flare names only.** `%APPDATA%\Flare`, `FLARE_TAB_ID`, `FLARE_RESUME_ID`, the `flare-status` plugin, `OSC 777;flare-agent`, `FLARE_E2E_*` and `FLARE_` dictation seams, never a PRISM or PrismTerminal name in any path, key, text or header. Exception: the imported `prism.*` setting key names if open decision O10 keeps them. A grep check in CI over code and UI strings (not docs, which name Prism Terminal as the source) lands with the first UI strings in M1. Source: design spec; roadmap.
124. **No em-dashes in any Flare text** (owner rule). Native: the `No em-dashes` step in `.github/workflows/ci.yml` greps every tracked file except `Cargo.lock`. Source: global rules.

## Defaults, drop and migration (added in the M0 review)

125. **New tab is Ctrl+T and opens in the user's folder; Ctrl+W closes a tab.** An empty folder means the user's own, resolved by the backend; there is no ask mode since 2026-10-27 (a stored "ask" opens in the folder; the folder menu's "Choose a folder…" row asks). Ctrl+T and Ctrl+W are taken from the shell knowingly (Ctrl+Shift+W still closes; Ctrl+Backspace covers delete-word). Native: the keymap table owns both chords; `new_tab_folder` reads "" as home. Source: regression-rules.md new-tab-ctrl-t; src/renderer/src/lib/newTabPrefs.ts; keymap.ts. M3 (amended): the label reads Ctrl+W, held by `ctrl_w_and_ctrl_shift_w_both_close_ctrl_t_and_ctrl_shift_t_both_open` (`flare-tabs` keymap).
126. **The accent and the background are settings; unset is the theme's.** Each row shows the theme's colour until a pick, then a plain Reset word forgets it. Applied once where the chrome is painted: a picked background replaces the theme's before the chrome tokens are measured, so mode, inks and the accent floor follow, and the terminal ground follows with text at 4.5:1 and the cursor and the sixteen colours at 3:1. A picked accent is kept where the ground can show it and moved only to the 3:1 floor where it cannot; an unpicked working indicator wears it. Native: `Option<Colour>` fields in settings, resolved in one `flare-theme` function feeding both chrome and grid. Source: regression-rules.md accent-and-background; backgroundPrefs.ts; accentPrefs.ts; colourPref.ts. M4 tests: `unset_colours_follow_the_theme_exactly`, `follows_the_theme_when_nothing_is_chosen`, `a_picked_background_replaces_the_themes_before_the_chrome_is_measured`, `keeps_the_picked_colour_where_the_ground_can_show_it`, `a_chosen_accent_the_ground_cannot_show_is_moved_to_the_floor_not_swapped`, `background_follows_the_theme_and_forgets_on_none`, `accent_follows_the_theme_until_chosen`, `the_working_colour_follows_the_theme_until_picked_and_can_be_given_back`.
127. **PT Default is the default theme and Custom comes before it in the wall.** A fresh profile wears PT Default; the wall is Custom (when saved), PT Default, then the rest. Native: the default theme id is one constant in `flare-theme`; the wall order is a tested pure function. Source: regression-rules.md pt-default-theme; src/renderer/src/termHost.ts defaults. M4 tests: `the_wall_is_custom_then_the_default_then_the_rest`, `holds_the_whole_order`, `pt_default_leads_the_list_and_prism_is_the_core_fallback`, `a_fresh_store_wears_pt_default_at_100_percent`.
128. **A dropped file types its quoted path, never Enter, only over a shell; the terminal answers a right-click.** The menu is Paste, Find in scrollback and Command help, led by Copy link over a link and Copy over marked text, with no Close tab. Nothing in a feature list is true until a test has done it: the e2e performs a real OS drop (`WM_DROPFILES` or an OLE drop carrying a real path), not a synthetic event. Native: the drop goes through `quote_path` and the paste-safe write path. Source: regression-rules.md file-drop-and-menu (#16); two-apps.md menu-fits.
129. **The legacy opacity is read defensively; never-set reads as opaque.** A missing or non-numeric value is opaque, never 0. Native: the import parses with a fallback of 1.0, never `unwrap_or(0.0)`. Source: regression-rules.md legacy-opacity; legacyTermOpacity.

133. **A cd line never carries a control character, and a UNC report never replaces the local folder.** `parse_osc9` and `cd_command` refuse any char below 0x20, 0x7F and 0x80-0x9F, cmd also refuses `!` and `^`, the emulator checks before emitting `TermEvent::Cwd`, and a UNC folder that arrived only through an OSC 9 report is never used to spawn, restore or stat: `folder_after_report` keeps the last local folder, `is_unc` also counts `\\?\UNC\`, `\\.\UNC\` and every other non-drive device form, and `resolve_term_path` never resolves a relative path against a folder that is not a drive path. M3 restore must go through `folder_after_report`. Native: `flare-shells` `cwd.rs` tests (`parse_osc9_refuses_control_characters`, `cd_command_refuses_control_characters_in_every_shell`, `cd_command_for_cmd_refuses_bang_and_caret`, `a_unc_report_never_replaces_the_local_folder`) and the emulator test `osc_9_9_with_a_control_character_never_emits_a_folder`. Source: security review of cwd.rs, 2026-10-17.

134. **A click opens only an allowlisted type; everything else is revealed.** `open_term_path` opens a file only when its extension is a known document, text, data, image, audio, video or non-running source type; unknown, runnable, script, shortcut and disk-image types are revealed in Explorer. A segment ending in `.` or space, or a `:` after the drive (an alternate data stream), is refused, and the extension is re-checked on the canonicalised path so a link cannot hide it. Native: `flare-term` `path_open.rs` `may_open`, pinned by `may_open_allows_documents_media_and_safe_code_only`, `trailing_dot_space_and_streams_are_refused`, `a_trailing_dot_exe_is_revealed_never_opened`, `an_unknown_type_is_revealed_never_opened` and `the_extension_comes_from_the_canonical_path`. Source: flare-followups M2 path_open review; docs/parity.md Decisions (O22).

135. **A pasted or dropped path is typed inert, or not at all.** Unknown and WSL shells get single quotes with `'` written as `'\''`; cmd refuses a path holding `%`, `!` or `"` (nothing is typed); every shell refuses a path with a control character (C0, DEL, C1), and every Text payload passes the paste control filter, so `ESC[201~` in a file name never reaches the pty. Native: `quote_path`/`quote_paths` return `Option<String>`, `decide_paste` answers `None` for a refused path; pinned by the attack-case tests in `flare-shells/src/paste.rs`. Source: security review of paste.rs, 2026-10-17.
136. **PowerShell 7 is never found in the current folder.** `where.exe` and `CreateProcess` search the current folder first, so a planted `pwsh.exe` would run as the user's shell. The standard `%ProgramFiles%\PowerShell\7\pwsh.exe` wins when it exists; otherwise `where.exe` runs with its own folder (System32) as the current directory and every result is canonicalised (8.3 names, trailing dots, `..`) and rejected when it lies in the canonical current folder or any subfolder. Native: `Probe::run` sets `current_dir` to the exe folder (`probe_dir`), `pwsh_path` takes a canonicaliser, `detect_shells_with` checks `SysTools::pwsh7` first; pinned by the attack-case tests in `flare-shells/src/list.rs` and `detect.rs`. Source: security review of list.rs pwsh_path, 2026-10-17.
137. **ConPTY gives no end-of-file when the shell exits.** Exit is detected by the process, and the pty is dropped only after the output has gone quiet. Native: `flare-term` `session.rs`, `handle.rs` and `quit.rs`, pinned by `an_exit_without_end_of_file_still_reports`, `the_exit_comes_after_the_last_output` and `ten_real_shells_killed_none_remain`. Source: M2 plan Verified facts; terminal.ts (exit polling).
138. **The bundled conpty.dll is preloaded by absolute path before the first pty opens, and the process must report that module path.** Otherwise the system conhost is used silently. Native: `conpty.rs` `verify_bundled`, pinned by `verify_loads_the_bundled_dll_by_absolute_path` and `the_conpty_dll_in_the_process_is_the_bundled_one`; `flare` exits 3 with a message when the bundle is missing (`a_missing_bundle_fails_loudly_and_exits_three`). Source: M2 plan Task 17 and Task 28.
139. **Always `env_clear()` before setting a pty's environment.** portable-pty merges the inherited registry environment otherwise, so a removed variable comes back. Native: `flare-term` `pty.rs` calls `env_clear()` before applying `pty_env`; pinned by `the_environment_reaches_the_shell` and the `flare-shells` environment tests. Source: terminal.ts ptyEnv (regression-rules same-command-line, #180).
140. **ConPTY startup queries (CPR, DA1) must be answered, or it can stall.** ConPTY asks for the cursor position and the device attributes before it draws. Native: `emulator.rs` answers DA1 and CPR, pinned by `da1_is_answered_the_xterm_way` and `cpr_is_answered_with_the_cursor`; the answer to DA1 is sent on kill too (commit 1512b1d). Source: M2 Task 17 and Task 24 (startup on a slow machine).
141. **A synchronized update (DEC 2026) is let go by the host at its 150 ms deadline; vte only stores the deadline.** A program that opens an update and never closes it must not freeze the grid. Native: `emulator.rs` `an_open_synchronized_update_is_let_go_at_its_deadline` and `handle.rs` `an_open_synchronized_update_still_shows`. Source: M2 Task 18 (vte synchronized output).

142. **The single-instance hand-off only ever goes to the same user in the same session.** The mutex and pipe names carry the user SID, the session id and the profile hash; the pipe is created with `FILE_FLAG_FIRST_PIPE_INSTANCE` and a protected DACL with one ACE for the current SID; the client opens with `SECURITY_IDENTIFICATION`, checks `GetNamedPipeServerProcessId`, the server's token user and `ProcessIdToSessionId`, and writes nothing to a server that does not match (it then starts as a normal instance). `AllowSetForegroundWindow` gets that verified pid, never `ASFW_ANY`. Native: `flare-win` `single_instance.rs`, pinned by `names_carry_the_user_sid_and_the_session`, `a_server_of_another_user_or_session_is_refused`, `nothing_is_written_to_a_server_that_is_another_user`, `the_dacl_is_one_ace_for_the_current_sid` and `a_second_server_on_the_same_name_fails_loudly`. Source: security review of single_instance.rs, 2026-10-17.
143. **The lock and the hand-off pipe are keyed by the profile folder, so an e2e run never reaches the owner's Flare.** The mutex and pipe names carry a hash of the profile folder, so a scenario with its own `--user-data-dir` has its own lock and pipe. Native: `flare-win` `single_instance.rs` (`profile_hash`), pinned by `profile_hash_matches_the_fnv_vectors`, `names_differ_per_profile_and_ignore_case_and_slash_direction` and `the_lock_names_carry_no_prism_name`. Source: M3 plan Task 15 (decision D4); Prism Terminal's userData lock.
144. **A hairline is `max(1, floor(scale))` device pixels and every tab edge a whole device pixel.** Native: the strip geometry is computed in device pixels in Rust, pinned by `every_tab_edge_is_a_whole_device_pixel_with_a_crisp_divider` and `whole_pixels_at_every_scale`. Source: M3 plan decision D8; measured at 225%.
145. **The sidebar slide holds the terminal's size and fits once.** The grid keeps its frozen width while the sidebar slides, and one fit runs at the end of the hold. Native: `fit_hold`, pinned by `the_slide_holds_the_terminals_size_and_fits_once` and `fit_hold_runs_each_waiter_once_when_it_ends`. Source: M3 plan decision D9; core/renderer/lib/fitHold.ts.
146. **Test-only environment variables are read only through `E2eFlag`.** Native: `flare` `e2e.rs` is the only reader, and `no_source_file_reads_an_e2e_variable_directly` scans `src` for any other read; `e2e_env_reads_none_unless_the_flag_is_on` checks the flag gate. Source: src/main/e2eOnly.test.ts.
147. **`tabs.json` is written only after the restore has answered, and is flushed before the shells are killed.** Native: the saved state stays unset until the restore answers, and end of life flushes both files before any shell is killed. Pinned by `nothing_is_written_before_the_restore_answered`, `saved_if_restored_is_none_until_the_restore_answered`, `end_of_life_flushes_both_files_then_kills_and_waits` and `the_close_flushes_tabs_json_before_the_shells_are_killed`. Source: src/main/index.ts awaitingRestore; M3 plan Tasks 13 and 29. (The plan numbered these 138 to 142; those numbers were taken by the M2 rules, so they are 143 to 147.)

## Theme, material and fonts (M4)

148. **The oracle is Prism Terminal's own code: a colour test compares exact strings against it.** Native: the fixtures in `crates/flare-theme/tests/fixtures` are written by `tools/oracle` from Prism Terminal's theme modules, and the `oracle_*` tests compare exact strings; if a Rust function disagrees with a fixture, the function is wrong, never the fixture (the one exception is the 1-unit floating-point note in the M4 plan). Pinned by `oracle_every_preset_resolves_as_it_did`, `oracle_alpha_sweep`, `oracle_every_preset_four_variants`, `presets_data_equals_the_oracle` and `retired_map_equals_the_oracle`. Source: tools/oracle/README.md; core/renderer/lib/termTheme.snapshot.test.ts.
149. **No frame shows a translucent ground without a backdrop behind it.** Native: going glass calls the backdrop first and only then lowers the UI ground alpha; leaving glass raises the UI to opaque first and only then removes the backdrop. Pinned by `order_only_material_first_when_glass_turns_on`, `going_glass_calls_the_frost_then_the_backdrop_and_the_ui_waits`, `going_glass_sets_the_frost_then_the_backdrop` and `the_order_follows_the_glass_change`. Source: core/main/material.ts; M4 plan decision D7.
150. **A Windows build without the backdrop makes no DWM call at all.** Native: the build number is read once at start (`RtlGetVersion`); under build 22621 the surface stays solid and no DWM call is made, so Windows 10 never sees a backdrop attribute. Pinned by `an_unsupported_build_makes_no_surface_call_at_all` (flare-win material) and `an_unsupported_build_makes_no_dwm_call` (flare material controller). Source: src/main/material.ts acrylicOk; M4 plan decision D6.
151. **No theme colour is typed outside the theme.** Native: colour literals of six or eight hex digits live only in `flare-theme` (the presets, the fixtures and `ground.rs`) or come from the look at run time; a scan of the app crates fails on any other `#rrggbb` literal not named in `crates/flare/tests/colour_literals_allow.txt`. Pinned by `no_theme_colour_literal_in_source` (`crates/flare/tests/colour_literals.rs`) and `no_source_file_types_the_default_ground_but_the_allowed_ones`. Source: core/renderer/lib/ground.ts; M4 plan Task 5.
152. **The window is created transparent only when glass can be used, and the software renderer never claims glass.** Native: the transparent attribute is set at creation only when the GPU renderer is in use and the backdrop build gate passes; the software presenter has no alpha, so a see-through level there leaves the window solid and says so. Pinned by `glass_plan_needs_the_gpu_and_the_22h2_backdrop`, `the_outcome_says_glass_is_possible_only_on_the_gpu_with_a_transparent_window`, `the_visual_swapchain_is_asked_for_only_with_a_plan` and `no_glass_without_a_possible_surface`. Source: M4 plan decisions D5 and D8; docs/material.md.
153. **An idle window wakes for the material only when something is due.** Native: the border waits 250 ms after a change (rule 75); the material arms one Slint single-shot timer at that deadline after every change and disarms it when nothing is pending, so no repeating material timer runs. The theme-report nudge (`LookCtl::tick`) follows the same rule once it is wired (docs/parity.md: deferred until `?2031h` is tracked per tab). Pinned by `the_wake_runs_only_while_the_border_is_due` and `under_e2e_the_wake_is_never_armed`. Source: M4 final review.
154. **Selection, find and links work on cell boundaries and logical lines, never on strings.** A selection end is a boundary in `0..=cols` of an absolute row; a wide character is taken whole; text comes from `cell_text` and `selection_text`, and a search maps characters back to cells through the line's index. Native: `sel_machine.rs`, `sel_text.rs`, `find.rs`, tests `a_wide_character_is_taken_whole`, `wide_characters_count_two_cells`. Source: rule 21; xterm `SelectionService`.
155. **A drop is typed, not pasted, and is inert.** Dropped paths go through `quote_paths` for the shell that RUNS in the tab (`TermHandle::shell_id`, never the requested id; nothing is typed before the shell started) and `send_text`; a refused path types nothing; no Enter, no trailing space; only over a live shell. Native: `drop_flow.rs`, tests `a_control_character_in_a_name_types_nothing`, `several_files_are_joined_by_one_space_and_never_end_in_enter`. Source: regression-rules file-drop-and-menu (#16); rule 135.
156. **Mouse reports are bytes, not typing, and Shift gives the selection back.** A program that asked for the mouse gets press, release, motion and wheel; a right click still opens the menu; no report marks the tab typed. Native: `mouse_report.rs`, `TermHandle::write_report`, tests `a_program_that_owns_the_mouse_gets_press_release_and_motion`, `write_report_is_not_typing`. Source: xterm `MouseStateService`; rule 9.
157. **A link click opens http(s) or an allowlisted path and a right click opens nothing; every path is opened through `open_term_path(base, text, ..)`, never by an absolute path from the UI; scans cover the viewport and no pass holds the UI.** Native: `link_target.rs`, `link_scan.rs`, `scan_plan.rs`, tests `any_other_button_opens_nothing`, `a_burst_of_scrollback_is_not_scanned`, `the_count_job_walks_in_slices_and_counts_in_reading_order`. Source: rules 14, 15, 16, 134; two-apps #95.
158. **The clipboard is read and written off the UI thread; a copy is refused, never trimmed; the program is told nothing.** Native: `ClipboardPort`, `copy_async`, `read_async`, `BoundedClip` (8 ms), tests `copy_async_refuses_empty_and_oversize_without_touching_the_clipboard`, `no_blocking_clipboard_call_on_the_ui_thread`. Source: `ipc.ts writeCapped`; rules 10, 34.
159. **Decorations are opaque solid colours in whole device pixels.** The selection is the theme colour blended over the ground in Rust; outlines and underlines are `max(1, floor(scale))` pixels; no alpha reaches a rectangle on either painter. Native: `decor.rs`, tests `the_selection_is_the_opaque_blend_over_the_ground`, `a_find_outline_is_hair_thick_and_outside_the_box`, `gpu_and_cpu_agree_with_a_decor`. Source: rules 23, 131; M3 Verified facts (hairline).
160. **A folder click never runs a file; a clipboard read never leaves its block.** Native: Explorer and ShellExecute get only the canonical path, checked on it immediately before the call (a folder open holds the canonical directory open without delete sharing, refusing a file, link or junction, from before the Explorer spawn until 10 s after it, so neither it nor a folder above it can be renamed or removed while Explorer resolves the name; a file open must not be a directory and must pass `may_open` again; a drive root is passed bare), COM is initialised once per worker thread, and every CF_UNICODETEXT read (the test helper too) stops at the block size (`GlobalSize / 2` units), null-checks the lock and unlocks and closes the clipboard on every path. Pinned by `a_folder_open_refuses_a_file_swapped_in_and_spawns_nothing`, `a_folder_is_held_against_rename_and_delete_while_explorer_starts`, `a_file_open_refuses_a_directory_and_never_executes`, `a_file_open_asks_the_allowlist_on_the_canonical_path_and_runs_that_path`, `the_verbatim_prefix_is_stripped_without_changing_a_unc_into_something_else`, `an_unterminated_block_is_read_exactly_to_its_end`. Source: M5 security review.
161. **A hook never runs a program found by name.** Claude Code (node/libuv) searches the current folder first on Windows, so a `cmd.exe` planted in a project folder would run on every hook. Native: the shipped `hooks.json` carries `@@FLARE_SYSTEM_CMD@@` and is never valid as-is; `install_plugin` writes the installed copy (`%LOCALAPPDATA%\Flare\claude-plugin-<tag>`, one per exe folder) with the absolute `System32\cmd.exe` from `GetSystemDirectoryW` (never from an environment variable alone), refuses a folder that is not a drive-rooted `System32`, and rewrites any installed file whose bytes differ (a bare name, a moved system folder); `plugin_dir()` hands out only the installed copy. Pinned by `the_template_is_never_valid_as_is`, `install_leaves_no_bare_program_name`, `an_installed_plugin_with_a_bare_name_is_rewritten`, `a_relative_or_misnamed_system_folder_installs_nothing`, `the_real_system_directory_is_an_absolute_system32`. Source: M6 security review; rule 1.

## Agent indicator, resume and plugin (M6)

The M6 rules are numbered 162 to 169 (the M6 plan's rules (a) to (h), in order).

162. **A hooked session is never scored from output, and its screen is read only to hold or settle.** Claude Code's hooks are the signal for a session they reach; output scoring and the screen would double-count or invent work. Native: `Indicator` routes a hooked tab past the scorer; the screen is read for a held question and for the settle check only. Held by `a_hooked_session_is_never_scored_and_its_screen_is_not_read_for_output`. Source: core/renderer/lib/useAgentIndicator.ts onTermData; docs/superpowers/plans/2026-10-17-flare-m6.md Task 18.
163. **Flare accepts only `flare-agent`.** The OSC 777 signal of another program, Prism Terminal's own `prism-agent` among them, is refused, and every other OSC 777 user is left alone, so no foreign program can light a Flare tab. Native: `flare-agent` `signal.rs` checks the first part against `flare-agent` only. Held by `prism_agent_is_not_accepted` and `leaves_other_osc_777_users_alone`. Source: core/renderer/lib/agentHookSignal.ts.
164. **A resume id is validated at the lookup, the host and the session manager.** Only a Claude session id or the Codex and agy markers `codex:last` and `agy:last` may reach a command line (a marker becomes `codex resume --last` or `agy -c`, never text of its own), and cmd receives only the hex and `-` of a valid id through `FLARE_RESUME_ID`. Native: `valid_resume` in `flare-agent` `resume.rs`, called by the Claude lookup, `tab_host.rs` and `flare-term` `session.rs`. Held by `an_invalid_resume_id_is_no_resume`, `a_bad_resume_id_never_reaches_a_command_line` and `a_bad_resume_id_never_reaches_a_command_line_from_the_lookup`. Source: core/main/agentResume.ts validResume; core/main/terminal.ts withResume.
165. **The mark clock runs only while something moves, never while minimised, and stops with Windows' animation effects.** A timer that ticks for a still tab wakes the CPU for nothing, and a motion that runs behind a hidden window costs power. Native: `AppCtl` owns one 16 ms timer that is stopped unless a mark moves, the window is shown and the effects switch is on; `MarkMotion` is the only animation source. Held by `the_clock_runs_only_while_something_moves` and `animation_effects_off_stills_every_mark`. Source: core/renderer/styles/marks.css; core/renderer/styles/marks.test.ts.
166. **Sonara is contacted on 127.0.0.1 only, off the UI thread.** The question post is a hand-written HTTP request to loopback, on a worker, with a 1500 ms timeout, and a failure is quiet. Native: `sonara.rs` connects only to 127.0.0.1; `agent_hub.rs` posts from a worker. Held by `only_loopback_is_ever_contacted` and `sonara_is_never_called_on_the_ui_thread`. Source: core/main/sonaraLink.ts. (This refines rule 66 for the native port.)
167. **The diagnostics hold no title, screen or conversation text.** A log line carries the four agent kinds (hook, title, mark, restore), counts and states, never free text. Native: `flare-diag` `agent_diag.rs` writes only the kinds and their folds; the sink takes no text. Held by `no_free_text_reaches_the_sink` and `diagnostics_get_the_four_kinds_and_no_text`, and in the app by `the_log_has_the_marks_the_rules_and_no_text`. Source: core/renderer/lib/agentDiag.ts.
168. **The indicator follows the measured timelines.** Each recorded hook timeline (a Yes, an Esc, auto mode with an ask rule, two prompts in a row, a subagent asking while its sibling works, the classifier denying) replays through `hook_step` and through the whole indicator, and must end in the measured state. Native: `tests/timelines.rs` is the TypeScript `replay`, not a simplification. Held by `timeline_a_a_yes`, `timeline_b_an_esc`, `timeline_c_auto_mode_with_an_ask_rule`, `timeline_d_two_prompts_in_a_row`, `timeline_e_auto_mode_with_a_hook_that_asks`, `timeline_g2_a_subagent_asks_while_its_sibling_works` and `timeline_c3_the_classifier_denies`. Source: core/renderer/lib/agentHookState.timelines.test.ts.
169. **An inherited copy of our plugin is dropped from a shell's environment.** A shell started by Flare inherits `CLAUDE_CODE_PLUGIN_DIRS` from the user; a copy of Flare's own plugin in it would load twice, so it is removed and Flare's own folder is added once. Native: `flare-term` `session.rs` drops an inherited copy identified by the manifest name. Held by `an_inherited_copy_of_our_plugin_is_dropped`. Source: core/main/terminal.ts withPluginDir.

### Amendments in M6

These older rules gained test names when M6 made them testable.

- Rule 2 (the only commands written into a shell): held by `a_bad_resume_id_never_reaches_a_command_line` and `a_cmd_resume_carries_only_hex_into_the_environment`; no pty write exists in an M6 file.
- Rule 3 (the resume id travels in the environment): held by `a_cmd_resume_carries_only_hex_into_the_environment` and `accepts_a_session_id_and_the_codex_marker_refuses_a_command`.
- Rule 38 (a resumed conversation is not new work): held by `a_resume_is_not_new_work` and `restored_tabs_carry_no_done_question_or_failed`.
- Rule 39 (the clear goes to the grid only): held by `a_resume_clears_the_grid_just_before_the_agents_title`, `a_shell_title_or_a_folder_called_claude_clears_nothing` and `the_first_frame_has_the_tabs_the_skeleton_and_the_ring`.
- Rule 48 (the close question): held by `a_tab_hosting_an_agent_asks_naming_it_and_working_for`.
- Rule 50 (keep agent sessions open): held by `keep_agent_sessions_open`.
- Rule 57 (the indicator is the agent's own word): held by `a_claude_title_is_the_agent_present_ahead_of_the_poll_and_asks_a_look` and `a_poll_report_makes_the_agent_present_and_births_it`.
- Rule 58 (Claude's hooks run in exec form): held by `hooks_json_matches_the_table`, `hooks_json_has_25_entries` and `hook_cmd_is_crlf_and_static`.
- Rule 59 (a hooked session is never scored from output): held by `a_hooked_session_is_never_scored_and_its_screen_is_not_read_for_output` and the seven `timeline_*` tests.
- Rule 60 (the question stands while the box is on screen): held by the seven `timeline_*` tests.
- Rule 61 (Failed and Finished are news, Question is state): held by `finished_is_not_raised_on_the_tab_in_front_but_a_question_is` and `each_attention_mark_is_behind_its_own_switch`.
- Rule 62 (every working tab has its own mark; the taskbar counts): held by `working_outranks_question_outranks_failed_outranks_done` and `the_badge_counts_marked_tabs_and_clears`.
- Rule 63 (Ring or Bar only): held by `the_stored_word_minimal_is_bar_everything_else_ring` and `the_indicator_pick_turns_a_ring_row_into_a_run_bar`.
- Rule 64 (a folder path is not the program): held by `a_title_names_the_agent_only_when_it_is_the_program` and `never_clears_on_a_shell_title_nor_on_a_folder_called_claude`.

## Parity sweep, the e2e and performance (M10)

These rules came with the M10 parity sweep. Each names the tests that hold it; `docs/e2e.md` and
`docs/performance.md` say how the suite and the bar are run.

170. **The test hook exists only under `--e2e`, and only for the current user.** Without the flag no pipe is
created, no handler is registered and no probe answers. The pipe's DACL grants the current user's SID alone
and `FILE_FLAG_FIRST_PIPE_INSTANCE` stops a second launch on the same profile taking it over. Held by
`start_without_the_flag_is_not_enabled_and_creates_no_pipe`, `the_pipe_is_refused_to_another_user`,
`a_second_server_on_the_same_profile_cannot_take_the_pipe` and `the_hook_is_only_started_from_one_place_behind_the_flag`.
171. **The e2e never takes the foreground and never touches the owner's apps.** The window is parked and
unfocusable; the foreground window and the OS clipboard sequence number are read around the run; the reaper
matches the world's profile path only. Held by `the_foreground_guard_flags_a_window_of_the_run_and_only_notes_another_app`,
`the_clipboard_guard_flags_a_sequence_change`, `the_reaper_spares_a_process_without_the_world_path`,
`park_moves_the_window_off_every_monitor` and `the_world_removes_itself_and_leaves_nothing_in_temp`.
172. **A selection stands while a program redraws.** A redraw that rewrites the whole screen keeps the
selection on its text, and a block selection survives the same redraw. Held by
`a_selection_survives_a_cursor_home_redraw_of_the_whole_screen` and `a_block_selection_survives_a_redraw`.
173. **No search or copy holds the UI thread over its slice.** Find walks the scrollback in slices of at most
8 ms that stop before the row that would cross the budget; a big copy is read on a worker. Held by
`walks_a_10000_line_backlog_bottom_up_in_slices_of_at_most_8_ms`, `count_slices_stay_at_or_under_the_budget`
and `a_big_copy_is_read_on_a_worker_and_the_badge_waits_for_the_clipboard`.
174. **A stroked glyph's box grows by its stroke.** A glyph drawn with a stroke takes its nominal size plus one
stroke width, so the stroke is never clipped. Held by `expected_extent_adds_one_stroke_at_the_nominal_size` and
`every_stroked_path_has_a_size_test`.
175. **The first-run import shares one 1.5 s budget.** Every profile's import draws on the same budget, and an
expired budget skips the next profile. Held by `two_profiles_share_one_budget`,
`the_whole_import_never_exceeds_the_budget_plus_a_margin` and `an_expired_budget_skips_the_next_profile_and_counts_a_retry`.
176. **A theme change reaches running programs as one report and a one-column nudge.** A tab that asked for
the mode gets one report and one nudge of one column, then the column comes back; a tab that never asked gets
neither. Held by `a_mode_change_pushes_once_then_nudges_one_column_and_back`,
`a_tab_that_never_asked_gets_neither_report_nor_nudge` and `the_nudge_is_one_column_narrower_and_back`.
177. **Start-up: the first frame waits only for what it draws.** Work that the first frame does not show is put
off until after it, and runs once. Held by `nothing_on_the_deferral_list_is_read_by_the_first_frame`,
`deferred_work_runs_exactly_once_after_the_first_frame` and `the_first_frame_wears_the_saved_theme`.
178. **Only the faces the terminal needs are loaded.** The font database holds the named faces and the fallbacks,
and the installed faces come from the registry, not from a walk of the font folder. Held by
`the_database_holds_only_the_named_faces_plus_the_fallbacks` and
`installed_faces_reads_the_registry_without_walking_the_font_folder`.
179. **The performance bar is measured the way `docs/performance.md` defines it.** Each rule passes or fails at
its edge, and a rule that needs a Prism Terminal baseline that is missing is unknown, never a pass. Held by
`the_bar_evaluator_passes_and_fails_each_rule_at_its_edge` and `a_missing_baseline_makes_pt_relative_rules_unknown_not_pass`.
180. **A display-scaling change leaves the prompt line, the grid size rule and the hairline widths intact.** The
grid follows the new cell size at every scale, a resize never loses the prompt line, and the hairline is the
floor of the scale. Held by `the_expected_grid_changes_with_the_scale`, `a_grid_that_kept_the_old_cell_size_is_not_the_rule`,
`a_hairline_is_the_floor_of_the_scale_and_at_least_one` and `a_resize_nudge_never_loses_the_prompt_line`.

## Dictation from any app (M8)

These rules came with the M8 dictation port at Prism Terminal `e5848cb`. Each names the tests that hold it. The
rule numbers continue from 180; rules 2 and 95 above carry their M8 amendments.

181. **The keyboard hook reads only the dictation key, and swallows only the picker's digits and Esc.** It reads the
binding's own keys (both sides of each modifier the binding names), reports that another key went down without saying
which while a binding key is held, and while the Claude picker is open swallows and reports digits 1 to 9 and Esc, on
down and up. It swallows nothing else, ever. Native: `flare-dictation` `keyhook.rs` and `keyfeed.rs` (`keys_for`,
`is_digit_key`). Held by `another_key_with_no_binding_key_held_is_not_read`, `an_up_of_another_key_says_nothing`,
`digits_one_to_nine_are_swallowed_and_reported_only_while_picking`, `zero_and_the_keys_beside_the_digits_are_never_taken`,
`escape_is_swallowed_and_reported_only_while_picking`, `keys_for_watches_both_sides_of_each_modifier_of_ctrl_win_and_masks_start`
and `parse_helper_line_reads_keys_another_key_digits_and_esc_and_nothing_else`. Source: appHelper.ps1; globalDictKey.ts
keysFor (N13, N16).
182. **Typing into a tab is checked three times, and dictation never presses Enter inside the window.** Before dictated
text goes into a Claude tab, the tab must still exist, run Claude by its mark with a session, and not be asking; Windows'
process tree under that tab's shell must show Claude; and the same tab and session must win again right before the paste
and again before the Enter (one lone CR after 300 ms, and only if the tab is still sendable, the session is the same and Windows still says Claude runs there; `type_checked_holds_the_enter_back_when_claude_quit_while_the_words_landed`). A bare shell never receives
dictated text with an Enter. Native: `route.rs` (`decide_into`, `type_checked`) and `service.rs` (`dictation_answers`),
with the Windows check through the process tree. Held by `type_checked_types_nothing_where_windows_says_no_claude_runs`,
`type_checked_types_nothing_when_the_tab_changed_while_windows_was_asked`,
`type_checked_never_presses_enter_after_words_that_were_not_written`, `type_checked_holds_the_enter_back_when_a_question_opened_before_it`,
`never_types_into_a_shell_or_into_a_question` and `dictation_answers_never_types_into_a_shell_or_a_question`. Source:
orchestratorSend.ts decideInto and typeable; dictationClient.ts typeChecked (N24, N30).
183. **The link is authenticated both ways.** Two copies prove the shared key with HMAC-SHA256 over fresh nonces, compared
in constant time; a peer that has not proved itself within 5000 ms, or that fails the proof, is answered nothing and
dropped. The pipe has a current-user DACL. A line over 64 KiB, or a buffer over 256 KiB, drops the peer. Native:
`hmac.rs`, `link.rs` (`current_user`, `create_instance`), `tests/link.rs`. Held by
`a_peer_that_fails_the_proof_is_answered_nothing_and_dropped`, `a_peer_that_proves_nothing_in_time_is_dropped`,
`a_line_over_64_kib_drops_the_peer`, `a_buffer_that_never_ends_a_line_drops_the_peer_too`,
`a_client_drops_a_service_whose_proof_has_the_wrong_mac`, `the_key_is_made_once_and_the_pipe_is_named_by_its_hash`,
`two_copies_starting_together_agree_on_one_key` and `never_answers_types_or_pastes_for_a_service_that_does_not_hold_the_key`.
Source: dictationLink.ts (N26).
184. **The widget never takes focus.** The widget window is frameless, transparent, never focusable, off the taskbar and
above the other windows, never navigates, and lets clicks through except while the picker is up. Showing it does not move
focus from the app in front, so the key keeps working where you are. It takes the scaling of the monitor it opens on.
Native: `crates/flare/src/dictation_widget.rs` (`show_now`, `click_through`, `keep_style`). Held by
`the_window_never_takes_focus`, `the_style_is_never_active_off_the_taskbar_and_on_top`,
`the_card_is_480_wide_and_the_heights_follow_the_state` (at all eight scales) and `the_bounds_follow_the_scale_of_the_monitor`.
Source: overlay.ts, overlay.css (N18).
185. **The e2e never installs the keyboard hook.** A run under `--e2e` starts no hook; the scenario's keys reach the
reducer only through the test seam (`DictLine` lines and `DictRoute`), and every key still takes the one path the real key
does. Native: `dictation_ctl.rs` (`e2e: bool` in the options, `e2e_line`), `service.rs` (`HookStart` never emitted under
the flag). Held by `the_hook_is_never_started_under_e2e` and `the_hook_starts_with_the_bindings_keys_and_mask_and_only_once`
(the second for the normal path). Source: globalDictKey.ts; src/main/index.ts E2E (N16).
1151. **A chord of modifiers dictates only when exactly its modifiers are down and no other key goes down while it
decides.** Owner report (Mac, Toggle, Ctrl+Cmd, 2026-11-11): Cmd+Shift+S, which Karabiner turns into Cmd+Ctrl+Shift+4
(the region screenshot), also started a dictation, and the next one stopped it and pasted. Root cause: the reducer
already matched exactly, but the hooks report only the binding's own keys, so the key feed never knew Shift was down
when Ctrl joined Cmd, and the 4 never reaches a listen-only tap (the system takes the screenshot shortcut): the press
was a clean Ctrl+Cmd tap. Now a hook says `Extra` (never which key) when a binding key goes down while a modifier the
binding does not name is already down (Mac: the event's flags; Windows: `GetAsyncKeyState`, `extra_after`, the AltGr
Ctrl excused), and when such a modifier goes down while a binding key is held (it said `Other` before). The feed reads
`Extra` as another key: a press still deciding (Hold before the hold time, Toggle before the release) is dropped with
no start, toggle, stop or paste, and a toggle's stopping tap with it stops nothing. A key after a Hold has begun keeps
the recording, as before. Such a cancel is logged as `dictation-key` with `cancel: extra`, `kind` `modifier` or `key`
and `from` `window` or `hook`, never the key (`hotkey::extra_cancel`). Same on Windows (Ctrl+Win with Shift+S).
Native: `crates/flare-dictation/src/{hotkey,keyfeed,keyhook,service}.rs`, `crates/flare-dictation/src/macos/keyhook.rs`,
`crates/flare/src/dictation_ctl.rs`. Held by `ctrl_cmd_inside_a_karabiner_screenshot_chord_never_dictates_hold_or_toggle`,
`the_three_flags_in_one_event_are_no_match_and_a_shift_added_later_cancels`,
`ctrl_cmd_alone_still_starts_and_stops_in_hold_and_toggle`,
`ctrl_cmd_then_a_letter_cancels_in_hold_and_toggle_and_a_toggles_stopping_tap_too`,
`a_key_after_a_hold_has_begun_keeps_the_recording_as_on_windows`,
`a_cancel_by_an_extra_modifier_or_key_is_told_by_kind_only`,
`a_modifier_the_binding_does_not_name_cancels_a_ctrl_win_press_in_hold_and_toggle`,
`karabiners_cmd_ctrl_shift_4_for_cmd_shift_s_never_dictates_in_hold_or_toggle`,
`ctrl_win_with_shift_already_held_or_added_says_a_modifier_and_never_dictates`,
`ctrl_win_with_an_extra_modifier_from_the_hook_starts_nothing_and_logs_the_kind` and
`a_window_press_cancelled_by_an_extra_key_or_modifier_logs_its_kind_only`. Source: Max on his Mac (2026-11-11).

## Surfaces: the control channel (stage 2)

The security rules of the design (`docs/superpowers/specs/2026-10-21-surfaces-design.md`, "Security rules"), numbered after
the last rule above and kept under their design names S1 to S6. The contract they protect is `docs/surfaces/protocol.md`.

186. **S1. The control channel is per user and per tab, and never listens on a network.** The pipe is
`\\.\pipe\flare-ctl-<hash of the SID>` (plus a hash of the profile folder for any profile but the default), created first-instance
with the descriptor `D:P(A;;GA;;;<SID>)` and `PIPE_REJECT_REMOTE_CLIENTS`; a client refuses a server of another user, **and a
client that was told which process serves the pipe (`FLARE_CTL_PID`, put in every shell next to `FLARE_CTL_PIPE`, an inherited copy
dropped, rule 12) refuses any other process**: the name is predictable from the SID, so a process of the same user at low integrity
or with a restricted token could create it first and would otherwise be handed the tab's secret and be able to put its words in a
tool result (`flare-mcp` also cleans a refusal's text to one short line and refuses to connect with no pid). The shells are told
about the channel only once its server is serving: a server that failed to start leaves no tab with a secret or a pipe name. On top
of that a connection acts as a tab only by presenting that tab's secret: 64 hex digits from two random UUIDs (the OS generator),
made when the tab's shell is started and put in its environment as `FLARE_TAB_TOKEN`; never on a command line (rule 3), never
logged, never sent to Sonara or printed in a hook line (those carry the public `FLARE_TAB_ID`). `hello` takes `tab_token` and
refuses a `tab` name (`bad_params`); an unknown token is `unauthorized` and closes; the token is looked up again on every request,
so a closed tab's token stops working at once; a restored tab gets a new secret each launch. Native: `flare-ctl` `pipe.rs`,
`session.rs`; `flare-term` `session.rs` (`new_ctl_token`, `tab_for_ctl_token`); `flare-shells` `env.rs`. Held by
`the_live_pipe_carries_the_owner_only_descriptor`, `the_pipe_is_closed_to_other_users`, `a_second_server_on_the_same_name_fails_to_start`,
`the_pipe_name_has_known_answers`, `every_shell_gets_its_own_unguessable_control_token_and_the_pipe`,
`an_unknown_empty_or_partial_token_names_no_tab`, `a_closed_tab_s_token_stops_working_and_a_new_shell_gets_a_fresh_one`,
`a_channel_that_did_not_start_leaves_no_control_variable_in_a_new_shell`,
`a_client_told_which_process_serves_the_pipe_speaks_to_no_other`, `a_pipe_served_by_another_process_is_never_given_the_secret`,
`a_refusal_shown_to_the_model_is_one_short_clean_line`, `a_warm_spare_belongs_to_no_tab_until_one_adopts_it`, `an_unknown_tab_token_is_refused_and_closes_the_client`,
`a_tab_name_is_refused_because_only_the_secret_names_a_tab`, `a_closed_tab_loses_its_connection_s_tab_at_once`, and the e2e
`ctlChannel` (a wrong secret, the tab's id and a tab named in `hello` are all refused; the shell's `FLARE_CTL_PID` is the app's) and
`mcpRoundTrip` (a server named as another process, or none, is never connected to). Source: the design spec; the stage 2 security
review (2026-10-21).
187. **S2. A `view.*` path is resolved again by the backend and shown only if the user could already open it.** The call's own
worker resolves it with the click's function (rule 13), requires the click allowlist as named AND as the file system names it
(rule 14), requires the kind asked for (a picture for `view.image`, text or code for `view.file`), and answers `not_found` for
anything else a probe could learn from (missing, a share, a program, outside the list); a program is never shown and never run.
`view.text` is a string, capped at 4 MB. The UI thread is only given a path that passed. Native: `crates/flare/src/ctl_app.rs`
(`view_file`). A call has ONE deadline for all its steps (10 s, inside the channel's 15 s), an absolute path never asks the UI for a
folder, and the UI thread does nothing for a request that arrives after its deadline (answers `timeout`), so a call told it timed
out never changes the window. A file shown by a click clears the title and the handed-over text of an earlier call, so a real file
is never under a title an agent chose. Held by `every_request_carries_one_deadline_inside_the_channel_s_own_timeout`,
`an_absolute_path_never_asks_the_ui_for_a_folder_and_a_relative_one_does`, `a_request_that_arrives_after_its_deadline_changes_nothing`,
`a_click_after_a_titled_call_shows_the_file_under_its_own_name`, `a_path_nobody_may_open_is_not_found_and_never_reaches_the_ui_thread`,
`the_kind_must_match_and_a_link_that_changes_kind_shows_nothing`, `view_text_is_capped_and_a_title_is_one_short_line` and the e2e
`ctlChannel` (a missing file, a program, a program named like a picture and a network path; a relative name is read from the folder
of the tab that asks, two pictures of different sizes telling the folders apart) and `mcpRoundTrip` (the server's own folder rules a
relative name).
188. **S3. The agent grant shows content and drives its own tab's panel; it cannot type into a shell, read other tabs' screens,
change settings or enable mods.** `shell.input`, `settings.set` and `mods.enable` exist as names only so the refusal is a pinned
test, and the `ui` grant never crosses the pipe. A plugin's typing (`tabs.input`, rule 2's plugin exception) is in the `Input`
namespace, which the agent grant never enters, and the dispatcher refuses any caller that is not a plugin; so are
`session.read` (the `Session` namespace) and a plugin's own `storage.*` (the `Storage` namespace) (plugins phase 5). `tabs.list` gives the agent its own tab only (ids and titles, never a screen); a
method acts on the connection's own tab and never on a `tab` in its params. Native: `flare-ctl` `grant.rs`, `session.rs`;
`ctl_app.rs` (`call`). Held by `rule_s3_the_agent_cannot_type_into_shells_change_settings_or_enable_mods` (with
`tabs.input`, `session.read` and `storage.*`), `the_agent_enters_no_plugin_namespace`,
`nothing_over_the_pipe_reaches_the_ui_namespace`, `a_ui_token_is_refused_at_hello`, `tabs_list_gives_the_agent_its_own_tab_only`,
`a_call_with_no_tab_is_no_tab_and_asks_the_ui_nothing`, and the e2e `ctlChannel` (`shell.input` and `browser.eval` are forbidden).
189. **S4. Web panels never use the user's own browser profile, and `browser_eval` is off by default.** The channel half is built
(`browser.eval` needs the user's switch, and `browser` and `eval` for a mod); the web panel (stage 3) keeps its profile in the run's
own folder and never in Edge's. Held by `browser_eval_needs_both_browser_and_eval`, `the_agent_cannot_reach_ui_methods_or_eval`
(the channel), `the_profile_lives_under_the_named_profile_or_local_appdata` and `the_profile_is_never_an_edge_folder`
(`flare-surfaces` `web.rs`, the profile folder), `every_other_scheme_is_refused`, `a_login_in_the_address_is_refused`,
`a_refused_navigation_answers_a_waiting_call` and `a_refused_navigation_is_said_in_the_header_and_leaves_the_page` (only http,
https and about:blank open, whoever asks), and the e2e `webPanel` (a location request is denied, a plain new-tab link stays in
the panel, a pop-up after a click opens as rule 249 says and one no click asked for opens nothing, the data folder is the
world's). A pop-up runs in the same profile as its panel, never Edge's. `browser.eval` itself is stage 4. A page's link to an app
scheme is asked about, never opened by the browser (rule 252).
190. **S5. A mod gets exactly its granted permissions, and a disabled mod's token is dead.** The channel half is built (the token
is looked up on every request, so an open connection dies at once); mods themselves are stage 8, and until then every mod token
is unknown. Held by `disabling_a_mod_kills_its_open_connection`, `a_mod_token_gets_the_mod_grant` and
`the_token_decides_the_tab` (no token resolves to a mod).
191. **S6. A download from a web panel is held until the person answers; no silent saves.** WebView2's own behaviour is to save
straight into Downloads, so Flare registers `DownloadStarting`, takes a deferral and asks in its own dialog ("Save this file?", the
file name and the host, Save or Cancel). A yes lets the browser save to its proposed path in Downloads; a no, a closed panel, a
closing window, a runtime that cannot hold a download back, or no answer at all means no file. A second download waits behind the
first. Electron must show the same question (`will-download`, `item.pause()`, the same dialog, then `item.setSavePath` or
`item.cancel()`); the channel has no method that saves a file. Held by `a_download_is_asked_about_and_saved_only_on_a_yes`,
`a_no_cancels_the_download_and_a_second_one_waits_its_turn`, `a_download_of_a_page_that_is_closed_is_not_asked_about`
(`web_tests.rs`), `download_ask.rs` (`the_question_names_the_file_and_where_it_comes_from`, `a_name_cannot_inject_markup_or_run_on`,
`a_second_download_waits_behind_the_first`) and the e2e `webPanel` (an `<a download>` asks, nothing reaches Downloads while the
question is open, and a no saves nothing).
192. **The plugin manifest never names the MCP server by a variable.** `plugin.json` is a template with `@@FLARE_MCP_EXE@@` and the
install step writes the absolute path of `flare-mcp.exe` beside the running exe into the installed copy (like the System32 `cmd.exe` in
`hooks.json`, rule 161); with no such file the `mcpServers` block is left out, so a build without it never gives Claude Code a server
that fails in every tab and the status hooks of the same plugin are never affected. No secret, pipe name or `${...}` is written in the
manifest. Native: `flare-agent` `plugin.rs` (`render_manifest`, `install_plugin_with`). Held by
`declares_the_flare_mcp_server_by_a_written_in_absolute_path_and_never_by_expansion` and
`install_writes_the_mcp_server_only_when_the_program_exists`. Source: the stage 2 review (2026-10-21).
193. **The agent's hands on a page never wait on the UI thread, and every call is answered exactly once.** A look, click, type, press,
scroll, wait or eval is a job (`flare_surfaces::agent_job`) that the window drives from the browser's completion messages; the
browser's completion handlers only post (rule 34). One call runs on a page at a time (a second is `unavailable` and the first is
untouched); a call is answered when it is done, when the person takes over (`interrupted`), at its own deadline (`timeout`), when the
panel or tab closes, the page's process ends or the window quits (`unavailable`), and a late answer for a finished job is ignored.
Held by `a_second_call_while_one_runs_is_refused_and_the_first_is_not_disturbed`, `closing_the_panel_answers_the_call_that_was_running`,
`a_page_whose_process_ends_answers_the_call_too`, `quitting_answers_a_call_that_was_running` (`web_tests.rs`),
`a_call_the_page_never_answers_times_out`, `answers_for_other_calls_are_ignored` (`agent_job.rs`),
`quitting_closes_every_web_view_before_the_window_goes` (`web_tests.rs`), the scan in `flare-web/tests/sync_calls.rs` and the e2e
`agentBrowser`. A call that is cut short in the middle of a press (a takeover, a timeout) lets go of the button or key it had pressed
(`a_click_cut_short_lets_go_of_the_button`, `a_click_that_times_out_still_lets_go`,
`the_person_taking_over_a_click_lets_go_of_the_button`), and an agent in a tab that is behind gets a hidden page made for it, never a
"no web page is open" loop (`an_agent_in_a_tab_behind_gets_a_hidden_page_and_does_not_loop_on_open_page`). That page paints off the window
(at (-32000, -32000), the panel's size) while a call runs on it and for 5 s after, so what it does in its next frame happens
(issue #120), and is never moved while it is the live page in front or its still is being taken for a Flare menu: it is hidden
for real first, which hands the keyboard back to the window (`a_page_behind_paints_off_the_window_while_the_agent_works_on_it`,
`a_page_in_front_is_not_parked`, `a_menu_over_the_page_the_agent_uses_keeps_it_in_place_until_the_still`, the e2e `agentBrowser`).
194. **The agent never runs its own script in a page unless the user switched it on, and a wait is never a script runner.**
`browser.eval` needs "Let agents run scripts in pages" (off by default, read at every request); a text or selector wait sends ONE fixed
checker whose only variable part is a JSON string literal. Held by `the_agent_grant_follows_the_users_switch_for_scripts`,
`scripts_for_agents_are_off_until_a_stored_one`, `the_wait_checker_is_a_fixed_template_around_one_string_literal`,
`a_wait_asks_the_page_with_the_fixed_checker_only`, `a_look_goes_through_devtools_and_answers_with_the_picture_and_the_elements` (only
protocol methods are sent, and the one script is exactly `window.devicePixelRatio`, with a time limit:
`the_pixel_ratio_question_has_a_time_limit_and_nothing_else_is_script`) and the e2e `agentBrowser` (eval is `forbidden` with the switch off, works
with it on, a hostile wait text leaves the page unchanged) and `mcpBrowser` (the refusal names the setting).
195. **Page text is data, never instructions.** What a page says reaches the agent only after Flare removed control, direction-override,
zero-width and other invisible characters (Unicode tag characters, variation selectors and fillers among them: one list,
`flare_ctl::text`), collapsed white space (line separators too) and capped every field, and `flare-mcp` does the same again, writes each name as a
quoted string, one element per line, between `BEGIN PAGE CONTENT` and `END PAGE CONTENT` marker lines that no page text can forge. Held
by `page_text_is_cleaned_before_it_goes_anywhere`, `a_long_name_is_cut_to_the_cap`, `a_hostile_tree_cannot_loop_or_blow_up`
(`agent_web.rs`), `hostile_page_text_cannot_end_the_block_pose_as_an_element_or_hide`, `a_huge_list_is_cut_and_says_so`,
`a_bad_or_missing_picture_is_left_out_without_losing_the_list` (`flare-mcp` `browser.rs`) and the e2e `mcpBrowser`. The title and the
address a navigation answers with are page text too: they sit between the markers, quoted and cleaned, never in Flare's own sentence
(`a_hostile_title_or_address_after_a_navigation_stays_inside_the_markers`, `an_eval_result_cannot_fake_the_end_with_a_line_separator`,
`hidden_characters_of_every_kind_are_dropped` in `flare-ctl` `text.rs`, and the e2e `mcpBrowser`).
196. **The person keeps control of the page.** A key of theirs in the page, or a mouse button pressed over it, ends the agent's running
call with `interrupted`, ends the hold (glow and cursor go) and forgets the refs; the mouse is read every 16 ms and a press that came and went between two reads still
counts (`GetAsyncKeyState`'s "pressed since" bit, cleared when the call starts), and the page taking the window's keyboard focus while a
call runs is a takeover too (`the_person_focusing_the_page_while_a_call_runs_takes_over`); the agent's own key presses are neither the person's nor the app's
chords (an agent pressing Ctrl+Shift+P closes nothing); a click in another window that covers the page is not a takeover. No keyboard
or mouse hook is installed and nothing is sent to a real window. Held by
`the_page_is_in_use_while_the_call_runs_and_the_person_taking_over_ends_the_call`,
`a_key_of_the_persons_posts_a_takeover_only_while_an_agent_works`, `the_agents_own_key_press_is_neither_the_persons_nor_a_chord`,
`a_point_is_inside_a_rectangle_by_its_near_edges` and the e2e `agentBrowser`. The real-button polling is on the by-hand list.
197. **The agent at work never changes the page.** A web page has no header and no ribbon (2026-10-27, Max): the address row is the
panel's top and holds the close, and an agent at work shows only as the edge glow and the cursor, windows over the page. The browser
window's rectangle is the same with and without an agent, so an agent reading coordinates never sees the page reflow. The page is in
use while a call runs on it and for 1.5 s after (the hold), and the timer that serves it exists only then. Held by
`a_web_page_has_no_header_and_the_address_row_holds_the_close`,
`the_hold_lasts_after_a_call_that_finished_until_the_person_acts`, `nothing_exists_before_a_web_panel_opens` and the e2e
`agentBrowser` (the rectangle is compared).
198. **Refs are only as old as the page they were made on.** A ref after a page load, a takeover, or for an element that is gone answers
`not_found` and sends nothing to the page; positions are resolved when the action runs. Held by
`a_ref_is_forgotten_when_the_page_loads_something_new`, `a_stale_ref_is_not_found_and_nothing_is_sent`,
`an_element_that_vanished_is_not_found`, `a_look_that_finishes_after_a_navigation_does_not_put_back_the_old_refs` (a look keeps its
names only when the page is still the document it read) and the e2e `agentBrowser`.
199. **A page the agent wrote runs in a sandbox that cannot reach out, and the page cannot lift it.** A mini-app (`show_html`) is served
from a host of its own (`flare-app-<n>.local`: its own SITE, so its storage and cookies are not another app's, not even through a `Domain=` cookie), in a private browser profile of
its own (never the web panel's, never Edge's), and the first element of the page is a Content-Security-Policy that Flare wrote
(`default-src 'self' 'unsafe-inline' data: blob:; connect-src 'self' data: blob:; object-src 'none'; base-uri 'none'; form-action 'none'`,
no `'unsafe-eval'`; `https:` is added only when the agent asked AND the user's setting "Let mini apps use the network" is on, plain `http:`
never). A script that removes the tag removes nothing, a second wall answers every request that is not the app's own with a refusal (every request source, workers too), and the page loses the peer-connection API three ways: a script before its own code in every frame whose completion is AWAITED before the view navigates (an error fails the view, no unprotected fallback), a browser environment of the mini-apps' own (never the web panel's; one for apps with no network, one for apps with it, each started with the WebRTC policy `disable_non_proxied_udp`, the sealed one also with a proxy that is not there) and a policy that shuts child frames (`frame-src 'none'; child-src 'none'; worker-src 'self' blob:`). The browser walls are tested alone, by seams that switch the policy and the script off, with a UDP socket as the STUN server (0 packets for an app, more than 0 for the web panel as the control) and a WebSocket to a loopback listener (stopped for a sealed app); held also by `a_sealed_app_browser_has_the_webrtc_policy_and_a_dead_proxy_a_networked_one_only_the_policy`, `the_policy_has_no_child_frames_and_only_own_or_blob_workers`, `the_two_app_environments_have_folders_of_their_own_beside_the_panels`.
The view cannot navigate away, a link or `window.open` opens nothing, developer tools are off. Native: `flare_surfaces::app` (`csp`,
`wrap_html`, `nav_allowed`, `request_allowed`) and the host's `configure_app`. Held by `the_policy_has_no_network_unless_asked`,
`the_policy_comes_before_anything_the_page_says`, `a_view_stays_on_its_own_page`, `a_request_leaves_only_as_far_as_the_network_switch_allows`,
`the_network_needs_the_agent_to_ask_and_the_users_switch_to_be_on` and the e2e `miniApp` (violation events for a fetch, an image and a
script from outside; the tag removed and a fetch still blocked; the second wall alone answering with the policy off by the `--e2e` seam;
a navigation, a sibling page and `window.open` going nowhere; the profile's name and private flag read back; the next app finding no
storage or cookie). Source: the Surfaces design, "Security rules" (S4) and the stage 5 review.
200. **A mini-app's files are the app's own and go with it.** One folder per app directly under `<profile>\apps`, holding only the page
(and a diagram's Mermaid file): files next to a `path` page are never served. The folder is written off the UI thread before the UI is
asked, then belongs to the UI thread, which removes it on a worker when the panel closes, another panel replaces the app, the tab closes,
the request arrives too late or the tab is gone; the whole folder is emptied at start; and only a folder directly inside the apps root is
ever deleted, whatever it is handed. A page named by `path` is resolved again by the backend (rule 187) and must be `html` or `htm` as
typed and as the file system names it; a click on a path never opens a page. Native: `app::write_app`, `remove_app`, `clean_root`;
`flare_term::path_open::resolve_html_file`. Held by `only_a_folder_directly_inside_the_apps_root_is_ever_removed`,
`startup_empties_the_apps_folder_and_leaves_the_profile_alone`, `retiring_the_root_renames_it_aside_and_the_sweep_deletes_every_old_one`, `the_page_loses_peer_connections_and_prefetch_before_its_own_code`, `a_page_for_a_mini_app_is_html_or_htm_and_nothing_else`,
`a_page_named_by_path_is_resolved_again_vetted_and_read`, `closing_a_mini_app_ends_it_and_removes_its_folder`,
`a_mini_app_for_a_closed_tab_is_refused_and_its_folder_removed`, `a_mini_app_asked_for_too_late_is_never_shown` and the e2e `miniApp`.
201. **A diagram is drawn by the bundled build and its error is the agent's to read.** Mermaid is the file in `assets/mermaid/` (hash held
by `the_bundled_mermaid_is_the_pinned_build`), compiled into the exe, never fetched; `securityLevel` is `strict`; the text reaches the page
as a JSON string in an inert script element with `<` escaped (`the_diagram_text_cannot_end_its_element`), never as markup; a diagram has no
network whatever the setting says. The call is answered only when the page says whether it drew, and a failure is `bad_params` with
Mermaid's own text cleaned and capped at 800 characters (`a_drawn_diagram_is_ok_and_an_error_carries_mermaids_words`,
`a_diagram_is_answered_by_the_pages_word_and_a_drawing_that_failed_says_why`, the e2e `diagram` and `mcpDiagram`). Replacing one page by
another keeps the browser environment (`a_page_replaces_a_mini_app_and_a_mini_app_replaces_a_page`, the e2e checks the browser process id).

199 (addendum, 2026-10-22). The sealed app's browser also resolves no name (`--host-resolver-rules="MAP * ~NOTFOUND"`), a WebView2 environment variable that would replace its arguments is removed and a policy value that does so refuses the app, an external protocol launch is cancelled and frames navigate only to blank, srcdoc or the app's page. Held by `an_overriding_policy_refuses_the_app_and_none_lets_it_start`, `a_sealed_app_browser_has_the_webrtc_policy_and_a_dead_proxy_a_networked_one_only_the_policy` and the e2e `miniApp` (unregistered-protocol navigation from page and frame, a listener on port 9 hears nothing).

## Surfaces: live preview, questions as notifications (stage 7; the agent board was removed)

202. **A Preview chip only ever offers a local address.** The host is compared whole against `localhost`, `127.0.0.1` and `[::1]`; a port (1 to 65535) is required; a user part, a space, a control, hidden or look-alike character refuses it; the address is checked again at the click and opened through the address bar's own normaliser. Output cannot make the chip open a remote page. The chip opens only the ORIGIN (`scheme://host:port/`; path, query and fragment are dropped in `parse_local`), the label and tooltip are that origin. Native: `flare_term::dev_server::parse_local`, `local_server_in`. Held by `a_remote_host_never_qualifies`, `a_port_is_required_and_must_be_a_real_one`, `a_confusable_or_hidden_character_is_refused`, `an_address_that_is_not_local_never_gets_a_chip` and the e2e `devPreview` (a remote line prints, nothing is tracked).
203. **A tab's screen is read for an address on demand, in bounded pieces, input excluded, and a clear is heard.** The finder reads the emulator's LIVE screen (its logical lines, wrapped rows joined; never the history, never the alternate screen, never the row the cursor is on) of any tab, at most once per 40 ms per tab and only after output arrived, so the cost is the screen's size whatever the volume; a different address on a row no older than the held one is news, the same one printed again is not; ED 2, ED 3 and RIS are heard in the bytes and drop the held address (rules 16 and 25). Held by the `ClearWatch` tests (every cut of a clear, strings skipped), `the_emulator_reads_a_local_server_address_off_its_screen`, `the_row_the_cursor_is_on_is_input_and_never_read`, `a_suggestion_painted_after_the_cursor_and_typed_over_is_not_read`, `an_address_that_scrolled_off_the_screen_is_still_the_one_held`, `a_cleared_screen_forgets_the_address_though_the_history_still_holds_it`, `a_long_address_wrapped_over_rows_is_read_whole`, `nothing_is_read_on_the_alternate_screen_and_no_remote_address_ever`, `a_reading_asked_for_too_soon_is_owed_not_lost`, `quiet_output_costs_nothing_here`.
204. **A chip needs a listener in the tab's own process tree and is looked up without connecting.** Every listener on the port (IPv4 and IPv6) must be the tab's shell or one of its descendants (`flare_win::tcp_owner::listening_in_tree`, `descendants`); a port held by any other process, or by a server outside the tree (WSL), gets no chip. The port is read from the system's TCP listener table (IPv4 and IPv6) on a worker, never by connecting to it; a port not listening is looked at again a few times then given up on, a shown chip is looked at every 2.5 s and goes after two failed looks in a row; one chip per tab for the newest address; a clear, a closed tab, an exited shell and the setting off take it. Native: `flare_surfaces::devpreview::DevChips`, `flare_win::tcp_owner::listening_in_tree`. **A foreign holder is not a miss, a click looks again, a stale chip stays hidden, an unreadable table is never trusted, a reused pid is no parent** (2026-10-22 review): a look answers nobody, in the tree or foreign (`PortLook`); a foreign holder drops the chip at once while waiting and while shown; a click opens the page only after a fresh look on a worker finds the SAME chip (token) in the tree, on the tab still in front; a tab coming to the front hides its chip until a look made after that answers; an IPv4 or IPv6 table that cannot be read (or a process list that cannot) answers unreadable, never in the tree; a parent link counts only when the child's creation time is not older than the parent's (and both are known). Held by `a_foreign_holder_takes_a_shown_chip_at_once_with_no_grace`, `a_foreign_holder_while_waiting_ends_the_wait`, `a_chip_coming_to_the_front_is_hidden_until_its_first_look_answers`, `a_look_already_out_when_the_tab_comes_forward_does_not_confirm_it`, `a_click_looks_again_and_opens_nothing_when_the_port_changed_hands`, `an_unreadable_table_never_reads_as_in_tree`, `a_reused_parent_id_does_not_pull_in_an_older_process`, `a_port_held_by_a_stranger_is_foreign_not_nobody`. Held by `descendants_include_the_root_and_every_depth_and_nothing_else`, `a_port_owned_by_another_process_is_not_in_the_tree`, `the_port_is_looked_up_against_the_tabs_own_shell`, `the_chip_shows_exactly_the_origin_and_drops_path_and_query`, `only_the_origin_is_kept_path_query_and_fragment_are_dropped`,  the `devpreview` tests (a scripted table), `a_port_is_listening_while_a_listener_holds_it_on_either_family`, the flow tests `the_chip_goes_when_the_process_that_printed_it_is_gone`, `clearing_the_screen_takes_the_chip_at_once`, `the_newest_address_is_the_only_chip_of_a_tab` and the e2e `devPreview` (a real listener, dropped).
205. **(RETIRED 2026-10-22, with the agent board (owner, 2026-10-22: "this is basically useless remove it").)** It said the board was the hub's view and cost nothing while closed. There is no board; the number stays so the others keep theirs. What replaced it: a stored `board` panel is dropped as an unknown kind (rule 40, `a_stored_agent_board_panel_is_dropped_on_load`) and `panel.open` kind `board` is refused.
206. **A question is one notification, and none while the person is looking.** One per question episode, none while the tab is in front AND the window has the keyboard, a question seen that way is never announced later, a 600 ms grace for its text, the setting off keeps a question quiet for good. The toast carries only escaped text (no action, no launch argument, no image) and nothing is registered to answer it after Flare quits; a toast Windows would not take flashes the taskbar button instead. Native: `flare_surfaces::notify::Notifier`, `flare_win::toast`. Held by the `notify` tests, `a_question_in_a_tab_out_of_sight_notifies_once_with_the_tabs_name_and_text`, `nothing_while_the_person_is_looking_at_that_tab`, `an_unfocused_window_notifies_even_for_the_tab_in_front`, `the_setting_turns_the_notification_off`, `a_notification_windows_would_not_take_flashes_the_taskbar_instead`, `nothing_in_a_title_or_a_body_can_become_markup` and the e2e `questionToast` (the recorder, never the desktop).


## Surfaces: mods (stage 8, the app part)

207. **A mod's page never holds a secret, and every call it makes is checked against the mod's live grant.** `window.flare` is frozen and holds three functions; the host looks the mod's grant up for every call (`unauthorized` once the mod is off), refuses what the manifest did not ask for (`forbidden`, before anything runs), refuses `hello` and `tools.respond` from a page, allows 16 calls of one mod at once, and runs an allowed call through the control channel's own dispatcher so there is one table of permissions. A mod opens only its OWN panel (`panel.open` of any other kind is `forbidden`: a web panel is the shared profile and the network) and never gets `allow_network` on `view.html`; a reply is given only to the document that asked (a generation per page load); a dead page is made again by the next call. A mod's own `browser.*` is `unavailable` in v1 (no second panel slot). Native: `flare_mods::bridge`, `ModCtl::on_page_message`. Held by `a_mod_opens_only_its_own_panel_and_never_gets_a_networked_page`, `after_the_page_dies_the_next_call_makes_a_fresh_page_and_is_answered`, `a_reply_meant_for_an_older_document_is_not_given_to_a_newer_one`, `a_mod_without_browser_is_refused_every_browser_method_with_forbidden`, `a_permission_the_mod_does_not_hold_is_forbidden_and_hello_and_respond_are_not_calls`, `a_mod_that_is_turned_off_is_refused_at_once_even_with_its_page_open`, `a_page_makes_only_so_many_calls_at_once`, the e2e `modsSample`. Source: the spec's section "Mods" and S5.
208. **A mod's files are served one request at a time, from the folder as it is NOW, with the host's policy on every answer.** No folder mapping; the path is checked again on every request (a junction or a swapped file serves nothing outside the folder), a file over 16 MB is 413, a miss 404, more than 8 reads at once 503; navigation goes only to the mod's own address, a new window is refused, and any request that is not the mod's own host, `data:` or `blob:` is refused 403. The browser is the mini-apps' sealed one, in a profile of the mod's own. Native: `flare_mods::serve`, `flare_surfaces::modview`, `flare_web::host`. Held by `a_junction_that_points_out_is_never_served`, `navigation_stays_on_the_mods_own_host`, `a_mods_requests_never_reach_the_network`, `a_mods_view_has_a_profile_of_its_own_and_is_the_only_one_a_message_reaches` and the e2e `modsSample` (an outside request blocked, a second page and an image of the folder served).
209. **A tool call has one owner, one answer and a bounded life.** `mods.call` routes by the agent-facing name to the one mod that offers it; only that mod answers, once (`not_found` after); the page gets the event once it has loaded and in order; the call fails at once when the panel closes or is replaced, the page's process ends, the view cannot be made, the mod is turned off or lost by a scan, or Flare closes; 8 calls may wait on one mod; after 30 s it is `timeout`, naming the mod and the tool. The wait is a worker's, never the UI thread's. Native: `flare_mods::hub`, `appctl::mods_glue`. Held by `a_mod_cannot_answer_a_call_that_was_routed_to_another`, `closing_the_panel_the_page_dying_or_the_mod_going_off_fails_the_waiting_call`, `an_unanswered_call_times_out_and_leaves_nothing_behind`, `a_mod_may_have_only_a_few_calls_waiting`.
210. **A switch takes effect at once and is saved off the UI thread.** Turning a mod on mints its token and offers its tools; turning it off kills the token, closes its panels, fails its waiting calls and tells the agent's clients (`mods.changed`), all in the same turn; the file is written on a worker with the state as it is when the write runs, a failed write is shown on the page and does not undo the switch, a scan never undoes a switch the file does not hold yet, and a token is never written anywhere. Native: `Hub::set_enabled`, `Hub::persist`. Held by `a_mod_is_off_until_turned_on_and_the_switch_mints_and_kills_its_token_and_tools`, `a_switch_that_cannot_be_saved_is_said_on_the_page_and_still_takes_effect`, `a_save_writes_the_state_as_it_stands_so_a_late_save_cannot_undo_a_later_switch`, `a_rescan_keeps_a_switch_the_file_does_not_hold_yet`, `the_snapshot_and_debug_never_carry_a_token`.
211. **Settings > Mods shows what a mod may do before it is on, and only what the manifest says.** The permissions are plain words (at most eight each), a folder that is not a mod is a warning with its reason, tools left out for a clash or length are listed, the page is empty where there are no mods, and a long word never changes a row's height. Native: `flare_settings::page::mods`. Held by `the_permissions_show_before_the_mod_is_turned_on_and_the_panel_button_waits_for_it`, `a_folder_that_is_not_a_mod_is_a_warning_row_with_its_reason`, `every_row_sits_whole_in_its_card_on_device_pixels_at_every_scale_and_long_words_do_not_grow_a_row` (100 and 225 percent).
212. **A mod runs with what the person allowed, never with what its manifest asks for today.** `mods.json` keeps the approved permissions with the switch (`{"notes":{"on":true,"permissions":["panel","tools"]}}`). A scan that finds the manifest asking for anything outside that set turns the mod off (token revoked, live connections refused, tools gone, panel closed) and Settings > Mods says it asks for new permissions; switching it on again approves the new list. Asking for less keeps it on with the smaller set (and the old list needs the person again). New tools under the same permissions need nothing. An on entry with no approved set (the old `"notes":true` form, a hand-edited one) reads as off. Native: `flare_mods::{hub, enabled}`. Held by `a_mod_that_asks_for_more_is_turned_off_and_its_token_dies`, `switching_it_on_again_approves_the_new_list`, `a_mod_that_asks_for_less_stays_on_with_the_smaller_set`, `tools_added_in_a_new_version_keep_the_mod_on`, `an_old_format_store_fails_closed`, `an_on_entry_without_an_approved_set_reads_as_not_enabled`, `a_mod_that_asks_for_new_permissions_says_so_under_its_off_switch`, and the e2e `modsSample`.
213. **A mod's page opens only its own panel, and `mods.call` always ends before the channel gives up.** `panel.open` from a mod page is `forbidden` for every kind but `mod:<its id>` (nothing is driven, the panel is not replaced); the hub's wait is whatever is left of `MOD_CALL_TIMEOUT` minus the margin, at most 30 s, so a silent page gets Flare's `timeout`. Held by `a_page_may_open_only_its_own_panel_not_another_kind`, `mods_call_ends_before_the_channels_own_timeout_whatever_the_window_took`.
214. **A page message counts for the mod whose page sent it, only while the tab shows that mod; a mod sees its own tab only; a switch approves the list that was shown; a call id cannot be guessed.** The host puts the host of the receiving view into the message (it names the mod); `WebCtl` drops it unless the tab's view is that host, and the window drops it unless the tab's panel and the controller's binding both name that mod, so a message queued by mod A's page runs with nothing after the tab moved to mod B. `tabs.list` gives a mod its own tab only (the `panel` permission never widened it). Settings hands what the plugin's details sheet lists with the switch (the permission words, and a plugin's author and homepage), whether the switch on the card or the sheet's Turn on was pressed (the card's switch no longer opens the sheet first: Max, 2026-11-12, rule 1188); `Hub::set_enabled_shown` refuses when the manifest found now says anything else, and the page draws again. A call id is `c` plus 128 random bits. Native: `flare_web::host`, `flare::web_ctl`, `flare::appctl::mods_glue`, `flare_mods::hub`. Held by `a_message_from_the_page_the_tab_left_is_not_run_with_the_mod_it_shows_now`, `a_mod_sees_its_own_tab_only_whatever_it_may_do`, `a_switch_approves_the_list_the_row_showed_and_nothing_else`, `a_switch_refuses_a_plugin_whose_maker_changed_since_its_card_was_drawn`, `call_ids_are_long_and_never_repeat`, `the_switch_reaches_the_mods_port_and_the_page_draws_what_the_port_then_says`.

## Surfaces: owner feedback, round 9

215. **"Show in side panel" shows only what a left click would preview, vetted the same way, and opens nothing in Windows.** The row exists for a file whose name is a picture or text kind the panel draws (never a folder, script, program, svg, pdf or page); picking it runs the click's worker (the backend resolves the path again, both names must be of the same previewable kind, local drive only) and a path that stopped qualifying does nothing, not an open. It is an explicit ask, so "Open previews in Flare" does not stop it. Native: `term_menu_rows`, `TermUi::preview_path_async`, `TermMsg::Preview { asked }`. Held by `show_in_side_panel_follows_open_for_a_file_the_panel_draws`, `show_in_side_panel_is_offered_only_where_a_left_click_would_preview`, `show_in_side_panel_on_a_text_file_menu_opens_the_preview_and_opens_nothing_in_windows`, `show_in_side_panel_works_even_when_plain_clicks_open_files_in_windows`, `show_in_side_panel_on_a_picture_menu_and_never_for_a_script_or_a_folder`.
216. **Wrapped text stays virtual, loses no character and numbers a line once.** One row is one line tall; the window builds only the rows in view whatever the file (300 000 rows draw in milliseconds); the rows joined are the lines; the gutter shows a line's number on its first row only and the header counts lines; nothing is drawn past the right margin while wrapped. Native: `flare_surfaces::wrap`, `panel_ctl::wrap_lines`, `TextView`. Held by `no_character_is_ever_lost_or_split_in_two`, `a_500_character_line_wraps_into_several_rows_and_none_is_wider_than_the_columns`, `the_gutter_shows_a_number_on_the_first_row_of_a_line_only`, `wrapped_rows_stay_virtual_only_the_ones_in_view_are_built`, `nothing_is_drawn_past_the_right_edge_of_the_slot_when_wrapped` (100 and 225 percent).
217. **A text preview wraps live while its width moves, the final wrap is exact, a stale wrap is dropped, and a drag is never stalled.** Every change of the view's columns (a splitter step, a window-edge step) wraps to the columns now and keeps the line at the top; under 200 KB of shaped text where the step lands (a refresh while the width moves is live too, `wrap_moving`; no worker answer wraps during a sidebar slide), a bigger file on a worker with one wrap in flight whose answer wraps again to the newest width; `settle_wrap` is the exact final wrap; a sidebar slide wraps once at its end; the terminal grid still fits once at the release (rule 24). A worker's answer for a wrap that is no longer the newest changes nothing. Held by `a_splitter_drag_wraps_live_at_every_step_and_the_release_changes_nothing_more`, `a_live_drag_keeps_the_line_at_the_top_in_place`, `a_big_file_drag_never_stalls_the_ui_and_ends_exact`, `a_small_file_wrap_per_step_is_cheap`, `a_window_edge_drag_never_wraps_a_mid_file_inline_nor_restarts_the_worker`, `a_big_file_wraps_on_a_worker_and_a_stale_answer_changes_nothing`, `a_long_line_wraps_by_default_and_the_header_button_turns_it_off_and_on`, `with_the_setting_off_a_file_opens_unwrapped_and_the_settings_switch_flips_it_too`, the e2e `panelSplitter`.
218. **The splitter holds its invariants under any hand.** Whatever the steps (quick, slow, shaky, past either limit and back, at 100 or 225 percent, over a picture or text): the drawn slot and the splitter stay inside the limits and on the slot's edge, the slot follows the pointer to the device pixel and never against it, the grid keeps its width and is not fitted while the button is down and is fitted exactly once on the release (rule 24), the settled slot is the dragged one, the grid ends where the slot starts, the panel's content is on every frame, and what shows beside a narrower slot is the terminal's ground. Held by `appctl::tests::splitter_tests` and the e2e `panelSplitter`.
219. **A splitter drag outlives the pointer leaving the window, and ends on the left button going up or the slot going away.** The window's hook holds back winit's `CursorLeft` while a drag runs (the OS keeps the pointer captured, so moves and the release still arrive from outside the window; the held leave is owed after the release), another button's release does not end it, and the left button up ends it wherever the pointer is (`AppWindow.end-splitter`). A cancelled grab, a tab switch or the slot going away still lets go and keeps the share dragged so far. The hook's CursorLeft hold, its CursorEntered clear and its swallowing of other buttons mid-drag are checked by hand (they live in the winit hook, which no harness drives). Held by `another_button_going_up_mid_drag_does_not_end_it_and_the_left_up_does`, `leaving_the_window_during_a_drag_lets_go_of_the_splitter` (a cancelled grab) and the e2e `panelSplitter`.
220. **A quitting instance takes no hand-off.** The end of life stops the hand-off server before it kills a single shell,
so a second launch finds no pipe, waits for the lock and becomes the first instance; a message that was written and
acknowledged would be dropped. A message over the caps is split by the sender (each part keeps the exe), the server
refuses with a log line, and a client that connects and never writes is cut off after 2 s so one stalled process cannot
jam the pipe. Native: `appctl.rs` (`finish`, `set_handoff_stop`), `single_instance.rs` (`split_handoff`,
`read_message_within`). Held by `the_end_of_life_stops_the_handoff_server_before_anything_slow`,
`a_stopped_server_refuses_a_handoff`, `a_big_handoff_is_split_under_the_caps_and_loses_no_argument` and
`a_stalled_client_does_not_jam_the_server`. Source: general review 2026-10-23 (app-1, app-5, platform-5); rule 32.
221. **Replaced by rule 1154 (2026-11-11).** Was: a zoom resizes one tab. Ctrl+wheel zoom, and a switch between a zoomed and an unzoomed tab, refit only the tabs
that wear the settings' size. A tab with a zoom of its own keeps its grid until it comes to the front, so no hidden shell
is reflowed. Native: `tab_host.rs` (`refit_unzoomed`), `appctl.rs` (`apply_font`). Held by
`a_zoom_never_resizes_the_other_shells_and_a_switch_keeps_each_zoomed_size`. Source: general review 2026-10-23 (render-2).
222. **An agent is a program or a package, never a word.** A command line is an agent only when its FIRST token is the
program (`claude`, `codex`, `agy`, `aider`, `gemini`, with or without a path and `.exe`, `.cmd`, `.ps1`), an argument is such a
program file, or it holds a package signature (`@anthropic-ai/claude-code`, `@openai/codex`, `@google/gemini-cli`, or the
agent's own folder followed by its entry file). A folder called `claude`, `claude-code-ui`, `gemini` or `agy`, or a word like `legacy`, never matches. A
process older than its listed parent (a recycled pid) is not counted under a shell. A change of kind under a present agent
reaches the tab. Native: `detect.rs` (`classify`), `procs.rs` (`born_after`), `poll.rs`. Held by
`a_path_or_word_in_the_arguments_is_not_an_agent`, `a_process_older_than_its_listed_parent_is_not_its_child` and
`a_change_of_kind_under_a_present_agent_is_sent`. Source: general review 2026-10-23 (agent-1, agent-2, agent-4).
223. **A store file that does not parse is kept, never overwritten.** Text that is not valid JSON is set aside as
`<file>.bad` before the first save, a leading BOM is dropped, and a quit waits (at most 2 s) for a save already in the
background writer. A blocked log rotation never shifts the history (the live file moves first). Native: `atomic.rs`
(`open_text`), `debounce.rs` (`flush`), `writer.rs` (`rotate_if_due`). Held by
`a_bom_is_dropped_and_broken_text_is_kept_as_bad`, `flush_waits_for_a_save_already_in_the_background_writer` and
`a_blocked_rotation_that_retries_never_costs_an_older_generation`. Source: general review 2026-10-23 (app-3, app-4,
platform-3); rule 41.

## Dictation: words that cannot be sent

224. **Dictated words that cannot be sent are kept, never dropped** (owner, 2026-10-23: the owner dictated 1m42s from
outside Flare, no Claude tab was open, the note said "No Claude session to send to" and the words were gone). Every
finished transcript that ends unsent is copied to the clipboard and the widget note says so: no Claude session, Esc or
the 30 s timeout on the picker, a refused or failed checked send, a copy of the app that has closed. A picker that is
dismissed, a new recording that replaces words still waiting, a newer text over an older one, and switching dictation
off with words waiting copy them too, with no note. "Cancelled", "Heard nothing", a paste and a successful send copy
nothing; a send whose words are in the prompt but whose Enter did not send them copies nothing either ("Typed into X,
press Enter there", rule 1142). A copy that fails puts the plain note back only while the exact "copied" note is still showing; a silent copy that
fails says "Words could not be copied" when nothing else is on screen. A paste into this copy's front tab with no shell
in front (Settings, an empty window) goes on to the list and copies, never claiming "Typed into the tab in front"
(`SvcEffect::PastePage` is answered with `Pasted`); words another copy routes here with no shell in front are copied by
that copy (the link has no reply for a paste, so the service copy still says typed: a follow-up). An in-window
recording whose tab closed copies the words (`Effect::Keep`, "That tab has closed, copied"), as does a paste that finds
the tab gone. A send carries its words with its answer (`SvcEvent::Sent(decision, text)`), so two sends in flight never
swap words; a send that cannot start answers No with its words; a refusal that lands after dictation was switched off
still copies. The new-recording and switch-off paths overwrite the clipboard on purpose (owner to confirm). Native: `SvcEffect::CopyText` in `service.rs` (the clipboard write runs
on a worker through `DictHost::copy_text`, `dictation_ctl.rs`). Held by `nothing_with_no_claude_session_says_so`,
`esc_keeps_it_unsent`, `thirty_seconds_without_a_pick_says_not_sent`, `a_click_picks_a_row_and_a_refusal_says_why`,
`a_tab_of_another_copy_with_no_service_is_not_sent`, `a_remote_send_that_is_refused_keeps_the_words`,
`a_dropped_paste_round_trip_asks_for_the_tabs_and_keeps_the_words_when_none_can_take_them`,
`dismissing_the_picker_keeps_the_words_without_a_note`,
`a_new_recording_keeps_the_words_an_earlier_one_was_still_waiting_on`,
`a_newer_text_never_loses_the_words_of_one_still_waiting`, `switching_off_with_words_waiting_keeps_them`,
`a_note_with_the_copy_stays_under_the_cap`, `cancelled_heard_nothing_and_a_successful_send_or_paste_copy_nothing`
(`service.rs`), `words_with_no_session_are_copied_and_the_note_says_so`, `a_failed_copy_puts_the_old_note_back` and
`a_failed_copy_never_replaces_a_newer_face`, `a_failed_silent_copy_says_so_only_when_nothing_else_is_showing`,
`a_route_to_this_copy_with_no_shell_in_front_keeps_the_words_and_never_claims_a_paste`,
`words_routed_here_by_another_copy_with_no_shell_in_front_are_kept`,
`a_refused_send_keeps_its_own_words_and_one_that_lands_after_switch_off_too` and
`a_remote_send_with_no_machinery_answers_no_and_keeps_the_words` (`dictation_ctl.rs`), plus
`a_paste_with_no_shell_in_front_goes_on_to_the_list_and_never_claims_it_typed` and
`two_sends_in_flight_each_copy_their_own_words` (`service.rs`). Quitting the app while words wait is not covered: the
process is ending.

## Agent claims and background work

225. **A claim is checked once, never on a timer.** An OSC 777 `flare-agent` signal or a Claude title can mark a tab
before the process poll has seen an agent there. The poll sends only changed answers, so a tab it already reported "no
agent" would keep a false claim (a Claude icon, a Question) until the tab closed. Each claim that asks the poll to look
also calls `PollWorker::again(tab)`: that tab's next answer is sent even if unchanged, and a "no agent" answer takes the
claim back. One check per claim (a second claim inside the 15 s gap asks for nothing), and no new timer or polling loop
(owner, 2026-10-23: no scheduled checking scripts). An e2e that sends hooks needs a REAL agent process the poll
recognises (agentHooks runs a copy of PowerShell named `claude.exe` in the tab), or the forced answer rightly clears
its Working. A tab the poll cannot see into (a WSL distro) is never asked about: its claim stays, as `arm.rs` keeps a
titled agent there (`a_wsl_tab_keeps_its_claim_because_no_one_asks_the_poll_about_it`; ssh from a Windows shell is not
covered yet). Native: `indicator.rs` `Effect::AskLook { tab }`, `agent_hub.rs` `run_effects`. Held by
`a_claim_with_an_agent_is_kept_by_the_look`,
`a_second_claim_inside_the_gap_asks_for_no_second_look`,
`a_hook_claim_asks_one_look_by_name_and_a_no_agent_answer_takes_it_back` and the e2e agentHooks check on a cat'ed
capture. Source: PT general review #47 (owner, 2026-10-23, "one check per claim").
226. **A footer shell count is not background work; a Stop with only that is Finished.** Claude's `· N shell(s)` after
the mode line (a run_in_background dev server can sit there for hours) no longer holds a tab Working at Stop: the turn
raises Finished and the close question follows the normal rule for an idle agent. Still Working: a workflow's progress
row (`7/8 · 11m03s`) and Claude's own `· N shells still running` summary line above the prompt box, which it writes while
it actually waits. Supersedes the footer-count line of the earlier background rule (PT `agentBackground.ts`, parity.md).
Native: `background.rs` (`looks_like_background_work`). Held by `a_dev_server_in_the_footer_alone_is_not_work`,
`a_stop_with_only_the_footer_shell_count_raises_finished`, `a_stop_with_a_workflow_progress_row_stays_working`,
`a_stop_with_claudes_own_still_running_summary_stays_working` and the e2e backgroundWork. Source: PT general review
#45 (owner, 2026-10-23, "Finished at turn end").
227. **Win32 stays in `flare-win` or behind `cfg(windows)`.** Shared logic must compile on macOS: Windows-only crates
(`windows`, `windows-sys`, `webview2-com`, `embed-resource`) sit under `[target.'cfg(windows)'.dependencies]`, and
`std::os::windows` imports, Windows paths and Windows tools live in Windows modules with a macOS counterpart or stub.
Held by the `macos` CI job (cargo check, clippy and nextest on macos-latest). Source: macOS port plan
(`docs/superpowers/specs/2026-11-08-macos-port.md`, owner, 2026-11-08).
228. **A click by ref that would land on something else is not pressed, and does not say it was.** The agent's `browser_click` answered "Clicked" over a cookie wall, and the agent believed it. Native: after the glide and right before the mouse events, one fixed script asks the page what is at the point (`HIT_TEST_FN`, `document.elementFromPoint` through open shadow roots, no page or agent text in it); the element there must be the target, inside it, a label of it, or a wrapper of it other than `html` and `body`. Otherwise the call answers `covered` and the agent is told what is on top, as page text between the markers. A page that cannot say is clicked as before, so an old page never loses a click. Held by `a_covered_click_presses_nothing_and_names_what_is_on_top`, `a_covered_click_lets_go_of_the_element_it_held`, `a_click_on_the_target_or_a_child_of_it_is_pressed`, `a_page_that_cannot_answer_the_check_is_clicked_as_before` (`agent_job.rs`), `the_covering_element_is_named_briefly_and_cleaned` (`agent_web.rs`), `a_hostile_covering_element_cannot_end_the_block_or_pose_as_an_element` (`browser.rs`), `a_covered_click_is_refused_and_presses_nothing` (`web_tests.rs`) and the e2e `agentCoveredClick`.
229. **A cancelled tool call stops, on both sides.** `flare-mcp` reads stdin on its own thread, so
`notifications/cancelled` is seen while a call runs: the call gives up and drops its connection, a queued request never
runs, and a cancelled request gets no response. The control channel keeps reading a connection while its call runs, so
the drop raises `CallContext.cancel`, and a call that waits (a browser job, a page load, a mod's answer) ends on it.
Replies stay in order, an unknown or finished id does nothing, stdin closing still ends the server. Held by
`a_cancel_during_a_call_ends_the_job_on_flares_side_and_the_next_call_still_works` (the exe over a real pipe),
`a_client_that_goes_away_mid_call_raises_that_calls_cancel_flag`,
`a_job_whose_caller_went_away_ends_and_the_page_is_free_for_the_next_call` and
`a_mod_tool_call_ends_when_the_caller_goes_away_and_the_late_answer_is_refused`. Source: channel-9
(`docs/surfaces/lessons.md`, owner, 2026-11-09).
230. **A web page keeps its width while the splitter beside it is held.** A new width makes the page lay itself out again,
so a resize at every pointer step flickers. During the drag the left edge follows the slot and the width stands still; the
page takes the slot's exact size after 120 ms without a change and when the grab ends for any reason (release, cancel, tab
change, closed panel, full screen). A window resize is not held. Native: `flare_surfaces::web::DragHold`, `web_ctl.rs`
(`drag_rect`). Held by `a_held_splitter_moves_the_pages_edge_but_never_its_width_until_it_is_let_go`,
`a_pause_in_a_splitter_drag_fits_the_page_once_and_the_next_step_holds_again`,
`a_splitter_grab_that_goes_away_leaves_the_page_on_the_slot`,
`a_panel_closed_during_a_splitter_drag_comes_back_at_the_slots_size` and
`a_window_resize_moves_the_page_at_every_step_with_no_hold`. Source: the note left in `docs/surfaces/lessons.md` Round 10.
231. **A warm spare answers ConPTY's opening cursor question itself.** A spare has no tab, so the question used to wait in its banner and be
answered when a tab adopted it. By then PowerShell 7 was at its prompt, and the late answer cost the next key: the first paste into a new plain tab
lost its first letter with a beep, and the second was fine. The spare now answers `ESC[6n` with `ESC[1;1R` when it is the first thing asked, before
anything is drawn, and removes it from the banner so the tab does not answer again. A question after output waits for the tab, which has a real grid.
Measured with real shells on 2026-11-09: a late cursor answer alone loses the letter (`cho firstpaste`); a late device attributes answer does not.
Native: `flare_term::spare_opening`, `Session::answer_opening`. Held by `the_question_is_found_behind_the_sequences_conpty_sends_first`,
`anything_drawn_before_the_question_ends_the_opening`, `a_spare_answers_the_opening_cursor_question_once_and_the_tab_is_not_asked_again`,
`a_spare_answers_only_the_opening_question`, `a_cursor_question_after_output_waits_for_the_tab_that_can_answer_it` and
`a_shell_started_for_a_tab_leaves_the_question_to_the_tab`.
232. **A file Flare's viewer shows is vetted like a click, but is not on the click allowlist.** `view.model` resolves the path again in the backend (rule 13) and needs the extension as typed and as the file system names it (rule 14), through `path_open::resolve_file_of` with the viewer's own list. `.glb`, `.gltf` and `.stl` must stay off `OPENABLE`: a click opens with the Windows default verb, and `.stl` is a certificate list to Windows. The page is Flare's: its policy is first, its only script is the pinned bundle beside it (no eval, no WASM, no worker) and `connect-src 'self'` is the only thing it can fetch, so three's blob: and data: fetches are routed around in `tools/model-viewer/src/viewer.js` rather than widening the policy. Native: `flare_surfaces::model`, `AppDispatcher::view_model`, `assets/three/viewer.js`. Held by `the_policy_comes_first_and_lets_in_only_the_viewer_and_its_model`, `a_model_file_is_known_by_both_its_names`, `view_model_refuses_what_is_not_a_model_a_link_a_share_and_a_file_over_the_cap`, `the_real_model_resolver_is_rules_13_and_14_with_the_viewers_extensions`, `the_bundled_model_viewer_is_the_pinned_build` and the e2e `model`.
233. **An STL's normals are worked out from its facets, never taken from the file.** three's `STLLoader` always sets a `normal` attribute, copied from the file, and STL files commonly leave it at 0,0,0, so a check for a missing attribute never runs and the shader normalizes a zero vector: the part draws as one flat silhouette, grey or black by GPU, with `{ok: true}` and no message. `readStl` calls `computeVertexNormals` always; the geometry has no index, so facets stay flat, and the corners' order is what decides the drawn side anyway. Native: `tools/model-viewer/src/viewer.js`, built into `assets/three/viewer.js`. Held by the e2e `model` (`is shaded, not a flat silhouette`, binary and ASCII), whose STL pyramid points at the opening camera: seen from that camera the plain pyramid shows one face, which is flat with or without normals.
234. **"Ask agent" types at the prompt and never submits, and a page cannot make it do more.** A person's gesture only (the panel's button, the
terminal menu's row): no ctl, MCP or plugin way in (rule 188). The text passes one gate, `ask_agent::frame_selection`: invisible characters and controls
gone (the C1 range too), at most 4 KB with a visible `> [cut]`, and framed `About this from <label>:` then `> ` on every line, Unicode line breaks
included, so a page's line cannot pass for the frame. It goes with `TermHandle::paste_unsent`: bracketed when the program asked; when it did not, a CR
inside the text would run that line, so the text goes as one line and no CR is written at all. Refused with a note when the tab has no live agent or a
question box is up (the hooks, the watch or the screen itself: a paste must not answer it), checked at the click and again when the page answers. A
page's answer from an earlier document, a closed panel, or a tab no longer in front is dropped. Native: `crates/flare/src/ask_agent.rs`,
`AppCtl::deliver_ask`, `WebCtl::ask_selection`. Held by `a_page_selection_is_pasted_bracketed_and_nothing_follows_it`,
`without_bracketed_paste_the_framing_goes_as_one_line_with_no_cr_at_all`, `a_hostile_selection_cannot_end_the_paste_or_press_enter`,
`an_answer_from_before_a_navigation_is_dropped`, `a_question_box_on_screen_refuses_so_a_paste_cannot_answer_it`,
`a_tab_with_no_agent_refuses_before_the_page_is_asked` and `a_line_of_the_page_cannot_pass_for_the_frame`. Source: issue #33 and
`docs/superpowers/specs/2026-11-09-surfaces-later-design.md`.

235. **"Ask agent" names the document its text came from, and a page's answer is bounded natively.** `t.url` and `t.doc` move at `NavigationStarting`,
while the old page still shows and is what `ExecuteScript` runs in, so the frame's label must never come from the tab's address: `SELECTION_SCRIPT` returns
`location.href`, one line break, then the title (rule 238), another line break and the text, and `WebCtl::selection_answered` labels with that first line (`location` is unforgeable and a URL has no
line break, so the text cannot pass for the address). The script's own 65536 cut runs in the page's world, where a page can replace `slice` or
`getSelection`, so `ask_agent::page_answer` refuses anything over `ANSWER_MAX` before it is parsed or copied, and the person sees the page-failed note. Held
by `a_click_while_the_next_page_loads_names_the_page_the_text_came_from`, `an_answer_longer_than_the_script_could_give_is_refused_and_types_nothing`,
`only_the_first_line_of_an_answer_is_the_address_and_only_the_second_the_title` and `the_address_comes_first_and_is_read_with_nothing_a_page_can_replace`. Source: review of issue #33, 2026-11-09.
236. **A hover thumbnail asks what is under the cell every tick, and is vetted like a click.** A stored key cannot know the screen moved under a pointer that never did, so `TermUi::thumb_tick` calls `link_at_view` again on every tick while one is waiting, loading or up, and once more when a worker's answer lands; the key is the tab, the first row on the screen and the mark, so a scroll is a change even under a long wrapped path. A worker resolves the path again with `resolve_openable_file`, then `thumb_target` asks `classify_resolved` for an image and `loadable` (rules 13, 14, 34). Only `LinkTarget::Path` with a picture's name ever starts a rest, so a web address is never fetched. The worker shares the panel's `DECODE` lock, runs one job with only the newest waiting, and has `catch_unwind`. The setting off means `TermUi.thumbs` is `None`. Native: `crates/flare/src/thumbs.rs`, `term_ui.rs`, `preview::decode_thumb`. Held by `another_picture_scrolled_under_the_cell_is_not_shown_as_the_old_one`, `scrolling_under_a_still_pointer_takes_the_picture_down_on_the_next_tick`, `an_answer_for_a_link_that_scrolled_away_while_it_loaded_never_goes_up`, `the_vetting_is_a_clicks_for_a_network_path_a_nul_and_a_link_that_changes_kind`, `a_web_address_of_a_picture_is_never_fetched_on_hover`, `only_the_newest_job_waits_behind_a_running_one` and `with_the_setting_off_a_rest_starts_no_state_and_no_worker`.
237. **(REPLACED 2026-11-11 by rules 266 to 269: there are no shown folders and no questions; a file link is a path between marks.)** It said: a question still open about the tab's folder does not hide what a shown folder already answered. A relative path can be a link only through a folder the tab has shown, while the tab's own folder said "no". That "no" runs out after `NO_TTL_MS`, the line has a hit so it is never scanned again, and `find_path` never asks: `find_inner` returning `Unknown` at that point left a lit link with no thumbnail and a click that did nothing, on every later try. It now reads the shown folders' cached answers first and still returns `Unknown` when none has the path. Only the questions wait for the tab's folder, so a path that is there never costs one ask per shown folder. A full path has no shown folders. Native: `flare_term::path_links::find_inner`. Held by `a_path_found_from_a_shown_folder_stays_found_after_the_no_for_the_tabs_folder_runs_out`, `the_shown_folders_are_asked_only_after_the_tabs_own_folder_says_no`, `a_no_that_ran_out_is_unknown_when_no_shown_folder_has_the_path`, `a_full_path_has_no_shown_folders_to_fall_back_on`, `resting_on_a_picture_found_through_a_shown_folder_shows_it_after_the_no_for_the_cwd_ran_out` and `a_click_on_a_path_found_through_a_shown_folder_opens_it_after_the_no_for_the_cwd_ran_out`.
238. **"Ask agent" with no selection on a plain web page sends a reference to the page the answering document names, never the tab's.** `SELECTION_SCRIPT` returns `location.href`, a line break, `document.title` (a non-string, such as a named element replacing it, counts as none), a line break, then the selection, so the address and the title are the same document's, read in one answer. `ask_agent::page_answer` takes only the first line as the address and only the second as the title; a title a page forged with a line break only adds selection text, which is quoted as such. The title is untrusted: `frame_page` gates it like the selection, puts it on one line and caps it at 160 characters. The address loses any user name and password and is capped at 2 KB with `[cut]`, and only an `http` or `https` address is a page; anything else keeps the note. `WebCtl::selection_answered` carries the page only when the tab has no `app_host`: a mini-app or plugin page keeps the "select some text" note, since its address means nothing to an agent. A plain page whose answer is not in the script's shape names no page and says the page-failed note. The frame (`About the page open in this tab (<host>):`, `> title:`, `> address:`, `> (use browser_look to read it)`) is typed by `deliver_ask` like a selection: same gates, never submitted. The button's tooltip is one fixed wording per kind, with no selection polling, placed beside the button so a page's native window cannot cover it. Native: `crates/flare/src/ask_agent.rs`, `WebCtl::selection_answered`, `AppCtl::ask_selection_done`, `AskAgentButton` in `ui/panel.slint`. Held by `no_selection_on_a_web_page_types_a_reference_to_the_page_and_no_enter`, `the_title_is_the_answering_documents_not_the_tabs`, `a_click_while_the_next_page_loads_refers_to_the_page_that_answered`, `a_hostile_title_cannot_end_the_paste_press_enter_or_pass_for_the_frame`, `no_selection_on_a_mini_app_keeps_the_note_and_types_nothing`, `a_hostile_title_stays_one_clean_quoted_line`, `a_user_name_and_password_never_reach_the_prompt`, `only_a_web_address_is_a_page_the_agent_can_open` and `ask_agent_says_on_hover_what_a_click_sends_with_one_fixed_wording_per_kind`. Source: `docs/superpowers/specs/2026-11-10-ask-agent-additions-design.md`, item 1.
239. **A markdown page is written by Flare, tag by tag, and nothing in the file reaches it as markup.** A `.md` or `.markdown` file is untrusted: `flare_surfaces::mdview` takes pulldown-cmark's events (the crate pinned with no HTML writer) and writes every element itself, escaping every piece of text and every attribute. Raw HTML in the file shows as text, a picture shows its alt text (no `<img>`, nothing read from disk or the network), and a link has no `href` (its target is a `title` tooltip) unless it is a `#` jump to a heading or a footnote whose anchor Flare named. The page has no script at all and its policy is the first thing in it: `default-src 'none'; style-src 'unsafe-inline'; img-src data:` with `base-uri` and `form-action` off. It is served as a mini-app of its own (`app::write_app`, a host and folder from the shared `app::next_seq`), in the sealed mini-app browser with the network off. Depth (`DEPTH_MAX`) and size (`PAGE_MAX`) are capped, and a render on its worker stops when the generation moves on (rule 34), the naming of heading anchors included. Anchors of one name are numbered by a counter kept per name (`Names`, as GitHub's slugger), never by trying `-1`, `-2` and so on from 1 each time: a 4 MB file of `#` lines is two million headings named `section`, which took n squared steps and held its worker for good. Native: `crates/flare-surfaces/src/mdview.rs`, `PanelCtl::spawn_render`. Held by `raw_html_in_the_file_shows_as_text`, `a_link_has_no_address_and_shows_its_target_as_a_tooltip`, `a_jump_to_a_heading_flare_named_works_and_any_other_hash_does_not`, `a_picture_shows_its_alt_text`, `a_code_fence_language_cannot_break_out_of_its_attribute`, `the_policy_comes_first_and_lets_in_no_script_and_no_request`, `deep_nesting_writes_no_more_than_the_cap_of_boxes`, `a_render_stops_when_the_panel_moved_on`, `many_headings_of_one_name_get_their_anchors_in_a_time_that_grows_with_the_file`, `naming_the_headings_stops_when_the_panel_moved_on` and `a_markdown_file_opens_compiled_as_a_page_flare_wrote`. Source: `docs/superpowers/specs/2026-11-10-ask-agent-additions-design.md`, item 4.
240. **A markdown file opens Compiled every time, its panel keeps its own view, and a page that cannot show falls back to Code.** Owner, 2026-11-10: "compiled by default", with no remembered global preference. `TabPanel.view` is None until the header's control picks a view; `TabPanels::show` (a click, `show_file`, a restore of another file) starts at None again. Only a picked view is written to `panels.json` (`"view"`), and a value this build does not know reads as None and is never written back (rule 40). When the render fails (a missing file, a write that failed, no worker) or the page's browser fails (no WebView2 runtime, a renderer that died), `PanelCtl::fallback` shows that file in Code until the person picks a view or another file shows: the panel is never left blank. A markdown page is not a web page: `web_target` stays None (the browser tools, fill and the address bar never see it), `page_target` is what the web controller shows, and a mini-app panel or a mod is never written to `panels.json` while a markdown panel is. Native: `PanelCtl::view_of`, `set_md_view`, `page_failed`, `AppCtl::md_page_fallback`. Held by `a_picked_view_stays_with_the_file_and_a_new_file_opens_at_the_default`, `a_markdown_panel_keeps_its_view_and_only_a_picked_one_is_written`, `an_unknown_view_reads_as_the_default_and_is_never_written_back`, `the_picked_view_is_written_and_comes_back_after_a_restart`, `a_file_shown_again_opens_compiled_again`, `a_page_that_cannot_be_shown_falls_back_to_code`, `a_page_whose_browser_fails_later_falls_back_to_code`, `a_render_that_fails_falls_back_to_code_and_says_why_there` and `the_header_control_switches_between_the_three_views`.
241. **"Ask agent" on a markdown page names the file, never the page's address.** The page is served from a `flare-app-<n>.local` address that means nothing to the agent: `AppSpec.source` carries the file, the web controller keeps it per tab, and `WebCtl::selection_answered` labels the frame with the file's name. With nothing selected, the file's path goes as it does from the Code view. Native: `web_ctl.rs`, `appctl/sessions_glue.rs`. Held by `ask_agent_on_a_markdown_page_names_the_file_and_sends_its_path_with_nothing_selected`.
245. **A plugin fill's dialog has Deny and Fill and nothing under them.** The owner took out "Deny for this session" and the line "<Plugin> fills it into the page; the agent never sees it." (2026-11-10). Each Deny answers one fill, `browser.fill` never answers `for_session`, and the plugin's next fill asks again; Fill stays pointer-only and unarmed for its first 0.7 s, Enter and Space press Deny, and six asks a minute per plugin still holds. A secret's question keeps its Cancel link and Flare's line naming the plugin. Native: `ui/plugin_ask.slint`, `plugin_ask::fill_view`, `AppCtl::answer_fill`, `flare_mods::fill::Asks`. Held by `deny_says_no_and_the_next_fill_asks_again`, `the_fill_question_names_the_login_the_site_the_tab_and_the_plugin` (an empty foot) and the e2e `pluginFill`.
242. **A selection in the Text panel is held in Rust as places in the file's own lines, never in Slint.** The panel draws only the rows in view and recycles them, so a read-only `TextInput` per row cannot hold a selection across rows. `panel_sel::TextSel` keeps two `Pos` (a line and a character of the shaped line) and `RowMap` says which rows a line was wrapped into (`wrap_line` made again at the same columns as `panel_ctl::wrap_lines`), so the selection survives scrolling, wrapping on and off and a splitter drag that wraps again. Slint reports the pointer in the content's coordinates and paints each row's share from five properties (`panel-sel-*`); the rows between the ends are drawn whole and a line's end as half a column. The text is the shaped text the panel shows (tabs as four spaces, a long line cut), soft-wrapped rows of one line join with no break and only real line ends become `\n`. Columns count a wide character or an emoji as two, as the wrap does. The selection goes when the generation moves on in `PanelCtl::sync` (another file, a reload, another tab, a markdown page view) and when nothing shows. Native: `crates/flare/src/panel_sel.rs`, `PanelCtl::text_down`, `push_sel`, `TextView` in `ui/panel.slint`. Held by `panel_sel::tests` (`wrapped_rows_map_back_to_the_line_and_character_they_show`, `the_selection_is_kept_in_lines_so_wrapping_on_and_off_keeps_it`, `a_wide_character_takes_two_columns_and_an_emoji_too`, `soft_wrapped_rows_of_one_line_join_without_a_break`), `turning_the_wrap_on_and_off_keeps_the_selection_on_the_same_text`, `wide_characters_and_emoji_are_selected_by_the_columns_they_take`, `the_selection_goes_with_a_reload_another_file_or_another_tab` and `the_selection_is_drawn_on_its_rows_and_only_there`. Source: `docs/superpowers/specs/2026-11-10-ask-agent-additions-design.md`, item 3.
243. **In the Text panel a mouse drag selects, the wheel still scrolls, and the panel's keys are its own.** The `Flickable` has `mouse-drag-pan-enabled: false` (a touch still pans); Slint 1.18's `Flickable::accepts_pan_event` takes a wheel whatever `interactive` and that property say, so the wheel, the keys and the scrollbar scroll as before. `panel.slint` holds no `Timer` (`the_panel_files_use_no_animation_and_no_timer`), so a drag held past an edge is scrolled from Rust: `panel_text_move` starts the `panel.sel-scroll` timer every `DRAG_SCROLL_MS`, `PanelCtl::drag_tick` scrolls by the terminal's `drag_scroll_amount` and extends to the text under the still pointer, and the release, a dropped selection or a pointer back inside stops it. Esc lets a selection go first and the panel keeps the keys; with nothing selected the keys go back to the terminal as before. While the panel has the keys the agent guard is off, so Ctrl+C there is a copy (on a worker, `COPY_MAX`, the "Copied" badge) and never the guard's; Shift with the arrows, Home and End, Ctrl+Shift+Home and End, and Ctrl+A are taken in `AppCtl::panel_text_key` before Slint, every other key still reaches the panel. Native: `appctl/text_sel_glue.rs`, `AppCtl::on_key`. Held by `a_drag_over_the_text_selects_and_never_scrolls_it_and_the_wheel_still_does`, `a_drag_held_below_the_view_scrolls_and_the_selection_follows`, `esc_lets_the_selection_go_first_and_gives_the_keys_back_only_with_nothing_selected`, `ctrl_c_in_the_panel_is_a_copy_even_where_the_agent_guard_would_take_it`, `shift_with_the_arrows_and_ctrl_a_select_from_the_keyboard` and `a_double_click_picks_a_word_a_triple_click_a_line_and_shift_click_extends`.
244. **"Ask agent" with text selected in a file sends that text through `frame_selection` unchanged, labelled with the file and its lines.** `PanelCtl::selection_ask` gives `<name> lines A-B` (or `line A`, counted from 1; a triple click's span that ends at a line's start does not count that line) with the name the header shows, cut first with an ellipsis so the whole label fits the frame's 64 characters, and the shaped text. `AppCtl::panel_ask_agent` tries it before anything else, so the Code view of a markdown file works the same; a selection of blanks frames nothing and the path goes, as it does with nothing selected. The file's text is untrusted like a page's: `frame_selection` gates it and `deliver_ask` types it, never submitted. No ctl, MCP or plugin method reads or sends the selection. Native: `PanelCtl::selection_ask`, `panel_sel::label`, `appctl/sessions_glue.rs`. Held by `ask_agent_sends_the_selection_framed_with_the_file_and_its_lines_and_no_enter`, `hostile_file_text_cannot_end_the_paste_or_press_enter`, `the_code_view_of_a_markdown_file_selects_and_asks_like_any_text_file`, `the_label_names_the_lines_and_cuts_the_name_to_fit` and `a_file_panel_sends_its_path_quoted_for_the_shell_and_no_enter`.
246. **An empty text file takes every click, drag and selection key without a crash.** `preview::read_text` makes an empty file (or empty text an agent shows) zero lines, yet the panel is still a ready Text panel the pointer and the keys reach, and `RowMap::pos_at` gives `Pos(0, 0)` with no lines. `panel_sel::word` and `line_span` indexed `lines[0]` on a double or triple click, and `step` did the same for Shift with Right or End, a panic on the UI thread. `word` and `line_span` now read the line with `get` and give an empty span, and `step` returns the start at once when there are no lines, so nothing is selected and nothing is drawn. Native: `crates/flare/src/panel_sel.rs`. Held by `an_empty_file_takes_every_click_drag_and_key_without_a_selection` and `an_empty_file_takes_clicks_and_shift_keys_without_a_crash_or_a_selection`. Source: review of "Select text in files", 2026-11-10.
247. **The web lines of the diagnostics log keep a page's site, never its address.** A 2FA hand-off that fails (a shop's pop-up to a bank) has to be readable afterwards, and an address can hold a session token, an account number or a search. `flare_surfaces::web::site_of` gives the host in lower case with no port, login, path, query or fragment, the scheme alone for an address with no host, and `odd` or `unparsed` otherwise: it never returns the text it was given (`host_label`, which falls back to the whole address, is not used for the log). `flare_web::WebNote::new` calls it where the browser's event arrives in `host.rs`, and every other field of a note is a fixed word (`web_error_name`, `permission_word`, `process_word`, `process_reason_word`), a bool or a number, so no address leaves `flare-web` for the log; `WebEvent`'s `Debug` prints the variant only. `web_ctl::web_line` maps a note to its line through the `WEB_LINES` table: `id`, `view` (`web`, `app`, `mod`), `site`, `from` and the words. Native: `crates/flare-surfaces/src/web.rs`, `crates/flare-web/src/backend.rs`, `host.rs`, `crates/flare/src/web_ctl.rs`. Held by `the_logs_site_is_the_host_alone_and_never_the_address`, `errors_permissions_and_processes_are_fixed_words`, `a_note_keeps_the_sites_and_never_the_addresses`, `an_event_prints_its_kind_and_never_its_data`, `no_line_holds_an_address_a_path_a_query_or_a_login` and `no_caller_puts_a_text_key_on_a_diagnostics_line`. Source: `docs/superpowers/specs/2026-11-10-web-popups-design.md`, section 3.
248. **Logging a browser problem changes nothing the browser does, and a flood of them is a few lines.** The certificate handler (`log_cert_errors`, through `ICoreWebView2_14`) reads `ErrorStatus` and `RequestUri` only and never calls `SetAction`, so the browser shows its own error page as before; a Runtime without that interface, or without `ICoreWebView2ProcessFailedEventArgs2` for a process's reason and exit code, logs less and fails nothing, as `hold_downloads` degrades. A failed load is not logged for a navigation Flare cancelled (14, its `web-refused` line says why) or a stream handed to the player (9) (`load_failure_logged`). The page events (`web-nav`, `web-load-failed`, `web-cert`) are the plain web panel's only. `flare_diag::web_diag::WebDiag` writes one line per `kind|site|word` per 10 s, counts the copies as `repeats` on the line its timer writes once the 10 s have passed (`repeats` is the events a line stands for besides itself, whether `log` or `tick` wrote it), and at most 10 lines per kind per 10 s; `next_due` never names a time before the held line's kind has budget again, so the timer never spins on the UI thread (34); `web-process` and `web-cert` always write their first line. `web-nav` (`STEPS`) is never folded by site: a hop back to a site seen a moment ago is its own line, in order. A mini-app's or plugin's launch and frame walls note what they refuse (`why: app-link`, `why: frame`; `app_link_refused`, `frame_refusal`). `web-nav` is verbose only (`sink_allows`). `cargo diag` counts `web-process`, `web-load-failed` and `web-cert` as problems and shows the rest under `--web` and `--all`. Native: `crates/flare-web/src/host.rs`, `crates/flare-diag/src/web_diag.rs`, `summary.rs`, `reader.rs`, `WebCtl::log_note`. Held by `only_a_load_that_failed_on_its_own_is_logged`, `a_repeat_inside_the_window_is_counted_and_said_once_it_has_passed`, `a_kind_writes_at_most_its_budget_per_window_and_the_rest_later`, `a_process_failure_and_a_certificate_error_always_write_their_first_line`, `every_line_says_its_events_whichever_path_wrote_it`, `a_spent_budget_puts_the_timer_at_the_windows_end_and_never_sooner`, `a_hop_back_to_a_site_is_its_own_step_in_order`, `a_refused_frame_or_app_link_of_a_mini_app_is_noted_by_site`, `takes_the_web_kinds_at_the_quiet_level_and_no_other_web_kind`, `keeps_the_high_rate_lines_for_verbose_only` and `the_web_panels_failures_are_problems_and_its_steps_show_under_web`. Source: `docs/superpowers/specs/2026-11-10-web-popups-design.md`, section 3.
249. **A web page's pop-up opens over the page in the panel, only after the person's click, and its request is always answered.** A 2FA check (3-D Secure) failed because the panel loaded every pop-up into itself and lost the link back to the shop's page. `flare_surfaces::web::popup_plan` decides each `NewWindowRequested` of the plain web panel and of its pop-ups: an address the panel may not open is refused, a request no click or key started (`IsUserInitiated`) is refused ("Pop-up blocked" in the bar, `web-refused why: popup-blocked`), a request with a size or position, a window name other than `_blank`, `_self`, `_parent` or `_top` (`ICoreWebView2NewWindowRequestedEventArgs2`; an older Runtime decides by the size, position and address alone) or for `about:blank` opens a pop-up, past 2 a tab or 4 in all it is refused (`popup-limit`), and anything else (a plain `target=_blank`) opens in the view that asked, as before. A pop-up is a second controller on the web panel's browser in Flare's window (`start_popup`, `adopt_popup`): `configure_popup` gives it the panel's saved passwords and autofill, the permission denial, the address rule on every navigation, downloads cancelled at once, nested requests stacked on the same tab, and it closes on `window.close()` or a lost process. It is handed to the browser with `SetNewWindow` and never navigated by Flare, so `window.opener`, the window's name and a form's POST are the page's own. It takes the page's box under Flare's strip ("Pop-up from <site>", `flare_surfaces::web::site_of`, never the title, with Close; `PopupStrip` in `panel.slint`, `WebEvent::Popups`, `WebCtl::push`), the page waits hidden behind it, and the bounds, the still, the hidden states and the parent's moves follow whatever is on top (`WebHost::front`). Every request is held with its deferral in `PopupBook` and completed on every path (made, failed, the panel or tab closed, quitting, the host dropped): `PopupAsk` completes it when it is let go. Mini-apps and plugin pages still open nothing (`refuse_new_windows`, rules at the mini-app and plugin rows). Native: `crates/flare-surfaces/src/web.rs`, `crates/flare-web/src/host.rs`, `popups.rs`, `backend.rs`, `crates/flare/src/web_ctl.rs`, `crates/flare/ui/panel.slint`. Held by `a_request_that_looks_like_a_popup_opens_one_after_the_persons_click`, `a_plain_new_tab_link_still_opens_in_the_panel`, `a_window_no_click_of_the_persons_asked_for_is_blocked`, `an_address_the_panel_may_not_open_is_refused_before_anything_else`, `past_two_popups_a_tab_or_four_in_all_the_next_is_refused`, the `popups.rs` tests (`closing_a_panel_completes_its_waiting_requests_and_hands_out_its_views_only`, `quitting_completes_every_waiting_request_and_hands_out_every_view`, `a_book_let_go_completes_what_still_waits`), `a_popup_shows_under_flares_strip_and_closing_it_brings_the_page_back`, `closing_the_tab_takes_its_popups_with_it`, `a_popup_no_click_asked_for_is_said_in_the_bar`, `the_popup_strip_takes_the_pages_top_and_its_close_calls_back`, `a_popup_is_a_popup_line_and_a_blocked_one_a_refusal_that_says_why` and the e2e `webPanel`. Source: `docs/superpowers/specs/2026-11-10-web-popups-design.md`, section 1.
250. **An agent never reaches a pop-up, and acts on nothing while one is open.** A card's or a bank's page is the person's. A pop-up's view is kept in `PopupBook`, never in the host's views, so `cdp`, `run_script`, `navigate` and a look go to the page behind it; its page events (loads, titles, the address) are never told as the panel's. While a pop-up is open over a tab's page, `WebCtl::start_agent` answers click, type, press, scroll and eval with `a pop-up is open, the person must finish it` (`agent_acts`, `POPUP_OPEN`) before anything is sent to the page, `browser.navigate` and `browser.open` (`open_page`, which would send the page behind it to a new address) answer the same, a look and a wait still read the page, and `panel.state` carries `popup: true` so `browser_state` says a pop-up is open. Native: `crates/flare/src/web_ctl.rs`, `appctl/web_glue.rs`, `appctl.rs` (`PanelState`), `crates/flare-mcp/src/browser.rs`. Held by `the_agents_input_calls_wait_for_the_person_while_a_popup_is_open`, `the_strip_names_the_site_and_only_reading_calls_pass_a_popup` and `browser_state_says_what_the_panel_shows_with_the_page_as_quoted_data`. Source: `docs/superpowers/specs/2026-11-10-web-popups-design.md`, section 1.
251. **The browser bar never moves the page behind a pop-up.** The page that opened a pop-up is hidden behind it and the pop-up's `window.opener` points at it, so a Back, Forward, Reload, Stop or address sent to it would break a 3-D Secure check unseen. While a pop-up is open over the tab's page, `web_nav` and `web_submit` send nothing and the address row says "Close the pop-up first" (`POPUP_FIRST`, cleared when the last pop-up closes); `WebCtl::go_to` and `WebCtl::nav` send nothing either, so no other path can move it. Found by the review of 2026-11-10. Native: `crates/flare/src/appctl/web_glue.rs`, `crates/flare/src/web_ctl.rs`. Held by `the_bar_leaves_the_page_behind_a_popup_alone` and `the_agents_input_calls_wait_for_the_person_while_a_popup_is_open`. Source: `docs/superpowers/specs/2026-11-10-web-popups-design.md`, section 1.
252. **A web page's link to an app is asked about, never opened by the browser, and only the person can say Open.** A bank's page opens its app (`bankid:`, `swish:`) and a mail link opens `mailto:`; silently blocking them broke card and bank checks, and letting the browser launch them would hand a page's data to any program. The probe of 2026-11-10 (a hidden view on the installed Runtime) found that `NavigationStarting` fires for a link to any scheme, with or without a click, and that cancelling it launches nothing, while `LaunchingExternalUriScheme` fires only for a registered scheme after a click (a frame's link raises `FrameNavigationStarting` only). So on the plain web panel and its pop-ups the navigation (and a new-window request) to an address the panel does not open is cancelled and `flare_surfaces::web::external_scheme` decides it: `Offer(scheme)` is told to the window once (`WebEvent::AppLink`, `AppLinks`, the same link within 1 s is one, `AppLinkDedupe`), and `Never` (`javascript`, `data`, `file`, `vbscript`, `about`, `blob`, browser pages and the Windows handlers of `NEVER_SCHEMES`, kept as data, the Office schemes whole so their `ofe|u|` and remote forms go too; over 2048 characters, a control character or a space, a path, a scheme not `[a-z][a-z0-9+.-]*`) is refused as before. `LaunchingExternalUriScheme` is registered on the panel and its pop-ups as a backstop that always `SetCancel(true)` and asks the same way. `navigation_allowed` is unchanged for the address bar and an agent's navigate. The window asks in the dialog with Cancel the default (`app_ask.rs`, `default-cancel` in `dialog.slint`): "Open an app?", "<site> wants to open <scheme> on this PC." (`site_of`), Enter, Space, Escape and a press outside cancel, Open needs a click or Tab then Enter and does nothing in its first 0.3 s; every other question keeps its default. Each app-link question raises the dialog's `question` (`dialog-question`), so one shown after another in the open dialog, with the same title and default, starts on Cancel again and a Tab given to the first never opens the next with a plain Enter (review of 2026-11-10). Limits (`AppAsks`): one question a tab, two waiting, one per site and scheme every 10 s, 5 a minute, a Cancel mutes that site and scheme in its tab until the page is on another site; a closed tab's questions go. An app nothing on the PC opens (`HKCR\<scheme>` with no `URL Protocol`, `url_handler_registered`) is "Nothing on this PC opens X:". While an agent drives the page (`agent_active` or a navigation call waiting) no question appears: the bar says so and each waiting call is answered once, `unavailable`, "only the person can allow opening an app". On Open the link is classified again and `flare_win::shell_open::open_app_uri` hands the bare link, quotes and spaces percent-encoded, to `ShellExecuteW` with the verb `open` and no parameters, never `cmd` or a shell (rule 188), on a worker thread of its own (`ShellLauncher`, `open_app_uri_on_worker`: an apartment with DDE off) and never on the UI thread (rule 34), since a handler that starts slowly or a stalled DDE talk holds the call; a failure comes back as `UiMsg::AppLinkFailed` and the bar says "The app could not be opened"; every test goes through `RecordingLauncher`, and `--e2e` keeps it. Native: `crates/flare-surfaces/src/web.rs`, `crates/flare-web/src/host.rs`, `backend.rs`, `crates/flare-win/src/shell_open.rs`, `crates/flare/src/app_ask.rs`, `appctl/web_glue.rs`, `web_ctl.rs`, `ui/dialog.slint`. Held by `a_link_to_an_app_is_offered_by_its_scheme_in_lower_case`, `every_scheme_on_the_never_list_is_never_offered`, `long_odd_and_path_like_text_is_never_offered`, `the_same_app_link_told_twice_within_a_second_is_told_once`, the `app_ask.rs` tests, `an_app_link_cannot_carry_quotes_or_arguments_to_its_app`, `with_cancel_the_default_enter_space_escape_and_a_press_outside_cancel`, `an_app_link_is_asked_about_with_cancel_the_default_and_opened_only_on_open`, `a_cancelled_app_link_is_not_asked_again_until_the_page_changes_site`, `an_app_nothing_on_the_pc_opens_is_said_in_the_bar_and_not_asked_about`, `an_app_link_of_a_closed_page_is_not_asked_about`, `while_an_agent_waits_on_the_page_an_app_link_is_never_asked_and_its_call_answered_once`, `an_agent_call_running_on_the_page_is_answered_that_only_the_person_can_open_an_app`, `a_new_question_in_the_open_dialog_starts_on_cancel_again`, `the_next_app_link_question_starts_on_cancel_after_a_tab_to_open_the_first`, `the_shell_launcher_opens_on_a_worker_and_answers_from_it` and `an_app_that_could_not_be_opened_is_said_in_the_bar_when_its_worker_answers`. Source: `docs/superpowers/specs/2026-11-10-web-popups-design.md`, section 2.
253. **A page may open a `blob:` it made itself, and only that one.** A shop's receipt or a bank's statement is often a `blob:` the page built; refusing it left the person with nothing. `flare_surfaces::web::own_blob` is true only for `blob:http(s)://<origin>/...` whose origin (scheme, host and port) is the opening page's: the panel's and a pop-up's navigation let it through, a new-window request for one opens in the view that asked after the person's click (none without one), and another origin's blob, a blob of a page that is not http or https, and every blob from the address bar or an agent's navigate (`normalize_url`, `address_or_search`, `navigation_allowed`) stay refused. Native: `crates/flare-surfaces/src/web.rs`, `crates/flare-web/src/host.rs`. Held by `a_page_may_open_its_own_blob_and_no_one_elses`. Source: `docs/superpowers/specs/2026-11-10-web-popups-design.md`, section 2.
254. **The agent's arrow points and never acts.** `browser.point` (MCP `browser_point`) is in the Browser namespace with the other browser tools' grant, is not a job and sends the page nothing but `DOM.enable`, `DOM.scrollIntoViewIfNeeded`, `Page.getLayoutMetrics` and `DOM.getContentQuads` (its own route in `WebCtl::on_event`, `arrow_cdp`, before the job's): no input event, no script. It answers `{pointed, offscreen}` once measured (`NOT_FOUND` for a ref the last look did not give or an element that is gone, `POPUP_OPEN` while a pop-up is open, rule 250), draws one orange arrow per tab with a label cleaned by `ask_agent::point_label` (48 characters), measures again every `POINT_MEASURE_MS`, and goes on `clear`, a new arrow, the person's click or key in the page (`arrows_user_input`, also from the agent tick's poll), a navigation (`arrows_new_doc`), the panel or tab closing (`arrows_tab_gone`) or `POINT_MS`. It is drawn only where `cursor_on` lets the cursor be and never while a pop-up is open (`update_arrows`). Native: `crates/flare/src/web_ctl/arrows.rs`, `crates/flare-win/src/agent_cursor.rs`, `crates/flare/src/ctl_app.rs` (`point_req`), `crates/flare-mcp/src/browser.rs`. Held by `the_agents_arrow_scrolls_its_element_into_view_answers_at_once_and_never_clicks`, `the_agents_arrow_follows_the_page_as_it_scrolls_and_hides_while_its_element_is_off_screen`, `the_agents_arrow_goes_on_clear_a_new_arrow_and_after_a_minute`, `the_agents_arrow_goes_when_the_person_acts_the_page_moves_on_or_the_panel_closes`, `the_agents_arrow_hides_under_a_menu_another_tab_and_a_popup_and_is_refused_over_a_popup`, `browser_point_takes_a_ref_or_a_point_with_a_label_or_clear_alone`, `browser_point_is_posted_for_the_callers_own_tab_only_and_is_listed` and `browser_point_sends_only_its_params_and_says_nothing_was_clicked`. Source: `docs/superpowers/specs/2026-11-10-ask-agent-additions-design.md`, section 2.
255. **The person's arrow is typed unsent, framed, and reaches the agent's next look once.** On release the browser is asked, with no page script, what is under the head (`DOM.getNodeForLocation`, then `Accessibility.getPartialAXTree` read by `agent_web::pointed_at`), and `ask_agent::frame_arrow` frames it: the site, `> points at:` with the role and the name gated, on one line, capped and stripped of double quotes, `> at:` and how to see it. It goes through `deliver_ask` like Ask agent (rules 188, 234: never Enter, never to a tab that is not in front). The arrow is kept for `USER_ARROW_MS` or until the page moves on; the next look gets `user_arrow` `{from, to, ref, role, name}` with a ref made by `RefMap::ref_for` when the look did not list the element, and the arrow drawn on its screenshot (`blend_onto_rgba`), then it is gone. Native: `crates/flare/src/web_ctl/arrows.rs`, `crates/flare/src/ask_agent.rs`, `crates/flare/src/appctl/sessions_glue.rs`, `crates/flare-surfaces/src/agent_web.rs`, `agent_job.rs`. Held by `the_person_drags_an_arrow_and_its_frame_is_typed_unsent`, `the_next_look_gets_the_persons_arrow_with_a_ref_once_and_then_it_goes`, `an_arrow_at_an_element_the_look_does_not_list_gets_a_ref_of_its_own`, `the_persons_arrow_is_kept_five_minutes_or_until_the_page_moves_on`, `when_the_browser_names_nothing_under_the_arrow_the_frame_says_so`, `a_hostile_element_name_stays_one_quoted_capped_line_of_the_frame`, `an_arrow_on_a_buttons_label_names_the_button_and_one_on_nothing_named_says_its_role` and `the_screenshot_gets_the_arrow_where_it_was_drawn_at_the_looks_scale`. Source: the same spec, section 2.
256. **The draw mode takes the mouse, never the keyboard, and every way out cancels it.** "Draw arrow" shows only on a tab with an agent, on a web page or a mini-app (never a plugin's page), and not while a pop-up is open (`push_draw_arrow`). The pad (`PadWindow`) is a layered popup owned by Flare's window, every pixel at alpha 1, `WS_EX_NOACTIVATE`, answering `MA_NOACTIVATE`; the probe of 2026-11-10 (`pad_probe`, a hidden desktop) found it catches the press, drag and release while the page keeps the focus. The agent tick reads the pointer (a press that started before the mode is not one; the left button must be seen up first). The button again, a right click, Escape where it arrives (the key filter in the page, `on_key` in the window), a Flare menu, a tab switch, a pop-up and a navigation end it with nothing asked and nothing typed (`update_arrows`, `draw_tick`). Native: `crates/flare-win/src/agent_cursor.rs`, `crates/flare/src/web_ctl/arrows.rs`, `crates/flare/src/web_ctl.rs` (`key_filter`), `crates/flare/src/appctl.rs` (`on_key`), `crates/flare/ui/panel.slint`. Held by `every_cancel_ends_the_draw_mode_with_nothing_asked_and_nothing_typed`, `the_draw_button_shows_for_an_agent_on_a_web_page_and_never_over_a_popup`, `a_click_with_no_drag_points_at_the_target_from_up_and_left` and `pad_probe` (by hand). Source: the same spec, section 2.
257. **The person's arrow stays on what it points at as the page scrolls.** Its ends are kept on the document (the viewport point plus the scroll `Page.getLayoutMetrics` gave at the release), never as fixed viewport points: a person who scrolled before sending saw the blue arrow sit over another element while the look named the first, and the agent got a picture and numbers that disagreed with the ref. While its page is in front the agent tick measures the scroll again every `POINT_MEASURE_MS` (`user_arrow_measure`, answered in `arrow_cdp`), the mark is drawn only while both ends show and not on a stale scroll (`update_arrows`), and a look gets `from` and `to` in its own viewport (`viewport.scroll`, added to `browser.look`), with `offscreen: true` and nothing drawn on the screenshot when the head is outside it; `browser_look`'s text says so instead of claiming it is drawn. Found by the review of 2026-11-10. Native: `crates/flare/src/web_ctl/arrows.rs`, `crates/flare-surfaces/src/agent_job.rs`, `crates/flare-mcp/src/browser.rs`. Held by `the_persons_arrow_stays_on_what_it_points_at_as_the_page_scrolls`, `a_look_after_a_scroll_gets_the_arrow_where_its_ends_are_now_and_off_screen_undrawn`, `a_look_with_the_persons_arrow_says_where_it_points_with_the_page_words_inside_the_markers` and `a_look_reports_the_picture_the_elements_and_numbers_them`. Source: the same spec, section 2.
258. **A staged build installs only on a click, and the click asks first under a working agent.** "Build ready" has no menu: a click is install now. With an agent working, the close question ("Stop the agent and install the update?", Install and restart / Cancel) asks first and Cancel keeps the build on offer; with none, the 10 s toast with Cancel runs at once. Nothing installs on its own, however long the app is idle, and there is no Not today or Discard: a build stays on offer until it is installed or a new stage replaces it. Native: `flare_update::staged_flow` (`StagedFlow::click`, no idle clock, `needs_tick` false while a build only waits), `appctl::staged_glue` (`staged_chip_clicked`, `staged_ask`); tests `staged_flow::tests::nothing_installs_on_its_own`, `ui_tests::staged_flows::nothing_installs_on_its_own_after_a_minute_idle`, `a_click_under_a_working_agent_asks_first_and_cancel_keeps_the_build_on_offer`, and the offscreen e2e `clickInstall`. Source: Flare-native, `docs/superpowers/specs/2026-11-10-build-ready-one-click.md` (owner, 2026-11-10).
259. **A test waits for the event it checks and holds on any account.** A pty resize lands on the `flare-pty-resize` helper and a restored shell is spawned on a worker, so a test counts resizes only once the helpers are quiet and speaks to a restored shell only once its spawn is seen; a fixed count right after a step failed on every busy CI runner. A security descriptor is compared in the form Windows writes it (`canonical_dacl`), since the CI runner's built-in Administrator reads back as `LA`. A match arm names one agent kind (or one kind and `None`), so `no_source_file_lists_the_resumable_agents_by_hand` keeps finding real second lists. Found 2026-11-10: these four failed on every run of main. Native: `crates/flare/src/ui_tests/flows.rs`, `ui_tests/agent_flows/convo_flows.rs`, `crates/flare-ctl/tests/pipe.rs`, `crates/flare-agent/src/keys.rs`. Held by `the_slide_holds_the_terminals_size_and_fits_once`, `a_stored_conversation_panel_reopens_empty_until_the_transcript_loads`, `the_live_pipe_carries_the_owner_only_descriptor`, `the_built_in_administrator_reads_back_as_its_alias` and `no_source_file_lists_the_resumable_agents_by_hand`.
260. **A test that names a well-known account builds it from the machine, not from the user.** Windows writes "LA" only for RID 500 of the local machine's account domain, so a test takes that domain from the computer name (`machine_account_domain`), never by cutting the RID off the current user's SID: on a domain or Entra ID account that prefix is another domain, whose RID 500 keeps its full SID. Found 2026-11-10 in review of 258: the alias test passed only on a local account. Native: `crates/flare-ctl/tests/pipe.rs`. Held by `the_built_in_administrator_reads_back_as_its_alias`.
261. **The web panel's profile lives under the profile folder, so a login lasts as long as the profile.** Owner, 2026-11-10: "if I'm signed into Google once in the browser it should still be signed in the next time I open Flare". The hidden e2e `webLogins` (`tools/e2e/src/scenarios/web_logins.rs`: a local site on `127.0.0.1` sets a cookie for a year, a session cookie and `localStorage`; the page reads `document.cookie` itself) found that a persistent cookie and `localStorage` already survived a panel close, a quit (one set the moment before it too: the quit closes the views and lets the browser go, rule 28, so it writes its cookies) and the staged install's restart. What lost them: the default run (no `--user-data-dir`, the Dev copy `%LOCALAPPDATA%\Flare\current\Flare.exe`) kept the panel's folder at `%LOCALAPPDATA%\Flare\WebView2`, inside the Velopack root, which Setup moves aside and deletes on a reinstall and the uninstall deletes, while Stable (`--user-data-dir=%APPDATA%\FlareStable`) kept it under its profile. Native: `flare_surfaces::web::webview_folder(profile_dir)` is always `<profile>\WebView2` (the mini-apps' and plugins' folders stay its `-<kind>` siblings, never shared with the panel or Edge), and at start the default run moves the old folders once with `move_old_webview_folders` (only a folder whose new place is empty, so nothing is replaced and a second start does nothing). Dev and Stable are two profiles with two panel profiles: a login in one is not in the other, by design. A session-only cookie (a site's sign-in without "stay signed in") ends when the last web panel closes, since the browser ends with it (panels cost nothing while closed); `webLogins` holds that too. Held by `the_profile_lives_under_the_profile_folder_never_the_install_folder`, `the_old_place_is_only_the_default_runs_local_appdata_folder`, `the_old_folders_move_once_and_never_replace_what_is_there` (`crates/flare-surfaces/src/web.rs`) and `webLogins` (22 checks).
262. **The old browser folders move all or none, and while they cannot move the browser runs in them, never in an empty new folder.** The review of rule 261 (2026-11-10) found the one-time move had no way back: when the rename failed (the run before, quitting on Build ready's restart, or a crash, left a `msedgewebview2` holding a file under `%LOCALAPPDATA%\Flare\WebView2`), the first web panel made an empty `<profile>\WebView2`, and from then on the new place was taken, so the old folder with every login never moved and the next Setup reinstall deleted it. Native: `move_old_webview_folders` moves the panel's folder and its `-<kind>` siblings all or none (one that fails sends the moved ones back), `webview_place` is the old folder while it is there and the new one is not, `settle_webview_folder` tries the move again when the first browser is made (`web_ctl::production_factory`) and gives that place, and the plugins' profile removal (`set_profile_dirs`) deletes in that same place. Nothing waits or retries on a clock: a held folder is used where it is and moves on a later try. A profile on another disk than `%LOCALAPPDATA%` (`rename` does not copy) keeps using the old place; nothing is lost by the run, but a reinstall still deletes it there. Held by `a_folder_the_last_run_still_holds_stays_in_use_and_moves_when_it_is_let_go` (a real open file that shares nothing), `a_sibling_that_cannot_move_takes_the_panels_folder_back_with_it` and `the_place_is_the_new_folder_once_nothing_waits_to_move` (`crates/flare-surfaces/src/web.rs`).
263. **A test that opens public pages runs only when named, and records no address of the machine.** `webBotChecks` (#146) opens public bot-check and Cloudflare pages from a panel parked off the screen, in a throwaway world, to measure what a bot check sees: it is in `OPT_IN`, so a plain run or the `webPanel` filter never touches the network, and the `ip=` line of Cloudflare's trace page is hidden in the page before it is read. It reads pages with plain navigations and the e2e script probe only, then once more after one `browser.look`, so the panel as the person has it and the panel after an agent's look are measured apart. The measurements and the decision (no product change, 2026-11-10) are in `docs/web-bot-checks.md`. Native: `tools/e2e/src/scenarios/bot_checks.rs`, `tools/e2e/src/scenarios/mod.rs`. Held by `a_plain_run_never_opens_the_public_bot_check_pages`.
264. **An opt-in scenario is picked only by its whole name.** `pick` takes an `OPT_IN` scenario only when a filter word equals its name (any case); the gate's own scenarios still match on a part of the name. A part such as `web`, `bot` or `check` once pulled in `webBotChecks`, so running the web panel group opened public bot-check pages and Cardinal's 3-D Secure host from the owner's address with no sign. Found 2026-11-10 in review of 263. Native: `tools/e2e/src/runner.rs`. Held by `a_filter_word_that_is_only_part_of_an_opt_in_name_never_picks_it`.
265. **A quit crash is chased with the quit stress, and a launch that fails says how the app ended.** Prism Terminal's #63 (two rounds of `quitManyShells` ended in 0xC0000005 under a full suite) is held in Flare by the opt-in e2e `quitStress`: many restored shell tabs, the first ones running busy commands written in full in the scenario, a web page in some rounds, the software and the GPU renderer in turn, then `Quit { force: true }` and the exit code of every round (one CSV line per round with `FLARE_QUIT_STRESS_OUT`). A launch whose test hook never opens no longer ends the run: its message carries the app's exit code, so a crash at start is told apart from a slow start. Run 2026-11-10 on the release build of `12b335aa`: 262 rounds of 32 and 40 shells, no 0xC0000005 (details in `docs/e2e.md`, "The quit stress"); no product change, #63 stays open. Native: `tools/e2e/src/scenarios/quit_stress.rs`, `tools/e2e/src/ctx.rs` (`launch_in`). Held by `a_plain_run_never_picks_the_quit_stress_and_naming_it_runs_it_alone`, `an_access_violation_reads_as_its_ntstatus` and `the_tally_counts_each_exit_code`.

## File links by marker (2026-11-11)

Spec and plan: `docs/superpowers/specs/2026-11-11-file-link-markers.md`. These replace rule 13's guessing, rule 237 and the
guess parts of rule 1003.

266. **A file link is only a path a session writes between `⟦` and `⟧`; no file path is guessed.** Owner, 2026-11-11: "the
link system we have is poor. many links dont work." A plain shell's output (`dir`, `git status`, a compiler error) shows no
file links, even for names that exist; web addresses (http and https) are still found in the text, also across a row
broken by hand (review #37), and OSC 8 links are unchanged. Nothing is statted before a marked path lights: the session
wrote it on purpose. A relative path between marks is a link only while the tab's folder is known. Rule 11 stands: Flare
never sets `FORCE_HYPERLINK`. Native: `flare_term::link_scan` (`file_links`; `PathLinks`, `paths.rs` and the retry table
are gone). Held by `a_plain_listing_has_no_file_links`, `a_marked_path_is_a_file_link_with_its_line`,
`a_relative_marked_path_needs_the_tabs_folder`, `a_marked_path_lights_up_at_once_and_a_plain_one_never`,
`a_path_is_a_link_only_between_marks_and_opens_by_the_allowlist` and the hidden e2e `markerLinks`.
267. **A span between marks is read exactly, over rows, and never eats the screen.** `flare_term::file_marks` reads from a
`⟦` to the next `⟧` in the same row or the rows below, at most 8 rows and 1,024 characters. A row the terminal wrapped
joins with nothing dropped; a row the program broke by hand (Claude Code breaks its own lines and indents the next row)
joins with that row's leading blanks and the row above's trailing blanks dropped. A second `⟦` starts the span again; a
blank row, no end mark, a `://` inside or nothing between the marks is no link. A trailing `:line` or `:line:col` is
taken off and kept. A web address stops at a mark (`links::find_links`). The scan's window reaches `MARK_ROWS` above and
below the view, so a link that starts above the first row is whole. Held by `file_marks::tests`,
`a_marked_path_broken_by_hand_and_indented_is_one_link`, `a_marked_path_that_starts_above_the_view_is_whole` (both
`emulator.rs` and `link_scan.rs`) and `find_links_stops_at_a_file_link_mark`.
268. **A click on a file link is vetted on a worker, tries the space a row break may have eaten, says when nothing is
there, and opens a text at its line.** The worker takes the first spelling that names something (`path_open::first_existing`):
as written, then with one space at each hand break alone, then at all of them. Nothing there: "Flare found no such file"
where a refused preview speaks (a note in a closed panel, the header or the page's bar in an open one;
`TermMsg::NoSuchFile`). Else the click goes the way a path click always went (rules 13 and 14: resolved again, the
allowlist, a picture or text file to the panel with "Open previews in Flare" on). A text preview, a click's or
`show_file`'s with `line`, opens with that line a few rows below the top and marked in the selection colour, once
(`PanelCtl::go_to_line`, `apply_goto`); a markdown file with a line opens in Code. Held by
`a_click_tries_a_space_where_the_row_broke_when_the_joined_path_names_nothing`,
`a_click_on_a_marked_path_that_names_nothing_says_so_and_opens_nothing`,
`a_click_on_a_marked_line_asks_for_the_preview_at_that_line`,
`a_preview_at_a_line_marks_that_line_and_a_later_one_without_marks_nothing`, `view_file_hands_its_line_to_the_panel`,
`show_file_hands_on_its_line_and_refuses_a_bad_one` and
`first_existing_takes_the_first_spelling_that_names_something`.
269. **Every Claude session in Flare is told to write file paths between the marks, and only inside Flare.** The plugin's
`SessionStart` hook (matcher `startup|resume|clear|compact`) runs `flare-mcp --hook-context --session-start`, which
prints the note (`flare_mcp::FILE_LINK_NOTE`) as `additionalContext` only when the tab's token is set, reads no input
and calls no one; it is left out with the prompt hook when there is no `flare-mcp`. The MCP server's `instructions` end
with the same note, so a session that skips the hook still sees it. It sets no agent state (the state hooks still have
no `SessionStart`). Held by `the_session_start_hook_tells_claude_about_file_links_on_every_start`,
`session_start_and_end_set_no_state`, `the_prompt_hook_gets_the_absolute_program_or_is_left_out`,
`the_session_start_note_asks_for_paths_between_the_marks`,
`the_session_start_hook_tells_a_flare_session_about_file_links_once` and
`the_server_instructions_end_with_the_file_link_note`.
270. **Every row that acts on a file link tries its spellings, and a wide character's empty last cell is no space.** Review
of 266 to 269 (2026-11-11): only the plain click tried a link's `alts`, so for a path Claude Code cut at a space the menu's
Copy path copied a path that does not exist, Open did nothing and Show in side panel said the file changed. Open, Show in
Explorer, Show in side panel, Copy path (`TermMenuAct::CopyPath`, only for a link with other spellings) and the hover
thumbnail (`ThumbJob::alts`) now take, on their worker, the first spelling that names something
(`path_open::first_existing`), else the one as written. On a row the terminal wrapped, Alacritty leaves the last column
empty (`LEADING_WIDE_CHAR_SPACER`, read as an empty cell of width 1) when a wide character does not fit there;
`file_marks` skips that cell when the next row starts with a wide character, and offers a space there as an alt. Held by
`the_menu_rows_act_on_the_spelling_with_a_space_where_the_row_broke`,
`copy_path_of_every_file_link_is_left_to_the_worker_that_finds_the_one_that_exists`,
`a_picture_named_with_a_space_where_its_row_broke_is_found_by_that_spelling` and
`a_marked_path_the_terminal_wrapped_before_a_wide_character_reads_with_no_space`.
271. **A marked path in a table cell is read inside the cell.** Owner, 2026-11-11, with a screenshot: Claude Code draws a
markdown table with box borders and breaks a long cell inside its column, and the reader read across the right border
and the other cells, then went on at the left border of the next row, so the link ran edge to edge on both rows with
borders in its path. A span whose start mark has a vertical border (`│`, `┃` or `║`) left of it and another right of it
on the same row is in a table cell (`file_marks::table_cell`). When it does not end on that row, each row's text stops at
the first border right of where reading started (the blanks before it dropped, the piece ends at the last path cell),
and the next row goes on at the first non-blank cell right of the border at the column of the cell's left border on the
start row; a next row with no border there (the table's bottom edge) is no link. A span that ends on its first row, and
a border only left of the mark (a quote bar), read as before. Held by
`a_path_in_a_table_cell_broken_by_hand_reads_only_inside_the_cell`, `a_path_over_three_rows_of_a_table_cell_is_one_link`,
`a_table_cell_that_ends_at_the_tables_bottom_edge_is_no_link`, `a_border_only_left_of_the_mark_is_no_table` and the
hidden e2e `markerLinks`.
272. **A relative file link is read from the folder the session works in.** Owner, 2026-11-11: every link was long
because the note asked for absolute paths. The note (`flare_mcp::FILE_LINK_NOTE`, both platforms, also the server's
`instructions`) now asks for a path relative to the session's working folder, an absolute path only for a file outside
it, and a short marked path in a table. The `UserPromptSubmit` hook (`flare-mcp --hook-context`) sends the `cwd` of its
input as the optional `cwd` param of `panel.state`, and the `PostToolUse` hook of `EnterWorktree|ExitWorktree`
(`--hook-context --folder`) sends only that and prints nothing; a `cwd` that is not an absolute path of at most 4,096
bytes with no control character is not sent. Flare takes it only from the agent's grant, vets it again on the call
thread (`ctl_app::agent_folder`; an odd one is ignored, never an error) and keeps it per tab
(`AgentHub::agent_folder`): the latest wins, and it goes when the agent leaves the tab (a shell prompt, an empty title,
the poll), when a new shell starts in the tab and when the tab closes. The front tab's link folder (`FrontTab::cwd`,
`TabInfo::cwd`) is the agent's folder when known, else the tab's own: the click, the menu rows, the hover thumbnail and
the lighting of a relative mark all read it, and every stat stays on a worker (rule 34). A Flare that does not know
the param ignores it; an older `flare-mcp` sends none. Held by `every_hook_that_calls_flare_carries_the_sessions_folder`,
`the_session_start_note_asks_for_paths_between_the_marks`,
`entering_or_leaving_a_worktree_tells_flare_the_sessions_folder`,
`panel_state_carries_the_agents_folder_only_when_it_is_a_plain_absolute_path`,
`a_relative_file_link_opens_under_the_agents_folder_not_the_shells` and the hidden e2e `markerLinks`.
273. **A file link that names nothing reaches no opener on any route, and every route says so.** Owner, 2026-11-11, on
the build before rule 271: right-click Open on a table link did nothing, and Prism (the default app for files and
folders) logged the path read across the table's borders. Prism's log shows a `navigate` inside a Prism window two
seconds after it opened the project folder, not a launch by Flare: the path came from Copy path (rule 274), and every
opener (`open_term_path`, `shell_open_file`, `explorer_open_folder`) already stats first. Now the plain click, Open,
Show in Explorer and Show in side panel each take the first spelling that names something on their worker
(`path_open::first_existing`) and, when none does, call no opener and post `TermMsg::NoSuchFile` ("Flare found no such
file"); `PreviewRefused` stays for a file that is there but not shown. Held by
`a_file_link_that_names_nothing_reaches_no_opener_from_the_click_or_any_menu_row`,
`show_in_side_panel_on_a_file_that_went_away_says_flare_found_no_such_file_and_opens_nothing`,
`show_in_side_panel_on_a_file_that_is_there_but_not_shown_says_it_cannot_and_opens_nothing` and
`a_table_cell_link_that_names_nothing_opens_nothing_and_copies_exactly_its_path`.
274. **Copy path copies the first spelling that names something, else the text as written.** Owner, 2026-11-11: Copy
path copied the path read across a table's borders. Every file link's Copy path is now `TermMenuAct::CopyPath`
(rule 270 made it so only for a link with other spellings): a worker copies the first spelling that names something as
a whole path, else the first spelling exactly as written, never a resolved path that names nothing. With rule 271 a
table cell's path is copied without borders. Held by
`copy_path_copies_the_spelling_that_exists_as_a_whole_path_else_the_text_as_written`,
`copy_path_of_every_file_link_is_left_to_the_worker_that_finds_the_one_that_exists` and
`a_table_cell_link_that_names_nothing_opens_nothing_and_copies_exactly_its_path` (the two-row table of rule 271).
275. **The agent's folder reads relative file links only, and only when a relative path resolves from it.** Review of
rule 272 (2026-11-11): the agent's folder replaced the tab's for every link, so `\x` took the agent folder's drive, and
a UNC or `\\?\` agent folder lit relative marks that then named nothing. `TabInfo::link_base` reads a relative
spelling (`path_grammar::is_relative`) from `cwd` and any other from `shell_cwd`, the tab's own folder;
`appctl::link_folder` uses the agent's folder only when `resolve_term_path(folder, "x")` answers, else the tab's.
Held by `only_a_relative_file_link_is_read_from_the_agents_folder` and
`the_agents_folder_is_used_for_relative_links_only_when_a_relative_path_resolves_from_it`.

## Remote sessions (2026-11-12)

Spec `docs/superpowers/specs/2026-11-12-remote-sessions-design.md`, plan `docs/superpowers/plans/2026-11-12-remote-sessions.md`.

276. **The remote grant exists only on the serve stdio path and never reaches the Ui namespace.** Extends S3 (rules 186
to 188). A remote link (`ssh <host> flare --serve-remote`) is a caller from another machine, so its grant is new, not
the agent's and not a mod's. `Grant::Remote` enters `Namespace::Remote` (`remote.*`) and nothing else: not `hello`,
not `tabs.list`, no `shell.input` or `tabs.input`, no setting, mod, browser, eval or session. It is issued only to the
one connection of `Shared::new_stdio` (`ConnOrigin::Stdio`, set by that constructor alone, so no pipe server can be
configured into it) after `remote.hello`, and asked of the app again on every request. On the pipe `remote.hello` is
`forbidden` before and after `hello`; a token or agent grant that resolves to `remote` is refused like `ui`
(`unauthorized`, `internal`); a remote link that says `hello` is `forbidden`; no permission opens `remote.*`, so no
`mod.json` can ask for it. Native: `crates/flare-ctl/src/grant.rs` (`may_enter`), `session.rs` (`pipe_grant`,
`remote_grant`, `answer`). Held by `remote_grant_cannot_reach_ui_or_browser`, `remote_grant_never_on_the_pipe`,
`a_remote_link_reaches_only_the_remote_methods`, `a_remote_link_takes_only_the_remote_grant_from_the_app`,
`permission_names_agree_with_what_each_grant_can_call`, `nothing_over_the_pipe_reaches_the_ui_namespace` and, over a
real pipe, `a_live_pipe_client_cannot_get_the_remote_grant`. Source: remote sessions PR 1 (2026-11-12).
277. **Stdout of the serve process carries only protocol lines.** The viewer reads the host's stdout as JSON lines, so
a log line, a panic message or a stray `println!` there breaks the link. In serve mode `StdioWire::from_process` is
the only writer to stdout (whole lines, locked and flushed); logs go to stderr, which ssh carries apart. The one
allowance is before the first message: a login banner or a shell's greeting is skipped by both ends until
`remote.hello` is answered, then the framing is strict. Native: `crates/flare-ctl/src/stdio.rs`, `session.rs`
(`serve_connection`), `client.rs` (`remote_hello`). Held by `a_write_reaches_the_stream_whole_and_flushed`,
`a_banner_line_before_the_first_json_line_is_skipped_on_the_stdio_path`, `remote_hello_skips_a_banner_before_the_answer`,
`noise_is_skipped_only_for_remote_hello_and_only_so_much` and `a_remote_hello_round_trips_over_os_pipes_after_a_banner`;
the serve process itself (PR 2) adds a test that runs it and reads its stdout. Source: remote sessions PR 1 (2026-11-12).
278. **A remote protocol mismatch refuses with one line naming both versions.** The two machines may run different
Flare builds. `remote.hello` carries `remote_protocol` (`REMOTE_PROTOCOL`, now 1); the host checks it before any other
param and answers `unsupported_protocol` (closes) with exactly `remote protocol N here, M there`, its own first; the
viewer's `Client::remote_hello` checks the answer the same way, its own version first. Nothing is guessed around a
mismatch. Native: `crates/flare-ctl/src/proto.rs` (`REMOTE_PROTOCOL`), `session.rs` (`remote_mismatch`,
`handle_remote_hello`), `client.rs`. Held by `a_remote_protocol_mismatch_refuses_with_one_line_naming_both_versions` and
`a_remote_hello_answer_of_another_version_is_refused_naming_both`. Source: remote sessions PR 1 (2026-11-12).
279. **`remote.input` is a typed intent path: a deliberate, documented exception to rule 2, called from one place.** A
remote viewer types on a host tab, so bytes from another machine reach a shell; they come only as typed intents, never
as raw bytes. An intent is a key (`key`, `code`, `text`, modifiers: encoded on the host by `encode_key` with the tab's
own modes, as a local key is), a committed text (no control character at all: an Enter or a Ctrl+C is a key), or a
paste (sanitised again on the host by `sanitize_paste` and bracketed when the program asked, rule 1). The worker checks
the whole call first (1 to 1024 intents, at most 64 KB of text, key names and key text short and free of control
characters, 50 calls a second on a tab) and types nothing of a call that fails. The one caller is
`ctl_app::AppDispatcher::remote_call` through `CtlOp::RemoteInput` into `TermHandle::remote_input`; the agent grant has
no way here (S3). Native: `crates/flare-ctl/src/remote.rs` (`parse_intents`), `crates/flare-term/src/handle.rs`
(`remote_input`), `crates/flare/src/ctl_app.rs`, `appctl/remote_glue.rs`. Held by
`remote_input_intents_round_trip_as_keys_text_and_pastes`, `remote_input_refuses_control_characters_in_text_and_keys`,
`remote_input_is_capped_at_64_kb_and_1024_intents_a_call`, `remote_input_types_keys_text_and_a_sanitised_paste` and
`remote_input_is_checked_and_rate_limited_before_the_window_types_it`. Source: remote sessions PR 2 (2026-11-12).
280. **A remote detach or a dead link never kills a host session.** The session belongs to the host (owner decision):
`remote.detach` only stops following the tab, and a link that drops (the serve process's stdin closes, its pipe
connection closes, `client_gone`) only removes that connection from the tab's followers; when the last follower goes,
the tab's size goes back to the window (`CtlOp::RemoteRelease`, `TabHost::take_size_back`) and nothing else happens.
Only `remote.close` ends a tab, and it goes through `request_close` like a person's close (rule 48). Rules 27 and 28
stay for quit. Native: `crates/flare/src/remote_host.rs` (`unfollow`, `gone`), `ctl_app.rs` (`client_gone`,
`remote.detach`), `appctl/remote_glue.rs`. Held by `a_remote_detach_or_a_dropped_link_never_closes_the_tab`,
`a_tab_is_followed_until_its_last_connection_detaches_or_goes` and `taking_the_size_back_never_kills_the_shell`.
Source: remote sessions PR 2 (2026-11-12).
281. **The output ring has a cap and never blocks the UI sink.** A followed tab's output is numbered (a `u64` that only
goes up) and kept in a ring of 4 MiB (`TAIL_CAP`); older bytes drop off, and a `since` older than the ring answers
`gap: true` and the bytes from the oldest kept, never a silent skip. The ring is fed in `Session::deliver` after the
tab's own sink, under the session's `dest` lock, so the tab sees each batch first and in order and the push only takes
the ring's own short lock; the polls (`remote.output`) and the snapshot run on the control channel's workers and never
on the UI thread (rule 34). A snapshot is read under the same `dest` lock (`with_tail_still`), so it holds exactly the
bytes before its `seq`. Native: `crates/flare-term/src/tail.rs`, `session.rs` (`subscribe_tail`, `with_tail_still`),
`handle.rs` (`remote_snapshot`). Held by `the_ring_wraps_at_its_cap_and_an_old_since_is_a_gap`,
`the_sequence_only_goes_up`, `the_output_tail_follows_the_tab_and_a_snapshot_meets_it_without_a_byte_twice` and
`remote_output_reads_the_followed_tail_and_never_asks_the_window`. Source: remote sessions PR 2 (2026-11-12).
282. **A remote resize sets the size owner; the window's refits skip remote-owned tabs until a local key.** The live
terminal has one size, the last typist's (owner decision). `remote.resize` (clamped, never refused) makes the remote
viewer the tab's size owner (`TabHost::remote_resize`); `refit_all`, `set_dims` and `activate` pass that tab by. A key,
text or paste typed on this machine (`Typist::Local`, read in `TabHost::drain_events`) gives the size back to the
window; remote typing (`Typist::Remote`) keeps it. Native: `crates/flare/src/tab_host.rs` (`SizeOwner`,
`remote_resize`, `take_size_back`), `crates/flare-term/src/handle.rs` (`last_typist`). Held by
`a_remote_resize_owns_the_size_until_a_key_is_typed_here` and `remote_resize_is_clamped_on_the_worker`. Source:
remote sessions PR 2 (2026-11-12).
283. **Serve mode takes no single-instance lock and opens no window.** `flare --serve-remote` is started by ssh, often
while the person's Flare runs: it must never hand its folders over, take the lock, park a window or set up the e2e.
`main.rs` branches to `serve_remote::run` right after the arguments are parsed, before `e2e::init`, the instance code,
the backend and the window; the GPU prewarm skips it; `--serve-remote` takes `--user-data-dir` and no folder or other
mode. Native: `crates/flare/src/main.rs`, `config.rs`, `backend.rs` (`prewarm_plan`), `serve_remote.rs`. Held by
`serve_remote_branches_before_the_lock_the_e2e_and_the_window`, `serve_remote_is_a_mode_of_its_own` and, as a
process, `tests/serve_remote.rs`. Source: remote sessions PR 2 (2026-11-12).
284. **The bridge grant comes only from the bridge key and reaches only the relayed `remote.*` methods.** The serve
process relays a remote link to the running Flare over its pipe, so the pipe needs one grant that reaches `remote.*`;
it is new (`Grant::Bridge`), not the remote grant (rule 276 stays: the remote grant never rides the pipe). It is issued
for one token, the bridge key: a fresh secret the running Flare writes into its profile at every start (owner-only, by a
rename), compared in constant time; never for "no token" (an agent grant of `bridge` is `internal`). It enters `hello`
and `remote.*` but `remote.hello`, and nothing else: no `tabs.list`, no shell input, setting, mod, browser or eval.
A process of the same user that can read the profile can also run `flare --serve-remote` itself, so the key adds no
reach beyond what the serve process already has. Native: `crates/flare-ctl/src/grant.rs`, `session.rs` (`pipe_grant`),
`crates/flare/src/remote_host.rs`, `ctl_app.rs` (`resolve_token`), `serve_remote.rs`. Held by
`bridge_grant_reaches_the_remote_methods_but_the_greeting_and_nothing_else`,
`the_bridge_grant_comes_only_from_its_key_and_reaches_only_the_relayed_methods`,
`the_bridge_key_is_the_bridge_grant_and_remote_methods_need_a_remote_host`, `only_this_runs_key_is_the_bridge_key`,
`a_wrong_key_is_no_bridge` and `the_bridge_relays_remote_calls_and_refuses_everything_else`. Source: remote sessions
PR 2 (2026-11-12).
285. **A remote snapshot lets an open synchronized update go before it reads the screen.** Bytes inside an open DEC
2026 update are in the tail before the snapshot's `seq` but not yet on the grid: the emulator holds them until the
update ends or its deadline. Read as they were, they were in neither the snapshot nor a later `remote.output {since:
seq}`, so a frame split over two batches lost its first half on the viewer. `remote_snapshot` calls
`Emulator::release_sync` under the emulator lock (inside `with_tail_still`) before it reads the text and hands the
replies the update held to the tab as delivery does. Native: `crates/flare-term/src/emulator.rs` (`release_sync`),
`handle.rs` (`remote_snapshot`). Held by
`a_snapshot_inside_an_open_synchronized_update_holds_the_bytes_it_kept_back`. Source: review of remote sessions PR 2
(2026-11-12).
286. **The bridge never holds one call behind another past its time, and a timeout never drops the follows.** The
serve process relayed every call on one connection under one lock for the whole round trip: a 40 s folder read held
typing and the polls, a call answered `timeout` to the viewer was still typed later, and a timeout dropped the
connection, so the running Flare unfollowed every tab. Now the calls that follow or change a tab (attach, detach,
input, resize, close) go in turn on one anchor connection, and every other call on an idle pooled connection or a new
one. A call's time (`method.timeout()` less the 2 s margin) counts from when the link handed it over, its wait for its
turn included; a call whose turn comes after it is answered `timeout` and never sent. A timeout keeps its connection
(a late answer is skipped by its id). Native: `crates/flare/src/serve_remote.rs` (`relay`, `relay_on_anchor`, `send`,
`on_anchor`). Held by `a_slow_open_never_holds_up_typing_or_the_polls` and
`a_call_late_for_its_turn_is_never_sent_and_a_timeout_keeps_the_anchor`. Source: review of remote sessions PR 2
(2026-11-12).
287. **A remote tab is a `TabKind::Shell` with a `remote` field; it owns a `RemotePty`, never a local pty.** Amends rule
45: a tab is still one shell and one folder, but a remote tab's shell and folder are the host's. `Tab.remote:
Option<MachineTag>` marks it, so every `kind == Shell` site keeps working, and the sites that would act on this
machine's files or processes skip it: no file link of a remote tab is opened, previewed or offered (`TabInfo.remote`),
a file dropped on it is not typed, its `localhost` gets no live-preview chip, its folder is never statted at restore
and never joins the recent folders, `cd` does nothing, and `live_pids` leaves it out (its pid is 0, no process here).
The spec decides the pty in one place, `RoutingSpawner` (`SpawnSpec.remote`); `PortableSpawner` refuses a remote spec.
Typing reaches the link as intents (`decode_typing` turns the bytes the tab wrote back into keys, text and pastes, and
drops what is no typing); the emulator's answers stay here (`SessionManager::write_reply`). Native:
`crates/flare-term/src/pty.rs`, `remote_pty.rs`, `session.rs` (`spawn_remote_reserved`), `crates/flare-tabs/src/tabs.rs`,
`crates/flare/src/tab_host.rs` (`spawn_remote`), `term_ui.rs`, `appctl/surfaces7.rs`. Held by
`a_remote_tab_never_starts_a_local_shell_or_takes_a_spare_and_carries_no_token`,
`typing_reaches_the_link_as_intents_and_replies_stay_here`, `decoded_typing_encodes_to_the_same_bytes_on_the_host`,
`mouse_focus_and_string_sequences_are_never_sent`,
`a_remote_tab_never_starts_a_local_shell_and_the_quit_leaves_its_session` and
`a_remote_tab_shows_the_hosts_output_and_takes_typing`. Source: remote sessions PR 3 (2026-11-12).
288. **A viewer's disconnect, quit or restart never ends the host session; the restore attaches again and is not new
work.** Rules 37 and 38 for a remote tab. Closing the tab (`kill`) sends `remote.close`, which asks on the host as any
close does (rule 48), and is still taken to the host when the link was down. The quit (`kill_all`, through
`PtyProcess::leave`) and a dropped pty only send `remote.detach`; a broken link only waits (1 s, doubling to 30 s) and
attaches again from the output number it read, no byte twice. `tabs.json` keeps a remote tab as `remote: {machine,
host_tab, last_seq}` (no agent: the host owns the resume); `last_seq` alone is no reason to write the file. The restore
never stats the host's folder, attaches to `host_tab` (a new tab only when the host never answered), starts from
`remote.snapshot` since this machine's screen is empty, and counts as touched (`AgentHub::on_remote_tab`). A host tab
that is gone ends the tab as a shell that exits. A shape it does not know reads as a local tab (rule 40). Native:
`crates/flare-term/src/remote_pty.rs` (`Leaving`), `session.rs` (`end_pty`), `crates/flare-tabs/src/restore.rs`
(`SavedRemote`), `model.rs`, `crates/flare/src/remote_link.rs`, `appctl.rs` (`restored`, `start_remote`). Held by
`the_quit_detaches_a_remote_tab_and_a_close_closes_it`, `a_close_ends_the_host_session_and_a_leave_only_detaches`,
`a_dead_link_never_holds_a_close`, `a_reconnect_goes_on_from_the_number_read_with_no_byte_twice`,
`a_gap_after_a_reconnect_takes_a_snapshot`, `a_restored_tab_attaches_to_its_host_tab_and_opens_none`,
`a_host_tab_that_is_gone_ends_the_tab`, `a_remote_tab_round_trips_through_tabs_json_beside_a_local_one`,
`the_restore_keeps_a_remote_tab_without_a_stat_and_attaches_it`, `a_remote_tab_saves_its_host_tab_and_the_output_read`
and the e2e `remoteViewer`. Source: remote sessions PR 3 (2026-11-12).
289. **Replayed output never raises an agent mark.** What a remote tab shows as it attaches is the past: the snapshot is
plain text (`plain_snapshot` takes every sequence out of it), and what a reconnect catches up on has the agent's
signals (OSC 777, OSC 7501) taken out (`SignalStrip`, which keeps a signal cut across the end of the replay out whole).
The hub reads a remote tab's screen as blank (no question box or background work is read off a replayed screen), keeps
no question watch for it, is blind to it in the process poll, and does not score its first 3 s of output as work. Live
output keeps its signals. The host's own state crosses the link as words in PR 4 (`remote.status`). Native:
`crates/flare-term/src/remote_pty.rs` (`SignalStrip`, `plain_snapshot`), `crates/flare/src/remote_link.rs`
(`catch_up`), `agent_hub.rs` (`on_remote_tab`, `HandleScreen`). Held by `signals_are_taken_out_of_replayed_output_only`,
`a_snapshot_is_text_with_no_sequence_of_its_own`, `a_catch_up_carries_no_agent_signal_and_live_output_does` and
`a_remote_tabs_replay_raises_no_mark_and_live_output_does`. Source: remote sessions PR 3 (2026-11-12).
290. **No token and no environment cross the link.** A remote tab's spec has no command line, no environment and no
`FLARE_TAB_TOKEN` (its session's control secret is empty, which no lookup matches); the host starts its shell with its
own `pty_env` (rule 12). The link is `ssh -o BatchMode=yes -T [-l user] -- <host> <command>`, every part an argv item
of its own and the host and user checked to be plain words that cannot be read as an option; the command is the host's
Flare path quoted for its login shell (single quotes for a Mac's sh, double quotes for a PC's cmd, no `&&`, a path that
cmd would expand refused) and `--serve-remote`, nothing else. ssh is started by its absolute path (rule 1) with no
console window. Native: `crates/flare-term/src/session.rs` (`spawn_remote_reserved`), `crates/flare/src/remote_link.rs`
(`ssh_argv`, `remote_command`, `SshConnector`). Held by
`a_remote_tab_never_starts_a_local_shell_or_takes_a_spare_and_carries_no_token`,
`the_ssh_command_is_argv_items_quoted_for_the_hosts_shell` and
`machines_json_keeps_good_machines_and_reads_an_unknown_kind_as_none`. Source: remote sessions PR 3 (2026-11-12).
291. **A remote spawn never uses a warm spare and never mints a `FLARE_TAB_TOKEN`.** A spare is a local shell with a
local secret; a remote tab adopts none and makes none (`spawn_remote_reserved` reads no shell list, takes no spare and
makes no `ShellTokens`), and the windows-only kill answer to ConPTY's opening questions is never sent to a remote tab's
writer (it would read as keys). Native: `crates/flare-term/src/session.rs`. Held by
`a_remote_tab_never_starts_a_local_shell_or_takes_a_spare_and_carries_no_token` and
`the_quit_detaches_a_remote_tab_and_a_close_closes_it`. Source: remote sessions PR 3 (2026-11-12).
292. **A paste on a remote tab always crosses as one paste.** A snapshot is text and carries none of the host's modes,
so after a restore or a gap the viewer's emulator may not know the host's shell asked for bracketed paste. A paste
framed by that stale mode went out unbracketed, `decode_typing` cut it into text and Enter keys, and the host shell ran
the first line at once. `TermHandle::paste` frames every paste on a remote tab, so the link sends one `Paste` intent
and the host brackets it with its own mode (rule 279); `paste_unsent` frames its one line too. Native:
`crates/flare-term/src/handle.rs` (`paste`, `paste_unsent`). Held by
`a_paste_on_a_remote_tab_crosses_as_one_paste_whatever_the_local_mode`. Source: review of remote sessions PR 3
(2026-11-12).
293. **A remote snapshot puts the viewer's cursor where the host's is.** Every byte after a snapshot is relative to the
host's cursor. The snapshot trimmed trailing blank lines and spaces, so the viewer's cursor sat at the end of the last
text (one column left of a `% ` prompt), and the echo and any relative redraw of a TUI landed on the wrong cells.
`remote_snapshot` now sends the whole screen (blank rows below the prompt included) with history above it, so its last
line is the screen's bottom row, then `snapshot_cursor`: CR, a move up and a move right to the host's cursor.
`plain_snapshot` keeps those two moves (`ESC [ n A`, `ESC [ n C`) and nothing else, so rule 289 holds. Native:
`crates/flare-term/src/handle.rs` (`remote_snapshot`), `remote_pty.rs` (`snapshot_cursor`, `plain_snapshot`). Held by
`a_snapshot_puts_the_viewers_cursor_where_the_hosts_is` and `a_snapshot_keeps_only_the_cursor_moves_up_and_right`.
Source: review of remote sessions PR 3 (2026-11-12).
294. **Remote status is host-derived words only; done and failed hold on the viewer's own looking.** A remote tab's
processes and screen are the host's (rule 289), so the host says what its own marks say: `remote.status` answers each
shell tab with one state word (`question`, `failed`, `working`, `done`, `idle`, in that order of weight), the agent,
its session, the folder, the title, the question's words and the failure's kind, and nothing it read from a file. The
viewer's link polls it every 1.5 s (at once after each attach) and hands the window its own tab's entry when it
changed; the hub applies a word only when it differs from the last one applied. Work and a question are shown as the
host says them (the tab holds an agent). A finished or failed turn is never copied: when the host stops working, the
viewer raises its own Finished or Failed mark only if it showed the tab working or asking and nobody here looks at it,
and its own looking holds and clears it (rule 61). So a status read again after a reconnect raises nothing twice, and
a host `done` the viewer never saw working raises nothing. A word the viewer does not know is no news (rule 40); a
host with no `remote.status` keeps its link and is not asked again on it. Native: `crates/flare-ctl/src/remote.rs`
(`TabState`), `crates/flare/src/appctl/remote_glue.rs` (`remote_status_tabs`, `sync_remote_status`),
`crates/flare/src/remote_status.rs`, `crates/flare/src/remote_link.rs` (`poll_status`), `agent_hub.rs`
(`on_remote_status`), `crates/flare-agent/src/indicator.rs` (`remote_mark`). Held by
`a_tabs_status_word_follows_the_weight_of_its_marks`, `the_status_number_grows_only_when_the_status_changes`,
`remote_status_is_the_windows_words_numbered_on_the_worker`, `remote_status_says_the_windows_own_marks_as_words`,
`each_host_state_word_maps_to_one_mark_and_an_unknown_word_to_none`,
`a_status_answer_is_read_for_the_one_host_tab_and_only_as_words`,
`the_poll_is_due_every_period_and_hands_on_only_a_change`,
`the_hosts_status_of_the_tab_reaches_the_window_on_change_and_wakes_it`,
`an_older_host_without_remote_status_keeps_the_link_and_is_asked_once`,
`a_remote_status_shows_work_and_questions_and_its_end_is_never_copied_twice` and
`a_remote_tabs_host_status_marks_it_and_a_reconnect_raises_nothing_twice`. Source: remote sessions PR 4 (2026-11-12).
295. **One notification per question across both machines.** A question in a tab a remote viewer follows is announced on
the viewer: its hub holds the question mark (live OSC 777, or the host's status) with the host's words for it, and the
viewer's notifier shows it once per episode as for any tab (rule 206). The host's notifier leaves out every tab a
remote viewer follows, so the same question never rings on both machines; a tab nobody follows notifies on the host as
before, and the host's strip still shows the mark. Native: `crates/flare/src/appctl/surfaces7.rs` (`notify_update`),
`agent_hub.rs` (`question_text` of a remote tab). Held by `a_question_in_a_followed_tab_is_the_viewers_to_notify` and
`a_remote_tabs_host_status_marks_it_and_a_reconnect_raises_nothing_twice`. Source: remote sessions PR 4 (2026-11-12).
296. **The host holds sleep only while a remote-followed session works, and lets go on every exit path.** Owner
decision 2026-11-12: the machine that runs a remote session stays awake while it works, and sleeps as usual once all
are idle. The window holds one `Hold` while any tab a remote viewer follows has an agent at work (`working` and an
agent in it), checked at every refresh, attach and release; it lets go when the work stops, when the last viewer
detaches or its link drops (the release), and when the window ends (the hold is dropped with it). Holds are counted, so
the system is asked once on the first and once after the last. Windows: one long-lived thread calls
`SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)` and `SetThreadExecutionState(ES_CONTINUOUS)` (the state
is per thread and ends with it); macOS: one IOKit `PreventUserIdleSystemSleep` assertion, released after the last hold
and by the system when the process ends. Only system sleep is held: the display still sleeps and the screen still
locks. Under `--e2e` a counter stands in. Native: `crates/flare-win/src/keep_awake.rs`, `macos/keep_awake.rs`,
`crates/flare/src/appctl/remote_glue.rs` (`sync_keep_awake`). Held by
`the_system_is_held_from_the_first_hold_to_the_last_drop`, `the_fake_counts_holds_and_a_drop_lets_go`,
`the_system_keep_awake_takes_and_lets_go_without_waiting`,
`a_followed_tab_holds_the_host_awake_while_it_works_and_lets_go_after` and, on a real Mac (CI's macOS job),
`keep_awake::sys::tests::a_hold_makes_one_real_assertion_and_its_last_drop_releases_it` (`pmset -g assertions` lists
the process's one Flare assertion while held and none after). Source: remote sessions PR 4 (2026-11-12).
297. **Conversation entries cross the link as numbered `Entry` values; no host path does.** `remote.convo {tab, since}`
answers the followed tab's Claude session as entries (role, text, tool, time) the host numbers from 1 per tab. The host
finds the transcript from the tab's session and folder with its own Claude folder, so a Mac's project slug and a PC's
are each read on their own machine; it reads the last 200 entries once (`said::read_last_until`, up to the byte its
tail then follows from, so no line is read twice or skipped) and then what is written after them, on the call's
worker. `reset` tells the viewer to start over when the transcript was cut or rewritten, the session changed, its
number fell behind the 200 kept, or the number is one this host never gave. The viewer (`ConvoMirror`) takes each
entry once by its number, so a poll sent again after a reconnect adds nothing twice. Native:
`crates/flare/src/remote_convo.rs`, `crates/flare/src/ctl_app.rs` (`remote_convo`), `crates/flare-convo/src/said.rs`
(`read_last_until`). Held by `the_host_numbers_the_backfill_then_each_new_entry_once`,
`a_reconnect_with_since_adds_no_entry_twice_on_the_viewer`, `a_cut_transcript_resets_the_viewer_and_the_numbers_go_on`,
`another_session_or_a_number_this_host_never_gave_is_a_reset`, `a_viewer_behind_the_kept_window_starts_over_from_it`,
`the_wire_form_carries_numbers_and_no_path`, `remote_convo_sends_the_hosts_transcript_as_numbered_entries_and_no_path`
and `read_last_until_stops_at_the_byte_it_is_given_and_drops_a_line_cut_there`. Source: remote sessions PR 4
(2026-11-12).
298. **A remote tab's status is never lost to a passing refusal, a host that looked, or a mark taken down here.** Three
holes in rule 294, each one a question or an end of work that rang on neither machine (rule 295 keeps the host quiet for
a followed tab). (a) Only `unknown_method` (an older host) turns the `remote.status` polls off for the link; any other
refusal (the host's 10 s UI wait timed out, a busy or rate refusal) is logged and the next poll asks again. (b) A host
whose window is focused on the tab raises no done mark of its own and says `idle` straight after `working`, so the
viewer takes `idle` as it takes `done`: it raises its own Finished mark when it showed the tab working or asking and
nobody here looks, and nothing otherwise. (c) The hub skips the word it applied last only while the tab's marks still
show it: an answer key here that read the mirrored box gone took the question down with the host's word still
`question`, and the host's next question (new words, no `working` seen between two polls) raises it again. Native:
`crates/flare/src/remote_link.rs` (`poll_status`), `crates/flare-agent/src/indicator.rs` (`remote_mark`),
`crates/flare/src/agent_hub.rs` (`on_remote_status`, `remote_marks_show`). Held by
`a_passing_status_refusal_keeps_the_polls_and_a_later_question_still_arrives`,
`a_remote_idle_after_work_shown_here_raises_the_viewers_own_finished_mark` and
`a_remote_question_answered_here_is_raised_again_by_the_hosts_next_question`. Source: review of remote sessions PR 4
(2026-11-12).
299. **An agent may open a remote session; it may not type into it.** Amends S3 (rule 188). Owner decision: an agent
may start a session on another machine with no prompt. `open.remote` (the agent's `open_remote_session` tool and
`flare open --on`) is the agent grant's and the app's only, in a namespace of its own (`open`) that no permission opens,
so no mod, remote link or bridge reaches it. The tab it opens takes no typing from the agent: the agent grant still has
no `remote.*`, `shell.input` or `tabs.input`, and it names no tab but its own. The agent's open leaves the front tab in
front, so what the person types there never lands in the new session; only the command (`front`) brings it forward. The
machine must be one `machines.json` names (`not_found` lists them); the folder is the other machine's and is never
statted or opened here. Native: `crates/flare-ctl/src/grant.rs`, `crates/flare/src/ctl_app.rs` (`open_remote`),
`crates/flare/src/appctl.rs` (`open_remote`), `crates/flare-mcp/src/lib.rs` (`open_remote_session`). Held by
`the_agent_may_open_a_remote_session_but_never_type_into_one`, `agent_can_show_and_drive_but_not_eval`,
`open_remote_names_a_known_machine_and_opens_a_tab_the_agent_cannot_type_into`,
`an_agents_remote_open_stays_behind_the_tab_in_front` and
`open_remote_session_checks_its_arguments_and_routes_to_open_remote`. Source: remote sessions PR 5 (2026-11-12).
300. **`--on` and other flag values never reach path parsing.** `flare open --on <machine> <folder> [--resume <id>]` reads
`--on` and `--resume` as values in `config.rs`, takes `open` and the folder out of the bare words, and runs before the
lock, the hand-off (whose `paths_from_argv` reads the raw command line for folders) and any window, then exits. So a
machine name or a session id is never opened as a folder here, and the host folder is never statted on this machine.
`--user-data-dir` is untouched. Native: `crates/flare/src/config.rs` (`open_on`), `crates/flare/src/main.rs`,
`crates/flare/src/cli_open.rs`. Held by `open_on_reads_its_values_and_leaves_no_folder_to_open`,
`open_on_refuses_what_it_cannot_use_with_its_usage`, `open_on_branches_before_the_hand_off_and_the_window` and
`the_command_asks_the_running_flare_as_the_agent_for_a_tab_in_front`. Source: remote sessions PR 5 (2026-11-12).
301. **`--start-hidden` never raises or takes the focus; it uses the e2e's park code.** When no Flare runs, the serve
process (before it answers `remote.hello`) and `flare open` start `<this exe> --start-hidden` and wait up to 60 s for its
pipe. The window is born parked (the e2e's attributes: inactive, off the taskbar, at the parking spot) and parked again
with the no-activate tool-window style by the same timer, and its parked place is never saved as the person's. A
`--start-hidden` launch that meets a Flare running hands nothing over and raises nothing. The first raise (a normal
launch later) takes the window off the parking spot with its normal style and its taskbar button. A Windows process in
session 0 (sshd's logins) starts nothing, and a host with no Flare to be had answers the hello `host_not_ready` in one
line, which the viewer shows once. The flag is `--start-hidden`, not the plan's `--background`: that name is the
background colour's look flag. Native: `crates/flare/src/hidden_start.rs`, `crates/flare/src/serve_remote.rs`
(`remote_ready`), `crates/flare/src/window.rs` (`parked_start`, `unpark_background`), `crates/flare/src/appctl.rs`
(`handoff`), `crates/flare-ctl/src/session.rs` (`Dispatcher::remote_ready`). Held by
`with_none_running_one_is_started_once_and_its_pipe_awaited`, `a_running_flare_is_used_and_nothing_is_started`,
`a_pipe_that_never_opens_is_host_not_ready_in_one_line`, `with_no_desktop_nothing_is_started_and_the_line_says_what_to_do`,
`the_hello_waits_for_a_flare_here_and_a_host_that_cannot_start_one_says_so`,
`a_host_that_is_not_ready_refuses_the_hello_in_one_line_and_takes_another`,
`a_background_start_is_parked_like_the_e2e_and_its_raise_undoes_the_style`,
`a_background_launch_that_meets_a_running_flare_raises_and_opens_nothing` and the e2e `remoteOpen`. Source: remote
sessions PR 5 (2026-11-12).

302. **A Mac `--start-hidden` run is held clear from its first window, and its first raise lets go of all of it.** Amends
rule 301 for macOS. A Mac draws nothing for a window on no screen, so the park puts it on the main screen at alpha 0
(rule 1109). `--start-hidden` therefore installs the e2e's whole Mac park before the first window is made, as `--e2e`
does (`keep_windows_off_screen`: no constraint to a screen, born clear, kept clear against Slint's reveal on the first
GPU frame), not App Nap's hold alone; without it the window showed as a strip at a screen edge, then at full alpha on
the host's screen. The hold is one flag (`window_park::HELD`) that an `--e2e` run never clears. The first raise calls
`window_park::unpark` before the window moves: the flag clears, so every stand-in passes on what it is asked and AppKit
constrains frames again; the app is a regular app (a Dock icon); every live window gets back the alpha and click
pass-through its own code last asked while held (opaque and taking clicks when it asked nothing); and the main window
is opaque, takes clicks and has its shadow. A park that comes after the raise parks nothing. Native:
`crates/flare/src/main.rs`, `crates/flare/src/window.rs` (`unpark_background`),
`crates/flare-win/src/macos/window_park.rs` (`unpark`, `Wanted`). Held by
`a_background_start_on_a_mac_is_born_clear_and_its_raise_undoes_the_mac_park` and, in macOS CI,
`a_held_park_keeps_windows_clear_and_a_released_one_passes_the_ask_on`,
`a_window_gets_back_its_last_ask_and_opaque_with_clicks_when_it_asked_nothing` and
`off_the_main_thread_nothing_is_touched`. Source: the review of remote sessions PR 5 (2026-11-12).

303. **A remote path is never opened as a local path; a host file comes through `remote.fetch` and the host's
allowlist.** Extends S2 (rule 187) and rules 157, 266 to 275 for a remote tab. A click on a file link in a remote tab is
never resolved, statted, previewed or opened on the viewer (its text names a file of the host): the viewer hands its
spellings to the tab's link, which asks `remote.fetch` for each in turn. The host resolves the path as a click there
would (relative to its folder of the tab, `~` from its home) and reads it only when the file, as its file system names
it (links followed, `..` resolved), is a plain file inside the tab's folder or the host's temp folder, with no device
name and no network path; anything else is `not_found`, and a file over 32 MiB `too_large`. It is read in 256 KiB
chunks on the call's worker, and the viewer checks each chunk is of the same file. The viewer keeps the bytes as a copy
in its own temp folder (`flare-remote-copies/<run>/<tab>`, removed when the tab closes) and shows the copy in the side
panel when it is a picture or a text file, or opens it as a clicked file of this machine otherwise (the click
allowlist, so a program is revealed and never run, rule 14). File work asked while the link is down, or after the tab's
session ended, is refused at once, never held. Native: `crates/flare/src/remote_files.rs` (`resolve_fetch`,
`read_chunk`, `Copies`), `crates/flare/src/ctl_app.rs` (`remote_fetch`), `crates/flare/src/remote_link.rs`
(`fetch_file`, the worker's errands), `crates/flare-term/src/remote_pty.rs` (`Errand`), `crates/flare/src/term_ui.rs`
(`click_link`), `crates/flare/src/appctl/remote_files_glue.rs` (`remote_fetch_link`). Held by
`a_fetch_reads_the_tabs_folder_and_the_temp_folder_and_nothing_else`, `a_link_out_of_the_tabs_folder_is_refused`,
`device_names_and_shares_are_never_fetched`, `a_fetched_file_is_read_in_chunks_that_join_up_and_a_big_one_is_refused`,
`remote_fetch_reads_only_the_followed_tabs_folder_in_chunks`,
`a_file_link_is_fetched_by_the_first_spelling_the_host_has_in_chunks`,
`file_work_asked_while_the_link_is_down_or_ended_is_refused_at_once`,
`errands_wait_in_order_and_none_is_lost_when_the_link_ends`,
`a_click_on_a_remote_tabs_file_link_asks_the_link_and_opens_nothing_here`,
`a_remote_file_link_is_fetched_by_the_link_and_shown_from_a_copy_in_the_panel`,
`a_remote_file_link_the_host_does_not_have_is_said_in_the_panel` and the e2e `remoteFiles`. Source: remote sessions
PR 6 (2026-11-12); owner decision "a link to a file on the other machine is fetched over SSH and shown in the side
panel; Open saves a copy and opens it locally".
304. **A paste or drop of files or a picture on a remote tab copies them to a host temp folder and pastes a quoted host
path.** A file dropped on a remote tab, copied files pasted there and a clipboard picture pasted there are never typed
as this machine's paths, and no image key is sent (the host's program would read the host's clipboard). The viewer
reads the files (only files, at most 32 MiB each; a picture is sent as a PNG, a bitmap made one) on a worker and the
tab's link copies each with `remote.put` into the tab's own upload folder on the host (`<temp>/flare-remote/<run>/<tab>`,
removed when the tab closes; a crashed run's folders go a day later). The host cleans the name (`put_name`: last part
only, letters, digits, space and `-_.()+,`, no device name, at most 100 bytes) and makes it unique, writes the file
whole before it is in place, and answers its path quoted for the tab's own shell (`quote_path`); the link then pastes
those paths as one `paste` intent, sanitised on the host as any paste (rule 1, rule 292). A put chunk is never sent
twice. Native: `crates/flare-ctl/src/remote.rs` (`put_name`, `FILE_*`), `crates/flare/src/remote_files.rs`
(`Uploads`), `crates/flare/src/ctl_app.rs` (`remote_put`), `crates/flare/src/remote_link.rs` (`put_files`),
`crates/flare/src/term_ui.rs` (`on_dropped`, the drop flush, `TermMsg::Clip`),
`crates/flare/src/appctl/remote_files_glue.rs` (`remote_upload`, `png_of`), `crates/flare-win/src/clipboard_port.rs`
(`read_image`, `bmp_of_dib`), `crates/flare-win/src/macos/pasteboard.rs` (`read_png`). Held by
`a_put_name_is_its_last_part_with_nothing_a_shell_acts_on`, `an_upload_lands_whole_in_the_tabs_folder_under_a_clean_unique_name`,
`a_closed_tabs_uploads_are_removed_and_old_runs_are_swept`,
`remote_put_lands_in_the_tabs_upload_folder_and_answers_a_quoted_path`,
`a_put_copies_the_files_then_pastes_the_hosts_quoted_paths_as_one_paste`, `a_file_over_the_cap_is_never_sent`,
`a_broken_pipe_is_opened_again_for_a_call_that_changes_nothing`,
`a_drop_on_a_remote_tab_is_never_typed_and_goes_to_the_app_to_copy`,
`a_paste_of_files_or_an_image_on_a_remote_tab_goes_to_the_app_to_copy`,
`a_file_dropped_on_a_remote_tab_goes_to_its_link_and_is_never_typed_here`,
`an_image_pasted_on_a_remote_tab_goes_to_the_host_as_a_png`, `a_pasted_image_goes_as_a_png`,
`a_dib_becomes_a_bmp_file_with_its_pixels_where_the_header_says` and the e2e `remoteFiles`. Source: remote sessions
PR 6 (2026-11-12); the design's "copied to a per-session temp folder on the host first, then the host path is pasted".
305. **The ssh command line is built per host OS from separate argv items and carries no secret.** Amends rule 290 with
the full table: a Mac's (and an unknown kind's) login shell gets the Flare path in single quotes (nothing expands
there, a `'` written `'\''`); a PC's cmd gets it in double quotes with no `&&`, and a path holding `%`, `!`, `^` or `"`
(which cmd still acts on inside quotes) or a control character builds no command at all. Host and user are argv items
of their own that cannot be read as an option. Native: `crates/flare/src/remote_link.rs` (`remote_command`,
`ssh_argv`). Held by `the_remote_command_table_quotes_each_hosts_shell` and
`the_ssh_command_is_argv_items_quoted_for_the_hosts_shell`. Source: remote sessions PR 6 (2026-11-12).

The UI rules of remote sessions PR 7 are numbered after main's last rule (1189) and the PR 6 fixes (1190, 1191). The
plan's R-new-26 (the folder browser) was dropped with the browser itself: the owner's + menu opens a shell on the
machine and nothing else.

1192. **"Close" on a remote tab ends the session everywhere and asks as any close does; "Hide here only" never ends
it.** Owner decision: closing a remote tab is closing the tab, on both machines (`remote.close`, which asks first in the
host's window while an agent works there, rule 48). The remote tab's right-click menu adds "Hide here only" right
after "Close tab": the tab goes from this window and its pty lets go the quit's way (`remote.detach`, rule 288), so its
session goes on on its machine and nothing asks. A local tab has no such row. Native:
`crates/flare/src/appctl/remote_glue.rs` (`hide_remote`), `crates/flare/src/tab_host.rs` (`TabHost::leave`),
`crates/flare-term/src/session.rs` (`SessionManager::leave`), `crates/flare/src/menus_ui.rs` (`HIDE_HERE`). Held by
`ui_tests::term::flow::hide_here_only_detaches_and_close_ends_the_session_everywhere` and the e2e `remoteUi`. Source:
remote sessions PR 7 (2026-11-12), plan R-new-25.
1193. **While the other machine sizes a remote tab, this screen shows its conversation fitted to the window, never a
terminal at the other window's size; a click or a key takes the terminal back.** The live terminal has one size, the
last typist's (rule 282). The host's `remote.status` says who that is (`sized`: `host` or `viewer`). While the host
sizes a remote tab that has a Claude session there, the viewer's link polls `remote.convo` (only then) and the tab in
front shows the conversation, wrapped to this window's width, newest at the foot, under one line that names the
machine. A click on it, or any key, takes the terminal back: the view goes, the link sends this tab's size again
(`remote.resize`, so this window sizes it), the polls stop, and the key goes on to the tab. The view comes back only
when the host says it sizes the tab again after saying this viewer did. A host that does not say `sized` (an older
Flare) is read as the viewer sizing it, so the terminal stays. Words only cross the link (rule 297). Native:
`crates/flare/src/appctl/remote_glue.rs` (`RemoteConvoView`, `sync_remote_convo`, `remote_convo_take_over`),
`crates/flare/src/remote_link.rs` (`poll_convo`, the retake), `crates/flare-term/src/remote_pty.rs` (`RemoteLive`'s
conversation and retake), `crates/flare/ui/remote_convo.slint`. Held by
`ui_tests::term::flow::a_remote_tab_the_host_sizes_shows_its_conversation_until_a_key_takes_it_back`,
`ui_tests::remote_convo::*` (fitted at three widths and two scales) and `remote_status::tests` (`sized`). Source: remote
sessions PR 7 (2026-11-12), owner decision "the other screen".
1194. **Machines are saved whole and at once, only a machine that answered is added, and nothing read from the ssh
config reaches ssh as an option.** Settings > Machines writes `machines.json` with a temp file and a rename (rule 36),
on a worker, and reads a `kind` it does not know as none, never written back as a guess (rule 40). Adding a machine
checks its link first (`remote.hello` over its own ssh command, the e2e's loopback in a test, never a real ssh) and
saves nothing when no Flare answered or the protocols differ; the failure is one red line under the form that says what
to do (`Flare there speaks remote protocol 0, here 1. Update it there.`). The form's hosts are the plain `Host` names of
`~/.ssh/config` and its `Include`s: a pattern, a negation, a `Match` block and any name that is not one plain argv word
(a space, a leading `-`) are left out (rule 305). The host's own `remote.hello` says its system (`os`), which picks the
logo and the login shell's quoting. Native: `crates/flare/src/machines.rs`, `crates/flare/src/machines_page.rs`,
`crates/flare/src/ssh_config.rs`, `crates/flare-settings/src/page/machines.rs`. Held by
`machines::tests::a_saved_list_reads_back_the_same_and_leaves_no_temp_file`,
`machines::tests::an_unknown_kind_reads_as_none_and_is_written_as_none`,
`machines_page::tests::a_failed_add_saves_nothing_and_shows_one_line_under_the_form`, `ssh_config::tests::*` and
`ui_tests::settings_machines::*`. Source: remote sessions PR 7 (2026-11-12), plan R-new-27.
1196. **macOS: the serve process is no app.** `flare --serve-remote` is what ssh runs on a Mac host, in a login that
may have no desktop. `main` branches to it (and to `flare open --on`) before `flare_web::host::prepare_app()`, so it
makes no `NSApplication`: it never checks in with LaunchServices, never reaches for the window server and owns no
window (before, every serve process checked in as a background app, `lsappinfo` listed it). Native:
`crates/flare/src/main.rs`. Held by `serve_order_tests::serve_remote_branches_before_the_lock_the_e2e_and_the_window`,
`serve_order_tests::open_on_branches_before_the_hand_off_and_the_window` and the process test
`a_mac_serve_process_is_no_app_and_owns_no_window` (`crates/flare/tests/serve_remote.rs`, which now runs on macOS
too, with `a_serve_process_takes_no_single_instance_lock` checking that a second probe meets the first's lock).
Source: the Mac parity audit of remote sessions (2026-11-13).
1197. **A Flare started hidden from an ssh login carries nothing of that login.** It outlives the login, and so do the
shells it starts later. `hidden_start::login_env` drops `SSH_CONNECTION`, `SSH_CLIENT`, `SSH_TTY`,
`SSH_ORIGINAL_COMMAND` and an `SSH_AUTH_SOCK` that sshd made for this login (`<tmp>/ssh-*/agent.<pid>`, gone with
it; the desktop's agent socket stays), and, when the login has no `TMPDIR` (sshd sets none on a Mac), gives it the
user's private temp folder (`confstr(_CS_DARWIN_USER_TEMP_DIR)`), as the Dock does, so its uploads (rule 304) are
never in the shared `/tmp`. On a Mac the started program is the serve process's own resolved (`canonicalize`): a
`flare` reached by a link into `Flare.app` starts the bundle's program, whose web engine and helpers sit next to it.
Native: `crates/flare/src/hidden_start.rs`. Held by `hidden_start::tests::the_started_flare_carries_nothing_of_the_ssh_login`,
`only_a_forwarded_agent_socket_is_dropped`, `a_login_with_no_tmpdir_gets_the_users_own_temp_folder`,
`a_mac_starts_the_program_a_link_points_to` and `the_mac_users_temp_folder_is_the_systems_private_one`. Source: the Mac
parity audit of remote sessions (2026-11-13).
1198. **macOS: a picture copied as a TIFF alone is pasted on a remote tab as a PNG.** Preview, Pages and many other
apps put only a TIFF on the pasteboard. `pasteboard::read_png` reads the PNG when there is one and else makes the TIFF
a PNG with AppKit (`NSBitmapImageRep`), the cap (`IMAGE_READ_MAX`) holding for both; before, a TIFF-only picture was
refused with one line and not sent. Native: `crates/flare-win/src/macos/pasteboard.rs`. Held by
`pasteboard::tests::a_tiff_only_picture_is_read_as_a_png_of_the_same_size`. Source: the Mac parity audit of remote
sessions (2026-11-13).
1199. **A Mac names itself in `remote.hello` by its host name.** No shell exports `HOSTNAME` to an app or to an ssh
command on a Mac, so the viewer's hello said "viewer" and the serve process's said nothing. `session::default_machine_name`
falls back to `gethostname` there; the viewer's `remote_link::this_machine` uses the same rule. Native:
`crates/flare-ctl/src/session.rs`, `crates/flare/src/remote_link.rs`. Held by
`session::tests::a_mac_has_its_host_name_when_the_environment_names_none`. Source: the Mac parity audit of remote
sessions (2026-11-13).

## macOS port

The rules the macOS port stack added (PRs #121 to #140) are numbered from 1001 so they never clash with main: main's
numbering continues below 1000. When the stack merges, this block is renumbered into the main sequence, or kept as it
is. A gap in the numbers is a rule of a sibling branch of the stack.

1001. **macOS: closing a tab ends its whole session, and throws away the output nobody will read.** The shell leads
its own session (portable-pty `setsid`); a kill sends SIGHUP to the session's process groups and to the shell's
descendants, then SIGTERM and SIGKILL after 250 ms and 500 ms more (`flare-term/src/macos/pty.rs`), sparing an
`.app` bundle the shell launched. While it waits the reaper runs `tcflush(TCOFLUSH)` on a duplicate of the master:
macOS makes an exiting process wait until its terminal's output is read, and the tab's reader has stopped, so
without the flush the shell hangs in its exit (`ps` state `E`) and the quit wait runs out. Every process a round
found to be the tab's (pid and start time) stays a target while it is that process, so one whose parent exits after
it called `setsid` is still ended. The quit (`shells_gone`) also hurries the reapers to SIGTERM and SIGKILL and waits
for them (well under a second), so a job that ignores SIGHUP never outlives Flare. Known gaps, with no macOS
mechanism to close them (no job object; `NOTE_TRACK` is gone, and since Darwin 25 `KERN_PROCARGS2` no longer hands
out another process's environment, checked 2026-11-08 on 27.2, so a tab marker cannot be read back): a daemon that
forks, calls `setsid` and forks again before any round sees it (parent launchd, a session of its own) survives the
tab, and a crash of Flare leaves a job that ignores SIGHUP running. Held by `quit_macos.rs`
(`ten_real_shells_with_jobs_killed_none_remain`,
`quitting_ends_a_job_that_ignores_the_hangup_before_the_process_exits`) and `pty::macos::tests`. Source: macOS port
phase 2, step 1 (measured 2026-11-08: 6 of 10 shells stuck in exit without the flush), and the phase 2 review.
1002. **macOS: every shell starts as a login shell and reports its folder with OSC 7, without touching the user's
dotfiles.** zsh, bash, fish and pwsh are found by absolute path (the login shell from the user record first, then
Homebrew, `/etc/shells`, `/bin`), never through PATH; the login shell is listed first and is the default
(`shell_by_id` prefers no name on macOS). zsh starts with `ZDOTDIR` on Flare's shim, whose `.zshenv` hands the
user's `ZDOTDIR` straight back and sources their `.zshenv` (the user's value waits in `FLARE_USER_ZDOTDIR`); bash
gets `--rcfile` with a file that reads the login files (bash ignores `--rcfile` with `-l`); fish gets
`-l --init-command`; pwsh gets `-Login` first and the Windows prompt wrapper reporting OSC 7. A missing shim is
never handed to zsh (it would read none of the user's files). OSC 7 counts only for this machine (empty host,
`localhost` or the host name; Flare's own hooks send no host, since zsh and bash read the name once at startup and
a Mac named by its network can be renamed mid-session), decoded, absolute, with no control character or `.`/`..` segment; on Windows
OSC 7 is no folder report. The files live in `~/Library/Application Support/Flare/shell-integration/<version>`
(`flare-shells/src/macos/`). Held by `flare-term/tests/real_shells_macos.rs` and `flare-shells` `macos::*` tests.
Source: macOS port phase 2, step 2 (2026-11-08).
1003. **macOS: paths are read with the POSIX grammar behind one seam, and a path that would reach a server is never
statted, restored or adopted.** `flare-term/src/path_grammar.rs` picks the grammar: Windows
(`path_grammar/windows.rs`, the drive and UNC rules moved there unchanged) or macOS (`macos/path_grammar.rs`): one
root `/`, `~` and `~/` for home (`~user` is refused), `/` the only separator (a `\` is a name character), case kept,
`..` never above the root. (REPLACED 2026-11-11 by rules 266 to 269: terminal text no longer links a `file://` URL or a word
whose spaces are escaped with `\`; no path is read out of terminal text.) A `//` path and anything under an automount point
(`/net`, `/Network`, `/home`, each also under `/System/Volumes/Data`, and any mount of type `autofs` that
`getfsstat` lists; any case, also after `..`) is never resolved, statted or read in a panel (`flare-surfaces`
`preview::loadable`). One function holds that refusal, `flare_shells::macos::paths::is_automount`, used by the
link grammar, `is_remote` and the panels' `is_local_path`, which also refuses a `.` or `..` segment; a mounted volume (`/Volumes/<name>`) gets its own slow-volume key in the
stat meter. A tab's reported folder and a restored one go through `flare_shells::cwd::is_remote`: on macOS it
refuses the same autofs and `//` paths, a `.` or `..` segment and the Windows UNC forms. The sidebar line and the +
menu read POSIX folders (`~/code`, `.../Claude/Github`). Held by the `posix` twins in `path_open`, the `macos::*` tests of flare-term, flare-shells, flare-tabs and flare-surfaces, and
`restore_never_spawns_in_a_remote_folder_on_macos`. Source: macOS port phase 2, step 3 (2026-11-08).
1004. **macOS: fonts come from the font folders, the stacks fall to SF Mono and Menlo, the UI is SF, and wgpu
draws on Metal.** `flare-render/src/macos/font_catalog.rs` reads `/System/Library/Fonts`, the downloadable
`AssetsV2` fonts, `/Library/Fonts` and `~/Library/Fonts` once (never `/Network/Library/Fonts`, an autofs mount). Its
start-up fallbacks are SF Mono, Menlo and every face cosmic-text's macOS fallback names (per-script, Han, common):
Arial Unicode MS covers most scripts, so a script face left lazy would lose to it and draw differently from an
all-fonts build. The generic `monospace` at the end of a stack is SF Mono, Menlo, Monaco (`MONOSPACE_FAMILIES`;
empty on Windows). The Slint faces come from `ui/platform_fonts.slint`: "System Font" (SF) and Menlo on macOS,
Segoe UI and Cascadia Mono on Windows; Helvetica, Slint's own fallback, has a one-em line and shrinks every row.
`flare_render::gpu::NATIVE_BACKEND` is DX12 on Windows and Metal on macOS, for the prewarm instance and the headless
painter. Held by `font_catalog::macos::tests`, `glyph_images_are_identical_to_the_all_fonts_build_for_the_theme_sample`,
`tests/metal_macos.rs`, `the_prewarm_instance_starts_metal_alone_unless_a_backend_is_named` and the `_on_macos`
twins of the Windows pixel tests. Source: macOS port phase 2, step 4 (2026-11-08).
1005. **macOS: the window is a native titled window under Flare's top row, and its glass is an effect view behind
the content.** `flare::backend::mac_chrome_attributes` makes the main window titled with a transparent titlebar, a
hidden title and a full-size content view; `no-frame` is off on macOS (`native-caption`), so Slint keeps the
decorations on and winit never swaps the style mask (that would drop the full-size content view). Flare's own
minimise, maximise and close buttons and the mark make way for the traffic lights, which `flare_win::chrome`
places in the 36 pt top row (container `CAPTION_GAP` wide, re-placed on every resize and on leaving full screen;
AppKit puts them back at its own 28 pt row otherwise). AppKit passes every point of the row but the lights to
Flare, which moves the window only by its `WindowMoveArea`s; a double click there does what System Settings says
(`AppleActionOnDoubleClick`). Resize edges, the shadow, full screen and zoom are AppKit's (`resize_edges` installs
nothing). The glass: `dwm::set_system_backdrop` and `set_immersive_dark` land in `flare_win::glass` on macOS, an
`NSVisualEffectView` (`flare.glass`) put BELOW winit's content view in the frame view (never replacing it: winit
casts `contentView` back to its own class), Acrylic as the under-window-background material, the window wearing
dark or light Aqua by the ground. The handle for these calls is `window::native_view_of` (the `NSView`);
`hwnd_of` stays Win32 only. The session end is `NSApplicationWillTerminateNotification` (Quit, log out, shut
down), run once, and `NSWorkspaceWillPowerOffNotification`, which flushes every time (rule 1008). Held by `crates/flare-win/tests/appkit.rs` (a
main-thread harness on a window that is never shown) and the `chrome`, `glass`, `session_end` and `window_park`
tests. Source: macOS port phase 3, step 1 (2026-11-08).
1006. **macOS: Cmd is the app's key, Ctrl is the shell's, and the menu bar is the keymap.** One chord table serves
both systems: `KeyContext::os` (`flare_term::keys::KeyOs`, default Windows) picks the modifier, so on macOS every
app chord is a Cmd chord and every Ctrl+letter reaches the shell as a control character; Ctrl+Tab and the agent
guard's Ctrl+C and Ctrl+D stay Ctrl on both (Cmd going down on its own, `"Super"` in winit's names, leaves a
Ctrl+Tab walk open on the Mac; Windows keeps its old list), Alt+Space is Windows only, the zoom keys and Cmd+Shift+[ and ] are
`MAC_ONLY`. `AppCtl::key_os` is `keymap::app_key_os()`: the Mac in a Mac build, Windows in every unit test, so the
Ctrl tests hold on both systems and the Mac tests set the Mac keys. The terminal's copy, paste and select all
follow `term_key_action_on(os)` (Cmd+C, Cmd+V, Cmd+A on a Mac), so do the Text panel's selection keys
(`AppCtl::panel_text_key`: Cmd+C, Cmd+A, Shift+Cmd with an arrow for a line's or the file's end), and a key left for the shell goes through
`keymap::pty_key`, which on macOS sends nothing for a Cmd key (except the line keys) and gives Option to the
layout unless `flare.term.optionAsMeta` is on. The menu bar (`mac_menu`, built by `flare_win::menu_bar`) never has
an action of its own for a key item: the item IS its key press, sent through `window::handle_key`, and its key
equivalent is made from that press, so the menu and the keyboard cannot drift apart (a key a Slint field has goes
on to Slint as Control, Slint's name for Cmd on macOS). Never add a Mac shortcut in the menu alone. Held by the
`mac_menu`, `chords`, `keymap` and `mac_keys` tests,
`on_the_mac_cmd_c_cmd_a_and_cmd_with_the_arrows_are_the_selections_and_ctrl_is_not` and `crates/flare-win/tests/appkit.rs`. Source: macOS port
phase 3, step 2 (2026-11-08).
1007. **macOS: the system glue is AppKit and the per-user Library, never a helper process.** The clipboard is
`NSPasteboard` (`flare_win::pasteboard`): text as an `NSString` (UTF-8 whatever the locale, no `pbcopy`), copied
files as the items' file URLs read back as POSIX paths, an image only seen (PNG or TIFF), the change count as the
sequence; tests use a private pasteboard and never the user's. The openers are `NSWorkspace`
(`flare_win::workspace`): a web link opens only when `NSURL` itself reads http or https with a host; a file opens in
its default app only after the canonical path is asked of the allowlist again; a folder, and whatever Settings
hands over, only ever opens in a Finder window (`selectFile:inFileViewerRootedAtPath:` or a reveal), never by
`openURL:`, which would launch a bundle such as `.app` (that is why the Mac needs no folder hold). A dropped or
pasted path is quoted for the tab's shell: zsh, bash and sh in POSIX single quotes, fish in its own, tcsh and csh in POSIX
single quotes with every `!` written `\!` (history expansion still acts inside their single quotes), pwsh as on
Windows (`path_shell_of`; the Windows ids keep their Windows meaning). The single instance is an `flock` lock
plus a Unix socket (`Flare.Handoff.<uid>.0.<hash>.sock`, mode 0600) in the per-user temp folder
(`_CS_DARWIN_USER_TEMP_DIR`); the second launch checks the server's uid (libproc, by `LOCAL_PEERPID`, and
`getpeereid`) before it writes a byte, the server checks the client's, a live server on the name is an error and a
dead one's socket is removed. The default profile is `~/Library/Application Support/Flare`
(`paths::roaming_app_data`), its log `~/Library/Logs/Flare` (`startup::logs_dir_of`), caches
`~/Library/Caches/Flare`; a `--user-data-dir` profile keeps everything inside it. Process memory is
`proc_pid_rusage` (resident set, physical footprint). Held by the `pasteboard`, `workspace`, `openers`,
`single_instance`, `startup`, `paths`, `memory` and `flare-proc` tests. Source: macOS port phase 3, step 3
(2026-11-08).
1008. **macOS: a power-off notice flushes but never uses up the final save.** `NSWorkspaceWillPowerOffNotification`
comes before the apps are asked to quit, and another app can still cancel the log out or shut down, leaving Flare
running. So `SessionEndWatch` flushes on it every time without arming anything, and only
`NSApplicationWillTerminateNotification` (the app really ends) is run once. A shared "fired" flag across both
would lose every change made after a cancelled log out. Held by
`the_end_runs_once_a_power_off_never_disarms_it_and_the_power_events_arrive_until_the_watch_stops`
(`crates/flare-win/src/macos/session_end.rs`). Source: macOS port phase 3 review (2026-11-08).
1009. **macOS: a file opens only in a handler that cannot run it.** The extension allowlist is not enough on a Mac: a
single file can carry its own "Open with" app (a `notes.txt` set to open with Terminal runs). `workspace::open_file`
refuses a file with any execute bit, looks the handler up (`URLForApplicationToOpenURL:`), refuses a terminal, a
script runner, an installer or an app without a bundle id (`workspace::RUNNERS`), and opens the file WITH the
checked app (`openURLs:withApplicationAtURL:`), never with `openURL:`. The denylist is the last wall, not the
only one: before it, a file with its own "Open with" choice (`com.apple.LaunchServices.OpenWith`) is refused, and so
is any type that runs, installs or redirects whatever opens it (`RUNNABLE_TYPES`: `.fileloc`, `.webloc`, `.terminal`,
`.command`, `.jar`, `.scpt`, `.workflow`, `.pkg`, `.app` and the like). Add a new terminal, runner or runnable type to
the lists. Held by `terminals_script_runners_and_unknown_apps_never_open_a_file`,
`the_handler_lookup_refuses_a_terminal_and_opens_nothing`,
`a_file_with_its_own_open_with_choice_is_refused_and_a_plain_one_is_not` and
`runnable_and_redirecting_types_are_refused_before_any_handler` (`crates/flare-win/src/macos/workspace.rs`). Source:
macOS port phase 3 review and the security review (2026-11-08).
1010. **macOS: an `--e2e` run is an accessory app from its event loop's first turn.** `backend::with_window_attributes`
hands Slint a winit event loop builder under `--e2e` with the Accessory activation policy and no activation at
launch (`mac_e2e_loop_plan`), so the app never shows a Dock icon or comes forward before `window_park::park`, which
runs only after `show()`. Without `--e2e` Slint keeps its own event loop. Held by
`a_mac_e2e_run_is_an_accessory_that_never_activates_from_its_first_event` (`crates/flare/src/backend.rs`). Source:
macOS port phase 3 review (2026-11-08).
1011. **macOS: every screen is measured in one space, the points times ONE scale.** winit reports a window's position
as points times that window's backing scale, so scaling each screen by its own factor makes a 2x screen's area
overlap the screen beside it. `chrome::areas_in` scales every screen by the same factor; the saved window state is
checked against the areas at its own saved scale (`monitors::work_areas_at`, `WindowStateStore::read_at`), and the
park report measures the screens at the window's scale. On Windows `work_areas_at` is `work_areas` (one device
pixel space already). Held by `screens_of_different_scales_never_overlap` (`crates/flare-win/src/macos/chrome.rs`)
and `the_areas_are_asked_for_in_the_saved_scale` (`crates/flare-win/src/window_state.rs`). Source: macOS port
phase 3 review (2026-11-08).
1012. **macOS: the control channel is a Unix socket that only its user can reach, checked on both ends.**
`flare-ctl/src/uds.rs` exports the named pipe's API (`pipe_name`, `Server`, `connect`, `connect_to_server`,
`PipeEnd`) with the same framing and hashes: the socket is `<per-user temp folder>/flare-ctl/<h>[-<s>].sock`, the
folder mode 0700 and the user's own with no group or other bits (else no server), the socket mode 0600. The server
drops a client whose `getpeereid` uid is not its own before reading a byte; the client refuses a server of another
uid, or (with `FLARE_CTL_PID`) any process but the one named, by `LOCAL_PEERPID`, before writing a byte. A live
server on the path fails the start (rule 1018), a dead socket is removed, stop removes only the socket it bound. Reads are
`recv(MSG_DONTWAIT)` probes, a stop shuts each socket down to free a write stuck on a client that never reads.
Tests cannot switch users, so the refusals are proven with `Server::start_as` and `connect_as`, each end believing
it is another uid. Held by `crates/flare-ctl/tests/pipe.rs` (the Windows tests run on both, `macos::*` twins of
the naming and access tests), `crates/flare-mcp/tests/stdio.rs` and
`appctl::tests::ctl_socket_tests` (every `show_*`, `set_tab_*`, `clear_tab_*`, `open_page`, `close_panel` and
`convo.state` from `flare_mcp` over the socket to the headless window). Source: macOS port phase 4, step 1
(2026-11-08).
1013. **macOS: the port owner table is libproc's, read without a connection, and it sees this user's processes
only.** `flare_proc::tcp_listeners` lists every listening TCP socket of the processes whose descriptors this user
may read (`PROC_PIDLISTFDS`, then `PROC_PIDFDSOCKETINFO`, the `socket_fdinfo` offsets measured with `offsetof` on
the macOS 27.2 SDK and proven by a real listener), never `lsof` and never a connect; `flare_win::tcp_owner` builds
`listener_pids`, `listener_pids_v6` and `tree_look` on it with the Windows rules (one table read for both families,
the tree from `flare_proc::table` with start times, a root's start unknown is no parent). Known gap: a listener of
another account (a root daemon) is not seen, so a port only it holds reads as nobody, never as in the tree, and a
port the tree holds on one loopback family while another account holds the other is caught by rule 1019. A
descriptor list that fills its buffer may have been cut, so it is grown and read again (at most five times, then not
trusted): a listener past the cut would make a port look held by fewer processes than hold it. The
Sonara and chip tests run on macOS; their stand-in servers accept with a 10 s deadline, so a post that never comes
fails the test instead of hanging it (the old blocking `accept` hung on macOS while the table was a stub). Held by
`flare-proc` `sockets::tests`, `a_listening_tcp_socket_is_read_with_its_port_and_family`, the `tcp_owner` tests
(now on both systems) and `a_childs_listener_is_in_this_processs_tree_on_macos`, the `sonara` tests and
`sonara_is_never_called_on_the_ui_thread`. Source: macOS port phase 4, step 2 (2026-11-08).
1014. **macOS: the taskbar button is the Dock tile.** The badge is the tile's badge label with the disc's own text
("3", "9+"; `OverlayShell::set_badge`, `flare_win::dock::DockShell`), set without a window handle; a bell asks for a
CRITICAL attention request (the icon bounces until Flare is activated, Windows' `FLASHW_TRAY | FLASHW_TIMERNOFG`)
and nothing while Flare is the active app, never an informational one; "the window is in front" is `NSApp isActive`
(`taskbar_flash::is_foreground`, also the web panel's `window_is_foreground`). Every call is main-thread only and
does nothing off it. Tests never make an attention request (an unbundled test app gets a real request id even with
no Dock tile). Held by `dock::tests`, `a_text_badge_gets_the_count_as_the_disc_shows_it_and_a_clear_takes_it` and
`dock_badge_reads_back_and_attention_is_asked_only_while_flare_is_not_active` (`crates/flare-win/tests/appkit.rs`).
Source: macOS port phase 4, step 2 (2026-11-08).
1015. **macOS: notifications wait for the signed app, and say so.** The toast path goes through
`flare_win::toast::NotifyBackend`; on macOS the backend (`MacBackend`) shows nothing and answers
`MAC_UNAVAILABLE` ("unavailable until the app is signed"), because `UNUserNotificationCenter` refuses an unsigned,
unbundled app and Flare has no Developer ID yet. The caller treats it as Windows treats a refused toast (the
fallback bounce, `UiMsg::ToastFailed`); nothing ever reports a notification it did not post. Because the answer is
known before any worker starts (`toast::system_unavailable`), the window's sink posts `ToastFailed` at once on macOS,
starts no worker per question and logs the reason once a run (`toast::FirstTime`), never once a question. TODO in
`toast.rs` for port phase 8. Held by `the_backend_gets_the_request_and_its_answer_and_click_come_back`,
`macos_says_notifications_wait_for_the_signed_app_and_shows_nothing` and
`the_system_says_up_front_whether_it_can_take_a_notification`. Source: macOS port phase 4, step 2
(2026-11-08), Max has no Developer ID yet. Since phase 8 step 2 this holds for a build outside the signed `Flare.app`
only (`MAC_NOT_IN_APP`, `MAC_UNSIGNED`); the signed app posts real notifications (rule 1044).
1016. **macOS: an agent is found by its macOS command line too.** Off Windows `flare_agent::detect::classify` also
counts the launcher script an interpreter runs (`node /opt/homebrew/bin/claude`, `bash ~/.claude/local/claude`,
`python3.12 ~/.local/bin/aider`): the interpreter's first argument that is not an option, by its exact lower-case
file name (never `~/Documents/Claude`, never a later argument). The value of an interpreter option that takes one
(`node -r ts-node/register`, `--require`, `--import`, `--loader`; bun's `--preload`, `--cwd`; deno's `--config`;
Python's `-W`, `-X`) is not the script, and neither is the `run` of `deno` and `bun`; a `deno install` wrapper
(`deno run -A npm:@anthropic-ai/claude-code`) is read by its package signature; an app bundle's own executable with a capital in
its name (`Claude.app/Contents/MacOS/Claude`, the desktop app) is never the CLI. The poll reads the shell's own
argv too (`tab_agent_kind`: a shell that ran `exec claude` is the agent; Windows never reads it, so nothing
changes there) and adopts the tab's terminal foreground job under its shell when its parent is gone
(`adopt_foreground`; only the tab's own session can be its foreground job). Windows reads its command lines
exactly as before. Held by `a_launcher_script_under_its_interpreter_is_the_agent_it_is_named_for`,
`macos_command_lines_are_read_for_the_agent_they_run`, `an_app_bundles_own_executable_is_not_the_cli_of_its_name`,
`a_shell_that_became_the_agent_is_the_agent`, `the_foreground_job_of_a_tab_is_adopted_only_when_it_is_outside_the_tree`
and `a_real_child_command_line_is_read_and_filtered`. Source: macOS port phase 4, step 2 (2026-11-08; the argv
forms read off this Mac's own `claude` and Homebrew `codex`).
1017. **macOS: the Claude plugin's hooks run `/bin/sh hook.sh`, the plugin list is `:`-separated, and the home is
`HOME`.** The install turns every state hook's `cmd.exe /d /c call ${CLAUDE_PLUGIN_ROOT}/hook.cmd` into
`/bin/sh ${CLAUDE_PLUGIN_ROOT}/hook.sh` with the same state words (a hook of any other shape fails the render), the
system folder is `/bin` (any absolute folder named `bin`, as any `System32`), every program written is an absolute
POSIX path, and each platform installs only its own script. `hook.sh` prints the same JSON with `printf`, never
expands a value as its format, and carries the session id only when it is 36 characters of `0-9 a-f -`. The
installed copy lives in `~/Library/Application Support/Flare/claude-plugin-<tag>` with `flare-mcp` (no `.exe`).
`CLAUDE_CODE_PLUGIN_DIRS` is split and joined on `flare_shells::env::PLUGIN_DIRS_SEP` (`;` on Windows, `:` on
macOS: Claude Code splits it on Node's `path.delimiter`, read in 2.1.319). Claude's home
(`flare_agent::resume::claude_home`, the Changes panel's transcripts and resume) and `app_facts::home` are `HOME` on
macOS. Held by the seven install tests (no longer ignored), `the_posix_render_keeps_every_state_word_and_refuses_what_it_cannot_convert`,
`every_hook_prints_what_the_parser_reads_on_macos`, `prints_the_state_as_valid_json_whatever_the_session_id_holds_on_macos`,
`hook_sh_is_lf_and_static`, `a_platform_installs_its_own_hook_script_and_never_the_other`,
`the_folder_list_is_split_the_way_claude_code_splits_it` and `claude_home_is_the_platforms_home`. Source: macOS
port phase 4, step 2 (2026-11-08).
1018. **macOS: the first control server on a name holds `<socket>.lock`, and a busy one is waited for, never
replaced.** A connect cannot tell a live server with a full backlog from a dead one (macOS answers both with
`ECONNREFUSED` at once), so `Server::start` first takes an exclusive `flock` (non-blocking, retried for 250 ms) on
`<socket>.lock` beside the socket (mode 0600, `O_NOFOLLOW`, the user's own file) and holds it for the server's life;
a start that cannot take it fails with `AddrInUse`, and only under the lock is a socket already there removed as a
dead Flare's (the kernel drops the lock with the process; a socket that still accepts a connect is still never
replaced). A lock taken on a file unlinked meanwhile is let go and taken again on the file at the path. Stop removes
the socket and then the lock file, both while holding the lock and only while the socket is still its own. A client
whose connect is refused while the socket is there and a server holds its lock (`server_alive`, a shared lock let go
at once) waits and retries within its timeout, as for a busy pipe; a socket with no lock or a free one fails at
once. The listener is non-blocking, so a client that leaves between `poll` and `accept` never parks the accept
thread past a stop. Held by `a_live_server_with_a_full_backlog_is_never_replaced`,
`a_client_of_a_busy_server_waits_and_then_gets_in`, `a_client_of_a_dead_socket_fails_at_once` and
`the_lock_file_sits_beside_the_socket`. Source: macOS port phase 4 review (2026-11-08; the reviewer's full-backlog
probe started a second server over a live one).
1019. **macOS: a port is in the tab's tree only when the loopback family the tree does not hold is shown free.**
The listener table shows this user's processes only, and binding 127.0.0.1:P and [::1]:P is never refused across
accounts (two addresses), so a tree on 127.0.0.1:P says nothing of another account on [::1]:P, and the chip's
`http://localhost:P` may reach either. So `tcp_owner::tree_look` keeps an `InTree` answer only when each family with
no listener in the table is shown free by `loopback_bind_refused`: a bind of that family's loopback address with
`SO_REUSEADDR` and no `listen` (no client can ever reach it). A wildcard listener of this user does not refuse it;
another account's listener on that address or that family's wildcard does (the kernel's per-uid bind check), and a
refusal or an unknown answer makes the look `Unreadable` (never trusted, one miss). A family the tree holds needs no
probe: the kernel refuses another account's bind on the same or an overlapping address within a family, unless that
account is root. Windows sees every account's listeners and is unchanged. Held by
`an_in_tree_answer_stands_only_when_the_unseen_family_is_shown_free` and
`the_loopback_probe_tells_a_held_address_from_a_free_one` (a second account cannot be staged in a test; the
same-user and wildcard answers were measured on this Mac). Source: macOS port phase 4 review (2026-11-08).
1020. **macOS: the web engine is CEF, pinned to one build, cached outside the repo, and run from a bundle CEF can
use.** `cef` and `cef-dll-sys` (tauri-apps/cef-rs) are pinned with `=156.2.0` (Cargo.lock: 156.2.0+156.0.14, CEF
156.0.14, Chromium 156.0.8078.48; the newest release two weeks old on 2026-11-08) under
`[target.'cfg(target_os = "macos")'.dependencies]` of `crates/flare-cef` only, so Windows builds nothing of it and
keeps WebView2 unchanged. The binary is downloaded by cef-dll-sys's build script into `$CEF_PATH/156.0.14`
(`~/Library/Caches/flare-cef`, the CI cache) or the target dir, never the repo; the build needs CMake and Ninja. The
dev bundle (`flare_cef::macos::bundle`, `flare-cef-bundle`, built under the target dir) has the framework in
`Contents/Frameworks` (an APFS clone of the cached build), five helper apps (`<Name> Helper`, `(GPU)`, `(Renderer)`,
`(Plugin)`, `(Alerts)`, each running `flare-cef-helper`, `LSUIElement`), and an ad-hoc signature applied inside out
(the framework's libraries, the framework, the helpers, the app) that verifies `--deep --strict`. Every helper
enters CEF's sandbox before the framework loads; the browser process keeps `no_sandbox` off. A Developer ID
signature, the hardened runtime, entitlements and notarization wait for the signed app (phase 8). Held by
`the_pin_names_one_build_and_the_workspace_pins_both_crates_to_it`,
`the_cache_is_outside_the_repo_and_the_helpers_are_the_five_cef_looks_for`,
`the_helpers_are_named_and_numbered_the_way_cef_looks_for_them`,
`a_helper_plist_hides_the_helper_and_the_probe_plist_is_background_only` and
`the_dev_bundle_holds_the_framework_and_five_signed_helpers`. Source: macOS port phase 5, step 1 (2026-11-08), the
phase 1 verdict (CEF).
1021. **macOS: a CEF browser process never takes the screen.** `FlareCefApplication` (an `NSApplication` with
`CrAppProtocol`, `CrAppControlProtocol` and `CefAppProtocol`, `isHandlingSendEvent` set around every `sendEvent:`)
is made the shared application before CEF loads, with activation policy Prohibited; a bundle that shows no window of
its own (the probe) is `LSBackgroundOnly`. Every browser is windowless (Alloy style, `on_paint` into a BGRA buffer
in device pixels), in an in-memory request context of its own. Nothing native may open: a popup (`window.open`,
`target=_blank`) is refused, the context menu is emptied and its run cancelled, JavaScript dialogs are suppressed, the
leave-page question is answered "leave", a file picker is cancelled, media and permission prompts are denied, and the
keychain is the mock one (`use-mock-keychain`: the login keychain's "Chromium Safe Storage" prompt is a system
dialog; its key is public, rule 1033; only the real window of the Developer ID app has an item of its own, rule 1045), no colour or print panel can be ordered in (rule 1034); a `<select>` popup is a
painted `PET_POPUP` layer, not a menu. The main thread's pump is AppKit's queued
events, one non-blocking turn of the CFRunLoop and `do_message_loop_work`: without the run loop turn, or with a CEF
log file, a windowless view never paints (measured on macOS 27.2; the log file is for diagnosis only). Held by the
engine tests (`tests/engine_macos.rs`): the window server lists no on-screen window of the bundle's app or helpers
before, during (every 50 ms) and after each run, the process owns 0 windows at its start and end, it is never
active, and no handler was asked for anything native. Source: macOS port phase 5, step 1 (2026-11-08).
1022. **macOS: the agent's DevTools calls go to CEF unchanged, and a runaway script is stopped with the page kept.**
`flare_cef::macos::view::View::cdp_send` hands `agent_job`'s calls to `SendDevToolsMessage` as
`{"id","method","params"}` (ids within CEF's `int`); answers come back by id through the DevTools observer, a
result's JSON as on Windows, an error as its message plus Chromium's error object. The real
`flare_surfaces::agent_job::Job` drives look, click, type, press, scroll, wait and eval over it, and all 14 methods
it sends (`DOM.enable`, `Page.getLayoutMetrics`, `Accessibility.getFullAXTree`, `Runtime.evaluate`,
`Page.captureScreenshot`, `DOM.getContentQuads`, `DOM.scrollIntoViewIfNeeded`, `DOM.focus`,
`Input.dispatchMouseEvent`, `Input.dispatchKeyEvent`, `Input.insertText`, and for the covered-click check of rule
228 `DOM.resolveNode`, `Runtime.callFunctionOn`, `Runtime.releaseObject`) answer in Chromium's shapes. A
`browser_eval` of `while(true){}`, of a page function that never returns, or one that runs into a page getter set as
a trap is stopped by `Runtime.evaluate`'s timeout (10 s; Chromium answers error -32603, which the job reports as
"the page did not run it"), and the same document answers again within milliseconds with its state and the agent's
own marks kept, as on Windows: the gap that ruled WKWebView out. The look's fixed `devicePixelRatio` question on a
trapped page costs its 2 s timeout, not the look. Held by
`the_devtools_methods_agent_job_uses_answer_in_chromiums_shapes` and
`a_runaway_eval_is_stopped_by_its_timeout_and_the_page_answers_again_with_its_state`. Source: macOS port phase 5,
step 1 (2026-11-08); the verdict's "If it is CEF: what phase 5 must prove on a Mac".
1023. **macOS: the web panel is CEF behind the same `WebBackend`, and nothing waits for it.** `flare_web::host` on macOS
(`crates/flare-web/src/macos/host.rs`) keeps the Windows host's shape: the first view asks the runtime question
(`WebEvent::Runtime`: is `Chromium Embedded Framework.framework` in this app's `Contents/Frameworks`; a plain `cargo
run` has none and its panel says so in a sentence, never with the Edge download link), the engine starts when it is
answered, a view's profile (request context) is made and initialized before its browser, and a browser is made on
`about:blank` and navigates only when its setup has answered (`Page.enable`, the script before the page's own through
`Page.addScriptToEvaluateOnNewDocument`, a mod's binding), as a mini-app waits for its script on Windows; then
`Page.resetNavigationHistory` forgets the blank page, so Back on the first page goes nowhere. CEF's handlers only queue
into the host's inbox; `pump` hands it to the window. CEF asks for its turns (`external_message_pump`): a scheduler
thread sends `WebEvent::Pump` when one is due, so winit keeps the run loop. CEF cannot be started twice in a process,
so the engine stays from the first view to `close_all` (Windows lets its environment go with the last view). The real
app makes the shared application `FlareCefApplication` at the start of `main` (`prepare_app`, before winit, which
takes it as it finds it and swizzles `sendEvent:` on its class); its activation policy, windows and `terminate:` stay
winit's and AppKit's, and Chromium's protocols are added to the class when the framework loads. Events are the
Windows ones: `Loading`/`Loaded` by the request's number, WebView2's error status for a failed load (CEF's net error
mapped, `flare_cef::status::web_error_status`, so the failure notice reads it), `Url` and `SameDocument` from
`Page.frameNavigated` and `Page.navigatedWithinDocument`, `Title` again after a load that succeeded, `History`. A
navigation the wall cancelled is `Refused` and its own aborted end is not reported. Held by
`the_web_panel_on_cef_navigates_draws_takes_input_and_holds_its_walls_with_no_window` (`crates/flare-web/tests/
cef_macos.rs`: the real host through the trait in a headless dev bundle), the `flare_cef::status` and
`flare_cef::macos::view` unit tests and `without_the_framework_beside_it_the_runtime_check_says_why`. Source: macOS
port phase 5, step 2 (2026-11-08).
1024. **macOS: the page is a picture in its box, and the window's input goes to it.** The browser is windowless: its
picture (`on_paint`, BGRA in device pixels, a `<select>` list's popup layer laid over it) is `WebBackend::frame`, which
the window shows in the panel's page box (`panel-web-frame`, `panel-web-live`) after every pump and sync; Windows'
backend has no picture, so the box stays empty and takes no input there. The pointer and the wheel over the box go to
`WebBackend::pointer` in device pixels (CSS pixels at the window's scale in the host; a wheel turned toward the person
scrolls down; a press within 500 ms and 5 px is a double click), the keys to `WebBackend::key` with the Win32 virtual
key, the macOS key code and the text (Slint's key names, `web_live::key_of`; a modifier alone goes nowhere), and every
key carries AppKit's own character, because CEF makes an `NSEvent` of it (without it a Backspace's key up deletes a
second time, measured on CEF 156). The input method's text is `WebBackend::ime`. The key filter decides first, as the
accelerator hook does on Windows; the backend's `KeyPress::ctrl` carries Cmd on a Mac (rule 1006), read as the event's
`meta` by `web_ctl::key_event_for` in a Mac key context, so Cmd+T opens a tab and never reaches the page. A press or a
typed key forwarded while an agent works in the page is the person taking it over (the stop-on-input monitor: the
agent's own input is DevTools events and never comes through the box). A still is the picture as a PNG. Held by
`an_off_screen_page_is_drawn_in_its_box_takes_the_input_there_and_a_press_stops_the_agent`,
`on_a_mac_the_backends_ctrl_is_cmd_and_the_app_takes_its_cmd_chords`, `slints_keys_become_the_keys_chromium_knows`,
`a_key_that_types_nothing_carries_appkits_character` and the panel scenario's frame, still, pointer, keys-and-ime,
key-filter, wheel and select-list checks. Source: macOS port phase 5, step 2 (2026-11-08).
1025. **macOS: the walls are the Windows walls, decided in one place and re-proved against the network observers.**
`flare_web::walls` decides for both hosts which address a frame may go to, what a request gets and which profile a view
runs in. On CEF every request of a walled view is answered by the view's request wall
(`ResourceRequestHandler::GetResourceHandler`, and the profile's own for requests of no browser): a mini-app's folder
and a mod's files are served from the folder's real path (`/var` is `/private/var`) by `flare_mods::serve`, a refusal
is a real response with its status and reason (an empty body reads as a broken response), and nothing else leaves.
Behind it the profile holds what Windows passes as browser arguments: a sealed page's profile sends every request to
the dead proxy with no bypass (`<-loopback>`), a networked one to Flare's filtering proxy, and WebRTC sends no UDP
outside a proxy (`webrtc.ip_handling_policy`), set when CEF initializes the profile, before a browser of it is made;
the peer-connection script runs before the page's own. A mod's page talks over `window.chrome.webview` (a shim over a
DevTools binding that it hides), so the bridge runs unchanged; a message reaches the window only from the mod's own
address and within its budget. The bindings' default for `on_before_resource_load` is `RV_CANCEL`: the wall answers
`CONTINUE` there, or every request would be dropped. The web panel goes to http, https and about:blank, a new window
loads in the same view, a mini-app or mod opens none; `mailto:` and other schemes never start a program. Held by
`mini_apps_mods_and_plugin_views_on_cef_keep_their_walls_against_the_network_observers` (the spike's exfiltration page:
19 channels reach the loopback server from a web panel control, none, no TCP accept and no mDNS query from a sealed
mini-app; the observers prove they listen first) and the `walls` unit tests. Source: macOS port phase 5, step 2
(2026-11-08); the verdict's request-interception item.
1026. **macOS: nothing native opens from a page, and nothing waits for the person in CEF.** Script dialogs are answered
at once (`alert` returns, `confirm` is false, `prompt` is null) and never shown; camera, microphone, location,
notifications and clipboard read are denied with no prompt; a file picker is cancelled; the context menu is emptied and
its run cancelled; a popup is no window (same view or nothing). A download waits for the person's yes: CEF holds its
before-download callback, nothing is written where the person keeps files while it is held or after a no (its bytes wait
in Flare's staging folder, rule 1031), a yes saves it in Downloads under its last name part, never over a file (` (2)` and so on); a bar or a background page refuses it
before it starts. CEF is never called, and no CEF object is let go, while a view's state is locked: releasing a
callback can run CEF's work and its handlers at once (a held download's no deadlocked that way). Held by the panel
scenario's dialogs, permissions, downloads, external-schemes and new-window checks, the walls scenario's plugin-bar
check, and `a_download_is_named_by_its_last_part_and_never_overwrites`. Source: macOS port phase 5, step 2
(2026-11-08).
1027. **macOS: the agent drives the web panel through the same `WebBackend::cdp`, and its editing chords edit.**
`agent_job` is unchanged: `web_ctl` sends its DevTools calls through `WebBackend::cdp`, the macOS host hands them to
CEF's `SendDevToolsMessage`, and the answers come back as `WebEvent::Cdp`. One difference is closed in the host: on a
Mac a chord becomes an editing command in the text system before the renderer sees it, and a DevTools key event skips
that step, so `Control+a` (the chord the tool's text teaches) and `Meta+a` selected nothing and `Control+z` undid
nothing (measured on CEF 156). `flare_cef::status::mac_devtools_params` gives a key down of an editing chord its
command, the way Chromium's own Mac input does: Control or Cmd with a (select all), z (undo), Shift+z or y (redo), c, x,
v; with Control the Windows meanings of the arrows (by word), Backspace and Delete (a word), Home and End (the
document's ends), Shift extending the selection. Alt chords, both modifiers at once, and a call that names its own
`commands` are left alone; plain keys (Backspace, the arrows, Home, End, Enter, Delete) already edit and get nothing.
The agent's keys never pass the window's key filter (they are not the person's, so they neither end the agent's call
nor fire an app chord); the person's go through the filter first. Held by
`the_agent_drives_a_web_panel_on_cef_through_the_backend_as_on_windows_with_no_window` (look, click by ref and point,
type, press-editing, scroll, the frames, wait, navigate and state, eval and its runaway, a download held for the yes,
input-is-not-the-persons), `an_agents_editing_chord_is_chromiums_command_on_a_mac_with_control_or_cmd`,
`only_a_key_down_of_an_editing_chord_gets_a_command` and
`an_off_screen_page_is_drawn_in_its_box_takes_the_input_there_and_a_press_stops_the_agent` (a press or a typed key ends
the call, a Cmd shortcut does not). Copy, cut and paste are mapped but not run in the tests: they would use the
person's clipboard. Source: macOS port phase 5, step 3 (2026-11-08).
1028. **macOS: nothing behind Flare's proxy reaches this machine or its network, in any spelling.** A networked
mini-app, a plugin with the network and a mod are re-proved against the spike's proxy page: loopback in five IPv4
spellings, `0.0.0.0`, `localhost`, `localhost.` and `a.localhost`, `[::1]`, `[::ffff:127.0.0.1]`, the LAN address,
`.local` names and names that resolve to loopback, each over http, https, ws and wss, reach the observers' dual-stack
server 0 times, with no TCP accept and no mDNS query for the view's own `.local` name, while the same page in the web
panel (no proxy) reaches it about 90 times and sends mDNS queries. The profile holds the proxy with no bypass
(`<-loopback>`), so CEF never resolves or connects by itself; the request wall refuses http, ws and wss; Flare's
filtering proxy refuses every address of this PC and every local name (`netproxy::public_rule`). This is what
`modview::net_browser_arguments` holds on Windows. An mDNS name in a test must be unique to the view under test: the
system resolver retries an earlier view's query for seconds after that view closed. Held by the walls scenario's
netproxy-literals check in `mini_apps_mods_and_plugin_views_on_cef_keep_their_walls_against_the_network_observers`.
Source: macOS port phase 5, step 3 (2026-11-08); the verdict's E.proxy-only finding.
1029. **macOS: every mini-app has a private profile of its own.** A CEF profile's request wall for the requests no browser
makes (a shared worker's, a service worker's) is built once, from the view the profile was made for. When every sealed
mini-app shared one in-memory profile, the second app's shared worker was judged by the first app's wall: it read the
first app's `secret.txt` (no-cors fetch, 200) and ran its script (`importScripts`), and got a 403 for its own file
(review of phase 5, 2026-11-08; a dedicated worker was walled correctly, it has a browser). The macOS host keys a
mini-app's profile by its host (`walls::context_name`: `flare-app/<host>`), so each app has an in-memory profile, a
wall and a shared-worker scope of its own, and the profile goes with the app's last view. A mod's or plugin's stays
one per mod, on disk. `ControllerProbe::profile` still reports `flare-app`, private. Windows maps each app's folder
on its own view, so it needs no such key. Held by the walls scenario's mini-app-shared-worker check (two sealed apps
open at once; a shared and a dedicated worker of the second get its own file and a 403 for the first app's file and
script; the check fails with the old shared key) and `every_kind_of_view_has_the_browser_windows_gives_it`. Source:
macOS port phase 5 review (2026-11-08).
1030. **macOS: CEF takes Flare's switches, never the process's command line.** `Settings::command_line_args_disabled`
is 1 and the switches are added in `FlareApp::on_before_command_line_processing` (`engine::SWITCHES`), which CEF still
asks. With it at 0, `Flare --remote-debugging-port=N` (or `open -a Flare --args ...`) opened a DevTools port anyone on
the machine could drive, and `--no-sandbox`, `--proxy-server` and `--disable-web-security` reached the helpers (the
GPU helper was started with `--no-sandbox`; measured). The probe takes its scenario from the environment for this
reason. Held by the quiet scenario's own-switches-only check in
`the_engine_takes_only_flares_switches_and_calls_no_one_on_its_own` (the probe is started with all four switches: the
port does not answer, there is no `DevToolsActivePort`, no helper has any of them; the check fails with the setting
at 0). Source: macOS port phase 5 review (2026-11-08).
1031. **macOS: a held download's bytes wait in Flare's staging folder, never in `~/Downloads`.** Chromium writes a
download's first part while CEF holds the before-download callback, into the profile's default download folder,
which is `~/Downloads`: a held download left `.<bundle id>.XXXXXX` there before the person's answer (and the tests
wrote into the real Downloads folder). Every profile's `download.default_directory` and `savefile.default_directory`
are `<data_dir>/download-staging` (`walls::download_staging`), set with the walls before a browser of it is made; the
folder is emptied when the engine starts (what a run left there is no one's download). A yes moves the file to
Downloads (with `com.apple.quarantine`, Chromium's own, held by the panel scenario's download-quarantine check); a no removes it. Held by the panel scenario's
download-staging check (a slow download held: its part is in the staging folder, nothing new in `~/Downloads`, nothing
left after the no; the profile's preference read back) and `every_kind_of_view_has_the_browser_windows_gives_it`.
Source: macOS port phase 5 review (2026-11-08).
1032. **macOS: the engine calls no one on its own.** With only a sealed mini-app open, CEF 156 connected to Google
within 3 s: the component updater (`update.googleapis.com`, then the on-device model manifest from `gvt1.com`), the
network time tracker (`clients2.google.com/time`) and the AI mode eligibility check (`www.google.com/async/folae`, with
the country), none from a page and none through a profile's proxy. `engine::SWITCHES` turns off component updates
and background networking, and `DISABLED_FEATURES` the network time queries, the AI mode checks, the optimization
guide's hints and models and the media router (CEF's own four disabled features are carried on: a `disable-features`
switch replaces CEF's list). `--disable-component-update` alone does not stop the manifest's registration (measured),
so the component updater's server is the dead address (`--component-updater=url-source=http://127.0.0.1:9/`): its check
is refused on loopback and nothing is fetched. Held by the quiet scenario's no-outside-calls check (10 s with a sealed
mini-app: no listener, no UDP socket, no connection past loopback in any process of the bundle, and Chromium's network
log holds no address but the app's own and the dead address) and
`the_engine_is_started_with_its_quiet_switches_and_keeps_cefs_own_disabled_features`. A new CEF pin re-runs the quiet
scenario first: a new service is a new feature to turn off. Source: macOS port phase 5 review (2026-11-08).
1033. **macOS: a profile on disk is protected by its file's permissions only, until the signed app (accepted gap).**
CEF runs with `use-mock-keychain` so Chromium never asks the login keychain for "Chromium Safe Storage" (a system
prompt on the screen, asked again after every ad-hoc rebuild). The mock keychain's key is fixed and public
(`mock_password` is in the framework), so what the web panel's profile (`<data_dir>/Default`) and each mod's and
plugin's keep on disk (cookies, storage) is readable by anything that can read the file, a backup included; Windows
encrypts it with DPAPI. The mini-apps' profiles are in memory and keep nothing. Since phase 8 step 2 the gap is
narrowed to builds that are not the Developer ID app (a dev or ad-hoc bundle, a test, a probe): the real window of
the signed `Flare.app` uses a keychain item of its own (rule 1045). For those builds this gap is written here, in
`engine.rs` and in `docs/parity.md`, and no one may describe their macOS profile as encrypted.
Source: macOS port phase 5 review (2026-11-08).
1034. **macOS: no colour or print panel can come on screen from a page.** A windowless browser has no window for a
sheet, so on a Mac Chromium would show AppKit's shared panels as windows of their own (`<input type=color>`, a print
from `window.print()` or Cmd+P). Every profile has `printing.enabled` false, so a print is never asked for;
`flare_cef::macos::panels::guard` (run by `app::install` and `app::install_for_app`, before CEF starts) replaces the
methods that would order in `NSColorPanel` and run `NSPrintPanel`, `NSPageLayout` and `NSPrintOperation`, on those
classes only (the app's own windows are untouched), and counts what it kept off. A test asks for a panel only after
`panels::installed` says every replacement is in place. On CEF 156 a trusted click on a colour input asks for no
panel at all (measured: 0 kept off, 0 windows); the guard stays for the build that does. Held by the panel scenario's
no-native-pickers check (a trusted click on a colour input and `window.print()`: no window, the print returns at once,
printing read back false) and `the_panels_are_replaced_on_their_own_classes_and_nothing_else`. Source: macOS port phase
5 review (2026-11-08).
1035. **macOS: a popup or new-tab request loads only a web address the navigation rule allows.** CEF's `on_before_popup`
and `on_open_urlfrom_tab` load the target into the view's own main frame (`NewWindows::SameView`). `load_url` of a
`javascript:` address runs it in the top frame without a navigation, so the browse wall never sees it: a cross-origin
frame could run script in the page above it. `popup_target_allowed` lets only `http`/`https` through, and only what the
view's `NavRule` allows for the main frame; `data:`, `file:`, `chrome:`, `devtools:`, `blob:` and every other scheme
are dropped. Held by `a_popup_loads_only_a_web_address_the_navigation_rule_allows`. Source: commit security review of
phase 5 (2026-11-08).
1036. **macOS: each program of `Flare.app` carries exactly the entitlements it is given, and never the broad ones.**
The hardened runtime is on for every Mach-O of a Developer ID signature (`flare_cef::macos::sign`). The keys are
Chromium 156.0.8078.48's own (`chrome/app/*-entitlements.plist`) minus its App Sandbox keys, plus a terminal's:
`Flare` has `device.audio-input`, `device.camera` and `automation.apple-events` (rule 1042); `Flare Helper (Renderer)`
has `cs.allow-jit` (V8); `Flare Helper (GPU)` has `cs.allow-jit` (SwiftShader's software GL, Chromium's
`helper-gpu-entitlements.plist`, read 2026-11-08); `Flare Helper`, `(Plugin)`, `(Alerts)` and `flare-mcp` have none
(no Pepper in CEF 156). Never `allow-unsigned-executable-memory`, `disable-library-validation`, `get-task-allow` or the
dyld environment key. Each key is written with why it is there, in the entitlements file itself. A signed run of the
engine probes under the hardened runtime is how a key is proved (`FLARE_TEST_SIGN_IDENTITY` with
`FLARE_TEST_SIGN_KEYCHAIN`, rule 1040); until one has run, the keys stand on Chromium's files. Held by
`each_program_carries_chromiums_keys_for_it_plus_the_terminals_and_never_the_broad_ones`. Source: macOS port phase 8,
step 1 and its review (2026-11-08).
1037. **macOS: `Contents/MacOS` holds only code; everything else `Flare.app` ships is in `Contents/Resources`.** A
folder or a text file beside the executable breaks the signature's seal, so the Claude Code plugin template, the
licenses and CEF's credits go to `Resources`, and `flare_agent::plugin::candidate_plugin_dirs` looks in
`<app>/Contents/Resources/claude-plugin` first when the app runs from a bundle (macOS only; the Windows candidates
are unchanged). `flare-mcp` stays beside the app (code, found as before). The main executable is `Flare`, because CEF
finds its helpers as `<executable> Helper (X).app`. The bundle id is `com.flare.terminal`, the id Windows gives the
app, and it never changes (the keychain item, the microphone grant and the notification settings hang on it).
`LSMinimumSystemVersion` is 15.0, the version is the workspace's, no document or URL types are declared. Held by
`a_mac_app_bundle_finds_the_plugin_in_its_resources_first`,
`the_app_plist_names_the_app_its_version_its_mac_its_category_and_why_it_wants_the_mic` (the plist is linted by
`plutil`) and `the_bundle_id_is_the_id_windows_gives_flare`. Source: macOS port phase 8, step 1 (2026-11-08).
1038. **macOS: a release is signed inside out with the Developer ID and checked Mach-O by Mach-O.**
`flare_cef::macos::bundle::sign_bundle` signs any Mach-O outside the frameworks, helpers and `Contents/MacOS`, each
framework's own Mach-O files, the framework, the five helpers, the other programs, then the app (`--options runtime`,
`--timestamp`, per-program entitlements and identifier); it works on a bundle a packager added programs to and refuses
a helper app CEF does not start. `sign::verify_release` then requires `codesign --verify --deep --strict`, and for every
Mach-O: team `WV7BY8CUM8`, a leaf certificate "Developer ID Application:" (an "Apple Development" one of the same team
fails here, not at notarization), the hardened runtime, a secure timestamp, and exactly the entitlement keys its role
(by its place in the bundle, `bundle::role_of`) is given, so a broad key or a key on the wrong program fails. Held by
`a_release_mach_o_has_exactly_its_roles_keys_and_a_developer_id_application_signer`,
`a_mach_os_role_is_read_from_its_place_in_the_bundle`,
`any_bundle_of_this_layout_is_signed_inside_out_with_programs_it_did_not_make`,
`a_developer_id_signature_has_the_runtime_and_a_timestamp_and_ad_hoc_has_neither` and
`codesigns_display_is_read_for_team_runtime_timestamp_and_authority`. Source: macOS port phase 8, step 1 and its
review (2026-11-08).
1039. **macOS: the signing key and the notary credentials never enter the repository, and the Mac build does not
touch the Windows release.** `tools/release/mac/make-app.sh` takes the identity and the notary credentials from the
environment (a notarytool keychain profile on a Mac, an App Store Connect API key in CI) and prints none of them;
`tools/release/mac/ci-keychain.sh` puts the secrets `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PASSWORD`,
`APPLE_API_KEY_P8_BASE64`, `APPLE_API_KEY_ID` and `APPLE_API_ISSUER_ID` into a throwaway keychain with a random
password that only `codesign` may use (`.github/workflows/macos-app.yml`, run by hand; no secret means an ad hoc build
that says so). The script names the keychain and the `.p8` in `$GITHUB_ENV` before it makes them and removes the
`.p12` on any exit (`trap`), and the cleanup step runs `if: always()`, so a failure halfway leaves no key material on
a self-hosted runner. The CEF framework the job signs is downloaded fresh into `$RUNNER_TEMP`, never restored from
`actions/cache`, and every action is pinned by commit SHA. The app is zipped with `ditto -c -k --keepParent`, notarized with
`notarytool submit --wait` (anything but Accepted fails with Apple's log), stapled and checked with `stapler validate`
and `spctl -a -t exec` ("source=Notarized Developer ID"); the DMG (the stapled app and an `/Applications` link) is
signed, notarized and stapled too, and checked attached with `-nobrowse` (no Finder window). Every product is under
`target/mac-release`. `release.yml` is unchanged. Source: macOS port phase 8, step 1 (2026-11-08).
1040. **macOS: `codesign` never runs with a Developer ID unless the key can be used without a prompt, and an
ordinary test run never signs with one.** A Developer ID `codesign` whose key's access list does not let it in puts a
SecurityAgent prompt on the screen that stays after `codesign` is stopped (measured on Max's Mac, 2026-11-08: the
key's partition list was only `apple-tool:`, and `/usr/bin/codesign` is in `apple:`). So `sign::sign` first runs
`flare_cef::macos::keychain::check`, in-process with `SecKeychainSetUserInteractionAllowed(false)` (the check itself
cannot raise a prompt; it reads the certificate and the key's access list, never the key): the key's keychain
unlocked, an access entry that allows signing naming `/usr/bin/codesign` (or any program) without the password, and
`apple:` in the partition list; else it refuses and says the one command that fixes it
(`security set-key-partition-list -S apple-tool:,apple: -s`). `make-app.sh` runs `flare-mac-app check-key` before
building, and signs the DMG through `flare-mac-app sign-file` too: no `codesign` with the identity runs from a script.
A Developer ID sign then runs one at a time across processes (a lock file in the temp dir), is stopped at once when a
SecurityAgent window appears that was not there before (that prompt's process is stopped too), and otherwise after
20 s plus 1 s per 4 MB hashed (plus 2 min for a timestamp), so a large legitimate sign is not cut short. The tests'
identity is `FLARE_TEST_SIGN_IDENTITY` and needs `FLARE_TEST_SIGN_KEYCHAIN` too; `Signing::for_tests` never reads the
release variable `FLARE_MAC_SIGN_IDENTITY`. Held by
`a_key_codesign_would_have_to_ask_for_is_refused_before_codesign_runs`,
`partitions_are_read_from_the_hex_plist_and_a_name_matches_like_codesign`,
`the_check_runs_with_the_keychain_ui_off_and_finds_no_identity_that_is_not_there`,
`the_tests_sign_only_with_their_own_identity_and_a_keychain_never_with_the_release_one` and
`the_sign_timeout_grows_with_what_codesign_hashes`. Source: review of phase 8 step 1 (2026-11-08).
1041. **macOS: every TCC entitlement of the app has its usage string.** `Flare.app`'s `Info.plist` carries
`NSMicrophoneUsageDescription`, `NSCameraUsageDescription` and `NSAppleEventsUsageDescription`, one for each key of
rule 1042 (`sign::Entitlement::usage_key`): without the string macOS refuses the request even with the key. Held by
`the_app_plist_names_the_app_its_version_its_mac_its_category_and_why_it_wants_the_mic`. Source: review of phase 8
step 1 (2026-11-08).
1042. **macOS: the app's TCC keys are a terminal's, and a library loaded later is signed by the team, never let in by
an entitlement.** macOS holds the app that started a program responsible for what the program asks for, so a
hardened Flare without a key refuses, silently, what Terminal and iTerm2 allow: `osascript` (`automation.apple-events`),
`ffmpeg -f avfoundation` or `imagesnap` (`device.camera`), `sox` or ffmpeg's microphone (`device.audio-input`). Those
three are given, as iTerm2 does; location, contacts, calendars and photos are not (no terminal program in common use
needs them; a deliberate difference in `docs/parity.md`). The microphone key is also dictation's, provisional for
dictation until the macOS dictation phase has a signed run. Dictation's engines are downloaded at run time on
Windows (`flare_dictation::catalog`, `flare-parakeet-host`): on macOS library validation refuses a dylib not signed
by the team in a hardened process, so an engine library is signed by the team when it is bundled or downloaded
(checked with `codesign --verify` before it is loaded), or loaded in a helper of its own, and never through
`disable-library-validation`. Source: review of phase 8 step 1 (2026-11-08).
1043. **macOS: the bundle that is notarized and checked is the one that ships in the update package.** `vpk pack` for
osx adds its update program and `sq.version` to `Contents/MacOS`, which breaks a seal made before it. So the Velopack
step signs again after packing what vpk produced (`flare-mac-app resign`, the roles of rule 1038, vpk's program as a
tool with no entitlements), or uses vpk's own signing only if it gives each program its own entitlements; then it
notarizes and staples that bundle, and runs `flare-mac-app verify`, `codesign --verify --deep --strict`,
`stapler validate` and `spctl -a -t exec` on the bundle extracted from the `.nupkg` and from the full update package,
not on the bundle before packing. Phase 8 is done only when such a signed, notarized release updates itself. Source:
review of phase 8 step 1 (2026-11-08).
1044. **macOS: notifications go through `UNUserNotificationCenter` only from Flare's own signed `Flare.app`, ask once,
lazily, and a click steers only this run's.** `flare_win::toast::system_unavailable` reads, once a run, the main
bundle and the process's own signature (`notify::bundle_facts`: `NSBundle` and `SecCodeCheckValidity` against
`identifier "com.flare.terminal" and info [CFBundleIdentifier] = "com.flare.terminal"`). That read can take a second
on a cold start (measured 2026-11-09: about 0.5 s each for `NSBundle` and the signature, nearly 3 s under a loaded
test run), so it runs on a worker (`toast::warm_up` at install) and the UI thread only asks
`toast::known_unavailable`, which never blocks; until the answer is known a question goes to the worker, which reads
it and answers through `on_done`. A process that is not a `*.app` with Flare's id answers `MAC_NOT_IN_APP`
(a `cargo run`, a test binary; the center would raise an Objective-C exception there and abort Flare), a bundle the
linker signed but nobody sealed answers `MAC_UNSIGNED`, and the caller bounces the Dock as before (rule 1015). In the
signed app `toast::CenterBackend` over `notify::SystemCenter` asks the center nothing until Flare first has
something to say; then it reads the permission (never a prompt), asks for it (the system prompt) at most once a run
and only when it was never answered, and honours a no for the run (`MAC_DENIED` names System Settings, nothing is
posted). Each notification's identifier is `agent-question.<tab tag>`, so a newer one for a tab replaces the older;
its user info carries the tab whole and the run's session id. The delegate (`FlareNotificationDelegate`, set with
the first post, kept for the run) shows the banner while Flare is in front too, as Windows does, and hands a click on
the notification itself to `on_click(tab)` (`UiMsg::ToastClicked`: the window comes forward and the tab is picked)
only when the session and the identifier are this run's; a click on an older run's notification only brings Flare
forward, as an orphan toast only starts the app on Windows. No test asks the real center for anything: the
permission request is a system prompt on the screen, so the logic is held with a fake center, and the real bundle
check with a copy of the test binary in a temp `Flare.app` signed ad hoc and run as a child that reads its verdict
only. Held by `the_permission_is_asked_once_lazily_and_only_when_flare_first_notifies`,
`a_no_is_honoured_for_the_run_and_nothing_is_posted`, `one_notification_per_tab_the_newer_replaces_the_older`,
`a_click_brings_back_the_tab_of_this_runs_notification_only`, `only_flares_own_signed_app_bundle_may_use_the_center`,
`a_copy_in_a_signed_flare_app_is_told_from_a_dev_build`, `a_dev_build_says_notifications_need_flare_app_and_shows_nothing`
and `the_framework_is_loaded_and_the_delegate_answers_both_center_calls`. A banner seen on a signed run and its click
are still to be checked by hand (`docs/parity.md`). Source: macOS port phase 8, step 2 (2026-11-09).
1045. **macOS: the Developer ID app keeps its profiles' key in a keychain item of its own, and nothing else touches
the keychain.** CEF 156 hard-codes Chromium's default item ("Chromium Safe Storage", the string is in the framework),
which Chromium itself and every unbranded CEF app share: an item another app made would be a keychain prompt naming
that app's item. `cef_settings_t` has `keychain_service_name` and `keychain_account_name`, so the engine always names
Flare's own (`engine::KEYCHAIN_SERVICE` "Flare Safe Storage", account "Flare"). `use-mock-keychain` is dropped only
for the real Flare window (`EngineConfig::adopt_app`) in a process that meets `engine::DEVELOPER_ID_REQUIREMENT`
(Flare's identifier, `anchor apple generic`, the Developer ID intermediate `certificate 1[field.1.2.840.113635.100.6.2.6]`
and leaf `certificate leaf[field.1.2.840.113635.100.6.1.13]`, and the team's OU, read from the running code's
signature with no keychain call; without the two OIDs an Apple Development or Mac App Distribution signature of the
same team would pass and make the item with an access list the Developer ID app would be asked about): an item the signed app makes on its first run trusts that signature, so an update keeps it without
asking. Every other build (a dev or ad-hoc bundle, whose access would be asked for again after every rebuild, a test,
a probe) keeps the mock keychain (rule 1033). A dev build and the Developer ID app share the default profile folder,
so a switch between the two keys would make the cookies unreadable; rule 1051 sets them aside instead. Held by
`only_the_real_window_of_the_developer_id_app_uses_a_keychain_item_of_its_own` (the switch list, the item's name, the
requirement's team, id and both Developer ID OIDs, and that a test binary is not Flare's Developer ID). The item made on a signed run is still to be seen by hand: no test may touch the keychain. Source: macOS port
phase 8, step 2 (2026-11-09); the CEF field is in `cef-dll-sys` 156.2.0's bindings.
1046. **macOS: the bundle `vpk pack` zips is signed again by role after vpk has added its updater, through
`flare-mac-app` as vpk's `codesign`.** vpk 1.2.161 for osx copies `UpdateMac` into `Contents/MacOS`, writes
`sq.version` into `Contents/Resources` with a link in `Contents/MacOS`, signs, notarizes and staples, and only then
makes the update package and the portable zip (`OsxPackCommandRunner`, read 2026-11-09 at commit 92d6a1c). Its own
signing gives `UpdateMac` Velopack's entitlements (`allow-jit`, `allow-unsigned-executable-memory`,
`disable-library-validation`, the dyld key: rule 1036 forbids them) and the app one file for every program; unsigned,
the bundle it zips fails `codesign --verify --deep --strict` ("a sealed resource is missing or invalid", measured
2026-11-09). So `tools/release/mac/pack.sh` runs vpk with `--signDisableDeep`, no `--signEntitlements`, and a link
named `codesign` to `flare-mac-app` first on vpk's `PATH`; `flare_cef::macos::vpk::parse` accepts only the two
calls vpk makes (`-s`, `-f`, `-v`, `--timestamp`, `--options runtime`, `--entitlements`, `--keychain`, one path last;
`--deep` and anything else refused, an identity other than the script's refused), leaves `UpdateMac` for the bundle's
signing and signs the bundle inside out by role (`bundle::sign_bundle`: `UpdateMac` a tool with no entitlements,
`com.flare.terminal.UpdateMac`), then checks it (`verify_release` for a Developer ID, else `codesign --verify`). vpk
notarizes and staples that bundle (a notarytool profile: `FLARE_NOTARY_PROFILE`), and `pack.sh` checks the bundle
inside each package, the `.nupkg` unpacked as Velopack's `extract_lib_contents_to_path` does (`.__symlink` files, mode
755) and the portable zip with `ditto`: `codesign --verify --deep --strict`, `UpdateMac`, `sq.version` and its link,
and with a Developer ID `flare-mac-app verify`, `stapler validate` and `spctl` "source=Notarized Developer ID". The DMG
is made from that bundle, never from the one before vpk. Channel `osx`, `--noInst` (the installer is the DMG).
Changing `VPK_VERSION` means reading vpk's osx pack code again. Held by
`vpks_two_calls_are_its_update_program_then_the_bundle`,
`anything_vpk_does_not_pass_with_disable_deep_is_refused`, `an_identity_other_than_the_scripts_is_refused`,
`the_shim_answers_to_codesign_only` and `the_pack_signs_by_role_through_flare_mac_app_and_checks_inside_the_packages`.
Source: macOS port phase 8, step 3 (2026-11-09).
1047. **macOS: an update swaps the whole bundle, and that is proved on a Mac headless, against the real feed and
Velopack's own updater.** The installed Mac app is the bundle vpk made (`flare_win::packaged::bundle_root`:
`<X>.app/Contents/MacOS/<exe>` with `UpdateMac` beside it and `Contents/Resources/sq.version`; macOS only, the root is
the `.app`, no Explorer verb; the Windows layout is unchanged), so `Ship::detect` calls it packaged and a
`live-updates` build updates it; the e2e package-folder seam takes an absolute local path there
(`ship_ctl::is_local_mac_path`). `UpdateMac` unpacks the new bundle beside the old and exchanges them in one
`renamex_np` (`apply_osx_impl`), so every seal is the new release's own. `tools/release/mac/update-e2e.sh` packs
`Flare.app` as the throwaway versions 0.0.1 and 0.0.2 (never a real release number; `flare-mac-app --version` puts
them in `Info.plist` too), and `crates/flare-update/tests/mac_update.rs` installs 0.0.1 from its portable zip into a
temp folder (never `/Applications`), then a child (the test binary) runs the real `VelopackFeed` over the folder
(check, download into a packages folder under the temp folder, apply with `--silent` and no restart:
`VelopackFeed::without_restart`, the test's only, the app always restarts); the parent samples the window server
until the swap is done (no window of any process under the temp folder, none owned by `UpdateMac`) and checks
0.0.2's `sq.version`, `Info.plist`, `UpdateMac` and link, `codesign --verify --deep --strict`, that nothing was
started, that only `Flare.app` is left, and Gatekeeper's verdict: unchanged by the swap, and with
`FLARE_MAC_EXPECT_NOTARIZED=1` "Notarized Developer ID" before and after plus `stapler validate`. Nothing is opened
with `open`. Measured ad hoc on Max's Mac (2026-11-09): 0.0.1 to 0.0.2 in about 9 s, 24 window samples, none shown;
the delta 0.0.1 to 0.0.2 patched 14 of 264 files. The signed, notarized run is one command for Max
(`docs/releasing-macos.md`). Held by `a_packed_flare_app_updates_itself_headless_and_the_swapped_bundle_still_verifies`
(ignored: run by the script), `a_mac_bundle_vpk_made_is_its_own_root_and_has_no_verb`,
`a_mac_update_source_is_an_absolute_local_path`,
`the_app_restarts_after_an_update_and_only_the_mac_test_turns_it_off` and
`the_update_test_uses_throwaway_versions_and_never_opens_the_app`. Source: macOS port phase 8, step 3 (2026-11-09).
1048. **macOS: the Mac release is a workflow of its own after the Windows one, from secrets only, and skips
cleanly.** `.github/workflows/macos-app.yml` runs on `workflow_run` of `release` (completed, success, `main`) and by hand.
After a release it builds only when the five secrets are set and `v<version>` exists without `releases.osx.json`
(and only the commit that release was tagged at, rule 1049), else it writes a notice and stops with success; then `make-app.sh` (`FLARE_NOTARIZE=0`),
`pack.sh` (rule 1046) and `vpk upload github --channel osx --merge` plus the DMG into that same release, and checks
the release holds the Mac files beside `releases.win.json`. By hand it keeps an artifact and never publishes.
`release.yml` is unchanged and names nothing of the Mac build, so a Mac failure can never fail or hold up a Windows
release. `ci-keychain.sh` also stores the API key as the notarytool profile `flare-ci` in the throwaway keychain (vpk
notarizes with a profile only); the keychain goes in the `if: always()` step. The updater follows the same rule as
Windows (`vars.FLARE_LIVE_UPDATES`, a public repository, the advisory gate). The runner is `macos-26`: the ci macos
job builds and links the workspace on macos-26-arm64 (run 42533159787, 2026-11-08), every Flare program is built for
macOS 11 and CEF's framework for 13, under `LSMinimumSystemVersion` 15.0, so no newer SDK is needed. The release notes
are the Windows release's text. Held by `the_mac_release_runs_after_the_windows_one_never_inside_it`,
`credentials_come_from_the_five_secrets_only_and_without_them_nothing_is_published` and
`vpk_is_pinned_to_the_velopack_crate_on_both_oses`. Source: macOS port phase 8, step 3 (2026-11-09).
1049. **macOS: the Mac build added to a release is the commit that release was tagged at, made once, and signed only
from `main`.** `release.yml` ends with success on every merge to `main`, also one without a version bump ("v<version>
is already released"), so the commit that ended the run (`workflow_run.head_sha`) can be newer than the release. The
`plan` job of `macos-app.yml` (no secret) reads the version there, then the release's own commit (`gh api
repos/<repo>/commits/v<version>`), and the `app` job checks out that commit and refuses when its `Cargo.toml` is not
that version (a release from before the Mac scripts gets a notice, no build). The `app` job's concurrency group is
`macos-app-<version>` (not the commit), and the release is read again in the gate and once more just before `vpk
upload --merge`, so two quick merges with the same version cannot both upload. The five secrets live in the GitHub
environment `mac-release`, whose deployment branch rule is `main` only (Max sets it, `docs/releasing-macos.md`); the
`plan` job names that environment only for a release run or a run by hand on `main`, and the gate also turns
`signed` off for a run by hand on any other branch. The environment is the wall: a branch can edit the workflow it
runs by hand, but GitHub refuses that branch the environment and its secrets. Held by
`the_mac_build_is_the_commit_the_release_was_tagged_at_one_run_per_version` and
`signing_secrets_reach_a_run_from_main_only`. Source: macOS port phase 8, review of step 3 (2026-11-09).
1050. **macOS: no signing key, password or scratch folder outlives its one use, and a script removes only what it
made.** In CI the programs are built first (`make-app.sh` with `FLARE_BUILD_ONLY=1`); only then does `ci-keychain.sh`
decode the `.p12` and the `.p8`, which its EXIT trap removes however it ends (the `.p8` right after `notarytool
store-credentials`: notarization uses the keychain profile `flare-ci`, never the file); nothing but the keychain's path,
the identity and the profile name goes into `$GITHUB_ENV`. The keychain locks after 9000 s without use (the job's
150-minute limit) and on sleep, and is deleted (`if: always()`) right after the packing, before the artifact and
upload steps run. Notarization never takes a password on the command line (`ps` shows it to every local process):
`lib.sh` knows a keychain profile or an API key file, no Apple ID. The `.p12` Max exports is deleted with `rm -P`,
never moved to the Bin, and `.gitignore` refuses `*.p12`, `*.p8` and `AuthKey_*`. The scripts share one EXIT trap
(`lib.sh`'s `on_exit` list, run newest first), so `make_dmg`'s detach no longer replaces `pack.sh`'s removal of its
work folder; paths in it are quoted with `printf %q`. `update-e2e.sh` empties its work folder only after refusing a
relative path or any `.` or `..` part, resolving it with `pwd -P`, and requiring `target/mac-update-e2e*` of this
repository or a named folder below the temp folder (never `target/`, the temp folder itself, or a `/tmp*` prefix
match). Held by `no_key_file_or_password_outlives_its_one_use` and
`the_scripts_remove_only_what_they_made_and_never_lose_each_others_cleanup`. Source: macOS port phase 8, review of
step 3 (2026-11-09).
1051. **macOS: a change of the profiles' key sets the old key's cookies aside; it never lets Chromium drop them.** A
dev or ad-hoc build uses the mock keychain's key and the Developer ID app Flare's own item (rule 1045), both on the
default profile folder (`~/Library/Application Support/Flare`), and Chromium encrypts cookie values and saved
passwords with that key, so a file written with one key is unreadable with the other. `Engine::start` calls
`profile_key::settle` on the engine's root folder before CEF opens it: the marker `flare-profile-key` (`mock` or
`own`; none: `mock`, every build before it used the mock key) names the key the profiles were written with; when it
differs from this run's, every `Cookies`, `Login Data` and their journals under the root (five levels, links not
followed) is renamed to `<file>.<old>-key`, the files set aside earlier for this run's key come back, the marker is
rewritten, and the host logs what moved. A root that does not exist yet is left for CEF to make (a root folder made
before `cef_initialize` changed how CEF made the profile folders under it: the mod probe's profile folder was then
missing, measured 2026-11-09), and `profile_key::mark` writes the marker after `cef_initialize`. Local Storage and IndexedDB are not encrypted with the key and stay. A file
that cannot be moved is named in the log, never fatal. Held by
`a_change_of_key_sets_the_old_keys_cookies_aside_and_brings_the_new_keys_back` and
`a_new_root_is_left_for_cef_and_marked_after_and_an_unknown_marker_counts_as_the_mock_key`. Source: macOS port phase 8, review of
step 3 (2026-11-09).
1052. **macOS: ad hoc signing gives each program its role too, so the ad-hoc pack proves the updater's signature.** An
ad-hoc `codesign` (`sign::codesign_args` with `Signing::AdHoc`) passes the role's `--identifier` and its entitlements
file, and `--options runtime` for a tool (`flare-mcp`, Velopack's `UpdateMac`: `sign::adhoc_runtime`); not for the
app, the helpers or the framework, which load the ad-hoc CEF framework that library validation would refuse ("no
Team ID"). `pack.sh`'s `check_bundle` asserts, ad hoc too, that `Flare` is `com.flare.terminal` with exactly its three
keys and each tool is `com.flare.terminal.<file>` under the hardened runtime with no entitlements, so vpk's own
signature or the linker's can no longer pass for the role signing. Measured ad hoc on Max's Mac (2026-11-09):
`UpdateMac` and `flare-mcp` `flags=0x10002(adhoc,runtime)`, no entitlements, and the update e2e from 0.0.1 to 0.0.2
passed with that `UpdateMac` doing the swap (17 window samples, none shown). Held by
`a_developer_id_signature_has_the_runtime_and_a_timestamp_and_ad_hoc_has_neither` and
`the_pack_checks_each_programs_role_ad_hoc_too`. Source: macOS port phase 8, review of step 3 (2026-11-09).
1053. **macOS: a plugin helper and every process it starts end when it is stopped, when it ends, and when Flare ends,
a crash included.** The Windows kill-on-close job has no Mac twin, so `flare_proc::guard` builds the same guarantee:
the helper calls `setsid` between fork and exec (a session and group of its own, its pid both ids, no terminal), and
before the exec forks a guard that is left an orphan in that session (rule 1054: it execs Flare's own executable). The
guard leads a group of its own in the helper's session (`setpgid(0, 0)`), so a helper that kills its own group
(`kill -9 0`, `process.kill(-pid)`) misses it. It holds one end of a Unix socket pair whose other end only Flare holds
(close-on-exec), closes every other descriptor, ignores HUP, INT, TERM, QUIT and PIPE, says its pid on the socket,
lets the helper go on (the helper waits in its start for that), and waits in kqueue for the socket's EOF (a stop, or
Flare's exit or crash, when the kernel closes Flare's descriptors), `NOTE_EXIT` of the helper (it crashed or ended, as
a job's last handle closing does) and `NOTE_EXIT` of Flare's pid. Then it sends `SIGKILL` to every process of the
group, of the session, and of the descendants it noted every 250 ms (libproc child lists, each with its start time),
so a process that left the group (`setpgid`, a `set -m` job) or the session (`setsid`) still ends, and exits. A guard
that cannot start fails the helper's start (`NO_GUARD`, "its guard could not start"). The helper's thread waits for the
guard by pid and start time (`GuardId`, at most `STOP_WAIT`), then makes its own pass (`guard::end_session`: every
process whose session or group is the helper's), so a helper that found and killed its guard by pid still has its
session and group ended; `Job::active` is 1 until that pass and the reap are done. `socketpair` and the `fcntl` that
makes it close-on-exec are two calls (macOS has no atomic form): a process another thread starts in between keeps a
copy of Flare's end and delays the EOF, which is why a stop also kills the helper (`NOTE_EXIT`) and the guard also
watches Flare's pid. What it cannot catch: a double-forking daemon that left the session and was orphaned within one
tick, and a process that left the session of a helper that killed its guard by pid. Held by
`a_program_that_kills_its_own_group_still_has_its_whole_tree_ended`,
`end_session_ends_the_session_and_the_group_but_never_this_process` (crates/flare-proc/src/guard.rs),
`mac_a_helper_whose_guard_was_killed_still_has_its_session_and_group_ended` (crates/flare-mods/src/helper.rs),
`mac_a_stopped_helper_leaves_no_process_and_is_not_restarted`, `mac_a_killed_helper_takes_its_whole_tree_with_it`,
`mac_stop_all_ends_every_helper_tree`, `mac_no_helper_process_outlives_a_crash_of_flare` (a stand-in Flare killed with
SIGKILL), `mac_a_helper_that_keeps_ending_is_started_three_times_more_then_stopped` (crates/flare-mods/src/helper.rs)
and `releasing_the_guard_ends_the_group_the_session_and_the_orphans`,
`a_descendant_that_left_the_session_is_ended_too`, `a_program_that_ends_by_itself_takes_its_tree_with_it`
(crates/flare-proc/src/guard.rs); each tree tries every way out (a child, an orphan, a `set -m` job, a `setsid`
escapee). Source: macOS port phase 6 step 1 (2026-11-08).
1054. **macOS: the guard is an exec of Flare's own executable, never a fork that lives on.** A fork that never execs
keeps a copy-on-write snapshot of Flare's memory: every page Flare changes after the fork is copied, and the guard
keeps the original, so each running helper held up to all of Flare's dirty memory (the CEF browser process, hundreds
of MB; an 800 MB probe rewritten after the start cost +843 MB while `ps` and `footprint` showed the guard at 1 MB).
So the guard's process execs the entry `guard::use_this_program` set (`current_exe`, `_NSGetExecutablePath`) at once,
with `argv[0]` `flare-guard:<socket>:<status>:<Flare's pid>`, stdio on `/dev/null`, and only its two descriptors kept
open across the exec; `ship_ctl::bootstrap` calls `guard::run_if_guard` first, before Velopack or anything else of
Flare runs, and a guard started by hand refuses (exit 2, nothing signalled) unless both descriptors are a socket and a
pipe and its session leader is Flare's child. A test binary uses `use_test_entry(<test>)`: the binary running only a
`#[test]` whose body is `run_if_guard`, never the whole binary. Until an entry is set `guard_command` refuses. Between
fork and exec the helper and the middle process call only system calls on memory set aside before the fork (`setsid`,
`pipe`, `fork`, `waitpid`, `read`, `open`, `dup2`, `close`, `fcntl`, `execve`, `_exit`; the argv, the environment and
the mark's buffer are made in `guard_command`), since a lock another thread held at the fork would never be
released. The guard never signals itself, Flare's pid, launchd (pid 1) or a zombie, and a pid it noted counts only
while its start time is the one it noted (a recycled pid is another process). Held by
`the_program_leads_a_new_session_and_its_guard_is_an_exec_d_orphan_with_a_group_of_its_own` (its `argv[0]` is the
mark), `the_mark_is_written_without_allocating_and_read_back_strictly`, `a_stray_guard_start_refuses_and_ends_nothing`
(crates/flare-proc/src/guard.rs) and `the_bootstrap_first_becomes_a_guard_when_started_as_one`
(crates/flare/src/ship_ctl.rs). Source: macOS port phase 6 step 1 (2026-11-08); the exec, review of phase 6
(2026-11-09).
1055. **macOS: a stop never signals a recycled pid.** `Job::terminate` kills the helper's pid only while the helper is
not reaped: its thread waits for the end with `waitid(WNOWAIT)` and `Job::finish` marks it ended under the job's lock
before anything reaps it, also when `waitid` failed (`ECHILD`, say with SIGCHLD ignored: then it is marked, reaped,
and the pass runs after), so a stop that races the end sees "ended" and lets the guard (which checks start times) do
the rest. Flare's own pass over the session and the group runs before the reap while the end was seen unreaped: the
unreaped helper still holds its pid, so no new group or session can take that id, and the pass never reaches one. A
stop that lands
while the helper is being started (turned off, updated or removed straight after it was turned on) has the start's
thread kill, mark and then reap what it spawned: a dropped `Child` would stay a zombie in its group, and every wait for
the group (an update's, a removal's) would run to `STOP_WAIT`. Held by
`mac_a_helper_stopped_while_it_starts_is_reaped_and_the_wait_returns` (fails without the reap),
`mac_the_end_is_marked_before_the_reap_and_the_pass_runs_while_the_ids_are_held` and
`an_update_of_a_plugin_whose_helper_runs_waits_for_it_to_end_and_starts_it_again` on a Mac. Source: macOS port
phase 6 steps 1 and 2 (2026-11-08).
1056. **macOS: a helper program is an executable file inside the plugin folder, checked again when it starts.**
`paths::unix_program_inside`: the shared path syntax (no `..`, no absolute path, no `:`), canonicalized inside the
folder (a link that points out is refused), a file, with the owner's execute bit; no extension is asked for. Windows
keeps `program_inside` (an `.exe`). The Mac program is `helpers.macos.command` (rule 1057). Held by
`mac_only_an_executable_inside_the_folder_starts`. Source: macOS port phase 6 step 1 (2026-11-08).
1057. **A plugin names its helper per system, and a manifest written for Windows means on Windows what it always
meant.** `plugin.json` keeps `helper` (`{command, args?}`, an `.exe`) as the Windows program and adds `helpers`
(`{"windows"?, "macos"?}`, each `{command, args?}`): `helpers.windows` is `helper` by another name (both at once is
refused), `helpers.macos` is an executable with `/` separators and no Windows extension (`paths::check_mac_program`).
`Manifest::helper` stays the Windows program and `helper_macos` is new; every reader that starts or routes to a helper
(`Hub::helper_of`, `with_helper`, `answers_with_helper`, the tool table and its notes, the details sheet's Program
tile, `discover::Found::helper`) asks `Manifest::helper_here`, so each system starts only its own program and a Mac
never tries an `.exe`. The manifest's rules (`process` both ways, tools need something to answer them) count a helper
of any system, and the scan checks every system's program is a file inside the folder, so a plugin is valid or not
the same way everywhere; on a system it names no program for, its pages run, tools only the helper answers are left
out with "that program is for another system", and the sheet says "None on this system". The channel's messages are
unchanged. Held by `helpers_name_a_program_per_system_and_a_windows_manifest_means_what_it_meant`,
`every_wrong_helpers_entry_is_refused_with_its_field` (crates/flare-mods/src/manifest.rs),
`a_windows_only_helper_runs_only_on_windows_and_a_mac_only_one_only_on_a_mac`,
`only_a_plugin_that_is_on_and_holds_process_has_a_helper` (crates/flare-mods/src/hub/lifecycle.rs),
`every_file_a_plugin_names_must_be_in_its_folder` (crates/flare-mods/src/discover.rs) and
`the_program_tile_is_this_systems_program_or_says_there_is_none` (crates/flare-mods/src/grants.rs). Source: macOS port
phase 6 step 2 (2026-11-08).
1058. **macOS: a quarantined helper is refused, never handed to Gatekeeper.** A helper starts in the background (the
plugin turned on, a restart after a crash), so a program macOS marked as downloaded (`com.apple.quarantine`) would make
Gatekeeper judge it with a dialog while the person may not be there. `paths::unix_program_inside` reads the attribute
(`getxattr`, links resolved) and refuses the start with a sentence Settings shows, naming `xattr -dr
com.apple.quarantine` for a plugin the person trusts; it also refuses while any file or folder inside the plugin
folder is quarantined (`paths::quarantined_in`, links read as themselves), since a script helper may run a downloaded
program beside it. Deliberate: Flare's own install from a zip writes every file itself and adds no quarantine, so an
unsigned program in a plugin zip runs with no Gatekeeper check; the person approved that install in Settings, and
Windows does the same (the zip's mark of the web is not passed on). Held by
`mac_only_an_executable_inside_the_folder_starts` (a quarantined copy of a working helper is refused) and
`mac_a_helper_is_refused_while_any_file_of_its_folder_is_quarantined` (crates/flare-mods/src/helper.rs). Source: macOS port phase 6 step 2
(2026-11-08).
1059. **An install keeps a zip entry's execute bit, and only that.** Off Windows a file the zip marks executable (any
`x` bit of its unix mode) is written `0755`, so a Mac helper installed from a zip can start; set-id, sticky and
others-write bits of the zip are never used, and every other file keeps the default mode. The program
`helpers.macos.command` names is made `0755` on the staged copy whatever the zip says, so a zip made on Windows
(Explorer, `Compress-Archive`: no unix modes) installs a Mac helper that starts. Windows has no such bit. Held by
`an_executable_in_the_zip_stays_executable_and_nothing_else_does`,
`a_mac_helper_from_a_zip_without_modes_is_installed_executable` (crates/flare-mods/src/install.rs) and the app's update
test on a Mac. Source: macOS port phase 6 step 2 (2026-11-08).
1060. **Every sample plugin runs, on Windows and on a Mac.** `plugin_samples` copies each folder of `sample-plugins/` as
it ships, turns it on (at most six background pages run at once, so one at a time), brings its background page up in
the fake browser and drives one round trip of its protocol: a call its own page makes, with the parameters it sends
(`storage.get`, `tabs.badge.set`, `tabs.progress.set`, `actions.set`, `sidebar.set`), or for the tour the tool its
page answers. A new sample fails the test until it has its round trip. The three app-level helper tests
(`plugin_install.rs`) run on a Mac too, with a `helpers.macos` `#!` script. Held by
`every_sample_plugin_loads_turns_on_and_answers_one_round_trip_of_its_protocol`
(crates/flare/src/appctl/tests/mods_tests/plugin_samples.rs). Source: macOS port phase 6 step 2 (2026-11-08).
1061. **macOS: "Open in Flare" makes the request a second launch makes, and nothing else.** Finder's service
(`openInFlare:userData:error:` on the services provider) and an open request (`application:openURLs:`) turn their
paths into a `Handoff` (`open_in::handoff_for`: absolute paths in order, each once, at most `ARGV_MAX`, non-file URLs
left out) delivered to the instance's `HandoffSink`, so the controller opens them through `folders_from_argv` as
before: a folder as a new tab even if one shows it, a file as the folder that holds it, every path checked on the
hand-off worker. No new route to a tab. Held by `the_request_is_a_second_launchs_hand_off_with_each_path_once`,
`a_folder_opens_as_itself_and_a_file_as_the_folder_that_holds_it` (crates/flare-win/src/macos/open_in.rs),
`open_in_service_turns_finders_files_into_one_tab_request` (crates/flare-win/tests/appkit.rs) and
`open_in_flare_feeds_the_hand_off_sink_before_the_loop_runs` (crates/flare/src/window.rs). Source: macOS port phase 6
step 3 (2026-11-09).
1062. **macOS: the open route is added to winit's delegate before the loop runs, never over a method it has.**
winit 0.30 sets its own `NSApplicationDelegate` when the event loop is built (slint builds it with the backend), so
`open_in::install` adds `application:openURLs:` to that class with `class_addMethod`, only when the class answers no
such method (`Added::HadOne` leaves it alone), and runs before `app.run()`: the request that launched Flare comes while
AppKit finishes launching. `NSTreatUnknownArgumentsAsOpen` is registered NO (registration domain, memory only), so a
path on the command line is opened once, by `main` (rule 32), and not again as a document. Held by
`open_in_open_urls_is_added_to_the_delegate_and_opens_tabs_but_never_replaces_one`,
`open_in_command_line_paths_are_not_open_documents` (crates/flare-win/tests/appkit.rs) and
`open_in_flare_feeds_the_hand_off_sink_before_the_loop_runs`. Source: macOS port phase 6 step 3 (2026-11-09).
1063. **macOS: the service's `Info.plist` keys live with its handler, and the tests never register it.**
`open_in::plist_keys` is the `NSServices` entry (message `SERVICE_MESSAGE`, the label "Open in Flare", `NSPortName`
the app's name, `NSSendFileTypes` `public.folder` and `public.item`, `NSSendTypes` `NSFilenamesPboardType` and
`public.file-url`, `NSRequiredContext` so it is on by default) in the bundler's (key, XML value) form, so the message
and the provider's selector cannot drift apart; it adds no document or URL type. Tests check it with `plutil -lint`
and `plutil -extract` and call the handlers as functions on a private pasteboard: no test sets the services provider
of a running app, runs `pbs`, or relaunches Finder. Held by `the_service_plist_is_valid_and_offers_folders_and_files`
and `the_service_selector_is_the_plists_message` (crates/flare-win/src/macos/open_in.rs). AppKit delivers the call
to the app registered under `NSPortName`, so once phase 8's bundle takes these keys a test on the built bundle must
check `NSPortName` equals its `CFBundleExecutable` (the process name); otherwise the item shows and the call never
arrives. Source: macOS port phase 6 step 3 (2026-11-09).
1064. **Off Windows a path argument is taken as it is.** `argv::paths_from_argv` turns Explorer's `C:"` (the
backslash before a closing quote) back into the root only on Windows (`unquote_arg` under `cfg!(windows)`): on a Mac
a Finder folder may be named `say "hi"`, and the fix-up made it `say "hi\`, a path that does not exist, so it was
dropped without a word. Held by `off_windows_a_name_that_ends_in_a_quote_is_kept_as_it_is` and, on Windows,
`a_drive_root_with_the_escaped_quote_is_the_root` (crates/flare-win/src/argv.rs). Source: review of phase 6
(2026-11-09).
1073. **macOS: the dictation link is a Unix socket only this user can reach, with the pipe's handshake on top.**
`link_pipe` is `<per-user temp folder>/flare-dictation/<24 hex of the key's hash>.sock` (the temp folder from
`confstr(_CS_DARWIN_USER_TEMP_DIR)`); the folder is made 0700 and a service refuses to start in one that is not a real
folder of this user's with no group or other bits; the socket is mode 0600; one service per name (an exclusive `flock`
on `<socket>.lock`, held for its life; only under it is a dead socket removed; a live socket without the lock or a file
in the way is never replaced); close removes the socket and the lock only while the socket is still the one it bound.
Both ends read the peer's uid with `getpeereid`: the service drops another user before reading a byte, the client
hangs up on another user's socket before sending its hello. The link key file is written mode 0600 (the umask would
leave it 0644). The protocol (`link.rs`: key, nonces, proofs, 64 KiB lines, the 5 s proof limit, the client's refusals)
is the same code on both systems, only `Event`, `Conn`, `connect` and the acceptor are per system
(`macos/link_sock.rs`). Held by every test of `crates/flare-dictation/tests/link.rs` (no longer Windows-only), its
`unix_socket` module and `link::sock::tests`. Source: macOS port phase 7 (2026-11-09).
1074. **macOS: microphone access is read, never asked for, and access that is off opens nothing.** Flare reads the
authorization with `+[AVCaptureDevice authorizationStatusForMediaType:]` only (no `requestAccess`, held by a scan in
`privacy.rs`); off or restricted is `OpenError::Denied` before any CoreAudio call (macOS would hand a denied app
silence, which would read as "Heard nothing"); never asked lets the open go ahead, and the system's own question then
comes once, worded by the bundle's `NSMicrophoneUsageDescription` (phase 8's Info.plist). The Settings row says
"Microphone access is off. Turn it on in System Settings > Privacy & Security > Microphone." when the page lists the
microphones and when a Test is refused; the pill keeps Prism Terminal's words. Held by
`access_that_is_off_refuses_before_any_device_is_touched`, `the_authorization_status_means_what_apple_says`,
`reading_the_access_asks_nothing_and_answers`, `access_that_is_off_names_the_settings_pane_and_nothing_else_is_a_problem`
and `the_mac_microphone_records_from_input_only_and_never_asks_by_itself`. Source: macOS port phase 7 (2026-11-09).
1075. **macOS: the microphone is an input queue on an input device, 16 kHz mono by the shared resampler.** `macos/mic.rs`
lists the HAL's devices with input streams by UID (opening none), opens an `AudioQueue` input on the chosen UID or on
the default input read at that open (so a new default is followed at the next press, as on Windows), mono `f32` at the
device's nominal rate in 50 ms buffers (800 samples at 16 kHz from 48 or 44.1 kHz, nothing lost between buffers), and
reports a device that stops being alive once, through the shared `pump`. The fallback to the default for a UID that is
gone, the file seam and the async open are the Windows code. Held by `the_c_layouts_are_the_frameworks_own`,
`os_statuses_map_to_the_open_outcomes`, `a_device_rate_the_resampler_cannot_take_becomes_48_khz`, the shared
`mic::tests`, and by hand `the_real_default_microphone_lists_and_records_a_second` (ignored). Source: macOS port phase 7
(2026-11-09).
1076. **Dictation runs end to end with only the device faked, on both systems.** A 48 kHz WAV played through
`MicSource::File` in real time reaches the controller, the final pass is that clip at 16 kHz (its pitch kept), and the
paste is the fake engine's words cleaned plus one space, into the tab in front at the start, never a newline. Held by
`a_wav_played_as_the_microphone_is_heard_resampled_and_pasted_into_the_tab_in_front`
(`crates/flare-dictation/tests/pipeline.rs`). Source: macOS port phase 7 (2026-11-09).
1077. **macOS: the dictation key from any app asks for no permission, and a chord needs none.** A binding that ends in
a key (F9, Ctrl+Shift+D) is a Carbon `RegisterEventHotKey` on the main thread (it swallows the key system-wide and
reports press and release with no permission). A modifier binding (the default Ctrl+Cmd, Right Option) needs the
key-up and "another key", so it is a LISTEN-ONLY `CGEventTap` on the hook thread, made only after
`CGPreflightListenEventAccess` reads true (creating a tap without it would make the system ask). Until then the hook
says nothing, holds no lock, re-reads the permission every 500 ms, and the window reads the key itself; the "Dictate
from any app" row's sub names System Settings > Privacy & Security > Input Monitoring. Nothing is ever posted or typed
(no `CGEventPost`, no Accessibility: the Windows build types only into Flare's own tabs), and no code calls
`IOHIDRequestAccess` or `AXIsProcessTrustedWithOptions`, and `CGRequestListenEventAccess` only in
`macos/listen_ask.rs` on the person's own act, never from the hook (rule 1137; scan). Held by
`a_chord_registers_says_ready_and_reports_its_keys_with_the_app_in_front`,
`a_modifier_key_waits_silently_for_input_monitoring_and_asks_for_nothing`,
`the_settings_problem_names_input_monitoring_only_for_a_modifier_key_without_it`,
`what_stops_dictating_from_any_app_replaces_the_rows_sub_only_while_it_is_on` and
`the_mac_key_hook_listens_only_posts_nothing_and_never_asks_by_itself`. Source: macOS port phase 7 (2026-11-09).
1078. **macOS: the hook's lines are the Windows hook's lines.** Carbon's press is the binding's modifiers down, then its
key down (a press while held is the key's repeat); its release is the key up, then the modifiers up, so the shared
`KeyFeed` and reducer decide as on Windows. The tap reports a binding modifier by its side's device bit (Right Option is
not Left Option), says only `Other` (never which key) for any other key, Caps Lock or Fn going down while a binding key
is held, ignores a key Carbon has taken, and finds a lost up from `CGEventSourceFlagsState`. The picker's 1 to 9
(digit row and keypad) and Esc, and Esc while recording, are Carbon hot keys registered only while switched on, with no
modifier and with each set of the binding's own modifiers (Cmd+2 stays the browser's, Option+2 stays @). One copy holds
the key through an exclusive `flock` on `$TMPDIR/Flare-dictation-key.lock` (`Waiting` once while another has it); a
binding the running way cannot serve, a chord another app holds or a tap that cannot be made says `Failed` once and
lets the keys and the lock go, and an earlier owner's let-go never removes a later owner's hot keys. Held by
`carbons_press_and_release_read_as_the_windows_hooks_lines_and_dictate`,
`option_typing_at_on_a_norwegian_mac_never_dictates_and_never_says_which_key`,
`ctrl_cmd_is_the_default_chord_of_modifiers_read_from_both_sides`, `a_key_carbon_has_taken_is_carbons_and_not_another_key`,
`the_picker_takes_digits_and_esc_alone_or_with_the_bindings_own_modifiers_only`,
`the_tap_reports_the_binding_says_other_and_sweeps_a_lost_up`, `a_second_copy_waits_and_takes_over_when_the_first_lets_go`,
`a_binding_this_way_cannot_serve_says_failed_once_and_lets_everything_go` and
`a_chord_another_app_holds_or_a_tap_that_cannot_be_made_is_failed` (`crates/flare-dictation/src/macos/keyhook.rs`).
Source: macOS port phase 7 (2026-11-09).
1079. **macOS: a binding is named and checked as a Mac key.** The stored binding is the same on both systems (W3C codes
by position): Meta is Command and Alt is Option, so AltGr's key, Right Alt, is Right Option. Settings names it the Mac
way ("Right Option", Control+Command as the symbols of the phase 3 key table); the capture and the stored-key reading
refuse Option with a key (it types a character on a Mac), the Command chords the Mac or Flare own (Cmd+C V W T F Q H M
A N, Tab, Space, the digits, with or without Shift) and keys a Mac lacks (Pause, Scroll Lock, F21 to F24); a binding
with such a key says so on the row. Windows names and checks are unchanged. Held by
`on_a_mac_a_binding_is_named_with_the_macs_keys_and_symbols`,
`on_a_mac_a_key_that_types_or_a_command_chord_the_mac_owns_is_refused` (`crates/flare-dictation/src/hotkey.rs`),
`every_binding_key_maps_to_the_mac_key_in_its_place_and_back` and `a_key_captured_from_the_page_is_stored_and_drawn`.
Source: macOS port phase 7 (2026-11-09).
1080. **macOS: media pause is off, with the reason, because no public API can do it.** What another app is playing is
known only to MediaRemote, private and closed to third-party apps since macOS 15.4; `MPRemoteCommandCenter` and
`MPNowPlayingInfoCenter` are an app's own player; the media key only toggles (with nothing playing it would start
music); scripting each player asks for Automation. So the Mac has no media port (nothing is ever paused), and the
"Pause media while dictating" switch is drawn off and dimmed with "Not on a Mac: macOS lets no app pause another app's
player and resume only that.", even when an import stored it on. Held by
`where_media_cannot_be_paused_the_switch_is_off_and_says_why` (`crates/flare-settings/src/page/dictation.rs`) and the
media tests. Source: macOS port phase 7 (2026-11-09).
1081. **macOS: no engine child runs outside the reaper.** The Windows kill-on-close job is, on a Mac, a process group
per child plus one reaper (`crates/flare-dictation/src/macos/job.rs`): every engine spawn calls `job::isolate`
(`process_group(0)`, a no-op on Windows) before `spawn` and `job.assign` after it; `Job::new` starts `/bin/sh` with the
fixed `REAPER` program, a clean environment and a pipe on stdin; `assign` writes `+ <pid>` (and `- <pid>` for each
child seen gone since, so a recycled pid is never killed); when the pipe closes (the job dropped, or Flare dead in
any way, `kill -9` included) the reaper sends `SIGKILL` to each group it holds. A job with no running reaper refuses
`assign`, and the callers kill a child they could not hand over, as on Windows. It never takes pid 0, 1 or Flare's
own. Held by `job::tests` (a child, a grandchild, an owner killed with `SIGKILL`, a withdrawn pid, a dead reaper,
garbage on the pipe), `audio_reaches_the_engine_only_over_loopback_and_pipes_on_both_systems` (isolate, then spawn,
then assign in each spawner) and `crates/flare-dictation/tests/engine.rs` and `host.rs`, which now run on both
systems. Source: macOS port phase 7 (2026-11-09).
1082. **macOS: the bundled engine is the Metal engine, and there is no pack.** Every Mac Flare targets is Apple Silicon,
whose GPU always has Metal, so `gpu::BUILT_IN_GPU` is true on a Mac (nothing is probed) and `EngineConfig::bundled_gpu`
follows it: the bundled folder is first started as the GPU engine (no `-ng`), and the once-and-for-good fall back of
Windows runs the same folder on the CPU (`-ng` to whisper-server and parakeet-cli, `0` to the host). The NVIDIA pack is
not in the Mac's store (`catalog::PACK_OFFERED`), no CUDA cache is set, the GPU card stays hidden (no NVIDIA card),
the Recommended pick is Large v3 Turbo (`gpu::gpu_ready`), and no model row says it is slow without the pack
(`DictationData::gpu_built_in`). Windows is unchanged: `bundled_gpu` is false there. Held by the Metal tests of
`tests/engine.rs`, `a_large_model_never_warns_where_the_engine_has_a_gpu_of_its_own`, the gpu and catalog tests and
`the_shipped_catalogue_opens_and_the_failure_texts_are_the_parity_line`. Source: macOS port phase 7 (2026-11-09).
1083. **macOS: models live in `~/Library/Application Support/FlareDictation`, never in the temp folder.** With no
`LOCALAPPDATA` the store used to fall back to the temp folder, which the system empties. On a Mac `default_root` is
`flare_win::paths::local_app_data()` (`~/Library/Application Support`) plus the same folder name as on Windows, apart
from the profile so a profile reset never deletes gigabytes of models. The neighbour app's models folder is a Windows
folder and is not looked for on a Mac. Source: macOS port phase 7 (2026-11-09).
1084. **macOS: the engine is built from one pinned source, never fetched as a binary.** whisper.cpp's b5130 release has
no macOS program, so `tools/build-whisper-macos.sh` fetches commit `927cfce3...` by its hash, refuses to build unless
the commit AND its tree hash are the pinned ones (`catalog::ENGINE_MACOS`), builds whisper-server and parakeet-cli with
Metal embedded, OpenMP, KleidiAI and curl off, keeps only the libraries the programs link to under their versioned
names with `@loader_path` rpaths, refuses any link outside `/usr/lib`, `/System` and the folder, and writes a stamp with
every file's SHA-256 last (a folder is reused only when every file still matches). The resident Parakeet host loads
`libggml` and `libparakeet` from that folder by name (`pick_lib`). The macOS CI job builds it (cached by commit),
fetches the catalogue's tiny model by size and SHA-256 and runs `tests/metal_real.rs`. Held by
`the_macos_build_pins_match_the_catalogue`, `the_macos_build_checks_the_commit_and_tree_before_it_builds_and_fetches_nothing_unpinned`,
`the_macos_ci_builds_the_pinned_engine_and_fetches_the_catalogues_tiny_model` and
`the_mac_host_picks_each_library_by_its_own_stem_and_version`. Source: macOS port phase 7 (2026-11-09).
1085. **macOS: what Flare fetches carries no quarantine, and audio stays on this machine.** A model is written by Flare
itself through ureq and `std::fs`, so it has no `com.apple.quarantine` flag and Gatekeeper never judges it; it is data,
never made executable, and runs only after its size and SHA-256 match. The engine is built (rule 1084) and ships inside
the signed bundle (phase 8), never downloaded on a Mac. The clip reaches the engine only over 127.0.0.1 (the one
address dialled and the one the server is told to listen on) and stdin (`-f -`), the Mac engine is built without curl,
and no engine file writes audio to disk. Held by `a_fetched_model_carries_no_quarantine_flag_and_is_not_executable`,
`audio_reaches_the_engine_only_over_loopback_and_pipes_on_both_systems` and `no_audio_is_written_to_disk` (now also
over `macos/job.rs` and the host). Source: macOS port phase 7 (2026-11-09).
1086. **macOS: a tap whose Input Monitoring is revoked ends and lets the key go.** While the listen-only tap runs, `tick`
reads `CGPreflightListenEventAccess` (never asks) every 10 ticks (500 ms). When it reads false the tap ends with
`Failed`, the hook lets go of its hot keys and the `flock`, and the owner re-arms a hook that waits for the permission
without the lock, so the window reads the key again instead of a deaf tap holding it for good. Held by
`input_monitoring_revoked_while_the_tap_runs_ends_it_lets_the_lock_go_and_waits_again`. Source: phase 7 review
(2026-11-09).
1087. **macOS: a pid leaves the reaper before its child is reaped.** Every engine child (whisper-server, parakeet-cli,
the Parakeet host) is waited only through `job.member()`, whose `try_wait` and `wait` read the exit with `waitid(...,
WNOWAIT)`, write `- <pid>` while the child is still a zombie, and only then reap it. The reaper therefore never holds a
pid the kernel may hand to another process, so its `kill -9` at Flare's end can never hit an unrelated process. A
source scan holds that no assigned child is waited with a bare `child.try_wait()` or `child.wait()`. Windows' `Member`
is the plain wait. Held by `a_child_waited_through_its_member_is_withdrawn_before_it_is_reaped` and
`audio_reaches_the_engine_only_over_loopback_and_pipes_on_both_systems`. Source: phase 7 review (2026-11-09).
1088. **macOS: the reaper reaps even when its pipe leaked.** std creates the reaper's pipe and then sets close-on-exec,
not atomically, so a process another thread spawns at that moment can inherit the write end and keep the pipe open
after Flare dies. The reaper therefore also watches its parent (`kill -0 $PPID` every 2 s) and reaps when Flare is
gone. It answers `ok` to every `+` and `-` line and Flare waits for the answer, so a pid is held before `assign`
returns and a reap the watcher starts never misses a line still in the pipe. SIGPIPE is ignored so an answer to a dead
Flare cannot end the reaper first. Held by `the_children_die_with_their_killed_owner_even_when_another_process_holds_the_pipe`
(it fails without the watcher) and the other `macos/job.rs` tests. Source: phase 7 review (2026-11-09).
1089. **macOS: a hot key register never stalls a quit.** Quit and switch-off run on the main thread and join the hook,
while a register waits for the main queue. `await_main` reads the answer in 20 ms slices and gives up as soon as the
hook is no longer wanted (the queued `unregister` undoes a late register), instead of waiting the full 2 s. Held by
`a_register_the_main_thread_cannot_answer_gives_up_as_soon_as_the_hook_is_stopped`. Source: phase 7 review
(2026-11-09).
1090. **macOS: the system's Control chords are never a binding.** Without Shift and Option, Control+Command+Q (Lock
Screen), Control+Command+F (full screen), Control+Command+Space (emoji picker) and Control with an arrow key (Spaces,
Mission Control, App Expose) are refused by `usable_hotkey_on(KeyOs::Mac, ..)`: the system wins over a Carbon hot
key, so such a binding would show as working and never fire or lock the screen. Windows is unchanged. Held by
`on_a_mac_a_key_that_types_or_a_command_chord_the_mac_owns_is_refused`. Source: phase 7 review (2026-11-09).
1091. **The microphone row says the access as it is now.** A refused Test says `denied_text(mic::access())`: on a Mac
"restricted" for a profile's restriction, else the "off" line with the pane; Windows keeps "Microphone access was
refused.". Each listing replaces a line that was only the access (off or restricted) with the new reading, so it goes
once access is allowed, and leaves a Test's own failure. Held by
`a_refused_test_names_the_access_as_it_is_and_a_listing_clears_access_that_came_back`. Source: phase 7 review
(2026-11-09).
1092. **macOS: this user's opened-up link folder is made private again.** When the link's folder is a real folder of
this user with group or other bits set, it is opened with `O_NOFOLLOW | O_DIRECTORY`, checked again on the
descriptor and `fchmod`ed to 0700, then the folder rule (rule 1073) is checked as before. A symlink or another user's
folder is still refused and never changed. Held by
`this_users_own_folder_left_open_is_made_private_and_a_symlink_is_still_refused`. Source: phase 7 review (2026-11-09).
1093. **The child's PATH is changed only on Windows.** `runtime_path` puts the CPU engine's folder in front of PATH
with `;` for a GPU folder on Windows (DLLs are found on PATH). A Mac finds libraries by `@loader_path` and joins PATH
with `:`, so nothing is added there. Held by `the_runtime_goes_on_path_only_on_windows_and_only_for_another_folder`.
Source: phase 7 review (2026-11-09).
1094. **macOS: the engine folder is reused only with exactly its stamped files.** `engine_ok` in
`tools/build-whisper-macos.sh` requires the stamp's commit, every listed file's SHA-256 AND that the folder holds no
file the stamp does not list (a library planted beside an `@loader_path` rpath would load), so anything else means a
rebuild. The stamp lists neither `.source` nor its own `.source.tmp` (which used to be listed and then missing, so the
check never passed). Held by `the_macos_engine_folder_is_reused_only_when_it_holds_exactly_the_stamped_files`, which
runs the script's own `engine_ok` and stamp block in bash. Source: phase 7 review (2026-11-09).
1095. **A media worker that dies mid-call closes its queue before its reply.** `run_worker` owns the request queue and
runs each port call under `catch_unwind`; when a call panics it drops the queue first and the reply second. The caller
learns of the death from the reply, so the next request finds the queue closed and starts a fresh worker. Before,
a request sent while the dead worker was still unwinding went into its queue and was answered empty as well
(`answers_empty_when_the_worker_dies_mid_call_and_starts_another_on_the_next_pause` failed about 1 run in 40 under
`--stress-count`; 300 of 300 pass now). Shared with Windows; what a caller is told is unchanged (an empty token for a
crash). Source: phase 7 review gate run (2026-11-09).
1103. **macOS: the `--e2e` test hook is a Unix socket with the named pipe's guarantees.** The hook serves
`flare_e2e_proto::pipe_name(profile)`, `e2e-<16 hex of the profile hash>.sock` in the control channel's private
folder (`<user temp>/flare-ctl`, 0700, this user's), bound 0600 through `flare_ctl::pipe::bind_private_socket` under
the control channel's first-instance lock, so a second `--e2e` launch on the same profile cannot take it over; a
client of another uid is dropped before a byte is read; every wait is a 50 ms `poll` that looks at the stop flag, so
dropping the server never hangs on a connected client (the Windows `CancelSynchronousIo`), and the socket is removed
when the server stops. Without `--e2e` there is no socket. Held by the e2e_hook tests that now run on macOS
(`a_hello_round_trips_without_the_ui_thread`, `a_second_server_on_the_same_profile_cannot_take_the_pipe`,
`dropping_the_server_returns_even_with_a_client_connected` and the rest) and
`the_mac_socket_is_this_users_alone_and_goes_with_the_server`, `the_mac_hook_is_a_short_socket_in_the_private_folder`.
Source: Mac E2E in CI, the port plan's Ongoing row (2026-11-09).
1104. **macOS: the harness never takes the screen, the keyboard or the owner's processes.** `tools/e2e` runs on macOS
with the Windows guarantees in Mac terms: a launched app leads its own process group and dropping it kills the group
and every descendant by parent pid (the job object's kill-on-close; a tab's shell sits in its own session); reaping
is by the world's path in a command line only (`ps`), so the owner's Flare is never touched; the world is a HOME of
its own (`<root>/home`, its `Library/Application Support/Flare` the profile) at its canonical path (`/private/var/...`),
so the app, the hook's socket name and the reaper read one spelling; pixels come only from the app's own render
(`Snapshot`, Slint's snapshot), never a screen capture, which would need Screen Recording. Around every scenario a
guard looks every 200 ms and fails the scenario when a process of the run is the frontmost app or owns a window that
overlaps a display (the window server's list: owner pid, bounds and alpha need no right); the pasteboard's change count
is compared before the first and after the last scenario. Held by `a_launched_tree_is_killed_when_the_child_drops`,
`the_reaper_spares_a_process_without_the_world_path` (proc_mac.rs),
`the_guard_flags_a_window_or_the_keyboard_of_the_run_and_only_those`, `a_parked_or_clear_window_shows_nowhere`,
`the_harness_itself_owns_no_on_screen_window`, and the CI job's first run, where the guard caught the clamped window
of rule 1107. Source: Mac E2E in CI (2026-11-09).
1105. **macOS: a scenario is gated only in `mac_gate::GATED`, with its reason, and a gated one is never a pass.** The
scenario table is the same on both systems. A Mac run does not start a gated scenario, prints it as `gated` with its
reason after the table and counts it apart; a module that needs Windows-only harness parts (the speech host's process
list, the tour's capture) is a stub on macOS that answers only that it is gated. Gate only what is Windows' own
(WebView2, Velopack's Windows swap, the Windows speech host and key hook, PrintWindow) or a Mac port not written yet,
and say which in the reason; a Mac bug a scenario finds is fixed, not gated. `docs/e2e.md` ("macOS E2E") lists both
sets. Held by `every_gated_name_is_a_scenario_once_with_a_reason` and `a_gated_scenario_is_never_started`. Source: Mac
E2E in CI (2026-11-09).
1106. **macOS: the scenarios keep their Windows keys and shell; the harness and one seam translate them.** A chord a
scenario presses goes through `chord::mac_app_chord` in `Session::call`: an app chord of the Windows table whose Mac
key is the same chord with Cmd in Ctrl's place is sent with Cmd (`Ctrl+T` is `Meta+T`, `Ctrl+Shift+P` is
`Meta+Shift+P`), paste is Cmd+V, and Ctrl+Tab and the shell's Ctrl keys (Ctrl+C, Ctrl+A) are unchanged, decided by
the app's own `flare_tabs::keymap::chord_of` for both systems, never a hand list. The tab's shell is pwsh when the
runner has it: `FLARE_E2E_SHELL` (read through `e2e::global().env`, macOS only, only when the profile saves no shell)
names the shell a new tab starts, so the PowerShell prompt checks run unchanged. Held by
`the_app_chords_take_cmd_on_a_mac_and_the_shell_keys_stay` and `the_e2e_seams_are_the_listed_ones`. Source: Mac E2E in
CI (2026-11-09).
1107. **macOS: an `--e2e` window is never pulled onto a screen.** AppKit keeps a titled window on a screen when it is
ordered in or moved, so the parking spot (-32000, -32000) became a 40 px strip at the display's edge on the macOS
runner ((-984, 673) to (40, 1347) on a 1024 by 768 display; the launch's `window.park` check and the screen guard
both failed it). Under `--e2e` only, before the first window, `window_park::keep_windows_off_screen` gives `NSWindow`
a `constrainFrameRect:toScreen:` that keeps the frame it is given, and `park` puts it on the window's own class too;
the real app never calls either. Held by `a_parked_class_keeps_its_frame_off_every_screen_and_nswindow_keeps_the_rule`
and every Mac launch's `window.park` check. Source: Mac E2E in CI, run 42558418417 (2026-11-09).
1108. **macOS: the hook's title rects and scale change follow the Mac window.** With the native caption the window
buttons are AppKit's traffic lights, so `ui.rect` answers `cog` and `help` counted from the right edge without the
three Windows buttons, refuses `close`, `max` and `min` with a reason, and starts the strip after the traffic lights'
corner (`Px.snap(caption-gap) + Px.hair()`), not after the 42 px mark; `DpiChange` takes the content view, since there
is no HWND. Held by `the_title_rects_follow_the_mac_caption`. Source: Mac E2E in CI (2026-11-09).
1109. **macOS: an `--e2e` window is clear and click-through on the main screen, and the run is never napped.** A Mac
draws nothing for a window on no screen (no display-link tick, no Metal drawable), so the parked window is put on the
main screen at alpha 0, shadowless and ignoring every click; under `--e2e` `NSWindow`'s `setAlphaValue:` and
`setIgnoresMouseEvents:` keep 0 and YES whoever asks (Slint reveals a GPU window at alpha 1 after its first frame),
and the process holds a user-interactive activity so App Nap never stretches a scenario's waits. `window.park`'s
report fails a window on a screen that is not clear. Held by `a_window_on_a_screen_in_the_dock_or_in_front_is_a_problem`
and every Mac launch's `window.park` check. Source: Mac E2E in CI (2026-11-09).
1110. **macOS: a chord reaches a Slint field with the keys a Mac keyboard gives it, and a field's End is Cmd+Right.**
Slint's winit backend reports Cmd as `Key::Control` and Ctrl as `Key::Meta` on Apple systems (it follows Qt), so
`e2e_glue::slint_modifiers` hands a chord's Meta (Cmd) to Slint as Control and its Ctrl as Meta there; Windows is
unchanged. A Mac text field scrolls on End and moves nothing (Slint's `TextCursorDirection` has no End on Apple), and
Ctrl+A moves to the line's start, so the scenarios that fill a field press `chord::FIELD_END` (Cmd+Right on a Mac) and
`chord::FIELD_SELECT_ALL` (Cmd+A). Before this the Mac runner typed every colour code after the field's first
character and no picked colour ever applied (themeEditorLive, themeSwitch, accent, accentAsk, colourPicker and five
more, run 42569349881). Held by `a_chord_with_modifiers_reaches_slint_as_modifier_presses_around_the_key`. Source: Mac
E2E in CI (2026-11-09).
1111. **macOS: every `--e2e` window is born clear.** The park makes the main window clear once it has a handle, a tick
after Slint has ordered it in; the screen guard saw windows of 18 scenarios on the runner's display at full alpha for
that moment (run 42569349881). Under `--e2e` only, `window_park::born_clear` replaces `NSWindow`'s two content-rect
initialisers with ones that call AppKit's own and then set alpha 0, no shadow, click-through and no order-in
animation, so no window of the run is ever drawn on a screen; the guard's line names the window number, alpha and the
owner's command line. Held by `a_born_clear_class_initialises_through_the_stand_in` and the screen guard. Source: Mac
E2E in CI (2026-11-09).
1112. **macOS: the world's pwsh, logs and program keys are the Mac's.** The world's pwsh profile sets PSReadLine's
Windows edit mode (pwsh on a Mac starts in Emacs mode, where Escape is a prefix: a scenario's Escape ate the next
paste's escape sequence and left the line it meant to clear); the harness reads a profile's diagnostics log where the
app writes it (`world::logs_dir`: `<home>/Library/Logs/Flare` for the default profile under the world's HOME, as
`startup::logs_dir_of`); and a key a scenario means as the program's own is sent as written
(`Session::key_as_is`), since on a Mac Ctrl+F is the program's and Cmd+F finds everywhere (the keymap's Mac rule).
The profile's Clear-Host also clears the scrollback, as Windows' does, with `cls` its alias, and the scenarios that
clear a question box type `world::CLEAR_HOST` (the same, spelled out) and read a file with `world::GET_CONTENT` (pwsh
on a Mac has no `cat` alias) (pwsh on a Mac sends the
terminfo `clear` alone; the screen went into the scrollback and a "cleared" question box was still the last thing
written, agentHooks in run 42587853627); a dropped path is quoted as a POSIX path; the bell burst has no sleep
between its ten bells (pwsh on the runner stretched it past the one-second gate).
Held by `the_app_chords_take_cmd_on_a_mac_and_the_shell_keys_stay`. Source: Mac E2E in CI, run 42569349881
(2026-11-09).
1113. **macOS: an `--e2e` window takes its focus from the hook alone.** The parked window is an accessory's that is
never key, so AppKit reported it as having lost the focus and every "the person is looking" rule read the run as
away: a visited tab kept its Finished mark and the badge counted it (indicator, attention, agentHooks on the runner).
`window::e2e_drops_native_focus` drops winit's `Focused` on macOS under `--e2e`; the scenarios move the focus with the
hook's `Focus`, as the no-activate Windows window, which never gets either event, always did. Held by
`only_a_mac_e2e_run_drops_the_native_focus`. Source: Mac E2E in CI (2026-11-09).
1114. **macOS: a resize to css px is asked in device px at the window's own scale, and the Windows DPI matrix is
gated.** Slint hands a logical size to winit, which converts it with the screen's backing scale (1 on the runner),
not the scale the hook's `DpiChange` gave the window, so at 225 percent a `Resize` to 1000 css came out 1000 px
(444 css) and every panel check of panelImage, panelSplitter and convoPanels at 225 percent failed (run
42569349881). On macOS `e2e_glue::resize_size` asks for `w` and `h` times the window's scale in device px; Windows
keeps the logical size. Slint's own `SLINT_SCALE_FACTOR` was tried and is no stand-in: the app's logical sizes then
missed the scale too (run 42587853627). `dpiMatrix` and `monitorMove`, which drive cmd.exe through the eight Windows
scale steps and per-monitor moves, are gated (a Mac's backing scale is 1 or 2 and changes only with the screen). Held
by `a_mac_resize_is_asked_in_device_px_at_the_windows_own_scale`. Source: Mac E2E in CI (2026-11-09).
1115. **macOS: the scenarios read the Mac title row as it is.** With the native caption the window's buttons are the
traffic lights, which have no hook rect, and no app mark is drawn (`AppMark` is hidden), so verticalTabs checks the
cog in the bar and the traffic lights' corner before the tabs and skips the mark with its reason; the ? is the second
button from the right edge, so its tip is held 6 px inside the window (`tip_place`'s margin) rather than centred, and
tooltips accepts that on a Mac only. Source: Mac E2E in CI, runs 42569349881 and 42587853627 (2026-11-09).
1116. **macOS: a UI-thread stall names the UI thread's frames, as on Windows.** The stall watchdog's `ui-stack` line
carried no frames on a Mac (`NoSampler`; diagLog's "with the frames of the UI thread named (0 lines)" on the runner).
On Apple silicon `flare_win::thread_stack` (macos/thread_stack.rs) suspends the UI thread, reads its registers
(`ARM_THREAD_STATE64`), walks the frame-pointer chain inside the thread's own stack bounds into a fixed buffer with
nothing allocated or locked while it is stopped, resumes it through a guard and names each address `image+0xoffset`
through `dladdr` afterwards; an Intel Mac keeps no sampler. Held by
`the_walk_follows_the_chain_inside_the_stack_and_stops_outside_it`,
`a_busy_thread_is_sampled_200_times_named_and_keeps_running` and `a_finished_thread_is_not_sampled`. Source: Mac E2E
in CI, run 42592969003 (2026-11-09).
1117. **macOS: a miniaturised window is minimised to Flare, and a hook scale change never outgrows its buffer.** A
window miniaturised by Cmd+M, the yellow light or the hook gets no resize on a Mac, and Slint updates its minimised
state on a resize only, so the marks kept moving behind the Dock (indicatorStyles' "minimising pauses the marks" on
the runner, where the window server said occluded and Slint said not minimised). `window::is_minimised` reads winit's
live `is_minimized` (`isMiniaturized`) on macOS, Slint's state elsewhere, for the marks' pause, the tick, the raise
and the saved bounds, read again on each occlusion change (the one event a miniaturise sends); the hook's restore
deminiaturises without the refusal it had. Under `--e2e`, the hook's `DpiChange` on a Mac sets the window item to the buffer's own size at
the new scale at once (`e2e_glue::item_size_for`), because Slint's scale change kept the item's logical size and a
frame drawn before the real window grew asked the software renderer for 2655 px of a 1180 px buffer: the app
panicked (tabLooks, run 42598992162). Held by `the_systems_minimised_answer_wins_over_slints` and
`the_item_fills_the_buffer_it_has_at_the_new_scale`. Source: Mac E2E in CI (2026-11-09).
1118. **macOS: a timer steps the sidebar's slide in a Mac `--e2e` run.** The parked clear window gets no
`RedrawRequested` from AppKit, and the slide (audit V3) starts at its first frame, so a reveal, the one slide that
never ends in a jump, never moved: every slid-out picture of verticalTabs showed no sidebar while the probes said it
was out (nine checks, runs 42612597989 and 42622873207). `appctl::timer_steps_slide` starts the slide timer where
there is no OS window and on a Mac under `--e2e`; the app with a real window keeps its frames. `sidebar.state` now
carries `slide_now`, `slot_now` and the OS frame count, so a slid-out picture with no sidebar names its cause. Held by
`a_timer_steps_the_slide_without_frames_and_in_a_mac_e2e_run`. Source: Mac E2E in CI, run 42624800535 (2026-11-09).
1119. **A scenario's picture or store check waits out what moves, on a Mac; Windows is as it was.** Seven checks held
a moving thing to one instant and failed on the Mac runner only by its timing: helpPanel's picture before a selection
is taken once two shots in a row agree (the row's 120 ms hover fade lit the "changed colour"), themeSwitch reads the
agent colour once `settings.json` has it (the store is written after a debounce; Cancel was held to a store that had
not caught up), clickCaret finds the old output once the prompt line shows "echo zz" and no longer PSReadLine's grey
guess from history, "echo ab1Xcdef" (the last row held the needle, and the click moved the caret), verticalTabs'
slid-out no-shadow check compares with the terminal's ground token (a sampled pixel landed on the Mac world's longer
prompt) and its ring check looks over a breath (a question's ring was caught at its faintest), as attention's question
line does (caught at 0 px once). exitClosesTab types "exit" once the tab it brought to the front shows its prompt and
its screen and Flare's answers to it have held still 300 ms (typed at once, pwsh read it inside the redraw's cursor
reports and never ran it); colourPicker types its first hook line once the process poll sees the stand-in agent, not
after a fixed 1.5 s. Every such wait is a condition (`ctx.until`, never chained `settle` calls) under
`cfg!(target_os = "macos")`: CI has no Windows E2E job, so the Windows path keeps its old checks until the owner's PC
has run them. The program-copy unit test ticks once before its press, so the press no longer races the window's real
timer (the Windows check job, run 42612597989). Source: Mac E2E in CI, runs 42624800535 and 42626884993; review of the
branch (2026-11-09).
1120. **macOS: a warm spare answers its cursor questions from its own grid, and its tab gets its kept output without
its questions to the terminal.** A spare shell (`SessionManager::prewarm`) has no tab yet. pwsh asks for the cursor
(`CSI 6n`) after drawing its prompt. Answered only at adoption (the kept output replayed into the tab's emulator), the
report came late, .NET read it as typed keys, echoed characters no key sends, asked again and never stopped
(helpPanel's new shell on the Mac runner, runs 42612597989 to 42630197431). Never answered (the question only taken
out of the kept output, the first fix), pwsh was still waiting when the tab typed, read the typed line as its answer
and garbled it: bytes no key sends ran as a command (helpPanel, runs 42674535599, 42712456085, 42727386311 and the hand
run 42727392100; the e2e write tap, `flare_term::session::set_write_tap` and the `pty.written` probe, showed Flare had
written only the clean line). So outside Windows a spare keeps a grid of its size (`Session::grid`) fed what it prints,
and every cursor report that grid gives is written at once (`answer_from_grid`), as a tab's would be; the opening
question of rule 231 is one of them, answered at the top left as before. `SessionManager::adopt` drops the grid and
hands the tab `banner_for_tab(kept output)`: outside Windows, `term_query::without_questions` takes out every whole
question (device status and attributes, mode, window and keyboard reports, colour queries, capability requests), so
none is answered twice or late; the rest stays byte for byte. The whole kept output is read at adoption, so a question
split over two reads is taken out too, and a question cut off at the end stays for the live output to finish. Spares
stay on for every shell, pwsh and fish included (a fix that let such a spare go stopped prewarming pwsh for the
session). Windows is as it was: the spare answers only the opening question (rule 231, `answer_opening`) and hands the
rest over (ConPTY stands between the shell and Flare there). Held by
`a_spare_that_asked_the_terminal_is_adopted_without_its_question`, `a_question_split_over_two_reads_is_taken_out_too`,
the `term_query` tests, and on the Mac by rule 231's `a_spare_answers_the_opening_cursor_question_once_and_the_tab_is_not_asked_again`,
`a_spare_answers_only_the_opening_question` (the second question answered by the grid too) and
`a_cursor_question_after_output_waits_for_the_tab_that_can_answer_it` (answered at the prompt's end, `1;4`), and the
Mac E2E's helpPanel. Source: Mac E2E in CI and review of the branch (2026-11-09, 2026-11-10); main's rule 231
(2026-11-09).
1121. **macOS: the Mac E2E runs with the runner quiet and its timers on time.** diagLog's "an idle 3 s writes nothing"
failed on the Mac runner only, with `main-lag` lines of 100 to 216 ms and nothing in flight. First the runner's own
`trustd`, `softwareupdated` and Spotlight took about one and a half of its three cores; then, with the runner quiet
and Flare under 1 percent of a core, the 50 ms beat still came late: the kernel coalesces the timers of an app that
is not in front by up to `kern.timer_coalesce_bg_ns_max`, 100 ms, so the beat gap reached 150 ms and more. The same
slack let termReplies' stand-in, which polls its width every 30 ms, miss the 80 ms redraw nudge. A beat posted from a
thread lagged as much, and asking `task_policy_set` for the foreground role was granted and changed nothing; neither
was kept. The `e2e-macos` job turns Spotlight, the update schedule and timer coalescing
(`kern.timer.coalescing_enabled=0`) off before the scenarios, diagLog waits 8 s on a Mac before it measures (1 s
elsewhere), gets three windows of 3 s there with one silent window answering, and every noisy window prints its lines
and the five busiest processes. So on a Mac the check proves one silent window in three, not that every idle window is silent; `docs/parity.md` says so. The app, its 100 ms lag threshold and Windows are unchanged. Open for the owner: a
person's Flare in the background gets the same timer slack, so its diagnostics can hold `main-lag` lines of about
100 ms with nothing in flight. Source: Mac E2E in CI, runs 42633987601, 42635727345, 42639363621, 42640936487 and
42648599333 (2026-11-09).
1122. **A Mac E2E run that runs nothing never passes.** A scenario filter that matches only gated scenarios (`web`,
`dictation`, a typo that hits one gated name) left nothing to run after the macOS gate, and the runner printed "all
scenarios passed" and exited 0. `run_all` now prints "only gated scenarios match" with the reasons and returns 1.
The `e2e-macos` job's hand-run filter (`e2e_filter`) is read into words with `read -ra` and passed quoted, so `*` and
`?` are never globbed against the repo's files, and a word that starts with `-` fails the step (a filter is scenario
names, never a harness flag such as `--exe`). Source: review of the branch (2026-11-09).
1123. **macOS: the web panel's pop-ups, app links and diagnostics notes are the Windows ones (rules 247 to 253), on
CEF.** Notes: the CEF host sends `WebEvent::Note` where WebView2's events would: `on_before_browse` (`web-nav` with the
page it leaves, `web-refused` with `scheme`, `not-its-page`, `not-its-host` or `frame`), a failed main-frame load
(`web-load-failed`, `load_failure_logged`), `on_render_process_terminated` (`web-process render` with its reason and
exit code), the permission handlers (CEF's permission and media bits read as WebView2's kinds, `permission_kinds`),
`on_certificate_error` (`web-cert`, still answered as CEF's default so Chromium shows its own error page) and every
new-window request (`web-popup`); sites only, through `WebNote::new`. Pop-ups: the web panel's `NewWindowRule` (and
each pop-up's, so a pop-up's own requests stack on the same tab) decides with `popup_plan` and `own_blob`; a pop-up is
a second windowless browser CEF itself makes in `on_before_popup` with Flare's client and `windowless_rendering_enabled`
(so `window.opener`, the window's name and a POST are the page's own and no native window exists), counted in the
shared `PopupBook` from the request on and handed over in `on_after_created` (`on_before_popup_aborted` lets it go).
Its view lives in `popup_views`, never in `views`, so no agent call, script, still or navigation of the window reaches
it (250); the panel's picture, pointer, keys, input method, focus and still go to the one on top (`front`), sized as
the page box under Flare's strip; its downloads are cancelled, and it closes on its `window.close()`
(`on_before_close`), its process ending, the strip's Close (`close_popup`) or its panel closing. A `SameView` answer
loads only a web address or a `blob:` (`same_view_target_ok`; the wall then checks the blob is the page's own, its
`NavRule` now given the page it leaves). App links: a main-frame navigation or a new-window request to an address the
panel does not open is offered by `external_scheme` and told once (`AppLinks`, `AppLinkDedupe`) as on Windows;
`shell_open` opens it with `NSWorkspace` and the very app LaunchServices names for its scheme
(`openURLs:withApplicationAtURL:`), refused when that app is a terminal or script runner (`handler_ok`: Terminal takes
`ssh:` and `x-man-page:`), and `url_handler_registered` asks the same question; the words say "this Mac". A pop-up no click asked
for is refused and noted as on Windows, and a page's `window.close()` is ignored and noted (rule 1170); a link to an
app in a frame is asked about through the browser's launch (rule 1171). Different from Windows: Alloy has no password
manager, so a pop-up offers no autofill (`PopupProbe::autofill` false).
Native: `crates/flare-cef/src/macos/view.rs`, `crates/flare-web/src/macos/host.rs`, `crates/flare-win/src/shell_open.rs`,
`crates/flare-win/src/macos/workspace.rs`, `crates/flare/src/app_ask.rs`. Held by the probe's `panel.external-schemes`,
`panel.popup`, `panel.popup-closes-itself`, `panel.popup-strip-close`, `panel.popup-blocked` and `panel.notes`
(`flare-web/tests/cef_macos.rs`), `a_same_view_answer_loads_a_web_address_or_a_blob_and_never_a_script`,
`open_app_uri_refuses_a_web_address_a_file_and_a_path_before_the_system_sees_it`,
`a_scheme_no_app_takes_or_only_a_terminal_takes_has_no_handler` and `a_scheme_that_is_not_one_is_never_looked_up`.
Source: main's #150, #151 and #153 carried to the macOS port (2026-11-10).
1124. **macOS: the agent's arrow points, and "Draw arrow" is not offered yet.** `browser.point` is the Windows one
(rule 254) on CEF: it measures, answers and follows its element, and the look's screenshot carries it, but over the
page it is drawn by the recording stand-in, as the agent's cursor and border are, until the overlay windows are
ported. The person's draw mode (rules 255 to 257) needs a pad that takes the mouse from the page and a read of the
pointer; neither exists on macOS (`real_pointer` is a stand-in that reads nothing), so the window turns the mode off
(`set_draw_offered(false)` in `window.rs`): "Draw arrow" never shows (`push_draw_arrow`), its callback starts nothing
(`draw_possible`), and turning it off ends a mode that was on. Windows keeps it. Held by
`with_the_draw_mode_not_offered_the_button_hides_and_starts_nothing_but_the_agent_still_points`. Source: main's #154
carried to the macOS port (2026-11-10).
1125. **macOS: the web panel's profile is `<profile>/WebView2`, so a login lasts as long as the profile, and nothing
moves.** Main's rule 261 moved Windows's default run out of the install folder; on macOS the panel's CEF root
(`webview_folder(profile_dir)`, the mini-apps' and plugins' `-<kind>` siblings beside it) has always been under the
profile in `~/Library/Application Support/Flare`, which replacing `Flare.app` never touches, so a persistent cookie and
`localStorage` already outlive a quit and an update. `old_webview_folder_here` is None on macOS: the start moves nothing,
`settle_webview_folder` gives the profile's folder, and rule 262's held-folder case cannot arise. Session cookies
match Windows: no CEF context keeps them, the disk profile included (`PERSIST_SESSION_COOKIES` is 0 in
`flare-cef/src/macos/context.rs`, held by `no_profile_keeps_session_cookies_past_the_quit`), so a sign-in without "stay
signed in" ends with the CEF process, at the quit, and does not survive a restart. Still different: CEF lives until the
quit, so such a sign-in survives closing the last panel, where WebView2's may end with its browser; the hidden e2e
`webLogins` is Windows only. Native:
`crates/flare-surfaces/src/web.rs`, `crates/flare/src/window.rs`. Held by
`a_mac_run_moves_nothing_and_keeps_the_panel_under_its_profile` and the POSIX twins of main's folder tests. Source:
main's #158 carried to the macOS port (2026-11-10).
1126. **macOS: a pixel check never reads cell (0, 0) of a fresh shell, and a prompt in `grid.text` is not yet in the
picture.** pwsh on a Mac prints its prompt about a second after the tab opens, and until then the block cursor
(grey on the dark theme) covers cell (0, 0); Windows' ConPTY keeps it hidden there. The shell's bytes reach the emulator on
its own thread, and the software grid paints them on the next frame poll (up to `FRAME_POLL`, 16 ms,
later), so a scenario that waits for the prompt text and snaps at once can still get the empty grid: run 42917628472
(main, 7a9ac8c9) waited for the prompt, did not time out (0.65 s from the window to the snapshot), and its
`theme-dark` shot shows an empty grid with the cursor on (0, 0), read as `[187, 187, 187]` at grid origin + 6 px. The
`theme` check reads the frame inside its padding and the rows in cell (last row, 0), and FAILs with the reason when
that row holds text or the cursor (`ground_spots` in `tools/e2e/src/scenarios/chrome.rs`). Windows reads the same
spots. Held by `the_spots_miss_cell_zero_where_the_cursor_sits`, `the_spots_scale_with_the_window` and
`a_last_row_with_text_or_the_cursor_fails_loudly`. Source: main's first full CI run after the macOS port (2026-11-11).
1135. **macOS: a removed or replaced plugin folder goes to the Trash, and "Reduce motion" stills the agent icons.**
Plugin Remove (and the copy an Update replaces) handed the folder to `flare_mods::install::recycle`, which on a Mac
was a refusal ("not built on this platform yet"), so Remove failed with the folder left in place and an update left
the old copy in `plugins/.staging`. It now moves the folder with `NSFileManager trashItemAtURL:` on the install
worker; Settings says "Remove (to the Trash)" and "Its folder is in the Trash." (`BIN` and `REMOVE_TIP` in
`flare-settings/src/page/mods.rs`). And `flare_win::motion::animations_enabled` answered "animate" on every Mac, so
System Settings > Accessibility > Display > Reduce motion never stilled the `agent-moves` sample's icon motions, the
marks or the slides; it now reads `NSWorkspace accessibilityDisplayShouldReduceMotion` (any thread), as Windows reads
its "Animation effects" switch. The app-level plugin tests stub the bin with a delete, so they never saw the refusal.
Held by `mac_remove_moves_the_plugin_folder_to_the_trash` (`flare-mods/src/install.rs`, the real Trash, taken back
out) and `mac_reduce_motion_decides` (`flare-win/src/motion.rs`). Source: porting the `agent-moves` sample to the
Mac (2026-11-11).
1133. **macOS: a tab that hosted Claude comes back resumed after a quit or restart, as on Windows.** `tabs.json` keeps
only the folder and `"agent": "claude"`; the restore finds the session id in Claude's store
(`~/.claude/projects/<encode_cwd>/*.jsonl`, newest interactive first) and hands it to the spawn. `with_resume`
built the resume only for pwsh, powershell and cmd, so a Mac zsh, bash or fish tab dropped the id and came back
as a plain shell (Max, 2026-11-11: "claude doesnt auto resume on restart either"). Now those shells keep their
plain login command line and get the id in `FLARE_RESUME_ID`; Flare's shell integration takes it out of the
environment before the user's startup files run, and runs `claude --resume <id>` (or `codex resume --last`,
`agy -c`) once those files have set PATH: zsh at the first prompt (`__flare_install` in `flare.zsh`, the id kept
by the `.zshenv` shim), bash at the end of its `--rcfile`, fish in `flare.fish`. The script runs only an id of
hex digits and dashes, quoted. Without the integration on disk a Mac tab still comes back as a plain shell.
`crates/flare-shells/src/resume.rs`, `crates/flare-shells/src/macos/scripts/`. Held by
`a_mac_shell_carries_the_resume_in_its_environment`, `a_restored_zsh_or_bash_tab_resumes_claude_through_the_login_shell`
(a real `/bin/zsh -l -i` and `/bin/bash --rcfile`, the fake `claude` on the PATH only through the user's
startup file), `a_shell_with_no_resume_starts_no_agent` and `a_mac_zsh_resume_carries_the_id_into_the_login_shell`.
Source: Max's restart on the Mac (2026-11-11).
1130. **A fresh profile has vertical tabs; a stored choice is kept.** Max asked for tabs down the side by default
(2026-11-11). Native: `flare.window.tabLayout` falls back to `vertical` (`crates/flare-settings/src/tab_layout.rs`),
the sidebar keeps its own defaults (open, 220 px), and nothing is stored until somebody picks, so a stored or
imported `horizontal` still wins. A test or e2e scenario written for the strip chooses Horizontal itself (the
fixtures, the `--scene` setups, `flare_e2e::world::tabs_along_the_top`) instead of leaning on the default. Held by
`tab_layout_is_vertical_until_chosen`, `a_stored_horizontal_choice_is_kept`,
`a_fresh_profile_starts_vertical_with_the_sidebar_open_at_220` and the e2e `verticalStart`. Source: Max (2026-11-11);
Prism Terminal's default is horizontal (lib/tabLayoutPrefs.ts).
1127. **macOS: attention reads a question line in a picture at full breath, and linkPaintStall never names its file
before the shell makes it.** Run 42953387295 (main, 34a6268a) failed two checks that pass on reruns. attention's "and it
is visible there" (the question line on the tab in front) read 0 px, 0% of the foot: its `question-active` shot shows
alpha in front with no line at its foot while the background shot a moment before showed it faint. The line breathes
from full to nothing every 2 s and that check took one picture; rule 1119 had only sampled the background one. Both
pictures are now `snap_full_breath` on a Mac (the app's own breath for the picture, read on the UI thread, at least
0.85), so the check reads the line, not the phase. linkPaintStall's "the path tab A printed wears the link colour" typed
`cls; Set-Content found-167.txt x; Write-Host 'made found-167.txt here'`: PSReadLine echoes the line before it runs,
the painter asks whether `found-167.txt` exists, and an answer that comes before Set-Content made it is a "no" kept
`NO_TTL_MS` (15 s), so the printed line lit only on the retry after 16 s and the 8 s wait missed it. On a Mac the
command builds the name in the shell (`'found-' + '167.txt'`), so the first question about the file is the printed
line's; a miss names whether the line is on screen, the ink and the links painted. The app is unchanged (the 15 s "no"
is PT's). Windows keeps both old checks (rule 1119). Held by `the_mac_command_names_the_file_only_in_what_it_prints`.
Source: Mac E2E in CI, run 42953387295 (2026-11-11).
1131. **macOS: a text-presentation symbol draws from a text face, and only VS16 makes it a colour emoji.** Claude Code's
message bullet U+23FA showed as Apple Color Emoji's blue tile on the Mac and as a plain circle on Windows (owner,
2026-11-11). cosmic-text's macOS common fallback is `.SF NS`, Menlo, Apple Color Emoji, and of the ~900 faces on
macOS 26 only STIX Two Math has U+23FA outside the emoji faces (Apple Symbols does not), so the emoji face won. The
rasteriser now follows UTS #51 per cluster (`presentation::wants_emoji`): VS15 forces text, VS16 or a keycap forces
emoji, otherwise Emoji_Presentation decides (a range table generated from Unicode 16.0 `emoji-data.txt`). A
text-presentation cluster that landed on an emoji face is drawn from the user's stack, then `TEXT_SYMBOL_FAMILIES`
(Apple Symbols, SF Mono, Menlo, .SF NS, Arial Unicode MS, STIX Two Math), when one of them has every glyph; an emoji
cluster a text face took (Menlo has U+2764, so U+2764 U+FE0F drew as text) comes from `EMOJI_FAMILIES`. STIX Two Math
moved from the lazy list to the end of the start-up fallbacks so it is there to be asked. Both lists are empty on
Windows, whose output does not change. Native: `crates/flare-render/src/presentation.rs`, `raster.rs`,
`macos/font_catalog.rs`. Held by `the_presentation_rule_follows_uts_51` and
`a_text_presentation_symbol_draws_from_a_text_face_and_vs16_makes_it_an_emoji`. Source: macOS port (2026-11-11).
1128. **macOS: a run from an `.app` bundle that Velopack did not make has updates off, so it shows no fake update
chip.** D11 previews in an unpackaged build so `cargo run` shows the update flow, but the ad hoc
`/Applications/Flare.app` that `tools/release/mac/make-app.sh` builds is unpackaged too (no `UpdateMac`, no
`Contents/Resources/sq.version`), so it showed "Update 0.2.0" from the preview feed to a user who had installed it.
`packaged::in_app_bundle` reads the path alone (`<X>.app/Contents/MacOS/<exe>`, no file probes); `Ship::from_parts`
sets `bundled` from it on macOS only (always false on Windows), and `mode_for(packaged, bundled, e2e, flag, live)`
previews only when `--preview-update` is given or the run is unpackaged, not bundled and not e2e. Unchanged: `cargo
run` previews, `--preview-update` previews everywhere, a packaged Mac build goes Live with `live-updates`, an e2e run
(which on a Mac runs from a bundle) stays Off as before, and Windows is as it was. Native:
`crates/flare-win/src/packaged.rs`, `crates/flare-update/src/engine.rs`, `crates/flare/src/ship_ctl.rs`. Held by
`in_app_bundle_reads_the_path_alone`, `mode_table`, `an_unpackaged_mac_bundle_is_off_and_the_rest_keep_d11` and
`an_unpackaged_app_bundle_is_off_on_a_mac_only`. Source: Max, the installed ad hoc app showing an update chip
1132. **macOS: a build an agent stages shows as "Build ready" in the running Flare.app, and only Max's click swaps the
bundle.** Max runs Claude Code inside `/Applications/Flare.app`, so an agent never replaces that bundle; it runs
`tools/release/mac/stage-install.sh`, which builds with `make-app.sh` into a temp folder and writes
`~/Library/Application Support/Flare/staged.tmp/Flare.app`, renames it to `staged/` and writes `staged/staged.json` last
(the Windows manifest; names relative to the bundle, `Contents/MacOS/Flare`). It never touches `/Applications`, and
refuses a root in an `Applications` folder or a bundle. A `.app` in an `Applications` folder is an installed copy whatever
the update mode (`Layout::from_bundle_exe`); an `--e2e` run never uses the real root. The swap journals `install.json`,
renames the running bundle to `Flare.app.old`, renames the stage in (the stage must be on the bundle's volume, else
`NotReady::OtherVolume` and no chip), checks every hash in place and puts everything back on any failure (the new bundle
goes back to `staged/` while it is on offer). The restart is `open -n <bundle> --args ... --after-pid <pid>`; the
start guard, the 20 s confirm (which deletes `Flare.app.old`) and the rollback after two early deaths are Windows'.
`install.lock` is an `flock`. A record naming another bundle is left alone. Native: `crates/flare-update/src/staged.rs`,
`crates/flare/src/appctl/staged_glue.rs`, `tools/release/mac/stage-install.sh`. Held by `staged::mac_tests` (layout,
check, swap, a bad hash, a failed move, a crash at each step, confirm, rollback, take back and abandon, another
bundle's record, the lock, the cross-volume refusal as a pure function), `stage_script::mac` (the real script on a fake
bundle with `FLARE_STAGE_APP` and `FLARE_STAGE_ROOT`) and `ui_tests::staged_flows` (`a_mac_bundle_...`,
`the_mac_restart_...`, `an_installed_mac_bundle_...`). An ad hoc signature changes per build, so macOS may ask for
permissions again after an install. Source: Max, "always stage apps so they show as update here in the ui"
(2026-11-11).
1137. **macOS: the default dictation key Ctrl+Cmd works in Flare's window, and a missing Input Monitoring is asked for
and logged, never silence.** Max switched dictation on (Large v3 Turbo downloaded) and the key did nothing, with not
one dictation line in the diag log (owner, 2026-11-11). Root cause: macOS' winit (`flagsChanged:`) queues a modifier's
`KeyboardInput` BEFORE the new `ModifiersChanged`, so a modifier's press never carried its own flag: Cmd down after
Ctrl carried ctrl only, Ctrl down after Cmd carried meta only, and the chord of modifiers, which wants both flags on its down, never matched. Without Input Monitoring the hook waits
silently (rule 1077) and the window is the only reader, so nothing ever happened. `keymap::with_own_modifier` now sets a
modifier key's own flag on its press and clears it on its release (what Windows' winit already reports), inside
`key_event_from_parts`, and the e2e's `event_from_chord` builds the same event. Second: nothing asked for Input Monitoring, so dictating from another app stayed off for good.
`keyhook::ask_for_anywhere` calls `CGRequestListenEventAccess` once per run (`macos/listen_ask.rs`, on a worker) on the
person's act only: switching "Use dictation" or "Dictate from any app" on, or pressing the key in the window, while the
binding needs Input Monitoring it lacks; the hook still never asks. Third: the diag log now shows it: `dictation-on`
(the key as the Mac names it, the mode, anywhere, the problem, the model) once per change of the preferences,
`dictation-key` (`start`/`stop`/`cancel`, `from: window`) for each action (the hook's key from another app was
already `dictate-anywhere` with `key`), `dictation-ask` when the system was asked,
and `dictation-engine` (`pass` or `warm`, ok, the reason and detail of a failure, a character count and never the
words). The microphone keeps its entitlement and usage sentence; Input Monitoring has neither. Native:
`crates/flare/src/keymap.rs`, `crates/flare/src/dictation_ctl.rs`, `crates/flare-dictation/src/keyhook.rs`,
`crates/flare-dictation/src/macos/listen_ask.rs`. Held by
`a_modifier_key_carries_its_own_flag_so_ctrl_cmd_dictates_with_the_macs_late_modifier_state`,
`a_modifier_key_alone_is_a_key_event_so_a_held_ctrl_can_be_let_go`,
`the_window_key_logs_its_start_asks_once_for_anywhere_and_the_engine_answer_is_logged`,
`the_mac_key_hook_listens_only_posts_nothing_and_never_asks_by_itself` and
`dictation_has_the_microphone_entitlement_with_its_usage_and_the_key_needs_no_entitlement`. Source: Max, dictation on
and the hotkey doing nothing (2026-11-11).
1138. **macOS: Flare.app carries its speech engine in `Contents/Frameworks/whisper`, signed with the app, and a missing
engine is named in the diag log.** Max's mic Test got audio, but nothing was ever transcribed: every warm-up and pass
logged `dictation-engine` with `reason: NoEngine` (owner, 2026-11-11). Root cause: phase 8 never put the engine of rule
1084 into the bundle, and the app looked for it in `Contents/MacOS/whisper`, where a signed bundle cannot hold a
folder with data in it. Now `make-app.sh` builds the pinned Metal engine (`tools/build-whisper-macos.sh`, cmake) and
`flare-parakeet-host`; `release::engine_code` copies only the Mach-O files of the stamped folder, each matching its
`.source` SHA-256, and refuses a file the stamp does not list; `bundle::build` puts them in `Contents/Frameworks/whisper`
(code only, so `codesign --deep --strict` checks each as code), the MIT notice in `Contents/Resources`, the host in
`Contents/MacOS` (a tool). `sign_bundle` signs each engine file as a library before the app: the hardened runtime and
the team's identity, no entitlements (the libraries are the team's, so library validation passes and no
`disable-library-validation` is needed; Metal's shaders are embedded, `GGML_METAL_EMBED_LIBRARY`). The app finds the
folder with `engine::bundled_engine_dir` (a dev build outside a bundle looks beside itself, as Windows does), counts it
only when its whisper-server is there (`engine_in`), and a `NoEngine` answer's detail names the folder looked in
(`EngineConfig::bundled_at`). `pack.sh` checks the engine and the host inside the packages. Also: the Parakeet host
ends with `_exit` on a Mac, because ggml-metal's static device list asserts at exit (b5130,
`ggml-metal-device.m:1021`) and every clean `quit` died of SIGABRT. Held by
`the_engine_is_copied_as_code_only_and_only_when_every_file_matches_its_stamp`,
`the_engine_names_are_the_ones_the_app_starts`, `tests/bundled_engine.rs` (the layout, a Metal start with language
`auto`, `NoEngine` naming the folder), `the_pack_checks_each_programs_role_ad_hoc_too` and the ignored
`the_engine_inside_a_built_app_transcribes_with_language_auto` and `the_real_host_loads_the_mac_libraries_and_answers_a_pass`
in `tests/metal_real.rs`. Source: Max, the mic test hears him and dictation transcribes nothing (2026-11-11).
1139. **macOS: dictating from another app ends in the session picker, a floating panel that never activates Flare,
and the picked session gets the words in the background.** Max held Ctrl+Cmd in another app, spoke and let go: the
words were transcribed (`dictation-engine` pass ok, 604 characters) and then nothing happened, no paste and no window
(owner, 2026-11-11: "it can't paste. It doesn't recognize that there's a window in front and that it needs to bring up
the secondary window"). Root cause: the route was right (the hook named the other app's process, the service asked for
the tabs and opened the picker: the next press logged `pick: not sent`), but the widget window's macOS half was a stub
(`dictation_widget::win`, "phase 7 of the port plan"): the window never got a handle (`hwnd_of` read only Win32), was
never placed, shown, styled or made to catch clicks, so it stayed parked at -32000 and the picker waited unseen for
30 s. Now `flare_win::float_panel` is the Mac half: the same Slint card in the same winit window, at the status-window
level on every Space and over full-screen apps, shown with `orderFrontRegardless` (never made key), AppKit's
non-activating-panel switch so a click on a row reaches the card while the other app stays in front (should a system
lack it and the click activate Flare, the app that was in front gets the focus back), clicks passing through except
while picking, placed in points at the bottom centre of the visible frame of the screen under the pointer. The digits
1 to 9 and Esc pick and cancel through the hook's Carbon hot keys (no focus needed, as on Windows); a click in another
app cancels the picker like Esc (a global mouse monitor, no permission) and the words are kept on the clipboard, as on
Esc and the 30 s timeout. The pick types into that tab's shell as one bracketed paste and the Enter 300 ms later,
through the checked send, without bringing Flare forward. The diag log (`dictate-anywhere`) now says the route
(`front tab`, `other copy`, `picker`) with a character count, the picker shown (rows), the row picked (its number and
tab id), the send (sent or refused, a count) and a cancel (`esc`, `click away`, `timed out`), never the words. A
process-check refusal names macOS, not Windows. Native: `crates/flare-win/src/macos/float_panel.rs`,
`crates/flare/src/dictation_widget.rs`, `crates/flare-dictation/src/service.rs`, `crates/flare/src/dictation_ctl.rs`.
Held by `the_route_is_named_by_the_app_in_front_at_the_press`,
`flare_in_front_pastes_and_another_app_in_front_opens_the_picker`,
`the_picker_logs_shown_picked_and_sent_with_counts_never_the_words`,
`a_click_in_another_app_cancels_the_picker_like_esc_and_keeps_the_words` and `esc_and_the_timeout_are_logged_as_cancels`
(service), `heard_with_another_app_in_front_the_picked_session_gets_the_words_in_its_pty` and
`a_click_away_from_the_picker_types_nothing_and_keeps_the_words` (dictation_ctl),
`the_panel_floats_never_activates_and_catches_clicks_only_while_picking`,
`hiding_while_picking_ends_the_click_away_watch` and `a_row_click_leaves_the_app_in_front_or_gives_it_the_focus_back`
(ui_tests/dictation_widget.rs, a stand-in window), and
`dictation_panel_floats_never_activates_and_is_placed_in_top_left_points` (crates/flare-win/tests/appkit.rs, a window
never shown). Source: Max, dictating from another app transcribes but never pastes or shows the session list
(2026-11-11).
1140. **Settings > Agents draws the Mark colours section (Agent working, finished and question colour, with its
Save changes) only while the tab marks are Bar; with Ring it is left out, and Find does not offer its three rows.**
The Ring mark (`MarkRing` in `crates/flare/ui/tabmark.slint`) is mono and draws with the tab name's ink, so the
three colours changed nothing a Ring user could see on the tab. `page::agents::build` reads
`env.look.agent_indicator()` as the indicator row does (`Minimal` is Bar, everything else Ring) through
`mark_colours_drawn`; the page rebuilds on every look change, so the segmented control hides and re-shows the
section live. `IndexEnv::mark_colours` drops the `colours` rows from the find index under Ring, and
`SettingsCtl::sync_index` rebuilds the index (and the hits of a query in the field) when the indicator changes, so
Find never leads to a hidden section. The one Ring surface that still wears the colours is the dots on the +N button
of an overflowing strip (`tabs_ui` `Dot.colour` is the row's `mark_color`); a pick made under Bar stays in force
there, and with no pick they follow the theme. Same on Windows and the Mac. Native:
`crates/flare-settings/src/page/agents.rs`, `find.rs`, `find_state.rs`, `crates/flare/src/settings_ctl.rs`. Held by
`ring_draws_no_mark_colours_section`, `bar_draws_the_mark_colours_with_their_save`,
`switching_the_indicator_hides_and_reshows_the_mark_colours`, `the_mark_colour_rows_are_indexed_only_for_bar`,
`a_new_index_redoes_the_hits_of_the_query_in_the_field` and
`the_mark_colours_show_and_are_found_only_with_bar_and_follow_the_switch_live`. Source: Max, "these should be hidden
if you use the ring style" (2026-11-11).
1136. **macOS: a scenario types into a stand-in agent only after its pwsh has run a line, and a Ctrl+Tab goes out only
after the last one landed.** Of the Mac E2E failures on main and PR runs from 2026-11-10 to 2026-11-11 (ci runs
42917628472 to 42999224318), the most common was colourPicker's trio "a working stand-in agent is on the strip",
"the working line is opaque" and "the red line is on screen" (3 runs: 42970917834 on main c789a5bb, 42973156632,
42983045830). The scenario typed the hook line as soon as the nested pwsh of the stand-in (`/bin/zsh <dir>/claude`,
rule 1109) showed its first prompt; on the runner that line can be lost, or kept without its Enter, so the hook never
ran and the red line typed later ran glued to it. markStrip's "six tabs in the mockups' states" and the reduced-motion
"the run is a full line and the question fully on" (42936130845) are the same lost hook line. `agent_proc::wait_ready`
now types `Write-Host ('stand-in' + '-ready-' + 'n')` and waits for the printed answer on a row of its own, up to
four tries of 4 s, each after a bare Enter so a half-taken line runs first; colourPicker, markStrip, indicators,
agentHooks and agent_misc call it after starting the stand-in. themeEditorLive's "an Appearance picker left open is
open again on return" (42932844893, 42999224318) sent the second Ctrl+Tab as soon as the picker shut, before the
shell was in front; a Mac now waits for Settings to leave the front and to come back as well. Windows is unchanged:
`wait_ready` returns at once and the Ctrl+Tab waits are as before (rule 1119). The other Mac misses of that window are
held elsewhere: the frame's `[187, 187, 187]` by rule 1126, the theme's `0.0:1` by #162, attention and linkPaintStall
by #163. Held by `the_ready_answer_is_the_printed_row_and_not_the_typed_line`. Source: Mac E2E in CI (2026-11-11).
1129. **macOS: the terminal scrolls like Terminal.app, line for line with the fingers and no momentum after they
lift.** Max (2026-11-11): "the scrolling in this app has inertia, I'd like it to be like it is in the normal terminal
app where it scrolls line for line with no momentum". AppKit sends a trackpad scroll in phases (`NSEvent.phase` Began,
Changed, Ended while the fingers are down) and then synthetic momentum events (`momentumPhase` not None, `phase` None);
winit 0.30 folds both into one `TouchPhase`, so a momentum Changed reads as a finger Moved. The window's wheel arm reads
the phases from `NSApp.currentEvent` (`flare_win::scroll_phase::current`, winit calls the handler from the view's
`scrollWheel:`; winit's phase stands in when the current event is not the scroll). `WheelAccum::add_mac` then drops
every momentum event, builds a drag's precise pixels into whole lines (one per cell height, `TermHandle::wheel_lines`:
one line of view, one alternate-screen arrow or one mouse report a line, so vim and less get no momentum either), and
resets the remainder when a gesture begins. A plain mouse wheel (both phases None) keeps its notches, three lines each;
Cmd+wheel zoom keeps its notches too. Only the grid's path changes: a web panel (CEF) scrolls itself, with its momentum.
Windows never reads the phase (`phase_from_winit` is `Wheel` off macOS). No setting. Native: `crates/flare/src/live.rs`,
`crates/flare/src/window.rs`, `crates/flare-win/src/scroll_phase.rs`, `crates/flare-term/src/wheel.rs`. Held by
`mac_scroll_moves_with_the_fingers_and_never_with_the_momentum`, `momentum_wins_and_each_finger_phase_is_read`,
`a_line_step_is_one_line_everywhere_and_a_notch_stays_three`,
`winits_phase_is_a_plain_wheel_off_the_mac_and_for_a_mouse` and `a_wheel_that_moves_the_view_asks_for_a_redraw`.
Source: Max on his Mac (2026-11-11).
1141. **The side-by-side run reads Prism Terminal's private installer with a read-only token, given to its step alone, and fails without it.** Prism Terminal moved to `arkivert/PrismTerminal` and became private (2026-11-11), so `gh release download` with the workflow's `GITHUB_TOKEN` answered "release not found". The side-by-side step of `release.yml` and `installer-e2e.yml` sets `GH_TOKEN` from the repository secret `FLARE_RELEASE_TOKEN` (a fine-grained token, Contents read on that one repository); no other step reads a secret. When the secret is missing the step prints one `::error::` line naming the secret, the repository and `docs/shipping.md`, and exits 1 before the test runs: never a skip, and the token is never printed. Native: the two workflow steps, `NEIGHBOUR_RELEASE_REPO` in `tools/e2e/tests/side_by_side.rs`. Held by `only_the_side_by_side_step_gets_the_neighbour_token_and_it_fails_without_it`, `no_secret_other_than_the_github_token_and_the_neighbour_release_token` and `the_release_goes_to_this_repository_only` (`crates/flare/tests/release_files.rs`), and the name scan's third widening (`the_widenings_are_narrow`, `crates/flare/tests/names.rs`). Source: owner decision 2026-11-11; `docs/shipping.md`. Retired 2026-11-11: the owner removed the side-by-side check.
1149. **Dictation from another app says "Sent" only when the words were sent, its key never gets out of step, and the
Flare window in front at the press is found.** Owner report (Flare Stable, toggle mode, Ctrl+Win, 2026-11-11): every
stop asked which session with Flare's tab in front; a card said "Sent to web-application-project" and the words never
arrived; a press after a recording did nothing and the next started a recording nobody wanted. (1) The front: the
chord's FIRST key-down names the window in front; a later down of the same chord (Win after Ctrl, when the shell's own
windows may be in front or none is) only fills in a front the first could not name (`Service::on_hook`). The hook reads
the foreground window, or with none the foreground thread's active window, and resolves it to its root owner
(`keyhook::front_of`). With this copy in front and no tab noted at the press (the window was busy), the paste asks the
window once more on the next turn before it goes to the list (`Dict::paste_page`). (2) The send: `type_checked` reads
Claude's prompt box (`question::prompt_draft`, `None` when no box is on screen). With a box, it waits until the words
show there (300 ms + 1 ms a character, at most 3 s), writes the Enter no sooner than 300 ms after them, and waits up to
2 s for the box to empty: emptied is `Ok` ("Sent to X"), still full is `SendDecision::Typed` (a warning, "Typed into
X, press Enter there", nothing copied), words never shown is `No` ("Not sent: the words did not show in X, copied"). An
Enter held back because the tab changed is now `No` too, never "Sent". With no box to read, the Enter follows after
300 ms as before. On the link `Typed` is `ok: true` plus `entered: false`, so an older copy still reads a send. (3) The
key: a start refused as busy and a recording the cap (or anything but the key) ended set the key feed back to idle
(`KeyFeed::to_idle`, the keys held stay held so the chord's key-up is still the binding's). (4) Crumbs, never the
words: the front at a start, the route taken with the text's length, the picker shown and how a row was picked, every
send stage and its result, the paste's outcome, a refused start, the cap (`dictation phase cap`), and a dropped PTY
write (`pty-write-dropped`, at most one a second). Native: `crates/flare-dictation/src/{route,service,keyfeed,keyhook,
link,controller}.rs`, `crates/flare/src/dictation_ctl.rs`, `crates/flare-agent/src/question.rs`,
`crates/flare-term/src/session.rs`. Held by `type_checked_says_sent_only_once_the_words_showed_and_the_enter_emptied_the_prompt`,
`type_checked_presses_no_enter_when_the_words_never_show_in_the_prompt`,
`type_checked_waits_longer_for_a_long_paste_but_never_past_three_seconds`,
`type_checked_says_typed_not_sent_when_the_enter_leaves_the_words_in_the_prompt`,
`a_send_typed_but_not_entered_stays_a_send_to_an_older_copy_and_an_older_answer_is_a_send`,
`the_chords_first_key_down_names_the_window_in_front_not_the_win_key_down`,
`the_front_names_the_app_that_owns_the_window_even_with_no_foreground_window`,
`a_route_to_this_copy_while_the_window_is_busy_pastes_into_the_front_tab_on_the_next_turn`,
`a_start_refused_while_transcribing_leaves_the_key_ready_to_start_again`,
`a_recording_the_cap_stopped_leaves_the_key_ready_to_start_again`,
`to_idle_makes_the_next_tap_a_start_and_still_swallows_the_held_chords_key_up`,
`the_route_and_pick_crumbs_name_the_way_and_lengths_never_the_words` and `the_kill_hook_writes_a_crumb`. Source:
owner report and the Stable diag log (2026-11-11).
1150. **A dictation long enough to push Claude's prompt box past the rows read is still sent.** The box is read in the
last 16 screen rows (`SCREEN_TAIL_ROWS`). A long one-line text pasted inline wraps over many rows, so in a narrow pane
the box's top rule leaves those rows and `question::prompt_draft` reads `None`. A box that read empty before the paste
and reads `None` after it grew with the words: `type_checked` counts them as shown (stage "words shown, box past the
screen"), still writes the Enter no sooner than 300 ms after them, and watches the box empty as before. Only a box that
stays empty for the whole bound is "the words did not show". Native: `crates/flare-dictation/src/route.rs`. Held by
`type_checked_presses_enter_when_a_long_paste_grows_the_box_past_the_rows_read`. Source: review of rule 1142
(2026-11-11).
1142. **The update e2e installs 0.0.1 from the setup kept aside after the 0.0.1 pack, never from the `Releases`
folder.** vpk writes `Flare-win-Setup.exe` for the version it packs, so the 0.0.2 pack overwrites the 0.0.1 setup and
the one left in `Releases` installs 0.0.2: every install scenario of `update_apply` then failed its first check
(`installed_version == 0.0.1`, left `0.0.2`), on every update-e2e run since the job was added (run 39113290734,
2026-10-18, to run 42986938206). `.github/workflows/update-e2e.yml` copies the setup out right after the 0.0.1 pack
and hands it over as `FLARE_SETUP_EXE`; `install` in `tools/e2e/tests/update_apply.rs` runs only that one. Held by
`the_scenarios_install_the_0_0_1_setup_kept_aside_before_the_0_0_2_pack`. Source: the update-e2e logs of #163 and
#166 (2026-11-11).
1143. **Under `--e2e` the update controller writes `update-calls.json` before `update-state.json`.** The tests wait on
the state and then read the calls. With the state written first, a reader could see the new state beside the calls
of the push before it: `a_real_shaped_offer_is_refused_before_any_request` saw `idle` and the refusal notice with
`installs 0` (the calls written when the download began), check job of run 42986938340 (2026-11-11). Both files
of a push come from one snapshot, so the calls written first are never older than the state a test sees
(`e2e_writes` in `crates/flare/src/update_ctl.rs`). Held by `the_e2e_calls_file_is_written_before_the_state_file`.
Source: the check job of #167 (2026-11-11).
1144. **An update offer is measured against the version Velopack installed, not the build's own version.** The engine
drops a find that is not newer than its current version. It was handed `CARGO_PKG_VERSION` (the workspace's 0.1.0),
while Velopack compares with its manifest (`sq.version`). update-e2e packs one exe as 0.0.1 and 0.0.2: Velopack found
0.0.2 over the installed 0.0.1, and the engine then dropped it as older than 0.1.0, so no chip ever showed
(`no real 0.0.2 offer`, job 130475061549, 2026-11-11, and every run since 2026-10-18). `choose_feed` now records
`VelopackFeed::installed_version` in `FeedChoice::installed`, and `window.rs` builds the engine and the update window
from `FeedChoice::current_version` (the installed version, else the build's). A release packs the Cargo version, so
a shipped app sees no change. The engine logs a find that is not newer, and `update_apply` prints the calls and the
app's update crumbs when no offer comes. Native: `crates/flare/src/ship_ctl.rs`, `crates/flare/src/window.rs`,
`crates/flare-update/src/velopack_feed.rs`, `crates/flare-update/src/engine.rs`. Held by
`an_offer_is_measured_against_the_installed_version_not_the_builds` and
`a_missing_offer_reports_the_calls_and_the_apps_update_crumbs`. Source: the update-e2e log of #173 (2026-11-11).
1147. **macOS: the path menu's reveal row says "Show in Finder" and shows the file selected in Finder.** A right-click on a printed path offered "Show in Explorer" on the Mac too. The label is one constant, `REVEAL_LABEL` in `crates/flare-win/src/openers.rs`: "Show in Finder" on macOS, "Show in Explorer" elsewhere, and the menu, its tests and the markerLinks E2E all read it, so no test names one OS's label. The row's action is unchanged: `OpenMode::Reveal` goes to `explorer_reveal`, which on the Mac is `NSWorkspace activateFileViewerSelectingURLs` (`crates/flare-win/src/macos/workspace.rs`), never `open` and never a run. Native: `crates/flare/src/term_menu.rs`. Held by `the_reveal_row_names_the_os_file_manager`, `the_menu_fits_what_was_clicked` and `open_path_rows_carry_the_path_and_the_mode` (`crates/flare/src/term_menu.rs`), `the_menu_fits_what_was_clicked_and_copies_exactly` (`crates/flare/src/ui_tests/term.rs`), and the markerLinks E2E (`tools/e2e/src/scenarios/input_b.rs`). Source: Max on his Mac (2026-11-11).
1148. **A unit test that writes a Windows path for a file link writes it through `path_grammar::native_text`.** A
`C:\...` path is relative on macOS (rule 227), so a marked `C:\...` with no tab folder is rightly no link there, and the
six marked-path tests in `link_scan` and `emulator` found none on the Mac runner (run 42983045830, PR 166). The helper
keeps the text on Windows and writes `/C/` for `C:\` and `/` for every `\` elsewhere, the same length, so piece
widths hold on both. Held by `native_test_text_is_an_absolute_path_of_the_same_length` and the
`a_marked_path_*` tests. Source: PR 166's macos check (2026-11-11).
1154. **One zoom for every tab.** Ctrl+wheel (Cmd+wheel on the Mac) and the Mac zoom keys move one app-wide zoom,
a pixel offset on the settings' size, held in `FontCtl` (`crates/flare/src/font_ctl.rs`) for the run and never saved
(no tabs.json field, no pref). A zoom in any tab sizes every tab: `AppCtl::apply_font` (`crates/flare/src/appctl.rs`)
refits every open tab through `TabHost::refit_all`, hidden ones included, so their PTYs are resized and the shells
reflow; a new tab spawns at the shared size, and Cmd+0 resets the zoom for all. A settings size change keeps the zoom
on top of the new base. The browser panel's page zoom is separate. Windows and the Mac share the code. Replaces rule
221; rule 26 keeps the range and the never-saved part. Held by
`a_zoom_in_one_tab_sizes_every_tab_and_a_new_tab_opens_at_it`,
`a_zoom_resizes_every_shell_hidden_ones_included_and_a_reset_brings_all_back`,
`on_the_mac_cmd_zoom_keys_size_every_tab_and_cmd_0_resets_them` and `the_zoom_is_one_offset_on_the_settings_size`.
Source: Max (2026-11-11): "Flare sessions have independent zoom levels when I Cmd+scroll. Make it so they all share
the zoom level when I zoom in or out dynamically."
1153. **Under `--e2e` the window's motion follows `FLARE_E2E_REDUCED_MOTION` alone, never the OS switch.** Since rule
1135 a Mac reads System Settings > Accessibility > Display > Reduce motion, and GitHub's macOS runner has it on, so
every Mac E2E run saw a still window: attention's question line did not breathe and its finished line did not flow,
indicatorStyles' rainbow stood still and minimising had no moving mark to pause, Settings search flashed no found row
(the flash is skipped under reduced motion), and verticalTabs' sidebar snapped open with the terminal already narrow
(880 vs 1100 px) instead of sliding (run 43054686157 on main 794a9a28). `e2e::animations_enabled` now answers, under
`--e2e`, "animate" unless the seam is `1`, and asks the OS only without `--e2e`, so the scenarios run the same on any
runner and a scenario that wants still still says so. A real user's Reduce motion and Animation effects are honoured
as before. Native: `crates/flare/src/e2e.rs`. Held by `under_e2e_motion_follows_the_seam_and_never_the_os` and the
attention, indicatorStyles, settingsSearch and verticalTabs scenarios on the Mac. Source: Mac CI on main (2026-11-11).
1152. **macOS: the web panel's host asks for its own turn whenever CEF's handlers leave it something, and keeps
turning while a view waits for CEF.** Max's web panel stayed dark (account.apple.com, then example.com: "did not finish
loading", no title, `browser_look` "not ready"), while the engine tests all passed. The window pumps the host
(`WebBackend::pump`) only on `WebEvent::Pump`, which came only when CEF asked for its own work
(`on_schedule_message_pump_work`); in the real window CEF's main-thread tasks run in AppKit's run loop, so CEF asks
rarely, and what its handlers set (a profile initialized, a view's setup answered, a load, a paint) sat until a turn
that never came: no browser made, no navigation, nothing painted. Always so after the last panel closed: `web_ctl`
drops the host (`drop_idle_backend`) and the next panel makes a new one in the CEF the first started, whose turns CEF
never asks for (Max's Flare had two `flare-cef-pump` threads; its profile had no `Cookies` and no cache: no request
was ever made). Signing, entitlements and the keychain were ruled out: the same probe loads and paints ad hoc, signed
with the Developer ID, and as `com.flare.terminal` with Flare's own keychain item. Now every view's and pop-up's
event and the profile's `on_request_context_initialized` wake the host through its scheduler (`WebHost::wake`, one
pump at a time), a setup answer is a `ViewEvent::SetupAnswered`, and while a view waits for its profile or its setup
the host turns again every 20 ms (`retry_while_waiting`). The engine tests pumped on every turn and hid it: the
`app-loop` probe scenario turns the window's way (`run_loop_turn`, pumps only when told) and reopens in a new host.
Native: `crates/flare-web/src/macos/host.rs`, `crates/flare-cef/src/macos/view.rs`,
`crates/flare-cef/src/macos/context.rs`, `crates/flare-cef/src/macos/engine.rs`. Held by
`a_web_panel_turned_as_the_window_turns_loads_and_paints_and_again_after_the_last_closed`
(`crates/flare-web/tests/cef_macos.rs`; without the fix its reopen check fails, nothing loads). Source: Max on his Mac
("the flare browser is just dark", 2026-11-11).
1183. **No CI step downloads Prism Terminal or needs a token for it.** Owner decision 2026-11-11 (Prism Terminal is deprecated): the side-by-side check that installed Flare beside Prism Terminal on the runner is removed, with its test (`tools/e2e/tests/side_by_side.rs`), its workflow (`installer-e2e.yml`, which ran only that test) and its step in `release.yml`. No workflow names `arkivert/PrismTerminal`, `FLARE_RELEASE_TOKEN` or `PRISM_RELEASE_TOKEN`, and `release.yml` reads no repository secret, so a release never waits on a token. Native: `.github/workflows/release.yml`. Held by `no_workflow_downloads_the_reference_app_or_needs_a_token_for_it`, `no_secret_other_than_the_github_token` and `the_release_goes_to_this_repository_only` (`crates/flare/tests/release_files.rs`). Source: owner decision 2026-11-11; `docs/shipping.md`.
1155. **macOS: the agent's cursor, the glow round the page and the agent's arrow show over the web panel, and never take
a click or the keyboard.** Max on his Mac: "it works but its missing teh overlay effect when claude is using it and its
missing the visible cursor when claude is moving aorund on the page". On Windows each picture is a layered popup owned
by Flare's window (`CursorWindow`, `GlowWindow`, `ArrowWindow`); on the Mac `window.rs` never set a real shell, so the
recording stand-ins `WebCtl` starts with drew nothing, and the tag's name had no ink (no GDI). Now the Mac gives
`WebCtl` its own shells (`flare_win::page_overlay`): each picture is an `OverlayView` (a view class of its own) in
the window's content view, raised over every other subview on each frame, its layer's contents the same premultiplied
BGRA bitmap Windows draws (`compose_cursor`, `edge_glow`, `compose_arrow`, at the window's backing scale), placed in
points from the device pixels `WebCtl` gives (the content view's top left is the origin, the Mac has no HWND), faded
by its alpha value and hidden when `WebCtl` hides it, so the glide, the ripple, the hold, the fade, Reduce motion and
the band layout are the shared code's. The view answers nil to `hitTest:` and refuses first responder (both
`acceptsFirstResponder` and `becomeFirstResponder`), so every click, move and scroll goes to the window's own view and
the person can take over at any time. The tag's text is drawn by AppKit (the system font, semibold). The pad the person
draws an arrow on stays off on the Mac (rule 1124). Not under `--e2e`, where the recording stand-ins stay. Native:
`crates/flare-win/src/macos/page_overlay.rs`, `crates/flare/src/window.rs`. Held by
`agent_overlay_is_click_through_on_top_and_follows_the_view` (`crates/flare-win/tests/appkit.rs`, a window never
shown), the `page_overlay` unit tests (device pixels to points, flipped or not; the glow's bands tiling the page's
rectangle; an empty picture or no view refused; the tag's ink) and `the_tag_is_an_orange_pill_with_the_name_readable_on_it`
(now also on macOS). Source: Max on his Mac (2026-11-11).
1160. **macOS: the page drawn in the web panel has its own keys; only the app's chords and the second Escape come back
to Flare, and Cmd+Shift+O and a click move the keys.** On Windows WebView2's own window takes every key and its
accelerator hook hands the app's chords back (`app_takes`); on the Mac the page is a picture in Slint's `page-keys`
scope, and `AppCtl::on_key` sent every key that was no chord to the shell (it read only `panel-has-focus`, the zero-size
`keys` scope). Now, while the page's scope has Slint's focus and the browser has the keyboard (`page_has_keys`:
`panel-web-focused` and `WebCtl::has_focus`), `on_key` returns every key to Slint before the chord table, so the page's
filter (`key_filter`: `EscTwice`, a held Escape's autorepeats ignored, `app_takes`) decides as on Windows; a key the
filter hands back runs through `on_key_from_page`, never the page again. Key ups follow (`window::handle_key`). The
chord moves Slint's focus too (`invoke_focus_web_page`), a press with the pointer over the page keeps the keys there
(no blur and focus again), a press anywhere else takes them back. Slint reports Cmd as `control` on a Mac (rule 1110),
so `web_live::mods_of` reads bit 2 as Cmd there (keys, pointer and wheel). The view's punctuation keys carry their
virtual key (`key_of`: Cmd+= and Cmd+Shift+[ reach the chord table). Windows is unchanged (`panel-web-live` is false
there). Native: `crates/flare/src/appctl.rs`, `crates/flare/src/web_live.rs`, `crates/flare/src/window.rs`,
`crates/flare/ui/panel.slint`. Held by `page_keys_tests` (`a_page_with_the_keys_gets_every_key_and_its_filter_hands_the_apps_chords_back`,
`the_second_escape_within_600_ms_gives_the_keys_back_and_a_held_escape_counts_once`,
`the_focus_chord_moves_the_keys_both_ways_and_a_press_off_the_page_takes_them_back`) and `panel.key-filter`. Source: the
browser parity audit (KEY-01, KEY-02, KEY-03, 2026-11-11).
1161. **macOS: Cmd+[ and Cmd+] go back and forward in the web panel, Cmd+Left and Cmd+Right too outside an editable
field, and a mouse's back and forward buttons step through the history, never a left click.** WebView2 keeps its
browser keys (Alt+Left, the XButtons) for a panel; windowless CEF has none. The view's `KeyboardHandler` runs, for a
key down the page left alone (`on_key_event`: not cancelled, not used by the renderer) and only for the person's own
Cmd keys (`View::key` records them; an agent's DevTools key never counts), Chrome's Mac meaning (`status::mac_page_key`;
`focus_on_editable_field` keeps Cmd+Left for the caret). The panel encodes back and forward as buttons 3 and 4
(`PointerEventButton.back`, `.forward`) and `page_pointer` sends them to `nav` on the press when the page can go there;
any other button is dropped. Native: `crates/flare-cef/src/macos/view.rs`, `crates/flare-cef/src/status.rs`,
`crates/flare/src/web_live.rs`. Held by `panel.history-keys` (`crates/flare-web/tests/cef_macos.rs`),
`a_persons_cmd_chord_the_page_left_is_the_frames_edit_command_or_a_step_through_the_history` and
`the_back_and_forward_buttons_step_through_the_history_and_are_never_a_left_click`. Source: the browser parity audit
(NAV-06, 2026-11-11).
1162. **macOS: Cmd+C, X, V, A, Z and Shift+Z are the frame's edit commands in the web panel, a link's address under
the pointer shows at the page's bottom left, and the input method composes in the page.** The same `on_key_event`
runs the focused frame's `copy`, `cut`, `paste`, `select_all`, `undo` and `redo` (Cmd+Y is no redo on a Mac), so a page
that cancels the key keeps it. `DisplayHandler::on_status_message` becomes `WebEvent::Status`, drawn by the panel as a
pill at the page's bottom left, for the plain web panel only (WebView2's status bar is off for a mini-app and a plugin,
seal.rs). winit's `Ime::Preedit` and `Ime::Commit` go to the page while it has the keys (`page_ime`, the caret in UTF-16
units), never to the shell. Left: the candidate window's place at the caret, drag and drop into the page, spellcheck
(it needs a context menu of Flare's). Native: `crates/flare-cef/src/macos/view.rs`, `crates/flare-web/src/macos/host.rs`,
`crates/flare/src/web_live.rs`, `crates/flare/src/window.rs`. Held by `panel.edit-keys` and `panel.status`
(`cef_macos.rs`; the probe never touches the pasteboard), and
`the_pages_status_text_shows_under_it_and_the_input_method_goes_to_it`. Source: the browser parity audit (NAT-09,
2026-11-11).
1163. **macOS: Cmd+F while the web panel has the keys opens Flare's find bar over the page, which drives CEF's find.**
WebView2 has its own find bar, so on Windows Ctrl+F stays the page's (`app_takes`); windowless CEF has none, so on a
Mac `app_takes` takes Cmd+F and the window opens the terminal's `FindBar` (placeholder "Find in page") at the page's
top right. Its text starts a search (`WebBackend::find`), Enter and Shift+Enter step (`again`), the readout is the
terminal's ("3 of 12", "none"; empty until counted) from `FindHandler::on_find_result` (`WebEvent::Found`). CEF selects
none of a new search's matches, so one step selects the first, as typing does in Chrome. Escape or Close stops the find
and gives the page the keys back; the bar closes when its page goes elsewhere or another tab comes to the front. Native:
`crates/flare/src/web_live.rs`, `crates/flare/ui/panel.slint`, `crates/flare/ui/find.slint`,
`crates/flare-cef/src/macos/view.rs`. Held by `panel.find`, `cmd_f_in_the_page_opens_flares_find_bar_which_drives_the_pages_find`,
`the_find_bar_closes_when_its_page_goes_elsewhere` and `on_windows_ctrl_f_stays_the_pages_and_on_a_mac_cmd_f_is_flares`.
Source: the browser parity audit (NAT-02, 2026-11-11).
1164. **macOS: Cmd+plus, Cmd+minus and Cmd+0 zoom the page when the web panel has the keys or the pointer, and
Cmd+wheel over the page zooms it; the agent's clicks land under the zoom.** WebView2 zooms its page with Ctrl+plus and
Ctrl+wheel. `zoom_front_step` (the chord and the View menu) zooms the page in Chromium's steps (`ZOOM_STEPS`, 25 % to
500 %, one per tab, Cmd+0 back to 100 %) instead of the terminal's shared size (#190) while `page_has_keys` or
`panel-web-hovered` holds, and the page keeps the keys through it. `page_wheel` turns a Cmd wheel into whole steps and
never scrolls with it. The agent's click at an element's CSS centre lands on it at 150 % (Chromium maps DevTools
coordinates through the zoom). Native: `crates/flare/src/appctl.rs`, `crates/flare/src/web_live.rs`,
`crates/flare/src/appctl/web_glue.rs`. Held by `the_zoom_keys_zoom_the_page_that_has_the_keys_and_keep_them_there`,
`the_zoom_steps_are_chromiums_and_stop_at_the_ends` and `panel.zoomed-agent-click`. Source: the browser parity audit
(NAT-05, 2026-11-11).
1165. **macOS: the web panel's script dialogs and leave-page question wait for the person, on a sheet of the window.**
Windows shows WebView2's own `alert`, `confirm`, `prompt` and `beforeunload` dialogs for the plain web panel
(`SetAreDefaultScriptDialogsEnabled(seal.script_dialogs)`); the Mac answered every one at once (`alert` returned,
`confirm` false, `prompt` null, leave). Now a view whose walls say `asks` (the plain web panel only:
`Walls::asks = app.is_none()`) holds the callback and sends `ViewEvent::Ask`; the host hands it to its asker
(`crate::asks::Sheets`: an `NSAlert` sheet, "<host> says", OK and Cancel, a text field for a prompt; "Leave site?" or
"Reload site?"), and the reply comes back through the host's queue on its next turn (`View::reply`, called with no lock
held). A held dialog is answered no and its sheet closed when CEF resets the dialog state (the page went on), the page's
process ends or the view closes. A mini-app's, a mod's and a plugin's views and the web panel's pop-ups keep the seal:
answered at once, never shown. Native: `crates/flare-cef/src/macos/view.rs`, `crates/flare-web/src/macos/asks.rs`,
`crates/flare-web/src/macos/host.rs`. Held by `panel.dialogs` and `panel.leave-page` in
`the_web_panel_on_cef_navigates_draws_takes_input_and_holds_its_walls_with_no_window` (`crates/flare-web/tests/cef_macos.rs`)
and `walls.plugin-bar` (a bar's dialogs still answered). Source: Max, "make sure it's porting all browser features"
(2026-11-11), gap NAT-06.
1166. **macOS: a file input in the web panel opens a file panel as a sheet of the window.** It was cancelled at once (no
upload possible). Now the view holds the chooser and asks with its mode (one file, several, a folder, a save), the
page's title and default path, and the extensions its `accept` names (CEF's `accept_extensions`, or the filter itself;
none, meaning any file, when a type has none CEF could name); the asker runs an `NSOpenPanel` (or `NSSavePanel`) sheet
and the page gets the paths picked, or the cancel. Mini-apps and plugins keep the cancel. Native:
`crates/flare-cef/src/macos/view.rs` (`file_types`), `crates/flare-web/src/macos/asks.rs`. Held by `panel.file-chooser`
(the page's input gets the file, `.txt` only, several) and `a_file_chooser_takes_the_pages_extensions_or_any_file`.
Source: gap NAT-07 (2026-11-11). The colour input stays off (rule 1034, an owner decision); a `<select>` list is the
`PET_POPUP` layer laid over the page, as before.
1167. **macOS: a right-click in the web panel shows the page's menu as a native menu where the click was.** It was
emptied and never run. Now the plain web panel keeps Chromium's rows (Back, Forward, Reload, Undo, Cut, Copy, Paste,
Select All and the spelling rows as Chromium gives them) without Print (rule 1034) and View Source (a window of its own),
with WebView2's Open Link and Copy Link first on a link (Open Link loads it in the panel, as the panel's new windows do;
Copy Link puts it on the pasteboard) and Inspect last. The host's asker shows it as an `NSMenu` at the click (the page's
box in points from the content view's top left, plus the click), after the host's turn (the menu's tracking loop never
runs inside the pump), and the row picked goes back as the menu's command, or a cancel. Mini-apps, mods and plugins keep
the empty menu. Native: `crates/flare-cef/src/macos/view.rs`, `crates/flare-web/src/macos/asks.rs`. Held by
`panel.context-menu` and `a_page_menu_has_no_stray_separator_and_no_mnemonic_marks`. Source: gap NAT-01 (2026-11-11).
1168. **macOS: F12, Cmd+Option+I and the menu's Inspect open the web panel's DevTools in a window of their own.** On
Windows WebView2's DevTools are on for the web panel and off for every sealed view (`seal.dev_tools`). Now the host
catches F12 and Cmd+Option+I (a press, not a repeat, after the app's own chords) and the view opens CEF's DevTools in a
window of their own (the person asked for it), only when its walls say `dev_tools` (the plain web panel); the menu's
Inspect opens them at the click. F12 reaches the page only while its key scope has the keyboard; elsewhere Slint's
debug overlay keeps it (`keymap::reserved_for_slint`). No test opens DevTools (the screen rule). Native:
`crates/flare-web/src/macos/host.rs` (`dev_tools_chord`), `crates/flare-cef/src/macos/view.rs`. Held by
`f12_and_cmd_option_i_open_devtools_and_nothing_else_does` and `panel.context-menu` (Inspect is in the menu). Source:
gap NAT-04 (2026-11-11).
1169. **macOS: nothing the page asks ever comes on screen in a test, and a list CEF lends a handler is read in place.**
A host with no window (parent 0) answers every ask no at once (`asks::NoWindow`), and the probe puts in an asker of its
own that shows nothing (`WebHost::set_asker`); the web panel's host is given the window's content view on the Mac
(`web_ctl::production_factory`, `native_view_of`), the HWND on Windows as before. A list CEF lends a handler (a file
chooser's `accept_filters`, `accept_extensions`) is read in place (`view::strings`): the binding's `clone` of a lent
list reads as empty, which made every chooser take any file. Native: `crates/flare-web/src/macos/asks.rs`,
`crates/flare-cef/src/macos/view.rs`, `crates/flare/src/web_ctl.rs`. Held by
`without_a_window_every_ask_is_answered_no_at_once` (`crates/flare-web/src/macos/asks.rs`) and `panel.file-chooser`.
Source: gap NAT-07 work (2026-11-11).
1156. **macOS: CEF is the process's, started once by the first web host and shut down once at quit; an idle host
never takes it down or unloads it.** The window lets its host go when the last panel, background page and bar are
gone (`web_ctl`'s `drop_idle_backend`, as on Windows, whose environment goes with its last view). On the Mac that
dropped the `Engine`, whose `LibraryLoader` unloaded the CEF framework while CEF was still initialized; the next panel
made a new host that ran `Engine::start` (`cef_initialize`) a second time in the same process, and the quit, with no
host left, never ran `CefShutdown`, so the cookies and profiles were not flushed. Now the engine, its scheduler and the
browsers still closing live in a holder of the main thread's (`Cef` in `host.rs`): a later host adopts it (the
scheduler's turns go to the newest host's sink), a dropped host hands over what it was closing (and closes what it
left open), `WebBackend::close_all` and `end_engine` (called by `web_ctl`'s `close_all` at quit even with no host)
pump until those browsers closed and shut CEF down once. An `Engine` dropped without `shutdown` keeps the framework
loaded. The quit still closes every view first and answers waiting `browser.*` calls `UNAVAILABLE` "Flare is closing"
(rule 28). Windows is unchanged. Native: `crates/flare-web/src/macos/host.rs`, `crates/flare-cef/src/macos/engine.rs`,
`crates/flare/src/web_ctl.rs`. Held by the `app-loop.one-engine` and `app-loop.quit-after-idle` checks of
`a_web_panel_turned_as_the_window_turns_loads_and_paints_and_again_after_the_last_closed`
(`crates/flare-web/tests/cef_macos.rs`: the second host never calls `Engine::start`, the quit with no host shuts
down exactly once). Source: browser parity audit, gaps LC-02 and LC-06 (2026-11-11).
1157. **macOS: the web engine's pump thread ends with its scheduler.** `flare-cef-pump` waited forever, so every
dropped host left one behind (Max's Flare had two). The scheduler is now the process's (rule 1156), and its `Drop`
sets a stop flag under the lock the thread waits on and wakes it, so the thread ends at the shutdown. Native:
`crates/flare-web/src/macos/host.rs`. Held by the `app-loop` probe scenario (the shutdown drops the scheduler before
`CefShutdown`). Source: browser parity audit, gap LC-02 (2026-11-11).
1158. **macOS: a GPU or utility helper that fails is a `Process` note on every live view and a line in the log, and
nothing more.** Windows notes every `ProcessFailed` kind and only logs the GPU and utility ones (`Failure::Recovers`:
the browser starts them again). CEF has no callback for any helper but a renderer, so the browser process watches its
own children (`flare_cef::macos::helpers`): after each `on_before_child_process_launch` (and at the start) it looks at
its children for 2 s, knows a helper by its `--type=` switch (`gpu-process` is `gpu`, `utility` is `utility`, a
renderer is left to `on_render_process_terminated`), and waits for its end with `EVFILT_PROC` `NOTE_EXIT` and the wait
status. A signal is `crashed`, a non-zero exit `unexpected`; a clean exit (a utility that finished) and every end during
the shutdown are not reported. The note's detail is the Windows word (`gpu`, `utility`) and it carries the reason and
the signal or exit code; no page ends. Windows' browser-process loss (restart the environment) does not apply: CEF runs
inside Flare's own process. Native: `crates/flare-cef/src/macos/helpers.rs`, `crates/flare-cef/src/macos/engine.rs`,
`crates/flare-web/src/macos/host.rs`. Held by `a_helper_that_crashes_is_reported_and_one_that_exits_cleanly_or_at_shutdown_is_not`
and the other `helpers` unit tests, and the `app-loop.gpu-note` check (the probe kills its own GPU helper). Source:
browser parity audit, gap LC-05 (2026-11-11).
1159. **macOS: `browser_navigate` and `open_page` answer on their own navigation's number when the host is turned the
window's way.** `web_ctl` binds a waiting call to the first `Loading` after it and answers on the `Loaded` with the same
`nav`; on the Mac that needs the host's own turns (rule 1152), without which no `Loading` or `Loaded` came at all. The
`app-loop` scenario checks that the `Loaded` of a navigation carries the number its `Loading` gave and that an earlier
navigation had another. The CEF probe tests run one at a time (`.config/nextest.toml`): each starts a browser with its
helpers, and side by side they starved each other. Native: `crates/flare-web/src/macos/host.rs`,
`crates/flare-cef/src/macos/view.rs`. Held by the `app-loop.nav-id` check (`crates/flare-web/tests/cef_macos.rs`).
Source: browser parity audit, gap AG-05 (2026-11-11).
1175. **macOS: the web panel's profile is Flare's own folder, and keeps across a restart what WebView2 keeps.** A cookie
with an expiry and a page's `localStorage` come back after the engine shut down and started again on the same data
folder; a session cookie does not (`PERSIST_SESSION_COOKIES` is 0 for every profile, rule 1125); the cookie and storage
files are under the host's data folder, nowhere else. Native: `crates/flare-cef/src/macos/context.rs`,
`crates/flare-cef/src/macos/engine.rs`. Held by
`a_kept_sign_in_and_stored_data_survive_an_engine_restart_and_a_session_cookie_does_not` (`crates/flare-web/tests/cef_macos.rs`,
probe scenarios `persist-write` and `persist-read`, `crates/flare-web/src/macos/probe/parity.rs`). Source: browser parity
gap PROF-01 (2026-11-11).
1176. **macOS: a page's `prefers-color-scheme` is the app's appearance and follows it; the user agent is a desktop
Chrome's on a Mac.** Windowless CEF reads `NSApp`'s effective appearance (the system's unless the app sets one) and
follows a change, as WebView2's follows Windows'; nothing in Flare sets a scheme or a user agent on either platform.
The user agent starts `Mozilla/5.0 (Macintosh; Intel Mac OS X`, names `Chrome/<major>` and `Safari/537.36`, has no
`Headless` or `CEF`, and `navigator.webdriver` is false. Held by `look.color-scheme` and `look.user-agent` in
`mini_apps_load_and_paint_and_pages_follow_the_apps_appearance_on_cef` (`crates/flare-web/tests/cef_macos.rs`). Source:
browser parity gap PROF-03 (2026-11-11).
1177. **macOS: a plugin's fill types into a sign-in form through `run_script` and stops on a page that moved.**
`flare_mods::fill::fill_script`, run as `web_ctl` runs it (CDP `Runtime.evaluate`, by value), types the user name, the
password and the code, fires `input` and `change`, and answers kinds of field only; on another site (scheme, host or
port) it answers `{ok:false, why:'moved'}` and types nothing. Held by `fill.types` and `fill.moved` in
`a_plugins_fill_types_into_a_form_and_its_bar_and_background_page_stay_sealed_on_cef` (`crates/flare-web/tests/cef_macos.rs`).
Source: browser parity gap PLG-01 (2026-11-11).
1178. **macOS: every mini-app Flare writes loads and paints in a windowless view.** `show_html` (the page behind its
policy), `show_diagram` (the bundled Mermaid draws an svg), `show_model` (WebGL 2 is there windowless; the viewer reads
the model and draws it), `show_audio` (the track beside the page loads and plays after the person's click; a page's
autoplay with sound waits for a gesture, Chromium's default) and the markdown Compiled view (no script). Each page is
written by the same `flare_surfaces` functions `ctl_app.rs` and `panel_ctl.rs` use. Held by `mini.html`, `mini.diagram`,
`mini.model`, `mini.audio` and `mini.markdown` in `mini_apps_load_and_paint_and_pages_follow_the_apps_appearance_on_cef`
(`crates/flare-web/tests/cef_macos.rs`). Source: browser parity gap MINI-01 (2026-11-11).
1179. **macOS: a plugin's bar and background page keep their seals in the web panel's host.** The background page
opens hidden in its plugin's own profile and talks over its bridge; a right click on the bar reaches the page but shows
no menu; Cmd+R, Cmd+=, Cmd+F and Cmd+P reach the page as keys and do nothing of the browser's (no reload, zoom, find or
print); a download is cancelled before it starts and a dialog answered unseen, on both (`seal::plugin_seal`). Held by
`plugin.background` and `plugin.sealed` in
`a_plugins_fill_types_into_a_form_and_its_bar_and_background_page_stay_sealed_on_cef` (`crates/flare-web/tests/cef_macos.rs`).
Source: browser parity gap PLG-03 (2026-11-11).
1170. **macOS: a pop-up no click asked for is refused by Flare's rule, noted and said in the bar; a page's own
`window.close()` closes only a pop-up.** Chromium's own pop-up blocker stopped such a request before CEF asked
(`on_before_popup` never ran), so there was no `web-refused popup-blocked` note and no "Pop-up blocked" in the bar. The
engine starts with `disable-popup-blocking` (`SWITCHES`), so every request reaches the view's `NewWindowRule`
(`popup_plan` refuses it as `blocked`, `window.open` stays null; a mini-app's or a mod's page is refused by
`refusing_rule`, and a view with no rule opens nothing). A script's `window.close()` reached CEF's default close and
the panel's browser died behind a stale view: `LifeSpan::do_close` now lets a close through only for a pop-up (its
own `window.close()` still closes it) or the owner's `View::close` (`owner_closing`), and otherwise cancels it and sends
`ViewEvent::CloseIgnored`, which the host notes as `web-close-ignored` by site for every view, as WebView2's
`WindowCloseRequested` does. A browser that still ends without the host closing it drops its view and the panel hears
`Gone`, never a frozen picture. Native: `crates/flare-cef/src/macos/engine.rs`, `crates/flare-cef/src/macos/view.rs`,
`crates/flare-web/src/macos/host.rs`. Held by the probe's `panel.popup-blocked` (the `blocked` note),
`panel.window-close-ignored` (the page stays, the note comes) and `panel.notes` (`flare-web/tests/cef_macos.rs`), and
`the_engine_is_started_with_its_quiet_switches_and_keeps_cefs_own_disabled_features`. Source: the browser parity audit
(POP-01, POP-03, DIAG-01), 2026-11-11.
1171. **macOS: a link to an app inside a frame is asked about ("Open an app?") as on Windows, never started by the
browser.** The web panel's navigation wall lets a frame go anywhere, so a `zoommtg:` or `mailto:` link in a frame
reached only the browser's own launch (`on_protocol_execution`), which refused it silently: no question, no note. Now
every navigation of a view gets a request handler (a pass-through one when the view has no request wall), whose
`on_protocol_execution` still refuses the launch and sends `ViewEvent::External` with the main frame's address; the
host offers it to `AppLinks` for the web panel and its pop-ups (the same `AppLinkDedupe`, so one link is one question),
and anything that is not an app link is noted `web-refused scheme` (and refused in the bar on the panel), as WebView2's
`LaunchingExternalUriScheme` backstop (`ask_external_uris`) does. A mini-app's or a mod's page is told nothing new.
Native: `crates/flare-cef/src/macos/view.rs`, `crates/flare-web/src/macos/host.rs`. Held by the probe's
`panel.app-link-in-frame` (the person's click on a `zoommtg:` link in a frame: one `AppLink`, an `app-link` note, no
refusal). Source: the browser parity audit (LINK-01), 2026-11-11.
1172. **macOS: a certificate the browser does not trust shows Chromium's error page, is noted by site and is never
overridden.** `on_certificate_error` answers 0 (CEF cancels the request): the load ends not ok with WebView2's status
(`CERTIFICATE_IS_INVALID`, 5, through `web_error_status`), the page is Chromium's "Privacy error", the log has
`web-cert cert-invalid` with sites only, and no request gets past the handshake. Native:
`crates/flare-cef/src/macos/view.rs`, `crates/flare-web/src/macos/host.rs`. Held by the probe's `panel.cert`: an HTTPS
server on loopback with a self-signed certificate made for the run by `/usr/bin/openssl req` (it listens on nothing;
the server is rustls, `probe/tls.rs`) counts the requests that got through: none. Source: the browser parity audit
(SEC-01), 2026-11-11.
1173. **macOS: the page's scrollbars are overlay scrollbars, as WebView2's Fluent overlay style draws them on
Windows.** Chromium on a Mac takes the scroller style from `+[NSScroller preferredScrollerStyle]` in the browser
process; with a mouse attached and "Show scroll bars: Automatically" that is the classic bar, 15 px of every page's
width. Before CEF starts, `scrollers::prefer_overlay` makes that method answer `NSScrollerStyleOverlay` for Flare's
process (once), so every page (the web panel, mini-apps and mods, as on Windows) draws thin bars over itself that fade
when it is still, and no page's DOM is touched (a page that styles its own scrollbars keeps them). Native:
`crates/flare-cef/src/macos/scrollers.rs`, `crates/flare-cef/src/macos/engine.rs`. Held by the probe's
`panel.overlay-scrollbars` (`innerWidth == clientWidth` on a page that scrolls; 15 without the fix on a Mac with a
mouse) and `appkit_answers_the_overlay_style_once_flare_asked_for_it`. Source: the browser parity audit (NAV-10),
2026-11-11.
1174. **macOS: while the splitter is held, the page drawn in its box keeps its width and its edge moves, as WebView2's
window does on Windows.** The hold (rule 230) kept the page's width, but the box drew the held picture with
`image-fit: fill` across its new width, so the page looked stretched or squeezed and a click landed off what it was
drawn on until the 120 ms settle. The live picture (`panel.slint`, `web-frame`) is now drawn at its own width from the
box's left edge (a pixel or two off, rounding, still fills), cut at the box's edge when wider, so input maps 1:1;
on release or the settle the page's next picture fills the box again. Windows draws no picture there (`web-live` is
false), so nothing changes on Windows. Native: `crates/flare/ui/panel.slint`. Held by
`a_page_drawn_off_screen_keeps_its_width_while_the_splitter_is_held` (`crates/flare/src/appctl/tests/web_tests.rs`,
rendered pixels). Source: the browser parity audit (LC-09), 2026-11-11.
1182. **The app icon has one source per platform; the in-app mark is not it.** Windows' `.ico` (the exe resource, the
taskbar and window icon, Velopack's `Setup.exe` and `Update.exe`) is rendered by `tools/make-icon.py` from
`assets/icon/app-windows.svg` with librsvg (`rsvg-convert` first: Homebrew's ImageMagick has no librsvg and its own
SVG reader draws the gradient fills as nothing). The Mac's is the owner's Icon Composer icon
`assets/icon/app-macos.icon`, compiled by `xcrun actool` (Xcode 26, headless) into `Contents/Resources/Assets.car`
(`CFBundleIconName`, the Liquid Glass icon of macOS 26+) and `Flare.icns` (`CFBundleIconFile`, macOS 15).
`assets/icon/flare-color.svg` stays the in-app mark (the start screen, the settings rainbow, the chrome probe) and
feeds neither. Held by `each_platform_app_icon_has_its_own_source_and_the_in_app_mark_stays`
(`crates/flare/tests/ship_text.rs`), `the_mac_app_icon_is_the_icon_composer_icon_compiled_by_actool` and
`actool_compiles_the_app_icon_into_a_catalog_and_an_icns` (`crates/flare-cef/src/macos/release.rs`) and
`tools/test_make_icon.py`. Source: the owner's new app icons, 2026-11-12.
1181. **Slint's GPU renderer keeps its GPU pipelines from one frame to the next; a frame never compiles them again.**
Max on his Mac: the UI, the caret in Claude Code and the tab icons "flicker very quickly, consistently". Sampled in his
running Flare (2026-11-12, `sample`, read only): about 80 % of the main thread was drawing (Slint's `CADisplayLink`
frames), 78 % of that inside wgpu's `create_render_pipeline` (naga to MSL, a Metal compile), 45 to 50 % CPU for hours.
Cause: with a rendering notifier installed (Flare's, for the grid) Slint's femtovg renderer flushes its clear on its
own before `BeforeRendering`, and femtovg 0.27.0 dropped at the end of every flush each pipeline that flush did not use,
so the clear-only flush left one and every frame built all the others again. Frames run while any tab's mark moves
(a working or asking agent, nearly always on Max's machine), so every one of them overran the display's refresh. An
idle window with nothing moving already asked for no frame (now held by a test). Now Flare uses a patched femtovg
(`third_party/femtovg`, `[patch.crates-io]`): a pipeline goes only after 240 flushes in a row left it unused. Windows has
the same double flush and gains the same. Not proved on screen: whether this alone ends the flicker Max sees (the
headless frames are pixel-identical with and without the rebuilds); if it remains, record the screen next. Native:
`third_party/femtovg/src/renderer/wgpu.rs`, `Cargo.toml`. Held by `a_clear_only_flush_keeps_the_pipelines_of_the_frame_before`
(`crates/flare-render/tests/femtovg_pipelines.rs`; on upstream femtovg the second frame starts with one pipeline) and
`an_idle_window_asks_for_no_frame` (`crates/flare/src/headless.rs`). Source: Max on his Mac, 2026-11-12.
1180. **macOS: Flare never smooths, eases or animates the terminal's scroll; a glide comes from a mouse utility, and
the cure is that utility's per-app rule for Flare, not a Flare setting.** Max (2026-11-12): "it scrolls like a browser
and not line by line", in plain shell tabs too, on the build with rule 1129. His Mac has no trackpad: a Razer Basilisk
V3 Pro through LinearMouse 0.12, whose `scrolling.smoothed.vertical` scheme (easeInOut, response 0.45, inertia 0.6) is
on for every app, with one app scheme for `com.apple.Terminal` that turns smoothing off and scrolls 3 lines a notch
(`~/.config/linearmouse/linearmouse.json`). LinearMouse's `SmoothedScrollingEngine` turns each notch into a run of small
precise scroll events with trackpad phases (`CGScrollPhase`, then `CGMomentumScrollPhase` for the inertia), so every
app but Terminal glides. Flare reads them as rule 1129 says: the inertia (momentum) moves nothing, the eased part moves
one line per cell height of pixels, whole lines only, and the view jumps to them in the next frame. There is no
animated offset, easing, velocity scaling or kinetic scroll anywhere in the grid's path (`WheelAccum::add_mac`,
`TermHandle::wheel_lines`, the renderer). Checked 2026-11-12: on a real winit 0.30.13 view, `scroll_phase::current`
reads a finger Changed, Ended and two momentum events correctly when AppKit dispatches them (so rule 1129 holds in the
shipped path), and CEF's `do_message_loop_work` with the external pump dispatched none of 40 queued AppKit events (no
nested loop steals the scroll from winit). The fix for line-by-line scrolling is a LinearMouse app scheme for
`com.flare.terminal` that matches the Terminal one (smoothing off, distance 3); Flare adds no "Smooth scrolling" setting,
since it has no smoothing to turn off. Held by `mac_scroll_moves_with_the_fingers_and_never_with_the_momentum`,
`momentum_wins_and_each_finger_phase_is_read` and `a_line_step_is_one_line_everywhere_and_a_notch_stays_three`.
Source: Max on his Mac (2026-11-12); the Mac setup notes (mouse, LinearMouse).
1184. **On a CI runner a check waits for what it tests, and skips a part only when the runner truly lacks the
hardware.** Main eede2a8b went red twice on GitHub's macOS VM while the PR runs passed. `mini.audio` (rule 1178) saw
`play()` take the click but `currentTime` stay at 0, because the VM has no audio output device: the probe now polls the
clock for up to 5 s and, only when it still has not moved and `system_profiler SPAudioDataType` lists no output device,
passes with `skipped` and the reason in the report (`cef_macos.rs` prints it); load, paint, the 2 s track and `play()`
are still required, and a Mac with an output device must still play. promptLayout read the last line once, right
after `ready` (the prompt after the echo had not come yet): it now waits for that prompt before resizing and gives the
redraw after the resize 5 s, failing on a prompt that stays broken. Native:
`crates/flare-web/src/macos/probe/parity.rs`, `tools/e2e/src/scenarios/lifecycle.rs`. Held by
`mini_apps_load_and_paint_and_pages_follow_the_apps_appearance_on_cef` and the promptLayout scenario. Source: CI run
43147800100 on main (2026-11-12).
1185. **macOS: while CEF has a browser, the web host keeps a turn of CEF's work at most 33 ms away.** CI's macOS VM
failed `app-loop.open` twice (run 43165739212, run 43168171020): the panel loaded (`Title`, `Loaded ok`) but never
painted in 10 s, and once its next navigation got only as far as `Loading` for 20 s; the next host's view then loaded
and painted at once. CEF's main-thread tasks run only in `do_message_loop_work` (a probe that never called it loaded
nothing), and CEF asks for that turn (`on_schedule_message_pump_work`) only when a task is posted: in a probe run all
152 asks were delay 0, none for its delayed work, so its timers (the windowless view's frame clock, a navigation's
later steps) ran only when some other ask came. A fast Mac's traffic of asks hid it; a slow or busy Mac stalls, until
another view's turns (`retry_while_waiting`) drain it. `WebHost::keep_turning` now schedules a turn 1000/30 ms after
every pump while the host has a view or a browser closing, as CEF's own external-pump sample does
(`MainMessageLoopExternalPump`, `kMaxTimerDelay`); with no panel there are no turns. Native:
`crates/flare-web/src/macos/host.rs`. Held by the probe's `app-loop.idle-turns` (a quiet page gets at least 30 turns
in 2 s: about 64 with it, 9 without) beside `app-loop.open` and `app-loop.navigate`. Source: CI run 43168171020 on
fix/main-ci-green-2 (2026-11-12).
1186. **macOS: a helper is known only once its arguments say what it is.** The helper watcher (rule 1158) marked every
child it saw as looked at before reading its arguments, so a child read before its exec (still this process's
arguments) or before they could be read was taken for good as no helper, and its crash never became a note:
`a_helper_that_crashes_is_reported_...` failed about one run in two on a Mac mini, and `app-loop.gpu-note` now and
then. A child that is not yet a helper is read again at the next look. Native: `crates/flare-cef/src/macos/helpers.rs`.
Held by `a_helper_that_crashes_is_reported_and_one_that_exits_cleanly_or_at_shutdown_is_not` (12 of 12 after) and
`app-loop.gpu-note`. Source: fix/main-ci-green-2 (2026-11-12).
1187. **macOS: the terminal is a focused text area whose cursor accessibility can read.** Magnifiers could not follow
typing on the Mac (issue #201, Wind for Mac #567): the app's `AXFocusedUIElement` was empty while the terminal had the
keyboard, because the tree held only Slint's controls (AccessKit) and nothing for the grid. The window's view now gets
one more dynamic subclass on top of AccessKit's (no instance variables, the state in a thread-local) that adds a
terminal element, an `NSAccessibilityElement` with the role `AXTextArea`, to the view's children and answers
`accessibilityFocusedUIElement` with it while the terminal holds the keyboard: the window has it and no field, dialog,
menu, side panel or panel page does (`live::terminal_has_keys`, read again when a tracker asks). Otherwise the class
below (AccessKit's Slint tree) answers, so a field keeps its own focus. The element's `AXSelectedTextRange` is the
cursor (zero length) on its value, the cursor's line, and `AXBoundsForRange` (`accessibilityFrameForRange:`) is the
cell `place_ime` gives the IME, in Cocoa screen space through the view and its window (every display and backing
scale); its frame is the grid. A focus change posts `AXFocusedUIElementChanged`, a new cell `AXSelectedTextChanged`,
once per change, never per frame. The value is the cursor's line only for now (whole-screen text and VoiceOver
reading are follow-ups). Native: `crates/flare-win/src/macos/ax_terminal.rs`, `crates/flare-win/src/ax_caret.rs`,
`crates/flare/src/window.rs` (`place_ime`, `handle_focus`). Held by
`the_focus_is_claimed_only_with_the_keyboard_and_a_cell`,
`a_frame_that_leaves_the_cell_posts_nothing_and_a_move_posts_once`,
`losing_the_keyboard_or_the_terminal_gives_the_focus_up_once`, `a_cell_becomes_points_in_a_plain_or_flipped_view`,
`a_bad_scale_places_at_one_and_an_empty_cell_is_one_pixel`,
`the_text_is_the_cursor_line_and_the_index_counts_utf16`,
`a_range_is_cells_beside_the_cursor_and_the_insertion_point_is_its_cell` (`ax_caret`),
`the_terminal_has_the_keys_only_when_no_field_panel_or_page_does` (`live`), and the AppKit probes
`ax_terminal_is_the_focused_text_area_with_the_cursor_cell_on_the_screen` and
`ax_terminal_keeps_a_toolkits_own_tree_and_focus_when_the_terminal_has_none` (`crates/flare-win/tests/appkit.rs`, a
window that is never shown). Source: issue #201 (2026-11-12).

1188. **Settings > Plugins: the card's switch alone turns a plugin on; the details sheet opens only from Details.** Max,
2026-11-12: "when you turn on plugins have it just be flipping the switch, dont show the details panel with the turn on
button, the user can get there by clicking details if they wanna". Since 2026-11-08 a switch flipped on for a plugin
that asked for anything opened its details sheet and the plugin stayed off until the sheet's Turn on. Now the switch
(or a press on the card's row) calls `ModsPort::set_enabled(id, true, shown)` at once, with `shown` what the sheet
lists now (`ModsPort::sheet(id).shown`), the same approval Turn on records, so the hub starts the plugin and its helper
as before and still refuses a list that changed since (rule 214). Off goes at once with nothing approved. No sheet
opens. The sheet is reached from the card's **Details** button (`INFO_BUTTON`, worded, between Update and Remove; it
was an info icon), and its Turn on still approves what it lists. Same on Windows and macOS. Native:
`SettingsCtl::event` (`crates/flare/src/settings_ctl.rs`), `flare_settings::page::mods` (the card). Held by
`the_switch_alone_turns_a_plugin_on_with_what_its_sheet_lists_and_opens_no_sheet`,
`the_details_button_opens_the_sheet_whose_turn_on_still_approves_what_it_lists`,
`a_mod_with_nothing_to_approve_goes_on_from_its_card`,
`every_card_is_one_row_with_update_details_and_remove_and_a_branded_one_draws_its_logo` (`ui_tests::settings_mods`),
`every_card_is_one_row_with_update_details_and_remove_and_no_heading` (`flare_settings::page::mods`),
and the e2e `modsSample` (`turn_on_by_switch`).
1189. **A prompt that redraws its line on every key does not flicker (Windows and macOS).** A lone chunk of output
(a typed key's echo) still reaches the grid at once, but one that leaves the line half drawn waits until the pty has
been quiet for 2 ms (`REDRAW_SETTLE`), never longer than the 8 ms window (rule 31): the caret hidden (`CSI ? 25 l`),
something erased (`CSI K`, `CSI J`, `CSI X`) with no text written since, or a sequence cut in the middle. `bw login`
(Node inquirer 8.2.6) writes each redraw as four writes 0.1 to 0.2 ms apart (`CSI 2K CSI G`, the prompt and the
text, `CSI nD`, `CSI nC`); the erase left alone, so every key showed a blank line with the caret at column 0 for
one window. Plain shell echoes (zsh: `l`, `\b ls`, `\b\bl \b`) are unchanged: about 0.04 ms to the sink before and
after (dev Mac 2026-11-12). DEC 2026 synchronized output was already honoured by the grid. Native:
`crates/flare-term/src/batcher.rs` (`RedrawScan`, `OutputBatcher::due_in`), `crates/flare-term/src/session.rs`
(`reader_loop` wakes the pump on each push to a held chunk). Held by `an_inquirer_redraw_presents_no_blank_line`,
`a_slow_redraw_still_presents_no_blank_line`, `a_hidden_caret_holds_a_lone_redraw_until_shown`,
`a_half_drawn_line_waits_at_most_the_window`, `a_plain_echo_still_leaves_at_once`,
`the_scan_follows_caret_erase_and_cut_sequences` (`batcher`), `a_single_echo_still_reaches_the_sink_within_the_budget`
and `a_prompt_redraw_reaches_the_sink_whole` (`session`). That session test runs on real threads with the settle
stretched to 25 ms (`SessionOptions::redraw_settle`), so a loaded CI runner that stalls a thread past 2 ms cannot
split the pair (macOS CI run 43258287798 split 3 of 20 at the real 2 ms); the real 2 ms is held on a fake clock
by the `batcher` tests. Number 1188 was left for a parallel PR. Source: Max, 2026-11-12.
1195. **A colour picker left open when Settings goes behind another tab is open again when it comes back, even
right after the theme editor closed.** Closing the editor asks the keys for its pencil for 600 ms
(`pencil-pending`, app.slint), and a wall made in that time hands them over from the card's `init`; the pencil's
`pencil-taken` is not wired, so the wait always ran its full 600 ms. A tab switch and back inside it rebuilt the wall
on return: the popover the return reopened took the keys first, the new card took them from it, and the popover shut
on `focus-left`, which also forgets a kept picker, so it never came back. themeEditorLive's "an Appearance picker left
open is open again on return" failed this way in Mac CI (runs 43340817015 and 43352885658 on main 80f682ca, and
11 of 60 local Mac runs, 9 of the 40 with 24 `yes` processes; 0 of 60 after), where the whole step takes under 600 ms; the Mac's
event loop runs the change handlers before the frame that makes the cards. Opening a picker and Settings going
behind a tab now end the wait (`SettingsCtl::drop_pencil_wait`, `crates/flare/src/settings_ctl.rs`); the pencil still
takes the keys when nothing else did. Held by
`a_picker_opened_just_after_the_editor_closed_is_open_again_after_a_tab_switch` (`ui_tests/settings_flows.rs`).
Numbers 1190 to 1194 were left for parallel PRs. Source: Mac E2E in CI (2026-11-13).
1190. **A paste on a remote tab chooses as a local tab's paste does.** Rule 304's upload ran before the paste's own
choice and ignored its mode and the agent: a plain paste (Ctrl+Shift+V) with files or a picture on the clipboard copied
them to the host, and an Excel or Word copy (text plus a bitmap) pasted at a plain prompt sent a `pasted-image-<ms>.png`
to the host and typed its path, where a local tab pastes the text. Now a plain paste never copies to the host; a normal
one copies files whenever there are some, and a picture only where a local paste would send the image key (an agent is
armed, or there is no text). Native: `crates/flare/src/clip_flow.rs` (`remote_upload`),
`crates/flare/src/term_ui.rs`. Held by
`clip_flow::tests::a_remote_paste_copies_files_or_an_image_only_where_a_local_paste_would_use_them` and
`term_ui::tests::a_remote_tab_pastes_the_text_of_an_excel_copy_and_a_plain_paste_never_copies_to_the_host`. Source:
review of remote sessions PR 6 (2026-11-12).
1191. **The leftover sweep never takes a live run's folder.** Rule 304's sweep judged a run folder under
`<temp>/flare-remote` and `<temp>/flare-remote-copies` by its own mtime, which changes only when a tab folder in it comes
or goes. So any new Flare process on the machine removed the uploads and fetched copies of another Flare still running,
once that run had made no new tab folder for a day. Each run now locks `.flare-run.lock` in its run folder when it first
makes it and holds it for its life (the system lets go when it ends or crashes); the sweep skips a folder whose lock
file is locked, or cannot be looked at, and judges a folder with none by age alone. Native:
`crates/flare/src/remote_files.rs` (`RunLock`, `run_alive`, `sweep_old`). Held by
`remote_files::tests::a_live_runs_folders_are_never_swept_however_old_and_a_gone_runs_are`. Source: review of remote
sessions PR 6 (2026-11-12).
1192. **Clawd's working hop lands on whole device pixels and blinks in one shape.** `AgentIcon`'s moving box (H1's
hop and `hop`, since 6b7307e1) took `size * sx` and `size * sy` with unsnapped, fractional x and y, so every 16 ms frame
fell on another sub-pixel box: the anti-aliasing smeared, and Clawd looked to shrink in width, wobble and leave
artifacts on the tab. The blink laid a second path of lids over the body and left a seam. Now the rest size, the
squashed size, the rise and the box's place snap to whole device pixels of the window (`Px.snap` on the icon's
`absolute-position`), a hop squashes about its art's feet (`flare_agent::icon::CLAWD_FEET` and the like), the keys
and timing stay, and the blink swaps in `Glyphs.clawd-shut` (`flare_agent::icon::clawd_shut_path`, the body with its eyes
filled) for the one Clawd path.
Native: `crates/flare/ui/tabmark.slint` (`AgentIcon`), `crates/flare-agent/src/icon_motion.rs` (`icon_box`),
`crates/flare-agent/src/icon.rs`, `crates/flare/ui/glyphs.slint`. Held by
`icon_motion::tests::every_frame_of_the_hop_is_a_whole_device_pixel_box_standing_on_its_feet`,
`ui_tests::marks::every_frame_of_the_hop_draws_clawd_on_whole_device_pixels` (100, 125, 150, 200 and 225 %),
`icon::tests::the_shut_clawd_is_the_body_with_its_two_eyes_filled_and_nothing_else`,
`icon::tests::the_feet_are_the_lowest_edge_of_each_marks_art` and the `ui_text_marks` tests
`the_agent_icons_moving_box_snaps_every_edge_to_whole_device_pixels` and
`the_shut_clawd_is_the_agent_crates_path_swapped_in_for_the_body`. Source: Max, 2026-11-13 ("Keep squash, snap to
pixels").
1200. **The Agent moves sample adds nothing to a tab's menu.** Max, 2026-11-13: "the plugin agent moves dont need to
add itsself to the reight click menu". Its "Agent motions" item (an `actions` entry, `where: ["tab-menu"]`) and the
picker panel it opened are gone, with the `panel` and `actions` permissions; the plugin asks for `agent-looks` only
and sets the owner's three motions from its background page. Picks saved by 0.1 (its store, key `picks`) still apply.
Native: `sample-plugins/agent-moves/`. Held by `the_sample_plugins_are_valid`
(`flare-mods/src/discover.rs`: no actions, no panel, no `panel.open`) and the sample round trip
`every_sample_plugin_loads_turns_on_and_answers_one_round_trip_of_its_protocol`. Source: Max, 2026-11-13.
