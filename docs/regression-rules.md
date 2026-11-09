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
13. **A path on screen is a link when it exists; the backend resolves it again.** Native: the UI sends only text and folder; the backend refuses UNC and NUL, caps length, checks with a metered stat, returns the kind. Source: regression-rules.md path-links (#99); core/main/termPathOpen.ts.
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
26. **Ctrl+scroll zoom is per session and never saved.** Native: a per-tab font override of 7 to 64 px in the tab model, wheel handler non-passive, never sent to the program. Source: TerminalPanel.tsx zoomSession.

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
109. **Both apps install side by side with Flare's own names.** Native: different app id, verb key, single-instance lock, profile, plugin name, hook signal and tab-id variable; `tools/e2e/tests/side_by_side.rs` proves no clash (Layer A on every run, Layer B on a clean CI runner). Source: design spec; roadmap.
110. **The diagnostics log holds no typed text, clipboard, audio, screen text, title text or conversation.** Native: the writer accepts typed kinds and fields; opaque channels are summarised by size (`string:n`, `bytes:n`); a test that a marker string never reaches the file. Source: core/shared/diagSummary.ts; agentDiag.ts header; PRIVACY.md.
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
https and about:blank open, whoever asks), and the e2e `webPanel` (a location request is denied, `window.open` goes to the same
panel, the data folder is the world's). `browser.eval` itself is stage 4.
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
214. **A page message counts for the mod whose page sent it, only while the tab shows that mod; a mod sees its own tab only; a switch approves the list that was shown; a call id cannot be guessed.** The host puts the host of the receiving view into the message (it names the mod); `WebCtl` drops it unless the tab's view is that host, and the window drops it unless the tab's panel and the controller's binding both name that mod, so a message queued by mod A's page runs with nothing after the tab moved to mod B. `tabs.list` gives a mod its own tab only (the `panel` permission never widened it). Settings hands what the card listed with the switch (the permission words, and a plugin's author and homepage); `Hub::set_enabled_shown` refuses when the manifest found now says anything else, and the page draws again. A call id is `c` plus 128 random bits. Native: `flare_web::host`, `flare::web_ctl`, `flare::appctl::mods_glue`, `flare_mods::hub`. Held by `a_message_from_the_page_the_tab_left_is_not_run_with_the_mod_it_shows_now`, `a_mod_sees_its_own_tab_only_whatever_it_may_do`, `a_switch_approves_the_list_the_row_showed_and_nothing_else`, `a_switch_refuses_a_plugin_whose_maker_changed_since_its_card_was_drawn`, `call_ids_are_long_and_never_repeat`, `the_switch_reaches_the_mods_port_and_the_page_draws_what_the_port_then_says`.

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
221. **A zoom resizes one tab.** Ctrl+wheel zoom, and a switch between a zoomed and an unzoomed tab, refit only the tabs
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
nothing. A copy that fails puts the plain note back only while the exact "copied" note is still showing; a silent copy that
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
232. **macOS: closing a tab ends its whole session, and throws away the output nobody will read.** The shell leads
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
233. **macOS: every shell starts as a login shell and reports its folder with OSC 7, without touching the user's
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
234. **macOS: paths are read with the POSIX grammar behind one seam, and a path that would reach a server is never
statted, restored or adopted.** `flare-term/src/path_grammar.rs` picks the grammar: Windows
(`path_grammar/windows.rs`, the drive and UNC rules moved there unchanged) or macOS (`macos/path_grammar.rs`): one
root `/`, `~` and `~/` for home (`~user` is refused), `/` the only separator (a `\` is a name character), case kept,
`..` never above the root. Terminal text also links a `file://` URL on this machine (percent-decoded, its
`:line:col` off) and a word whose spaces are escaped with `\`. A `//` path and anything under an automount point
(`/net`, `/Network`, `/home`, each also under `/System/Volumes/Data`, and any mount of type `autofs` that
`getfsstat` lists; any case, also after `..`) is never resolved, statted or read in a panel (`flare-surfaces`
`preview::loadable`). One function holds that refusal, `flare_shells::macos::paths::is_automount`, used by the
link grammar, `is_remote` and the panels' `is_local_path`, which also refuses a `.` or `..` segment; a mounted volume (`/Volumes/<name>`) gets its own slow-volume key in the
stat meter. A tab's reported folder and a restored one go through `flare_shells::cwd::is_remote`: on macOS it
refuses the same autofs and `//` paths, a `.` or `..` segment and the Windows UNC forms. The sidebar line and the +
menu read POSIX folders (`~/code`, `.../Claude/Github`). Held by the `posix` twins in `path_open`, `path_links`,
`paths`, the `macos::*` tests of flare-term, flare-shells, flare-tabs and flare-surfaces, and
`restore_never_spawns_in_a_remote_folder_on_macos`. Source: macOS port phase 2, step 3 (2026-11-08).
235. **macOS: fonts come from the font folders, the stacks fall to SF Mono and Menlo, the UI is SF, and wgpu
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
236. **macOS: the window is a native titled window under Flare's top row, and its glass is an effect view behind
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
down), run once, and `NSWorkspaceWillPowerOffNotification`, which flushes every time (rule 239). Held by `crates/flare-win/tests/appkit.rs` (a
main-thread harness on a window that is never shown) and the `chrome`, `glass`, `session_end` and `window_park`
tests. Source: macOS port phase 3, step 1 (2026-11-08).
237. **macOS: Cmd is the app's key, Ctrl is the shell's, and the menu bar is the keymap.** One chord table serves
both systems: `KeyContext::os` (`flare_term::keys::KeyOs`, default Windows) picks the modifier, so on macOS every
app chord is a Cmd chord and every Ctrl+letter reaches the shell as a control character; Ctrl+Tab and the agent
guard's Ctrl+C and Ctrl+D stay Ctrl on both (Cmd going down on its own, `"Super"` in winit's names, leaves a
Ctrl+Tab walk open on the Mac; Windows keeps its old list), Alt+Space is Windows only, the zoom keys and Cmd+Shift+[ and ] are
`MAC_ONLY`. `AppCtl::key_os` is `keymap::app_key_os()`: the Mac in a Mac build, Windows in every unit test, so the
Ctrl tests hold on both systems and the Mac tests set the Mac keys. The terminal's copy, paste and select all
follow `term_key_action_on(os)` (Cmd+C, Cmd+V, Cmd+A on a Mac), and a key left for the shell goes through
`keymap::pty_key`, which on macOS sends nothing for a Cmd key (except the line keys) and gives Option to the
layout unless `flare.term.optionAsMeta` is on. The menu bar (`mac_menu`, built by `flare_win::menu_bar`) never has
an action of its own for a key item: the item IS its key press, sent through `window::handle_key`, and its key
equivalent is made from that press, so the menu and the keyboard cannot drift apart (a key a Slint field has goes
on to Slint as Control, Slint's name for Cmd on macOS). Never add a Mac shortcut in the menu alone. Held by the
`mac_menu`, `chords`, `keymap` and `mac_keys` tests and `crates/flare-win/tests/appkit.rs`. Source: macOS port
phase 3, step 2 (2026-11-08).
238. **macOS: the system glue is AppKit and the per-user Library, never a helper process.** The clipboard is
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
239. **macOS: a power-off notice flushes but never uses up the final save.** `NSWorkspaceWillPowerOffNotification`
comes before the apps are asked to quit, and another app can still cancel the log out or shut down, leaving Flare
running. So `SessionEndWatch` flushes on it every time without arming anything, and only
`NSApplicationWillTerminateNotification` (the app really ends) is run once. A shared "fired" flag across both
would lose every change made after a cancelled log out. Held by
`the_end_runs_once_a_power_off_never_disarms_it_and_the_power_events_arrive_until_the_watch_stops`
(`crates/flare-win/src/macos/session_end.rs`). Source: macOS port phase 3 review (2026-11-08).
240. **macOS: a file opens only in a handler that cannot run it.** The extension allowlist is not enough on a Mac: a
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
241. **macOS: an `--e2e` run is an accessory app from its event loop's first turn.** `backend::with_window_attributes`
hands Slint a winit event loop builder under `--e2e` with the Accessory activation policy and no activation at
launch (`mac_e2e_loop_plan`), so the app never shows a Dock icon or comes forward before `window_park::park`, which
runs only after `show()`. Without `--e2e` Slint keeps its own event loop. Held by
`a_mac_e2e_run_is_an_accessory_that_never_activates_from_its_first_event` (`crates/flare/src/backend.rs`). Source:
macOS port phase 3 review (2026-11-08).
242. **macOS: every screen is measured in one space, the points times ONE scale.** winit reports a window's position
as points times that window's backing scale, so scaling each screen by its own factor makes a 2x screen's area
overlap the screen beside it. `chrome::areas_in` scales every screen by the same factor; the saved window state is
checked against the areas at its own saved scale (`monitors::work_areas_at`, `WindowStateStore::read_at`), and the
park report measures the screens at the window's scale. On Windows `work_areas_at` is `work_areas` (one device
pixel space already). Held by `screens_of_different_scales_never_overlap` (`crates/flare-win/src/macos/chrome.rs`)
and `the_areas_are_asked_for_in_the_saved_scale` (`crates/flare-win/src/window_state.rs`). Source: macOS port
phase 3 review (2026-11-08).
243. **macOS: the control channel is a Unix socket that only its user can reach, checked on both ends.**
`flare-ctl/src/uds.rs` exports the named pipe's API (`pipe_name`, `Server`, `connect`, `connect_to_server`,
`PipeEnd`) with the same framing and hashes: the socket is `<per-user temp folder>/flare-ctl/<h>[-<s>].sock`, the
folder mode 0700 and the user's own with no group or other bits (else no server), the socket mode 0600. The server
drops a client whose `getpeereid` uid is not its own before reading a byte; the client refuses a server of another
uid, or (with `FLARE_CTL_PID`) any process but the one named, by `LOCAL_PEERPID`, before writing a byte. A live
server on the path fails the start (rule 249), a dead socket is removed, stop removes only the socket it bound. Reads are
`recv(MSG_DONTWAIT)` probes, a stop shuts each socket down to free a write stuck on a client that never reads.
Tests cannot switch users, so the refusals are proven with `Server::start_as` and `connect_as`, each end believing
it is another uid. Held by `crates/flare-ctl/tests/pipe.rs` (the Windows tests run on both, `macos::*` twins of
the naming and access tests), `crates/flare-mcp/tests/stdio.rs` and
`appctl::tests::ctl_socket_tests` (every `show_*`, `set_tab_*`, `clear_tab_*`, `open_page`, `close_panel` and
`convo.state` from `flare_mcp` over the socket to the headless window). Source: macOS port phase 4, step 1
(2026-11-08).
244. **macOS: the port owner table is libproc's, read without a connection, and it sees this user's processes
only.** `flare_proc::tcp_listeners` lists every listening TCP socket of the processes whose descriptors this user
may read (`PROC_PIDLISTFDS`, then `PROC_PIDFDSOCKETINFO`, the `socket_fdinfo` offsets measured with `offsetof` on
the macOS 27.2 SDK and proven by a real listener), never `lsof` and never a connect; `flare_win::tcp_owner` builds
`listener_pids`, `listener_pids_v6` and `tree_look` on it with the Windows rules (one table read for both families,
the tree from `flare_proc::table` with start times, a root's start unknown is no parent). Known gap: a listener of
another account (a root daemon) is not seen, so a port only it holds reads as nobody, never as in the tree, and a
port the tree holds on one loopback family while another account holds the other is caught by rule 250. A
descriptor list that fills its buffer may have been cut, so it is grown and read again (at most five times, then not
trusted): a listener past the cut would make a port look held by fewer processes than hold it. The
Sonara and chip tests run on macOS; their stand-in servers accept with a 10 s deadline, so a post that never comes
fails the test instead of hanging it (the old blocking `accept` hung on macOS while the table was a stub). Held by
`flare-proc` `sockets::tests`, `a_listening_tcp_socket_is_read_with_its_port_and_family`, the `tcp_owner` tests
(now on both systems) and `a_childs_listener_is_in_this_processs_tree_on_macos`, the `sonara` tests and
`sonara_is_never_called_on_the_ui_thread`. Source: macOS port phase 4, step 2 (2026-11-08).
245. **macOS: the taskbar button is the Dock tile.** The badge is the tile's badge label with the disc's own text
("3", "9+"; `OverlayShell::set_badge`, `flare_win::dock::DockShell`), set without a window handle; a bell asks for a
CRITICAL attention request (the icon bounces until Flare is activated, Windows' `FLASHW_TRAY | FLASHW_TIMERNOFG`)
and nothing while Flare is the active app, never an informational one; "the window is in front" is `NSApp isActive`
(`taskbar_flash::is_foreground`, also the web panel's `window_is_foreground`). Every call is main-thread only and
does nothing off it. Tests never make an attention request (an unbundled test app gets a real request id even with
no Dock tile). Held by `dock::tests`, `a_text_badge_gets_the_count_as_the_disc_shows_it_and_a_clear_takes_it` and
`dock_badge_reads_back_and_attention_is_asked_only_while_flare_is_not_active` (`crates/flare-win/tests/appkit.rs`).
Source: macOS port phase 4, step 2 (2026-11-08).
246. **macOS: notifications wait for the signed app, and say so.** The toast path goes through
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
only (`MAC_NOT_IN_APP`, `MAC_UNSIGNED`); the signed app posts real notifications (rule 275).
247. **macOS: an agent is found by its macOS command line too.** Off Windows `flare_agent::detect::classify` also
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
248. **macOS: the Claude plugin's hooks run `/bin/sh hook.sh`, the plugin list is `:`-separated, and the home is
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
249. **macOS: the first control server on a name holds `<socket>.lock`, and a busy one is waited for, never
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
250. **macOS: a port is in the tab's tree only when the loopback family the tree does not hold is shown free.**
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
251. **macOS: the web engine is CEF, pinned to one build, cached outside the repo, and run from a bundle CEF can
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
252. **macOS: a CEF browser process never takes the screen.** `FlareCefApplication` (an `NSApplication` with
`CrAppProtocol`, `CrAppControlProtocol` and `CefAppProtocol`, `isHandlingSendEvent` set around every `sendEvent:`)
is made the shared application before CEF loads, with activation policy Prohibited; a bundle that shows no window of
its own (the probe) is `LSBackgroundOnly`. Every browser is windowless (Alloy style, `on_paint` into a BGRA buffer
in device pixels), in an in-memory request context of its own. Nothing native may open: a popup (`window.open`,
`target=_blank`) is refused, the context menu is emptied and its run cancelled, JavaScript dialogs are suppressed, the
leave-page question is answered "leave", a file picker is cancelled, media and permission prompts are denied, and the
keychain is the mock one (`use-mock-keychain`: the login keychain's "Chromium Safe Storage" prompt is a system
dialog; its key is public, rule 264; only the real window of the Developer ID app has an item of its own, rule 276), no colour or print panel can be ordered in (rule 265); a `<select>` popup is a
painted `PET_POPUP` layer, not a menu. The main thread's pump is AppKit's queued
events, one non-blocking turn of the CFRunLoop and `do_message_loop_work`: without the run loop turn, or with a CEF
log file, a windowless view never paints (measured on macOS 27.2; the log file is for diagnosis only). Held by the
engine tests (`tests/engine_macos.rs`): the window server lists no on-screen window of the bundle's app or helpers
before, during (every 50 ms) and after each run, the process owns 0 windows at its start and end, it is never
active, and no handler was asked for anything native. Source: macOS port phase 5, step 1 (2026-11-08).
253. **macOS: the agent's DevTools calls go to CEF unchanged, and a runaway script is stopped with the page kept.**
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
254. **macOS: the web panel is CEF behind the same `WebBackend`, and nothing waits for it.** `flare_web::host` on macOS
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
255. **macOS: the page is a picture in its box, and the window's input goes to it.** The browser is windowless: its
picture (`on_paint`, BGRA in device pixels, a `<select>` list's popup layer laid over it) is `WebBackend::frame`, which
the window shows in the panel's page box (`panel-web-frame`, `panel-web-live`) after every pump and sync; Windows'
backend has no picture, so the box stays empty and takes no input there. The pointer and the wheel over the box go to
`WebBackend::pointer` in device pixels (CSS pixels at the window's scale in the host; a wheel turned toward the person
scrolls down; a press within 500 ms and 5 px is a double click), the keys to `WebBackend::key` with the Win32 virtual
key, the macOS key code and the text (Slint's key names, `web_live::key_of`; a modifier alone goes nowhere), and every
key carries AppKit's own character, because CEF makes an `NSEvent` of it (without it a Backspace's key up deletes a
second time, measured on CEF 156). The input method's text is `WebBackend::ime`. The key filter decides first, as the
accelerator hook does on Windows; the backend's `KeyPress::ctrl` carries Cmd on a Mac (rule 237), read as the event's
`meta` by `web_ctl::key_event_for` in a Mac key context, so Cmd+T opens a tab and never reaches the page. A press or a
typed key forwarded while an agent works in the page is the person taking it over (the stop-on-input monitor: the
agent's own input is DevTools events and never comes through the box). A still is the picture as a PNG. Held by
`an_off_screen_page_is_drawn_in_its_box_takes_the_input_there_and_a_press_stops_the_agent`,
`on_a_mac_the_backends_ctrl_is_cmd_and_the_app_takes_its_cmd_chords`, `slints_keys_become_the_keys_chromium_knows`,
`a_key_that_types_nothing_carries_appkits_character` and the panel scenario's frame, still, pointer, keys-and-ime,
key-filter, wheel and select-list checks. Source: macOS port phase 5, step 2 (2026-11-08).
256. **macOS: the walls are the Windows walls, decided in one place and re-proved against the network observers.**
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
257. **macOS: nothing native opens from a page, and nothing waits for the person in CEF.** Script dialogs are answered
at once (`alert` returns, `confirm` is false, `prompt` is null) and never shown; camera, microphone, location,
notifications and clipboard read are denied with no prompt; a file picker is cancelled; the context menu is emptied and
its run cancelled; a popup is no window (same view or nothing). A download waits for the person's yes: CEF holds its
before-download callback, nothing is written where the person keeps files while it is held or after a no (its bytes wait
in Flare's staging folder, rule 262), a yes saves it in Downloads under its last name part, never over a file (` (2)` and so on); a bar or a background page refuses it
before it starts. CEF is never called, and no CEF object is let go, while a view's state is locked: releasing a
callback can run CEF's work and its handlers at once (a held download's no deadlocked that way). Held by the panel
scenario's dialogs, permissions, downloads, external-schemes and new-window checks, the walls scenario's plugin-bar
check, and `a_download_is_named_by_its_last_part_and_never_overwrites`. Source: macOS port phase 5, step 2
(2026-11-08).
258. **macOS: the agent drives the web panel through the same `WebBackend::cdp`, and its editing chords edit.**
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
259. **macOS: nothing behind Flare's proxy reaches this machine or its network, in any spelling.** A networked
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
260. **macOS: every mini-app has a private profile of its own.** A CEF profile's request wall for the requests no browser
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
261. **macOS: CEF takes Flare's switches, never the process's command line.** `Settings::command_line_args_disabled`
is 1 and the switches are added in `FlareApp::on_before_command_line_processing` (`engine::SWITCHES`), which CEF still
asks. With it at 0, `Flare --remote-debugging-port=N` (or `open -a Flare --args ...`) opened a DevTools port anyone on
the machine could drive, and `--no-sandbox`, `--proxy-server` and `--disable-web-security` reached the helpers (the
GPU helper was started with `--no-sandbox`; measured). The probe takes its scenario from the environment for this
reason. Held by the quiet scenario's own-switches-only check in
`the_engine_takes_only_flares_switches_and_calls_no_one_on_its_own` (the probe is started with all four switches: the
port does not answer, there is no `DevToolsActivePort`, no helper has any of them; the check fails with the setting
at 0). Source: macOS port phase 5 review (2026-11-08).
262. **macOS: a held download's bytes wait in Flare's staging folder, never in `~/Downloads`.** Chromium writes a
download's first part while CEF holds the before-download callback, into the profile's default download folder,
which is `~/Downloads`: a held download left `.<bundle id>.XXXXXX` there before the person's answer (and the tests
wrote into the real Downloads folder). Every profile's `download.default_directory` and `savefile.default_directory`
are `<data_dir>/download-staging` (`walls::download_staging`), set with the walls before a browser of it is made; the
folder is emptied when the engine starts (what a run left there is no one's download). A yes moves the file to
Downloads (with `com.apple.quarantine`, Chromium's own, held by the panel scenario's download-quarantine check); a no removes it. Held by the panel scenario's
download-staging check (a slow download held: its part is in the staging folder, nothing new in `~/Downloads`, nothing
left after the no; the profile's preference read back) and `every_kind_of_view_has_the_browser_windows_gives_it`.
Source: macOS port phase 5 review (2026-11-08).
263. **macOS: the engine calls no one on its own.** With only a sealed mini-app open, CEF 156 connected to Google
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
264. **macOS: a profile on disk is protected by its file's permissions only, until the signed app (accepted gap).**
CEF runs with `use-mock-keychain` so Chromium never asks the login keychain for "Chromium Safe Storage" (a system
prompt on the screen, asked again after every ad-hoc rebuild). The mock keychain's key is fixed and public
(`mock_password` is in the framework), so what the web panel's profile (`<data_dir>/Default`) and each mod's and
plugin's keep on disk (cookies, storage) is readable by anything that can read the file, a backup included; Windows
encrypts it with DPAPI. The mini-apps' profiles are in memory and keep nothing. Since phase 8 step 2 the gap is
narrowed to builds that are not the Developer ID app (a dev or ad-hoc bundle, a test, a probe): the real window of
the signed `Flare.app` uses a keychain item of its own (rule 276). For those builds this gap is written here, in
`engine.rs` and in `docs/parity.md`, and no one may describe their macOS profile as encrypted.
Source: macOS port phase 5 review (2026-11-08).
265. **macOS: no colour or print panel can come on screen from a page.** A windowless browser has no window for a
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
266. **macOS: a popup or new-tab request loads only a web address the navigation rule allows.** CEF's `on_before_popup`
and `on_open_urlfrom_tab` load the target into the view's own main frame (`NewWindows::SameView`). `load_url` of a
`javascript:` address runs it in the top frame without a navigation, so the browse wall never sees it: a cross-origin
frame could run script in the page above it. `popup_target_allowed` lets only `http`/`https` through, and only what the
view's `NavRule` allows for the main frame; `data:`, `file:`, `chrome:`, `devtools:`, `blob:` and every other scheme
are dropped. Held by `a_popup_loads_only_a_web_address_the_navigation_rule_allows`. Source: commit security review of
phase 5 (2026-11-08).
267. **macOS: each program of `Flare.app` carries exactly the entitlements it is given, and never the broad ones.**
The hardened runtime is on for every Mach-O of a Developer ID signature (`flare_cef::macos::sign`). The keys are
Chromium 156.0.8078.48's own (`chrome/app/*-entitlements.plist`) minus its App Sandbox keys, plus a terminal's:
`Flare` has `device.audio-input`, `device.camera` and `automation.apple-events` (rule 273); `Flare Helper (Renderer)`
has `cs.allow-jit` (V8); `Flare Helper (GPU)` has `cs.allow-jit` (SwiftShader's software GL, Chromium's
`helper-gpu-entitlements.plist`, read 2026-11-08); `Flare Helper`, `(Plugin)`, `(Alerts)` and `flare-mcp` have none
(no Pepper in CEF 156). Never `allow-unsigned-executable-memory`, `disable-library-validation`, `get-task-allow` or the
dyld environment key. Each key is written with why it is there, in the entitlements file itself. A signed run of the
engine probes under the hardened runtime is how a key is proved (`FLARE_TEST_SIGN_IDENTITY` with
`FLARE_TEST_SIGN_KEYCHAIN`, rule 271); until one has run, the keys stand on Chromium's files. Held by
`each_program_carries_chromiums_keys_for_it_plus_the_terminals_and_never_the_broad_ones`. Source: macOS port phase 8,
step 1 and its review (2026-11-08).
268. **macOS: `Contents/MacOS` holds only code; everything else `Flare.app` ships is in `Contents/Resources`.** A
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
269. **macOS: a release is signed inside out with the Developer ID and checked Mach-O by Mach-O.**
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
270. **macOS: the signing key and the notary credentials never enter the repository, and the Mac build does not
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
271. **macOS: `codesign` never runs with a Developer ID unless the key can be used without a prompt, and an
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
272. **macOS: every TCC entitlement of the app has its usage string.** `Flare.app`'s `Info.plist` carries
`NSMicrophoneUsageDescription`, `NSCameraUsageDescription` and `NSAppleEventsUsageDescription`, one for each key of
rule 273 (`sign::Entitlement::usage_key`): without the string macOS refuses the request even with the key. Held by
`the_app_plist_names_the_app_its_version_its_mac_its_category_and_why_it_wants_the_mic`. Source: review of phase 8
step 1 (2026-11-08).
273. **macOS: the app's TCC keys are a terminal's, and a library loaded later is signed by the team, never let in by
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
274. **macOS: the bundle that is notarized and checked is the one that ships in the update package.** `vpk pack` for
osx adds its update program and `sq.version` to `Contents/MacOS`, which breaks a seal made before it. So the Velopack
step signs again after packing what vpk produced (`flare-mac-app resign`, the roles of rule 269, vpk's program as a
tool with no entitlements), or uses vpk's own signing only if it gives each program its own entitlements; then it
notarizes and staples that bundle, and runs `flare-mac-app verify`, `codesign --verify --deep --strict`,
`stapler validate` and `spctl -a -t exec` on the bundle extracted from the `.nupkg` and from the full update package,
not on the bundle before packing. Phase 8 is done only when such a signed, notarized release updates itself. Source:
review of phase 8 step 1 (2026-11-08).
275. **macOS: notifications go through `UNUserNotificationCenter` only from Flare's own signed `Flare.app`, ask once,
lazily, and a click steers only this run's.** `flare_win::toast::system_unavailable` reads, once a run, the main
bundle and the process's own signature (`notify::bundle_facts`: `NSBundle` and `SecCodeCheckValidity` against
`identifier "com.flare.terminal" and info [CFBundleIdentifier] = "com.flare.terminal"`). That read can take a second
on a cold start (measured 2026-11-09: about 0.5 s each for `NSBundle` and the signature, nearly 3 s under a loaded
test run), so it runs on a worker (`toast::warm_up` at install) and the UI thread only asks
`toast::known_unavailable`, which never blocks; until the answer is known a question goes to the worker, which reads
it and answers through `on_done`. A process that is not a `*.app` with Flare's id answers `MAC_NOT_IN_APP`
(a `cargo run`, a test binary; the center would raise an Objective-C exception there and abort Flare), a bundle the
linker signed but nobody sealed answers `MAC_UNSIGNED`, and the caller bounces the Dock as before (rule 246). In the
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
276. **macOS: the Developer ID app keeps its profiles' key in a keychain item of its own, and nothing else touches
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
a probe) keeps the mock keychain (rule 264). A dev build and the Developer ID app share the default profile folder,
so a switch between the two keys would make the cookies unreadable; rule 282 sets them aside instead. Held by
`only_the_real_window_of_the_developer_id_app_uses_a_keychain_item_of_its_own` (the switch list, the item's name, the
requirement's team, id and both Developer ID OIDs, and that a test binary is not Flare's Developer ID). The item made on a signed run is still to be seen by hand: no test may touch the keychain. Source: macOS port
phase 8, step 2 (2026-11-09); the CEF field is in `cef-dll-sys` 156.2.0's bindings.
277. **macOS: the bundle `vpk pack` zips is signed again by role after vpk has added its updater, through
`flare-mac-app` as vpk's `codesign`.** vpk 1.2.161 for osx copies `UpdateMac` into `Contents/MacOS`, writes
`sq.version` into `Contents/Resources` with a link in `Contents/MacOS`, signs, notarizes and staples, and only then
makes the update package and the portable zip (`OsxPackCommandRunner`, read 2026-11-09 at commit 92d6a1c). Its own
signing gives `UpdateMac` Velopack's entitlements (`allow-jit`, `allow-unsigned-executable-memory`,
`disable-library-validation`, the dyld key: rule 267 forbids them) and the app one file for every program; unsigned,
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
278. **macOS: an update swaps the whole bundle, and that is proved on a Mac headless, against the real feed and
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
279. **macOS: the Mac release is a workflow of its own after the Windows one, from secrets only, and skips
cleanly.** `.github/workflows/macos-app.yml` runs on `workflow_run` of `release` (completed, success, `main`) and by hand.
After a release it builds only when the five secrets are set and `v<version>` exists without `releases.osx.json`
(and only the commit that release was tagged at, rule 280), else it writes a notice and stops with success; then `make-app.sh` (`FLARE_NOTARIZE=0`),
`pack.sh` (rule 277) and `vpk upload github --channel osx --merge` plus the DMG into that same release, and checks
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
280. **macOS: the Mac build added to a release is the commit that release was tagged at, made once, and signed only
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
281. **macOS: no signing key, password or scratch folder outlives its one use, and a script removes only what it
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
282. **macOS: a change of the profiles' key sets the old key's cookies aside; it never lets Chromium drop them.** A
dev or ad-hoc build uses the mock keychain's key and the Developer ID app Flare's own item (rule 276), both on the
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
283. **macOS: ad hoc signing gives each program its role too, so the ad-hoc pack proves the updater's signature.** An
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
