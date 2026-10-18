# Test port map

Every Prism Terminal v0.55.6 unit test file, the Flare crate that ports it, or `dropped` with the reason.
Sorted by crate. "port (split)" means the file is shared between crates, see the note. Paths are
relative to the Prism Terminal export. Plain tests of Electron plumbing are dropped; their rules are
re-expressed natively and listed in `docs/regression-rules.md`.

| Crate | PT test file | Action | Note |
|---|---|---|---|
| flare-tabs | `core/renderer/components/Modal.test.ts` | port (split: flare-tabs T9, flare T24) | one modal frame rules; the aria-modal source scan is dropped |
| flare-agent | `core/renderer/components/TabMark.test.ts` | port (partial) | `tab_mark.rs` (M6 T13) and 19 pixel tests in `ui_tests/marks.rs`; the DOM attribute and CSS class checks are dropped, their facts are pixel tests; the Prompt, Full and Off cells are dropped (D6) |
| flare-tabs | `core/renderer/lib/fitHold.test.ts` | port | hold the terminal size while the sidebar animates |
| flare-tabs | `core/renderer/lib/hiddenTitle.test.ts` | port | tooltip only when text is cut or the label is hidden |
| flare-agent | `core/renderer/lib/tabMark.test.ts` | port | `tab_mark.rs` (M6 T13): which mark a tab wears for each indicator, state, front or back, rainbow; the Prompt, Full and Off cells are dropped (D6) |
| flare-tabs | `core/renderer/lib/tipPlace.test.ts` | port | tooltip placement: centred under, inside right edge, above, beside and flip |
| flare | `core/renderer/styles/marks.test.ts` | dropped | CSS stylesheet test; its rules (every motion has a rule, reduced motion stills all, minimised pauses, transform-only motion) are re-pinned as Slint animation tests |
| flare-tabs | `src/main/closeFlow.test.ts` | port | window close question state: veto, go-ahead, quit resume, install pre-agree |
| flare | `src/main/e2eOnly.test.ts` | port | every test-hook registration sits behind the test flag (re-expressed as a Rust test) |
| flare-tabs | `src/main/planRestore.test.ts` | port | restore plan: folder gone, active index, claude newest and second newest, codex marker, no-agent no-resume |
| flare-tabs | `src/main/tabsStore.test.ts` | port | tabs.json shape, cap 50, debounce, flush, pending load |
| flare | `src/renderer/src/components/modals.test.ts` | dropped | React source scan "declares no modal of its own"; kept as a review check that every dialog is its own module |
| flare | `src/renderer/src/components/tabParts.test.ts` | dropped | reads TabStrip.tsx and TabSidebar.tsx source text; Slint has one tab component set, the rule is kept as a design rule |
| flare | `src/renderer/src/components/termMenu.test.ts` | port | terminal menu rows fit what was clicked, Command help last, no Close tab |
| flare-tabs | `src/renderer/src/lib/hoverReveal.test.ts` | port | open delay 60 ms, close grace 100 ms, hold |
| flare-tabs | `src/renderer/src/lib/keymap.test.ts` | port | chord table, labels, agent hotkeys, vertical-only sidebar key, F1 and Ctrl+F rules, agent guard keys |
| flare-tabs | `src/renderer/src/lib/restorePlan.test.ts` | port | peek placeholders and settleRestore |
| flare-tabs | `src/renderer/src/lib/revealZones.test.ts` | port | sidebar reveal zone |
| flare-tabs | `src/renderer/src/lib/sidebarRows.test.ts` | port | folder line and find-a-tab word matching |
| flare-tabs | `src/renderer/src/lib/stripPlan.test.ts` | port | three-step overflow, front tab never hidden |
| flare-tabs | `src/renderer/src/lib/tabDrop.test.ts` | port | drop slot from the carried tab's leading edge |
| flare-tabs | `src/renderer/src/lib/tabMru.test.ts` | port | most-recent list and the Ctrl hold walk |
| flare-tabs | `src/renderer/src/lib/tabs.test.ts` | port | tab list model: add, close handover, step wrap, setCwd, labels, settings toggle, reorder |
| flare | `src/shared/channels.test.ts` | dropped | Electron IPC channel table parity; Flare has typed channels checked by the compiler |
| flare-agent | `core/main/agentDetect.test.ts` | port | signatures, first-token rule, dev-folder rule, tree walk with injected rows |
| flare-agent | `core/main/agentKinds.test.ts` | port (M3: isResumable only; detection is M6) | resumable kinds list |
| flare-agent | `core/main/agentPoll.test.ts` | port | back-off, failure back-off, forced-look gap, ask-again, forget; injected pids, ticks, query and clock |
| flare-agent | `core/main/agentResume.test.ts` | port | validResume, claude store per folder, interactive head, async walk; temp home folder |
| flare-agent | `core/main/sonaraLink.test.ts` | port | SONARA_HOME / LOCALAPPDATA resolution, POST to a local fake, quiet failure |
| flare-agent | `core/renderer/lib/agentArm.test.ts` | port | arm and disarm by resume, title, poll, prompt; doubted resume; WSL title-armed case |
| flare-agent | `core/renderer/lib/agentBackground.test.ts` | port | screen fixtures from agentBackground.screens.json |
| flare-agent | `core/renderer/lib/agentClock.test.ts` | port (M3: humanFor only; the clock reconcile group is M6) | clock reconcile and humanFor wording |
| flare-agent | `core/renderer/lib/agentClose.test.ts` | port (M3: ported; the close rule now lives in flare-agent close.rs) | close question rule (plain shell silent, agent asks, window held only while working) and dialog texts |
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
| flare-agent | `src/renderer/src/lib/taskbarBadge.test.ts` | port (split) | `badge.rs` in flare-agent (M6 T14): attentionCount, badgeText and badgePixels; the disc itself (the pixels, GDI text and `ITaskbarList3`) in `flare-win/taskbar.rs` (M6 T20); the taskbar switch in flare-settings |
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
| flare-term | `core/renderer/lib/linkScanPlan.test.ts` | port (M5: was flare-render) | sliceRows 8 ms budget, liveRange, resume after trim, cancel; in `scan_plan.rs` |
| flare-render | `core/renderer/lib/lookApply.test.ts` | port | changed-only look apply, once per frame |
| flare-term | `core/renderer/lib/termLinkPaint.test.ts` | port (M5: was flare-render; DOM part dropped, D5) | painter rules as scanner-cache tests in `link_scan.rs`; DOM decorations become grid colour overlays |
| flare-render | `src/renderer/src/lib/devicePixels.test.ts` | port | edges end on a whole device pixel at any scale |
| flare-settings | `core/main/atomicWrite.test.ts` | port | no temp file left, create new, save off-thread, newest of a burst wins, saveNow on disk at return |
| flare-settings | `core/renderer/host.test.ts` | port (split) | see-through slider cases go to flare-settings (T16); the `paintsAlpha` case is flare-theme (M4 T11b); the host-seam and Prism-style cases are dropped |
| flare-settings | `core/renderer/lib/colourFormat.test.ts` | port | HEX/RGBA/HSLA cycle, junk reads HEX, remembered choice |
| flare-settings | `core/renderer/lib/helpPrefs.test.ts` | port | help on by default, shell choice, width default and clamp, listeners |
| flare-settings | `core/renderer/lib/prefStore.test.ts` | port | enum and bool prefs, defaults, byte-for-byte keys, listeners, blocked store |
| flare-settings | `core/renderer/lib/recentRoots.pins.test.ts` | port | pin toggle, order, pins outlive history |
| flare-settings | `core/renderer/lib/recentRoots.test.ts` | port | cap 12, dedupe, parent labels, storage nonsense |
| flare-theme | `core/renderer/lib/seeThrough.test.ts` | port | default levels 0xb9 dark and 0xd1 light, light by luminance (M4 D3: moved from flare-settings to flare-theme, where the default levels feed `ground_alpha`); used by the PT import |
| flare-settings | `core/renderer/lib/termLook.test.ts` | port (split) | prefs, custom parse, indicator, ground alpha and applyCustomExtras in flare-settings (T16); font steps and faces in flare-theme (T8); Prism host-config branches dropped; duplicate "colours with alpha" block merged; rainbow and the switches are M6 (D9); the split is reworded in M4 (M4 ledger) |
| flare-settings | `core/renderer/lib/termPrefs.test.ts` | port | saved shell id, undefined until chosen, in the Flare settings file |
| flare-settings | `core/renderer/settings/fields.test.ts` | port | colour commit rules: nothing typed, same colour, typed alpha |
| flare-settings | `core/renderer/settings/layout/search.test.ts` | port | word-prefix match, folding, label-first ranking, hidden keywords |
| flare (`ui_tests/settings_ink`), flare (`tests/settings_text`) | `core/renderer/settings/neutralControls.test.ts` | port (rewritten) | asserts the Slint style constants (only Save and on-switch use the accent); the Prism source-scan form is dropped |
| flare-settings | `core/renderer/settings/options.test.ts` | port | list parity, section order, storage keys, copy rules, flat entries as tests over the row tables |
| flare-settings | `core/renderer/settings/sections/opts.test.ts` | port | see-through row name, Find words the row as the page does, Save title |
| flare-settings | `core/renderer/settings/theme/useTermSetup.test.ts` | port | dirty state, see-through level in Save, picked background never lights Save, editor opens on opaque ground |
| flare-help (`tests/entries`, `catalogue`, `speed`) | `core/shared/help/catalogue.test.ts` | port (M5: the shellOfShellId group is flare-shells `path_shell_of`) | ids, categories, task and summary shape, one-line ASCII commands, placeholders, keywords, danger lines, key-press detection, shell filtering, blankless, row names, real questions, speed |
| flare-help (`tests/words`, `golden`) | `core/shared/help/search.test.ts` | port | tokenise, stop words, stems, synonyms, one-slip typos, ranking, ties, empty query, cost bound |
| flare-settings | `core/shared/settingsCopy.test.ts` | port | symbol rule, key-name rule, ternary and template descriptions, eight-word limit |
| flare-settings | `src/renderer/src/components/settings/appOptions.test.ts` | port | unique ids, known icon/page/section, literal row ids, order, GPU gating, no Tab style row |
| flare-settings | `src/renderer/src/components/settings/optionKeys.test.ts` | port | every storage key is one its module stores under |
| flare-settings | `src/renderer/src/components/settings/settingsCopy.test.ts` | port | app settings copy and subtext length |
| flare-settings | `src/renderer/src/lib/accentPrefs.test.ts` | port | window accent store, hex6 or hex8, Reset forgets |
| flare-settings | `src/renderer/src/lib/backgroundPrefs.test.ts` | port | window background store, opaque, Reset forgets |
| flare-settings | `src/renderer/src/lib/customWindowColours.test.ts` | port | window colours kept with Custom |
| flare-settings | `src/renderer/src/lib/edgesPrefs.test.ts` | port | edges store, validation, change listener |
| flare-settings | `src/renderer/src/lib/newTabPrefs.test.ts` | port | new-tab mode and folder persistence |
| flare-settings (`import/legacy`) | `src/renderer/src/lib/opacityMigration.test.ts` | port | one-time import of PT Opacity N into the alpha byte |
| flare-settings (`import/legacy`) | `src/renderer/src/lib/seeThroughMigration.test.ts` | port | one-time import of PT switch and alpha into a see-through level |
| flare-settings | `src/renderer/src/lib/sidebarPrefs.test.ts` | port | open or collapsed, width 150 to 420, initial 220, slide flag |
| flare-settings | `src/renderer/src/lib/tabLayoutPrefs.test.ts` | port | horizontal default, unknown reads horizontal, listeners |
| flare-settings | `src/renderer/src/lib/tabSwitchPrefs.test.ts` | port | In order default, unknown reads In order, listeners |
| flare-shells | `core/main/shells.test.ts` | port | parseWslList, shellById, pwsh path discovery, System32 tool paths |
| flare-shells | `core/main/termPrompt.test.ts` | port | OSC 9;9 prompt hook, cmd PROMPT, PS file style (test the emitted strings) |
| flare-shells | `core/main/terminal.test.ts` | port (split) | ptyEnv and withResume groups in flare-shells; OutputBatcher and pollExitCode groups in flare-term; plugin-dir group in flare-shells (`pty_env` takes it); isOurPlugin in flare-agent |
| flare-shells | `core/renderer/lib/termPaste.test.ts` | port (split) | decidePaste, sanitizePaste, quotePath here; imagePasteKey and newlineKey groups in flare-agent (M5, D13) |
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
| flare-theme | `core/renderer/lib/colour.test.ts` | port | all 39 in flare-theme (M4 D3): colour maths, legibility, alpha steps, colourCommit, alphaPct and snapAlphaStep (the ledger's flare-settings part moved here) |
| flare-theme | `core/renderer/lib/ground.test.ts` | port | DEFAULT_GROUND, prism preset equality, ground constants |
| flare-theme | `core/renderer/lib/markColours.test.ts` | port (M4, consumed by M6 T27) | rainbow seven, floorMark on light and dark grounds, unreachable floor, gradients, opaqueOver |
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
| flare-theme | `src/renderer/src/lib/agentColors.test.ts` | port (M4, consumed by M6 T27) | working = accent, finished, question and failed colours per theme preset; marksApart |
| flare-theme | `src/renderer/src/lib/chromeTheme.opacity.test.ts` | port | ground alpha as a byte, opacity migration snapshot values (case list not enumerated; read the file) |
| flare-theme | `src/renderer/src/lib/chromeTheme.test.ts` | port | 30 cases (M4 T12; was 21): chromeTokens, edges, see-through accent, custom floors, secondary, light controls, switch knob |
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
| flare-win | `src/main/dwmHelper.test.ts` | port (partial) | COLORREF arithmetic ported; the native-buffer handle read is dropped (an `isize` is handed over, no helper process); the "-Command not -EncodedCommand" launch test dropped (Flare calls DWM in process) |
| flare-win | `src/main/e2eClipboard.test.ts` | port | in-memory clipboard stand-in under the test flag, re-expressed as a trait |
| flare-win | `src/main/edgeWatch.test.ts` | port | cursor reading at the window edge, far-cursor watch for reveal zones |
| flare-win | `src/main/material.test.ts` | port | Windows 11 build check, validBg, isLightBg for frost; the Electron mock is dropped |
| flare-win | `src/main/shellVerb.test.ts` | port | HKCU keys, %1 vs %V, label by REG_SZ, pointsAt; key name and label per decision O4 |
| flare-win | `src/main/shellVerbParity.test.ts` | port | uninstaller deletes every key the app writes, checked against Flare's installer script |
| flare-win | `src/main/shellVerbRelabel.test.ts` | port | stale-label detection and label-only rewrite |
| flare-win | `src/main/verbSwitch.test.ts` | port | default on, off marker, reconcile, dev/test write gate, boolean guard |
| flare-win | `src/main/windowEdge.test.ts` | port | DWM border step and edges setting; pure edgeFor beside the border call |
| flare-win | `src/main/windowState.test.ts` | port | window.json parse, defaults, minimum size, off-screen position drop |
| flare-agent | `src/renderer/src/lib/marksPause.test.ts` | port | `mark_motion.rs` (M6 T14, `MarksPause`): minimise sets the pause, restore clears it; flare-win only answers the animation switch (`motion.rs`); the DOM attribute check is dropped |

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
| `core/main/termPathOpen.test.ts` | 10 of 12 | `path_open.rs` (15) | `resolve_against_the_shell_folder`, `resolve_refuses_unc_nul_and_no_folder`, `path_kinds_says_what_exists`, `open_a_file_and_a_folder`, `a_runnable_file_is_revealed_never_run`, `reveal_on_request_and_nothing_for_missing`, `may_open_allows_documents_media_and_safe_code_only`, `a_folder_uses_open_folder_a_file_never_does`, `never_more_than_three_stats_in_flight`, `a_hanging_drive_answers_nothing_for_a_minute` | `openFolderDefault` x2 (flare-win, M9) |
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

## M3 ledger

`n` = Prism Terminal `it(...)` cases; "Flare" = the test names in the task's table (the task is the authority). Dropped = not ported, with the reason.

| PT test file | n | Flare crate and task | Flare tests / note |
|---|---|---|---|
| `src/renderer/src/lib/tabs.test.ts` | 13 | flare-tabs, T5 | `adds_at_the_end_and_activates` ... `reorders_correcting_for_the_removed_slot` (13) + 4 new |
| `.../lib/stripPlan.test.ts` | 7 | flare-tabs, T6 | the seven of the table + layout tests |
| `.../lib/tabDrop.test.ts` | 7 + 2 | flare-tabs, T7 | 7 ported; the 2 "Prompt segments overlapping" dropped (Prompt tab style removed; one tab style, parity.md) |
| `.../components/useTabReorder.ts` (no test) | 0 | flare-tabs, T7 | `TabDrag`: 9 new tests from its behaviour |
| `.../lib/tabMru.test.ts` | 12 | flare-tabs, T8 | all 12 (the identity case as `touching_the_front_tab_changes_nothing`: equality in Rust) |
| `.../lib/agentKeysPrefs.ts` (no test) | 0 | flare-settings, T3 | `agent_keys_are_on_until_turned_off` (new) |
| `.../lib/sidebarRows.test.ts` | 3 | flare-tabs, T9 | all 3 |
| `core/renderer/lib/tipPlace.test.ts` | 4 | flare-tabs, T9 | all 4 |
| `core/renderer/lib/hiddenTitle.test.ts` | 3 + 3 | flare-tabs, T9 | 3 ported; the 3 TSX source scans dropped (design rule, ledger note) |
| `core/renderer/components/Modal.test.ts` | 3 | flare-tabs T9 + flare T24 | `returnsFocus` -> `focus_returns_only_when_nothing_else_took_it`; the two source scans -> `only_dialog_slint_declares_a_scrim` (T24) |
| `src/renderer/src/lib/keymap.test.ts` | 12 | flare-tabs, T10 | all 12 (the source scan becomes the `.slint` scan) + 4 new |
| `.../lib/hoverReveal.test.ts` | 6 | flare-tabs, T11 | all 6 |
| `.../lib/revealZones.test.ts` | 4 | flare-tabs, T11 | all 4 |
| `core/renderer/lib/fitHold.test.ts` | 3 | flare-tabs, T11 | all 3 |
| `.../lib/sidebarPrefs.test.ts` (slide part) | 1 | flare-tabs, T11 | `pinning_and_unpinning_slide_and_hold_the_fit` |
| `src/main/closeFlow.test.ts` | 6 | flare-tabs, T12 | all 6 |
| `core/renderer/lib/agentClose.test.ts` | 3 | flare-agent, T12 | all 3 (+ body, label tests) |
| `core/renderer/lib/agentClock.test.ts` (`humanFor`) | 3 | flare-agent, T12 | 3 ported; the clock reconcile group is M6 |
| `core/main/agentKinds.test.ts` | 3 | flare-agent, T12 | `isResumable` only; detection and prefilter groups are M6 |
| `src/main/tabsStore.test.ts` | 10 | flare-tabs, T13 | 4 parse + 5 store + the shape test; the "same empty object" case is n/a in Rust |
| `src/main/planRestore.test.ts` | 8 | flare-tabs, T13 | all 8 |
| `src/renderer/src/lib/restorePlan.test.ts` | 7 | flare-tabs, T13 | all 7 |
| `core/main/atomicWrite.test.ts` | 6 | flare-settings, T2 | all 6 (+ 2 new) |
| `src/renderer/src/lib/tabLayoutPrefs.test.ts` | 4 | flare-settings, T3 | all 4 |
| `.../lib/tabSwitchPrefs.test.ts` | 4 | flare-settings, T3 | all 4 |
| `.../lib/sidebarPrefs.test.ts` | 5 + 1 | flare-settings, T3 (+ T11) | 5 + the toggle test in T3; the slide test in T11 |
| `.../lib/newTabPrefs.test.ts` | 4 | flare-settings, T3 | all 4 |
| `core/renderer/lib/recentRoots.test.ts` | 8 | flare-settings, T4 | all 8 |
| `core/renderer/lib/recentRoots.pins.test.ts` | 3 | flare-settings, T4 | all 3 |
| `core/renderer/lib/prefStore.test.ts` | (M7) | flare-settings, T3 | the store and its helpers are built here with their own tests; M7 ports the rest of `prefStore.test.ts` on top |
| `core/renderer/lib/termPrefs.test.ts` | (M7) | flare-settings, T3 | the getter only: `saved_shell_id_is_none_until_chosen` |
| `src/main/argv.test.ts` | 12 | flare-win, T14 | all 12 (+ the cap test) |
| `src/main/windowState.test.ts` | 5 | flare-win, T14 | all 5 (+ DPI, scale, normal-bounds tests) |
| `src/main/edgeWatch.test.ts` | 8 | flare-win, T17 (+ T11) | 4 + 3 + the shared-zone test in T11 |
| `src/renderer/src/lib/devicePixels.test.ts` | 2 | flare-render, T6 | both (+ 1 new) |
| `src/main/e2eOnly.test.ts` | 1 | flare, T29 | `e2e_env_reads_none_unless_the_flag_is_on`, `no_source_file_reads_an_e2e_variable_directly` (the registration scan is re-expressed: there are no `ipcMain` handlers) |
| `src/renderer/src/components/termMenu.test.ts` | | M5 | terminal menu (not tab menus) |
| `core/renderer/components/TabMark.test.ts`, `core/renderer/lib/tabMark.test.ts`, `styles/marks.test.ts` | | M6 | marks; the mark slot exists (`mark-kind`, `mark-color`) |
| `src/renderer/src/components/tabParts.test.ts` | dropped | | reads TSX text; Slint has one component set per layout; the rule (one close X and ring for both layouts) is kept by Task 20 and 21 sharing the specs |
| `src/renderer/src/components/modals.test.ts` | dropped | | React source scan; replaced by `only_dialog_slint_declares_a_scrim` |

Total M3 PT test files: 33 touched (31 ported in whole or part, 2 dropped), plus 3 deferred by group to M5, M6, M7.

## M4 ledger

Prism Terminal v0.55.6 test files that the M4 areas map to (Appendix A of the M4 plan, copied verbatim). The rows above that M4 changed carry "M4" in their text.

`n` = Prism Terminal `it(...)` cases (an `it.each` over the presets counts once). "Flare" = the test names in the task's table (the
task is the authority). Dropped = not ported, with the reason. The last column names the `docs/test-port.md` row Task 32 changes.

| PT test file | n | Flare crate and task | Flare tests / note | Ledger row change |
|---|---|---|---|---|
| `core/renderer/lib/termAnsi.test.ts` | 9 | flare-theme, T3 | all 9 + 3 new + the oracle vectors | none |
| `core/renderer/lib/colour.test.ts` | 39 | flare-theme, T4 (13) and T9 (26) | all 39; D3 | `flare-settings` part moves to `flare-theme` |
| `core/renderer/lib/ground.test.ts` | 3 | flare-theme, T5 | the source scans re-expressed over `crates/*/src` | none |
| `core/renderer/lib/termThemeRetired.test.ts` | 5 | flare-theme, T5 (4) and T10 (1) | all 5 + the oracle's 31 pairs | none |
| `core/renderer/lib/termTheme.legible.test.ts` | 7 | flare-theme, T5 (4) and T10 (3) | all 7 | none |
| `core/renderer/lib/termTheme.test.ts` | 12 | flare-theme, T10 | 11 ported; "what xterm is handed" becomes `every_resolved_value_is_hex6_or_hex8` (no xterm) | none |
| `core/renderer/lib/termTheme.snapshot.test.ts` | 3 | flare-theme, T10 | the three snapshots become `oracle_*` tests against PT's own answers (verified equal to its `.snap`) | none |
| `core/renderer/lib/termGround.test.ts` | 3 | flare-theme, T10 | all 3 + the oracle vectors | none |
| `core/renderer/lib/themeDraft.test.ts` | 6 | flare-theme, T10 (4 pure) and flare-settings, T16 (the store half) | all 6 | none |
| `core/renderer/lib/themeOrder.test.ts` | 8 | flare-theme, T7 | all 8 (the snapshot is the explicit list) | none |
| `core/renderer/lib/nameInk.test.ts` | 6 | flare-theme, T11 | all 6 | none |
| `core/renderer/lib/switchKnob.test.ts` | 4 | flare-theme, T11 | all 4 | none |
| `core/renderer/lib/markColours.test.ts` | 8 | flare-theme, T11 | all 8 | none |
| `core/renderer/lib/markPalette.test.ts` | 7 | flare-theme, T11 | all 7 | none |
| `core/renderer/lib/seeThrough.test.ts` | 5 | flare-theme, T11b | all 5; D3 | `flare-settings` -> `flare-theme` |
| `core/renderer/lib/termLinks.test.ts` (linkColor group) | 5 | flare-theme, T11b | all 5 (findLinks is M5) | none |
| `core/renderer/lib/termReplies.test.ts` (themePush, nudgeWidths; groundMode on the real presets) | 6 (+3) | flare-theme, T11b (themePush 4, nudgeWidths 2) and T11 (`ground_mode` 1 test over the real presets); flare-term, T11c (the cross-crate agreement) | M2 already ported the colour-query, DSR/DECRQM/params, XTVERSION, bell and (with fixed values) `ground_mode` groups into `flare-term/src/replies.rs`; T11c makes that one use flare-theme's measure | the `termReplies` rows say "(flare-theme, M4)" for the two deferred groups: unchanged |
| `core/renderer/host.test.ts` (the see-through slider group) | 9 | flare-settings, T16 (7); T11b (`paintsAlpha` 1) | the host seam and Prism-style cases and the channel table are dropped (one host, no IPC table) | the row says the `paintsAlpha` case is flare-theme (T11b) |
| `core/renderer/lib/termLook.test.ts` | 34 | flare-theme T8 (sizes, faces), T11b (levels), flare-settings T16 (stores, ground alpha) | dropped: "acrylic is off until asked for" (no switch in Flare) and the duplicate "colours with alpha" block (6 cases + the second `termGroundAlpha`; the block is ported once); the Prism host-config branches of the indicator cases are re-expressed for Flare's one host (T16 `readIndicator choices`); "the legacy opacity reads never-set as opaque" goes to M7 with the import; "keeps the rainbow on by default" goes to M6 with the switches (D9); "nothing writes an opacity" is T16 `no_extra_carries_an_opacity` | the row's split is reworded: font steps and faces in flare-theme (T8), ground alpha in flare-settings (T16), not the other way round |
| `core/renderer/lib/colourFormat.test.ts` | 4 | flare-settings, M7 | with the picker | none |
| `core/renderer/settings/theme/useTermSetup.test.ts` | 4 | flare-settings, T16 | all 4 | none |
| `src/renderer/src/lib/chromeTheme.test.ts` | 30 | flare-theme, T12 | all (the `index.css` fallbacks test becomes T22's `every_chrome_token_is_an_in_property_with_its_pt_default`; "reads anything it does not know as the default" is T6's `default_to_hairline_and_read_anything_else_as_it`) | the row's "21 cases" becomes "30 cases" |
| `src/renderer/src/lib/chromeTheme.opacity.test.ts` | 2 | flare-theme, T12 | `oracle_alpha_sweep`, `oracle_every_preset_four_variants` (verified equal to PT's `.snap`, 0 differences) | none |
| `src/renderer/src/lib/sidebarGround.test.ts` | 2 | flare-theme, T7 and T12 | both | none |
| `src/renderer/src/lib/accentLift.test.ts` | 4 | flare-theme, T7 (2, as 3 tests) and T12 (2) | all 4 | none |
| `src/renderer/src/lib/switchAccent.test.ts` | 2 | flare-theme, T12 | both | none |
| `src/renderer/src/lib/agentColors.test.ts` | 7 | flare-theme, T13 | all 7 | none |
| `src/shared/windowEdges.test.ts` | 3 | flare-theme, T6 | all 3 | none |
| `src/renderer/src/lib/accentPrefs.test.ts` | 5 | flare-settings, T15 | all 5 | none |
| `src/renderer/src/lib/backgroundPrefs.test.ts` | 4 | flare-settings, T15 | all 4 | none |
| `src/renderer/src/lib/customWindowColours.test.ts` | 3 | flare-settings, T15 | all 3 | none |
| `src/renderer/src/lib/edgesPrefs.test.ts` | 4 | flare-settings, T15 | all 4 | none |
| `src/renderer/src/lib/opacityMigration.test.ts` | 15 | flare-settings, M7 | with the import | none |
| `src/renderer/src/lib/seeThroughMigration.test.ts` | 9 | flare-settings, M7 | with the import | none |
| `src/main/material.test.ts` | 5 | flare-win, T17 | all 5 + 10 new | none |
| `src/main/dwmHelper.test.ts` | 4 | flare-win, T18 | 1 ported (COLORREF); the native-buffer handle read and the two PowerShell launch tests are dropped (an `isize` is handed over; no helper process) | the "port (partial)" note is reworded |
| `src/main/windowEdge.test.ts` | 7 | flare-win, T18 | all 7 (the unknown-word case is the enum) + applier tests | none |
| (no PT test: `secondaryPrefs.ts`, `paintChrome.ts`, `sidebarGround` first group, `lookApply.ts`) | 0 | T15 (secondary), T14 and T26 (paint, once per turn), T7 | new tests named in the tasks | none |

Counts: the files above hold about 290 PT cases. 28 are deferred to M7 (`colourFormat`, `opacityMigration`, `seeThroughMigration`), about 12 are dropped
as named (the acrylic-switch cases, the duplicate "colours with alpha" block, three `dwmHelper` tests), and the rest, about 250, are ported. The new
Flare-only tests (oracle vectors, state machines, wiring) are listed per task; Task 32 reports the exact numbers from `cargo nextest`.

## M5 ledger

Prism Terminal v0.55.6 test files that the M5 areas map to (Appendix A of the M5 plan, copied). The rows above that M5 changed carry "M5" in their text.

`n` = Prism Terminal `it(...)` cases. "Done (M2)" = ported by M2, kept. Flare names are in the task tables of the M5 plan.

| PT test file | n | Flare crate and task | Note |
|---|---|---|---|
| `core/renderer/lib/termKeys.test.ts` | 6 | flare-term `keys.rs` (M2) | done; M5 adds the window-level key tests (Task 24, 25) |
| `core/renderer/lib/termPaste.test.ts` (decide 8, sanitize 3, quotePath 4, quotePaths 1) | 16 | flare-shells `paste.rs` (M2) | done (+ the security cases of rule 135) |
| `core/renderer/lib/termPaste.test.ts` (`imagePasteKey` 3, `newlineKey` 2) | 5 | flare-agent `keys.rs`, Task 9 | moved from M6 (D13); + 1 new |
| `core/shared/help/catalogue.test.ts` (`the shells`, first test) | 1 | flare-shells `paste.rs`, Task 8 | `shell_ids_read_as_the_language_they_speak`; the help catalogue itself is M7 |
| `core/renderer/lib/termClickCaret.test.ts` | 17 | flare-term `click_caret.rs` (M2) | done; wired and re-tested in Task 16e |
| `core/renderer/lib/termSelectionEdit.test.ts` | 11 | flare-term `selection_edit.rs` (M2) | done; wired in Task 16e |
| `core/renderer/lib/termLinks.test.ts` (`findLinks`) | 5 | flare-term `links.rs` (M2) | done; the `linkColor` group is M4 (flare-theme) |
| `core/renderer/lib/termOsc8.test.ts` | 25 | flare-term `osc8.rs` (M2) | done; D18: `osc8_target`, `is_web_link` used, the span tracker is library code only |
| `core/renderer/lib/termPaths.test.ts`, `termPathLinks.test.ts` | 6 + 17 | flare-term (M2) | done; wired in Tasks 13, 16c, and the ask/answer pump in Task 23 (rule 13) |
| `core/renderer/lib/termOsc52.test.ts`, `core/shared/webUrl.test.ts` | 11 + 2 | flare-term (M2) | done; wired in Task 23 |
| `core/renderer/lib/appCopyNotice.test.ts` | 9 | flare-term `app_copy.rs` (M2) | done; wired in Tasks 21, 23 |
| `core/renderer/lib/termCaretHold.test.ts` | 5 | flare-term `caret_hold.rs` (M2) | done; wired in Task 24b |
| `core/main/termPathOpen.test.ts` | 10 of 12 | flare-term `path_open.rs` (M2) | done; real openers in Task 11; `openFolderDefault` x2 stay M9 |
| `core/renderer/lib/linkScanPlan.test.ts` | 8 | flare-term `scan_plan.rs`, Task 12a | all 8; used by find's count job |
| `core/renderer/lib/termLinkPaint.test.ts` | 6 | flare-term `link_scan.rs`, Task 13 | the 6 re-expressed as scanner-cache tests (`an_unchanged_line_is_not_scanned_again` ...); the DOM decoration counting is dropped (D5) |
| `src/renderer/src/components/termMenu.test.ts` | 4 | flare `term_menu.rs`, Task 20 | all 4 + 4 new |
| `core/main/ipc.clipboard.test.ts` (clipboard:write 2, clipboard:term-write 2) | 4 of 6 | flare-win `clipboard_port.rs`, Task 10 | the 2 `term:bell` tests are not M5 (the taskbar flash is M9) |
| `src/main/e2eClipboard.test.ts` | 1 | flare-win `MemoryClipboard`, Task 10; flare Task 24 `under_e2e_the_clipboard_is_in_memory_and_openers_record` | re-expressed as the port's in-memory stand-in |
| `core/renderer/components/TermFind.tsx`, `CopiedBadge.tsx`, `TerminalPanel.tsx` | no unit tests | flare, Tasks 12b, 17, 18, 23 | new tests from the e2e scenarios `findKey`, `selectionEdit`, `paste`, `imagePaste`, `links`, `osc8Links`, `pathLinks`, `linkPaintStall`, `termClipboard`, `dropAndMenu`, `mouseReport`, `clickCaret`, `reviewKeys` (Task 25) |
| xterm.js behaviour (no PT test) | - | flare-term Tasks 4 to 7, 12b | tests written from the xterm sources named in Verified facts |
| xterm.js `src/browser/input/MoveToCell.test.ts` (Alt+click, `altClickMovesCursor`) | 4 | flare-term `alt_click.rs`, M5 final review | all 4 (the 24-target normal table, application cursor, alt buffer) + 10 new; wired in `term_ui.rs` (O25) |

Rows changed by M5 (the M5 plan's Task 28 list): `imagePasteKey` and `newlineKey` (termPaste.test) now belong to flare-agent, M5 (D13); `termMenu.test` to flare, M5; `termLinkPaint.test` and `linkScanPlan.test` to flare-term (`link_scan.rs`, `scan_plan.rs`) instead of flare-render (their DOM-decoration part does not exist in Flare, D5); the `shellOfShellId` group of `catalogue.test` to flare-shells (`path_shell_of`).

## M6 ledger

Appendix A of the M6 plan, copied verbatim: Prism Terminal test ledger (every file `docs/test-port.md` maps to the M6 areas; 263 `it` blocks in 28 files)

| PT test file | `it` | Flare home | Tests | Note |
|---|---|---|---|---|
| `core/main/agentDetect.test.ts` | 16 | `flare-agent/detect.rs` (T8) | 11 + 3 new | the compact process listing group (5) dropped: native rows (D4); a four-deep tree test replaces it |
| `core/main/agentKinds.test.ts` | 3 | `detect.rs` (T8) + M3's `kinds.rs` | 3 | the poll-prefilter-by-reading-source group becomes `the_resumable_agents_pass_the_prefilter` |
| `core/main/agentPoll.test.ts` | 14 | `poll.rs` (T9) | 14 + 3 new | `agentBusy` disappears (D3); the in-flight ask test uses `PollHandle` |
| `core/main/agentResume.test.ts` | 10 | `resume.rs` (T10) | 8 + 2 new | sync and async walks are one function; the 60-file test checks the same answer |
| `core/main/sonaraLink.test.ts` | 3 | `sonara.rs` (T12) | 3 + 3 new | a std TCP fake server replaces node's http |
| `core/main/terminal.test.ts` (`isOurPlugin` 2) | 2 | `plugin.rs` (T3) | 3 | + the inherited-copy drop in T23 |
| `core/renderer/lib/agentArm.test.ts` | 20 | `arm.rs` (T4) | 20 | |
| `core/renderer/lib/agentBackground.test.ts` | 5 | `background.rs` (T6) | 5 | fixtures copied byte for byte |
| `core/renderer/lib/agentClock.test.ts` | 10 | `clock.rs` (T7) + M3's three `human_for` tests | 7 + 3 (M3) | |
| `core/renderer/lib/agentClose.test.ts` | 3 | M3 (`close.rs`, Task 12) | 3 | ported in M3 |
| `core/renderer/lib/agentHookSignal.test.ts` | 10 | `signal.rs` (T2), `plugin.rs` (T3) | 6 + 7 | the real-run test uses the real hook.cmd |
| `core/renderer/lib/agentHookState.test.ts` | 31 | `hook_state.rs` (T5) | 31 | |
| `core/renderer/lib/agentHookState.timelines.test.ts` | 7 | `tests/timelines.rs` (T5), `indicator.rs` (T18) | 7 + 7 | replayed twice: through `hook_step` and through the whole indicator |
| `core/renderer/lib/agentIcon.test.ts` | 2 | `icon.rs` (T13) | 2 + 1 | |
| `core/renderer/lib/agentQuestion.test.ts` | 12 | `question.rs` (T6) | 12 | |
| `core/renderer/lib/agentTitle.test.ts` | 14 | `title.rs` (T4) | 12 | the two `titleAgent` groups are 2 tests each |
| `core/renderer/lib/resumeReveal.test.ts` | 9 | `reveal.rs` (T11) | 9 + 1 | |
| `core/renderer/lib/termActivity.born.test.ts` | 5 | `activity.rs` (T7) | 5 | |
| `core/renderer/lib/termActivity.test.ts` | 9 | `activity.rs` (T7) | 9 | |
| `core/renderer/questionWatch.test.ts` | 6 | `question_watch.rs` (T6) | 6 | |
| `core/renderer/questionWidget.test.ts` | 8 | `question_widget.rs` (T6) | 8 + 1 | |
| `src/renderer/src/lib/taskbarBadge.test.ts` | 7 | `badge.rs` (T14), `flare-win/taskbar.rs` (T20) | 7 + 6 | `badgePixels` and `badgeText` in `badge.rs`; the disc is Flare's own pixels |
| `core/renderer/lib/agentDiag.test.ts` | 21 | `flare-diag/agent_diag.rs` (T15) | 21 + 1 | |
| `core/renderer/lib/tabMark.test.ts` | 8 | `tab_mark.rs` (T13) | 5 + 3 new | the Prompt, Full and Off cells are dropped (D6) |
| `core/renderer/components/TabMark.test.ts` | 10 | `tab_mark.rs` (T13), `ui_tests/marks.rs` (T24, T25) | 1 + 19 pixel tests | DOM and CSS class checks dropped; their facts are pixel tests |
| `core/renderer/styles/marks.test.ts` | 5 | `mark_motion.rs` (T14), `ui_tests/marks.rs` | 4 | every motion has a rule, reduced motion stills all, minimised pauses, transform-only (a text scan: `MarkMotion` is the only animation source) |
| `src/renderer/src/lib/marksPause.test.ts` | 2 | `mark_motion.rs` (T14) | 1 | the DOM attribute check is dropped |
| `core/renderer/lib/markColours.test.ts` | 8 | M4 (`flare-theme/marks.rs`) | M4 | consumed by T27 |
| `src/renderer/src/lib/agentColors.test.ts` | 5 | M4 (`agent_colours.rs`) | M4 | consumed by T27 |
| `termPaste.test.ts` (`imagePasteKey`, `newlineKey`) | 5 | M5 (`keys.rs`, Task 9) | M5 | M6 supplies the `AgentView` (T28) |
| `core/main/terminal.test.ts` (`ptyEnv` plugin 9, `withResume` 5) | 14 | M2 (`flare-shells`) | M2 | M6 adds the guard in front of `with_resume` (T23) and wires `is_ours` |

Tests with no Prism Terminal source (new in M6): the indicator's own unit tests (Tasks 17 and 18, about 50), the mark component and flow pixel tests (Tasks 24, 25, 29, 31), the real-process tests (Task 32), the hub tests (Task 28), the restore and badge wiring tests (Task 30).
