# Test port map

Every Prism Terminal v0.55.6 unit test file, the Flare crate that ports it, or `dropped` with the reason.
Sorted by crate. "port (split)" means the file is shared between crates, see the note. Paths are
relative to the Prism Terminal export. Plain tests of Electron plumbing are dropped; their rules are
re-expressed natively and listed in `docs/regression-rules.md`.

| Crate | PT test file | Action | Note |
|---|---|---|---|
| flare | `core/renderer/components/Modal.test.ts` | port | one modal frame rules; the aria-modal source scan is dropped |
| flare | `core/renderer/components/TabMark.test.ts` | port (partial) | mark assertions as render-model tests; DOM attribute and CSS class checks dropped |
| flare | `core/renderer/lib/fitHold.test.ts` | port | hold the terminal size while the sidebar animates |
| flare | `core/renderer/lib/hiddenTitle.test.ts` | port | tooltip only when text is cut or the label is hidden |
| flare | `core/renderer/lib/tabMark.test.ts` | port | which mark a tab wears for each indicator, state, front or back, rainbow |
| flare | `core/renderer/lib/tipPlace.test.ts` | port | tooltip placement: centred under, inside right edge, above, beside and flip |
| flare | `core/renderer/styles/marks.test.ts` | dropped | CSS stylesheet test; its rules (every motion has a rule, reduced motion stills all, minimised pauses, transform-only motion) are re-pinned as Slint animation tests |
| flare | `src/main/closeFlow.test.ts` | port | window close question state: veto, go-ahead, quit resume, install pre-agree |
| flare | `src/main/e2eOnly.test.ts` | port | every test-hook registration sits behind the test flag (re-expressed as a Rust test) |
| flare | `src/main/planRestore.test.ts` | port | restore plan: folder gone, active index, claude newest and second newest, codex marker, no-agent no-resume |
| flare | `src/main/tabsStore.test.ts` | port | tabs.json shape, cap 50, debounce, flush, pending load |
| flare | `src/renderer/src/components/modals.test.ts` | dropped | React source scan "declares no modal of its own"; kept as a review check that every dialog is its own module |
| flare | `src/renderer/src/components/tabParts.test.ts` | dropped | reads TabStrip.tsx and TabSidebar.tsx source text; Slint has one tab component set, the rule is kept as a design rule |
| flare | `src/renderer/src/components/termMenu.test.ts` | port | terminal menu rows fit what was clicked, Command help last, no Close tab |
| flare | `src/renderer/src/lib/hoverReveal.test.ts` | port | open delay 60 ms, close grace 100 ms, hold |
| flare | `src/renderer/src/lib/keymap.test.ts` | port | chord table, labels, agent hotkeys, vertical-only sidebar key, F1 and Ctrl+F rules, agent guard keys |
| flare | `src/renderer/src/lib/restorePlan.test.ts` | port | peek placeholders and settleRestore |
| flare | `src/renderer/src/lib/revealZones.test.ts` | port | sidebar reveal zone |
| flare | `src/renderer/src/lib/sidebarRows.test.ts` | port | folder line and find-a-tab word matching |
| flare | `src/renderer/src/lib/stripPlan.test.ts` | port | three-step overflow, front tab never hidden |
| flare | `src/renderer/src/lib/tabDrop.test.ts` | port | drop slot from the carried tab's leading edge |
| flare | `src/renderer/src/lib/tabMru.test.ts` | port | most-recent list and the Ctrl hold walk |
| flare | `src/renderer/src/lib/tabs.test.ts` | port | tab list model: add, close handover, step wrap, setCwd, labels, settings toggle, reorder |
| flare | `src/shared/channels.test.ts` | dropped | Electron IPC channel table parity; Flare has typed channels checked by the compiler |
| flare-agent | `core/main/agentDetect.test.ts` | port | signatures, first-token rule, dev-folder rule, tree walk with injected rows |
| flare-agent | `core/main/agentKinds.test.ts` | port | resumable kinds list |
| flare-agent | `core/main/agentPoll.test.ts` | port | back-off, failure back-off, forced-look gap, ask-again, forget; injected pids, ticks, query and clock |
| flare-agent | `core/main/agentResume.test.ts` | port | validResume, claude store per folder, interactive head, async walk; temp home folder |
| flare-agent | `core/main/sonaraLink.test.ts` | port | SONARA_HOME / LOCALAPPDATA resolution, POST to a local fake, quiet failure |
| flare-agent | `core/renderer/lib/agentArm.test.ts` | port | arm and disarm by resume, title, poll, prompt; doubted resume; WSL title-armed case |
| flare-agent | `core/renderer/lib/agentBackground.test.ts` | port | screen fixtures from agentBackground.screens.json |
| flare-agent | `core/renderer/lib/agentClock.test.ts` | port | clock reconcile and humanFor wording |
| flare-agent | `core/renderer/lib/agentClose.test.ts` | port | close question rule (plain shell silent, agent asks, window held only while working) and dialog texts |
| flare-agent | `core/renderer/lib/agentHookSignal.test.ts` | port | parseAgentSignal, failedLabel, manifest and hooks.json table reads of Flare's flare-status folder |
| flare-agent | `core/renderer/lib/agentHookState.test.ts` | port | hookStep rules, raisedWhileSeen, settle and background steps |
| flare-agent | `core/renderer/lib/agentHookState.timelines.test.ts` | port | measured timelines A, B, C, C3, D, E, G2 replayed through hookStep (fixtures copied from PT) |
| flare-agent | `core/renderer/lib/agentIcon.test.ts` | port | iconKind pure here; Clawd pixel layout in flare (Slint) |
| flare-agent | `core/renderer/lib/agentQuestion.test.ts` | port | looksLikeQuestion on screen fixtures, answersQuestion, questionAnswered |
| flare-agent | `core/renderer/lib/agentTitle.test.ts` | port | Claude and Codex title dialects, Codex asking, titleAgent, titleArmsAgent |
| flare-agent | `core/renderer/lib/resumeReveal.test.ts` | port | clear and reveal rules, folder-named-Claude rule, ceiling |
| flare-agent | `core/renderer/lib/termActivity.born.test.ts` | port | startup paint ends at first silence, typing ends birth |
| flare-agent | `core/renderer/lib/termActivity.test.ts` | port | input echo window, suppression, looksTyped, idleAtPrompt, forgetSession; injected clock |
| flare-agent | `core/renderer/questionWatch.test.ts` | port | change-only reports, settle delay, scrollback reads, dispose; xterm slice becomes a grid trait |
| flare-agent | `core/renderer/questionWidget.test.ts` | port | extractQuestion, wrapped and multi-select questions, review, stale frame |
| flare-agent | `src/renderer/src/lib/taskbarBadge.test.ts` | port (split) | attentionCount and badgeText in flare-agent; badgePixels and overlay in flare-win; the taskbar switch in flare-settings |
| flare-diag | `core/main/crashHooks.test.ts` | port | observe-only exception and rejection logging, 10 s repeat, gone line, listener removal; panic hook and exit observers |
| flare-diag | `core/main/diagIpc.test.ts` | port (partial) | pageLine allowlist, batch cap, -slow rule, verbose-only kinds, openFolder limited to the log dir; IPC plumbing dropped |
| flare-diag | `core/main/diagLog.test.ts` | port | batching, rotation at 2 MB keeping 4, queue cap, logger-error once, failed rotation retry, flushSync |
| flare-diag | `core/main/diagSummary.test.ts` | port | string and stack caps, array and object summaries, opaque args, error fields |
| flare-diag | `core/main/diagnostics.test.ts` | port | no log folder logs nothing; session and quit lines |
| flare-diag | `core/main/ipcTiming.test.ts` | port (partial) | slow over 500 ms, sync over 100 ms, LONG_WAIT, opaque channels as a UI-thread work-item timer; Electron ipcMain wrapper dropped |
| flare-diag | `core/main/memWatch.test.ts` | port | thresholds 2048/1024/2048 MB, doubling, 5-minute memory crumb |
| flare-diag | `core/main/stallWatch.test.ts` | port | 50 ms tick lag, 500 ms beat, page-stack after 2 s, canary fs-slow, suspend and resume; native UI-thread watchdog |
| flare-diag | `core/renderer/lib/agentDiag.test.ts` | port | hook fold, title meaning-only-on-change, mark log with rules, line cap, privacy of logged fields |
| flare-diag | `core/renderer/lib/diag.test.ts` | port (partial) | crumb ring, crumbsBefore, errorLine, stall line shape; shortSrc and LoAF parts dropped (no web page) |
| flare-diag | `core/shared/diagGate.test.ts` | port | once per 10 s with repeats, budget 10 per kind, sweep |
| flare-dictation | `core/main/dictationEngine.test.ts` | port | whisper-server and parakeet-cli handling, stand-down, queues (41 tests); Node fake fixtures become Rust test doubles |
| flare-dictation | `core/main/dictationStore.test.ts` | port | downloads, verify, pack install, remove (27 tests); the Expand-Archive test becomes a real unzip test |
| flare-dictation | `core/main/mediaPause.test.ts` | port (partial) | 15 of 18 tests (lifecycle, token, timeouts, numbering, dispose); 3 PowerShell script-shape tests dropped; the never-the-play-pause-key rule kept as a native test |
| flare-dictation | `core/renderer/lib/dictation.test.ts` | port | controller pastes once, Escape while transcribing drops the clip (2 tests) |
| flare-dictation | `core/renderer/lib/dictationClean.test.ts` | port | pure text cleanup (26 tests) |
| flare-dictation | `core/renderer/lib/dictationKey.test.ts` | port | pure hotkey reducer, parse and format (55 tests); DOM inputs become Flare's native key event struct |
| flare-dictation | `core/renderer/lib/wav.test.ts` | port | WAV encode, tail window, downsample, rms (22 tests) |
| flare-dictation | `core/shared/dictationCatalog.test.ts` | port | catalogue entries, pins, languages; the fetch-whisper.mjs text reads become tests against Flare's pin file |
| flare-render | `core/renderer/lib/linkInkRows.test.ts` | dropped | xterm DOM decoration loss on redraw; Flare draws the grid every frame so no row can lose its link ink |
| flare-render | `core/renderer/lib/linkScanPlan.test.ts` | port | sliceRows 8 ms budget, liveRange, resume after trim, cancel |
| flare-render | `core/renderer/lib/lookApply.test.ts` | port | changed-only look apply, once per frame |
| flare-render | `core/renderer/lib/termLinkPaint.test.ts` | port | painter rules; DOM decorations become grid colour overlays |
| flare-render | `src/renderer/src/lib/devicePixels.test.ts` | port | edges end on a whole device pixel at any scale |
| flare-settings | `core/main/atomicWrite.test.ts` | port | no temp file left, create new, save off-thread, newest of a burst wins, saveNow on disk at return |
| flare-settings | `core/renderer/host.test.ts` | port (split) | see-through slider cases go to flare-settings and flare-theme; the host-seam and Prism-style cases are dropped |
| flare-settings | `core/renderer/lib/colourFormat.test.ts` | port | HEX/RGBA/HSLA cycle, junk reads HEX, remembered choice |
| flare-settings | `core/renderer/lib/helpPrefs.test.ts` | port | help on by default, shell choice, width default and clamp, listeners |
| flare-settings | `core/renderer/lib/prefStore.test.ts` | port | enum and bool prefs, defaults, byte-for-byte keys, listeners, blocked store |
| flare-settings | `core/renderer/lib/recentRoots.pins.test.ts` | port | pin toggle, order, pins outlive history |
| flare-settings | `core/renderer/lib/recentRoots.test.ts` | port | cap 12, dedupe, parent labels, storage nonsense |
| flare-settings | `core/renderer/lib/seeThrough.test.ts` | port | default levels 0xb9 dark and 0xd1 light, light by luminance; used by the PT import |
| flare-settings | `core/renderer/lib/termLook.test.ts` | port (split) | prefs, font steps, custom parse, indicator, colours with alpha, applyCustomExtras, rainbow here; ground-alpha cases in flare-theme; Prism host-config branches dropped; duplicate "colours with alpha" block merged |
| flare-settings | `core/renderer/lib/termPrefs.test.ts` | port | saved shell id, undefined until chosen, in the Flare settings file |
| flare-settings | `core/renderer/settings/fields.test.ts` | port | colour commit rules: nothing typed, same colour, typed alpha |
| flare-settings | `core/renderer/settings/layout/search.test.ts` | port | word-prefix match, folding, label-first ranking, hidden keywords |
| flare-settings | `core/renderer/settings/neutralControls.test.ts` | port (rewritten) | asserts the Slint style constants (only Save and on-switch use the accent); the Prism source-scan form is dropped |
| flare-settings | `core/renderer/settings/options.test.ts` | port | list parity, section order, storage keys, copy rules, flat entries as tests over the row tables |
| flare-settings | `core/renderer/settings/sections/opts.test.ts` | port | see-through row name, Find words the row as the page does, Save title |
| flare-settings | `core/renderer/settings/theme/useTermSetup.test.ts` | port | dirty state, see-through level in Save, picked background never lights Save, editor opens on opaque ground |
| flare-settings | `core/shared/help/catalogue.test.ts` | port | ids, categories, task and summary shape, one-line ASCII commands, placeholders, keywords, danger lines, key-press detection, shell filtering, blankless, row names, real questions, speed |
| flare-settings | `core/shared/help/search.test.ts` | port | tokenise, stop words, stems, synonyms, one-slip typos, ranking, ties, empty query, cost bound |
| flare-settings | `core/shared/settingsCopy.test.ts` | port | symbol rule, key-name rule, ternary and template descriptions, eight-word limit |
| flare-settings | `src/renderer/src/components/settings/appOptions.test.ts` | port | unique ids, known icon/page/section, literal row ids, order, GPU gating, no Tab style row |
| flare-settings | `src/renderer/src/components/settings/optionKeys.test.ts` | port | every storage key is one its module stores under |
| flare-settings | `src/renderer/src/components/settings/settingsCopy.test.ts` | port | app settings copy and subtext length |
| flare-settings | `src/renderer/src/lib/accentPrefs.test.ts` | port | window accent store, hex6 or hex8, Reset forgets |
| flare-settings | `src/renderer/src/lib/backgroundPrefs.test.ts` | port | window background store, opaque, Reset forgets |
| flare-settings | `src/renderer/src/lib/customWindowColours.test.ts` | port | window colours kept with Custom |
| flare-settings | `src/renderer/src/lib/edgesPrefs.test.ts` | port | edges store, validation, change listener |
| flare-settings | `src/renderer/src/lib/newTabPrefs.test.ts` | port | new-tab mode and folder persistence |
| flare-settings | `src/renderer/src/lib/opacityMigration.test.ts` | port | one-time import of PT Opacity N into the alpha byte |
| flare-settings | `src/renderer/src/lib/seeThroughMigration.test.ts` | port | one-time import of PT switch and alpha into a see-through level |
| flare-settings | `src/renderer/src/lib/sidebarPrefs.test.ts` | port | open or collapsed, width 150 to 420, initial 220, slide flag |
| flare-settings | `src/renderer/src/lib/tabLayoutPrefs.test.ts` | port | horizontal default, unknown reads horizontal, listeners |
| flare-settings | `src/renderer/src/lib/tabSwitchPrefs.test.ts` | port | In order default, unknown reads In order, listeners |
| flare-shells | `core/main/shells.test.ts` | port | parseWslList, shellById, pwsh path discovery, System32 tool paths |
| flare-shells | `core/main/termPrompt.test.ts` | port | OSC 9;9 prompt hook, cmd PROMPT, PS file style (test the emitted strings) |
| flare-shells | `core/main/terminal.test.ts` | port (split) | ptyEnv and withResume groups in flare-shells; OutputBatcher and pollExitCode groups in flare-term; plugin-dir group in flare-shells (`pty_env` takes it); isOurPlugin in flare-agent |
| flare-shells | `core/renderer/lib/termPaste.test.ts` | port (split) | decidePaste, sanitizePaste, quotePath here; imagePasteKey and newlineKey groups in flare-agent |
| flare-shells | `core/shared/psQuote.test.ts` | port | PowerShell single-quote helper |
| flare-shells | `core/shared/termCwd.test.ts` | port | parseOsc9, decideFollow, cdCommand quoting |
| flare-term | `core/main/termPathOpen.test.ts` | port (split) | resolveTermPath, pathKinds, the stat meter and openTermPath here; openFolderDefault in flare-win |
| flare-term | `core/main/terminal.sessions.test.ts` | port | kill-wait for shells, pending spawn, warm shell eviction |
| flare-term | `core/renderer/lib/appCopyNotice.test.ts` | port | program-selection copy notice |
| flare-term | `core/renderer/lib/termCaretHold.test.ts` | port | caret-hold logic; the text input rectangle itself is flare (Slint IME) |
| flare-term | `core/renderer/lib/termCells.test.ts` | port | cellText, cellFrom, logicalLine, charAtCell, scratch-cell reads |
| flare-term | `core/renderer/lib/termClickCaret.test.ts` | port | caretClickAllowed, caretDelta, arrowKeys |
| flare-term | `core/renderer/lib/termKeys.test.ts` | port | key decision order, physical-key matching pending decision O2 |
| flare-term | `core/renderer/lib/termLinks.test.ts` | port (split) | findLinks here; the linkColor group goes to flare-theme (4.5:1 floor) |
| flare-term | `core/renderer/lib/termOsc52.test.ts` | port | write-only parse, base64 and UTF-8 checks, 1 MB cap; the clipboard write goes through flare-win |
| flare-term | `core/renderer/lib/termOsc8.test.ts` | port | parseOsc8, isWebLink, osc8Target, Osc8Spans follow and cap |
| flare-term | `core/renderer/lib/termPathLinks.test.ts` | port | path cache, asked-line bookkeeping, relative resolution, 300-path pieces; the repaint wake-up maps to flare-render |
| flare-term | `core/renderer/lib/termPaths.test.ts` | port | findPaths candidate rules |
| flare-term | `core/renderer/lib/termReplies.test.ts` | port | bell gate (1000 ms) and answered-only-when-asked replies; theme-push and redraw-nudge groups belong to flare-theme |
| flare-term | `core/renderer/lib/termSelectionEdit.test.ts` | port | selectionDeleteAllowed, selectionDeleteKeys, linkAt |
| flare-term | `core/shared/termVersion.test.ts` | port | version constants match the crate and renderer versions |
| flare-term | `core/shared/webUrl.test.ts` | port | isWebUrl for OSC 8 and link opening |
| flare-term | `src/main/nodePtyPin.test.ts` | dropped | node-pty pin; Flare pins portable-pty and the bundled conpty.dll in Cargo.lock and adds a test for the bundled DLL instead |
| flare-theme | `core/renderer/lib/colour.test.ts` | port (split) | colour maths, legibility, alpha steps here; colourCommit, alphaPct, snapAlphaStep in flare-settings |
| flare-theme | `core/renderer/lib/ground.test.ts` | port | DEFAULT_GROUND, prism preset equality, ground constants |
| flare-theme | `core/renderer/lib/markColours.test.ts` | port | rainbow seven, floorMark on light and dark grounds, unreachable floor, gradients, opaqueOver |
| flare-theme | `core/renderer/lib/markPalette.test.ts` | port | palette lines, ink, fills, rainbow fill, loops |
| flare-theme | `core/renderer/lib/nameInk.test.ts` | port | NAME_FLIP name ink, consumed by tab names |
| flare-theme | `core/renderer/lib/switchKnob.test.ts` | port | knob black on dark, white on light, flip under 2:1 |
| flare-theme | `core/renderer/lib/termAnsi.test.ts` | port | normalizeColor, ensureContrast, deriveAnsi, legiblePalette, contrastRatio |
| flare-theme | `core/renderer/lib/termGround.test.ts` | port | onGround floors text 4.5, cursor and sixteen 3, derived vs chosen selection |
| flare-theme | `core/renderer/lib/termTheme.legible.test.ts` | port | every preset and Custom at alpha 0.1 to 0.9 reads |
| flare-theme | `core/renderer/lib/termTheme.snapshot.test.ts` | port | golden fixtures of every preset resolved, opaque Custom, picked-ground variants; regenerate only on a deliberate look change |
| flare-theme | `core/renderer/lib/termTheme.test.ts` | port | preset and Custom resolution, alpha selection, renderer value shape |
| flare-theme | `core/renderer/lib/termThemeRetired.test.ts` | port | retired id map, kept ids, wearer resolves to replacement |
| flare-theme | `core/renderer/lib/themeDraft.test.ts` | port | draft worn while open, Cancel drops it, origin accent kept on Custom |
| flare-theme | `core/renderer/lib/themeOrder.test.ts` | port | achromatic, neutral-then-coloured black to white, stable ties, 0.001 gap guard |
| flare-theme | `src/renderer/src/lib/accentLift.test.ts` | port | near-black accent lifted to a visible red |
| flare-theme | `src/renderer/src/lib/agentColors.test.ts` | port | working = accent, finished, question and failed colours per theme preset; marksApart |
| flare-theme | `src/renderer/src/lib/chromeTheme.opacity.test.ts` | port | ground alpha as a byte, opacity migration snapshot values (case list not enumerated; read the file) |
| flare-theme | `src/renderer/src/lib/chromeTheme.test.ts` | port | 21 cases: chromeTokens, edges, see-through accent, custom floors, secondary, light controls, switch knob |
| flare-theme | `src/renderer/src/lib/sidebarGround.test.ts` | port | sidebar ground one OKLab step darker, black kept (case list not enumerated; read the file) |
| flare-theme | `src/renderer/src/lib/switchAccent.test.ts` | port | on switch fill is the accent, knob rule (case list not enumerated; read the file) |
| flare-theme | `src/shared/windowEdges.test.ts` | port | four words weakest to strongest, none has no border, default hairline, unknown reads hairline |
| flare-update | `core/main/updatePreview.test.ts` | port | flag detection, next-minor version, sample notes, preview progress and cancel timing (12 tests) |
| flare-update | `core/renderer/lib/updateFlow.test.ts` | port | the pure reducer, every action and phase (28 tests) |
| flare-update | `core/shared/releaseGroups.test.ts` | port | section order, prefix classification, capitalisation, (#N) removal |
| flare-update | `core/shared/releaseNotes.test.ts` | port | parser caps, stripping, author tail, PR number, New Contributors, Full Changelog (31 tests) |
| flare-update | `src/main/update.test.ts` | port | newerVersion, installer-name anchors, release-asset URL guard, latest-release parse; re-pointed to Flare and Velopack |
| flare-update | `src/main/updateGate.test.ts` | port | offer-only install, mock to preview, one run at a time, cancel |
| flare-update | `src/main/updateHandoff.test.ts` | port (partial) | relaunch with profile arg and temp cleanup ported; stable-copy cases dropped (no stable copy in Flare) |
| flare-win | `core/main/ipc.clipboard.test.ts` | port | exact text, refuse non-string, empty or over cap, bell calls host attention once; IPC wrapper dropped |
| flare-win | `src/main/argv.test.ts` | port | argv parsing, relative paths, drive-root quote, ARGV_MAX, own-entry skip by identity, switches skipped |
| flare-win | `src/main/bellFlash.test.ts` | port | flash only when unfocused, stop on focus, handler registered once, no window is a no-op |
| flare-win | `src/main/dwmHelper.test.ts` | port (partial) | COLORREF arithmetic and handle read ported; the "-Command not -EncodedCommand" launch test dropped (Flare calls DWM in process) |
| flare-win | `src/main/e2eClipboard.test.ts` | port | in-memory clipboard stand-in under the test flag, re-expressed as a trait |
| flare-win | `src/main/edgeWatch.test.ts` | port | cursor reading at the window edge, far-cursor watch for reveal zones |
| flare-win | `src/main/material.test.ts` | port | Windows 11 build check, validBg, isLightBg for frost; the Electron mock is dropped |
| flare-win | `src/main/shellVerb.test.ts` | port | HKCU keys, %1 vs %V, label by REG_SZ, pointsAt; key name and label per decision O4 |
| flare-win | `src/main/shellVerbParity.test.ts` | port | uninstaller deletes every key the app writes, checked against Flare's installer script |
| flare-win | `src/main/shellVerbRelabel.test.ts` | port | stale-label detection and label-only rewrite |
| flare-win | `src/main/verbSwitch.test.ts` | port | default on, off marker, reconcile, dev/test write gate, boolean guard |
| flare-win | `src/main/windowEdge.test.ts` | port | DWM border step and edges setting; pure edgeFor beside the border call |
| flare-win | `src/main/windowState.test.ts` | port | window.json parse, defaults, minimum size, off-screen position drop |
| flare-win | `src/renderer/src/lib/marksPause.test.ts` | port | minimise sets the pause flag, restore clears it; DOM attribute check dropped |

Totals: 166 test files mapped, 160 ported (some in part or split), 6 dropped.

Not unit tests, handled elsewhere: `tools/e2e/run.mjs` (Electron/Playwright driver dropped as code, its 73 scenarios are
ported as the native e2e, see the e2e section of `docs/parity.md`); `tools/diag.mjs` (reader, re-created in `flare-diag`);
`tools/make-icon.py` (icon generator, build tooling); view files with no PT test (HelpPanel, DictationPage, AboutPage,
UpdateChip, TitleBar) are covered by their e2e scenario in `docs/parity.md`.
Tests for `tabMark.test.ts` and `TabMark.test.ts` in flare cover the mark spec; `core/renderer/components/TabMark.test.ts` DOM checks are dropped.

## M2 ledger

228 Prism Terminal tests are ported here. "Deferred" lines are the groups `test-port.md` assigns to another crate.

| PT file | Tests ported | Flare module (the number in brackets is the Task) | Ported as | Deferred (to) |
|---|---|---|---|---|
| `core/main/shells.test.ts` | 9 | `list.rs`, `sys.rs` (2) | `parse_wsl_list_*` (3), `shell_by_id_*` (3), `every_windows_tool_lives_under_system32_never_a_bare_name`, `pwsh_path_skips_the_current_directory`, `pwsh_path_is_none_when_where_finds_nothing` | none |
| `core/main/termPrompt.test.ts` | 4 | `prompt.rs` (3), `env.rs` (6) | `ps_prompt_hook_wraps_once_and_reports_the_filesystem_folder`, `cmd_prompt_goes_in_front_of_the_existing_prompt`, `ps_file_style_paints_directories_bold_blue_text`, `prompt_reaches_cmd_through_its_environment_only` | none |
| `core/main/terminal.test.ts` | 27 of 29 | `batcher.rs`, `exit_poll.rs` (13), `env.rs`, `resume.rs` (6) | `coalesces_chunks_and_flushes_once_per_window`, `flush_empties_at_once_and_is_idempotent`, `polls_fast_then_slow_says_stuck_once_and_hears_a_late_exit`, 10 `ptyEnv` tests (`names_the_tab_and_never_keeps_an_inherited_one` ... `passes_everything_else_through`), 9 plugin tests (`plugin_dir_added_when_on` ... `with_plugin_dir_drops_empty_entries`), 5 `withResume` tests | `isOurPlugin` x2 (flare-agent, M6) |
| `core/main/terminal.sessions.test.ts` | 7 | `session.rs` (21) | `shells_gone_*` (3), `a_tab_closed_while_its_shell_starts_starts_no_shell`, `a_second_spawn_for_a_pending_id_is_refused`, `closing_an_adopted_tab_keeps_the_next_warm_shell`, `a_warm_shell_with_the_other_plugin_answer_makes_way` | none |
| `core/main/termPathOpen.test.ts` | 10 of 12 | `path_open.rs` (15) | `resolve_against_the_shell_folder`, `resolve_refuses_unc_nul_and_no_folder`, `path_kinds_says_what_exists`, `open_a_file_and_a_folder`, `a_runnable_file_is_revealed_never_run`, `reveal_on_request_and_nothing_for_missing`, `is_runnable_knows_programs_not_documents`, `a_folder_uses_open_folder_a_file_never_does`, `never_more_than_three_stats_in_flight`, `a_hanging_drive_answers_nothing_for_a_minute` | `openFolderDefault` x2 (flare-win, M9) |
| `core/renderer/lib/appCopyNotice.test.ts` | 9 | `app_copy.rs` (12) | 9 tests of Task 12 | none |
| `core/renderer/lib/termCaretHold.test.ts` | 5 | `caret_hold.rs` (9) | `caret_hold_*` (5) | none |
| `core/renderer/lib/termCells.test.ts` | 9 | `cells.rs` (8) | `cell_text_*` (3), `cell_from_*` (3), `logical_line_joins_from_any_row`, `char_at_cell_clamps`, `buffer_row_cells_reuses_one_scratch` | none |
| `core/renderer/lib/termClickCaret.test.ts` | 17 | `click_caret.rs` (9) | `caret_click_plain_click_is_allowed`, 11 `caret_click_refuses_*`, `caret_delta_*` (4), `arrow_keys_right_left_application` | none |
| `core/renderer/lib/termKeys.test.ts` | 6 | `keys.rs` (10) | the 6 decision tests (+2 new) | none |
| `core/renderer/lib/termLinks.test.ts` | 5 (+ linkColor group) | `links.rs` (8) | `find_links_*` (5) | `linkColor` group (flare-theme, M4) |
| `core/renderer/lib/termOsc52.test.ts` | 11 | `osc52.rs` (11) | `osc52_*` (11) | none |
| `core/renderer/lib/termOsc8.test.ts` | 25 | `osc8.rs` (11) | `osc8_target_*` (4), `parse_osc8_*` (4), `is_web_link_http_https_only`, `spans_*`/`newer_span_*`/`clear_one_screen` (8), `span_rows_*` (2), `span_text_*` (4), `pieces_overlap_*` (2) | none |
| `core/renderer/lib/termPathLinks.test.ts` | 17 | `path_links.rs` (16) | 17 tests of Task 16 | none |
| `core/renderer/lib/termPaths.test.ts` | 6 | `paths.rs` (8) | `find_paths_*` (6) | none |
| `core/renderer/lib/termReplies.test.ts` | 19 of 25 | `replies.rs` (11) | `osc_colour_*` (2), `colour_query_*` (4), `ground_mode_*` (3), `dsr_theme_reply_values`, `decrqm_reply_values`, `mode_params_flattens`, `xtversion_*` (4), `bell_*` (3) | `themePush` x4, `nudgeWidths` x2 (flare-theme, M4) |
| `core/renderer/lib/termSelectionEdit.test.ts` | 11 | `selection_edit.rs` (9) | 11 tests of Task 9 | none |
| `core/shared/termVersion.test.ts` | 2 | `version.rs` (11) | `flare_term_version_matches_the_workspace_version`, `alacritty_version_matches_cargo_lock` | none |
| `core/shared/webUrl.test.ts` | 2 | `web_url.rs` (8) | `is_web_url_*` (2) | none |
| `core/shared/psQuote.test.ts` | 3 | `ps_quote.rs` (4) | `ps_quote_*` (3) | none |
| `core/shared/termCwd.test.ts` | 8 | `cwd.rs` (4) | `parse_osc9_*` (2), `decide_follow_tells_same_inside_outside`, `cd_command_*` (5) | none |
| `core/renderer/lib/termPaste.test.ts` | 16 (+ 2 groups) | `paste.rs` (5) | `decide_paste_*` (8), `sanitize_paste_*` (3), `quote_path_*` (4), `quote_paths_joins_with_spaces` | `imagePasteKey`, `newlineKey` groups (flare-agent, M6) |
| `src/main/nodePtyPin.test.ts` | dropped | `conpty.rs` (17) | replaced by `bundled_files_match_the_pinned_hashes`, `verify_loads_the_bundled_dll_by_absolute_path` | none |
