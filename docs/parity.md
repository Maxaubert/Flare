# Parity checklist

**Target:** Prism Terminal v0.55.6, commit `cd26356c6463510677377b2a8a2d36058f19f4c4`. This file is the
definition of done for every Flare milestone. Nothing newer than this release is in scope.

How to read it:

- Every line is a `- [ ]` item. A milestone is done when every line in its section is ticked or has a
  recorded owner decision.
- Source paths are relative to the Prism Terminal v0.55.6 export (`core/`, `src/`, `docs/`,
  `tools/e2e/run.mjs`). `core/` is the shared terminal core, `src/` is the app.
- Flare names replace Prism Terminal names: plugin `flare-status`, signal `OSC 777;flare-agent`, tab id
  `FLARE_TAB_ID`, resume variable `FLARE_RESUME_ID`, profile `%APPDATA%\Flare`, e2e variables `FLARE_E2E_*`.
- `DECISION` marks a place where Prism Terminal and the Flare brief disagree, or where an owner choice is
  open. Open decisions are also gathered in the last section.
- Milestones follow `docs/superpowers/plans/2026-10-17-flare-roadmap.md`: M1 window and render, M2 pty and
  shells, M3 tabs, M4 themes, M5 input, M6 agent, M7 settings, M8 dictation, M9 ship, M10 sweep.

## M1: Window, chrome, rendering

- [ ] Window is created hidden and shown on ready; maximised is applied after creation (src/main/index.ts createWindow, ready-to-show).
- [ ] Custom title bar keeps the DWM frame, never a frameless window, because acrylic needs a composited frame (src/main/index.ts createWindow; docs/regression-rules.md title-bar-style-hidden).
- [ ] The system menu on a right-click is suppressed; Alt+Space still opens it (src/main/index.ts system-context-menu; e2e systemMenu).
- [ ] Background throttling is off, so agent clocks run while minimised; only mark animation pauses while minimised (src/main/index.ts webPreferences; src/renderer/src/lib/marksPause.ts).
- [ ] F11 toggles full screen and the shell still receives it; entering or leaving full screen notifies the page and reapplies the edge (src/renderer/src/lib/keymap.ts 'fullscreen'; src/main/index.ts enter-full-screen).
- [ ] Top row carries the tabs and, at the right end: update chip, command help (when on), Settings cog, Minimise, Maximise, Close; no app menu (src/renderer/src/components/TitleBar.tsx; regression-rules settings-cog-no-menu).
- [ ] Caption buttons are 28 px tall and 32 px wide; icons 15 px (cog, help) and 13 px (others); hover fill `--p-hover-hi`; focus ring 2 px accent-hi; Close turns red at 80 percent with a white icon on every theme (TitleBar.tsx BTN).
- [ ] Close button carries a window-close marker; the Settings tooltip reads "Settings (<shortcut>)" (TitleBar.tsx).
- [ ] With no tabs the top row still holds the buttons; the strip's empty space drags the window, the buttons do not (e2e titleBar).
- [ ] The strip is a drag handle except while any drag is in flight (tab drag or a folder dragged from Explorer); it returns on dragend, drop, or the next pointer move or press (TabStrip.tsx dragInFlight).
- [ ] The title bar's quiet app mark is the two-path mark, painted in the buttons' ink, no name (AppMark.tsx; build/icon-mark.svg).
- [ ] Minimise and restore message the page so mark animation pauses while hidden (src/main/index.ts minimize/restore; marksPause.ts).
- [ ] Terminal grid is drawn by one renderer path in a GPU mode and a software mode, chosen in Settings; a GPU failure at start falls back to software and says so (design spec Goals 3; roadmap M1).
- [ ] The panel paints the ground exactly once; the glyph layer is clear (cursor accent = ground), so a block cursor is never a hole (regression-rules panel-paints-ground; termXterm.ts).
- [ ] Rows are whole; the grid reports real columns and rows; a refit skips a hidden or zero-size panel (TerminalPanel.tsx refit; regression-rules panel-paints-ground).
- [ ] Scrollbar is 6 px wide and high, no track, no steppers, thumb radius 3 px, thumb is the foreground at 40 percent and 65 percent on hover (src/renderer/src/index.css; e2e scrollbar).
- [ ] Reduced motion app-wide: every transition and animation drops to 0.01 ms, one iteration (index.css).
- [ ] Whole device pixels where two see-through surfaces meet; follows a DPI change (src/renderer/src/lib/devicePixels.ts onDevicePixels; devicePixels.test).
- [ ] Rendering is correct at 100 percent to 225 percent DPI (roadmap M1; owner machine is 225 percent).
- [ ] Overlay order, bottom to top: panel, pop, menu, picker, modal, toast, tip (core/renderer/components/layers.ts LAYER).
- [ ] Tooltips: plain `title` is kept; while the pointer rests the app moves the title aside and draws its own tip, then gives the title back; a title that changes while shown is followed (core/renderer/components/Tooltips.tsx).
- [ ] A tip shows after 1000 ms rest (SHOW_MS), centred under the item with a 6 px gap and at least 6 px from the window edge; above when no room; beside the item where an ancestor sets a side (sidebar rows); flips if the side does not fit; whole pixels (Tooltips.tsx; core/renderer/lib/tipPlace.ts placeTip).
- [ ] Context menu panel: flat surface, 1 px divider border, 2 px radius, shadow `0 10px 28px rgba(0,0,0,.5)`, min width 156 px, rows 28 px, 12 px text, 11 px side padding, hint 10.5 px dim, danger rows red (core/renderer/components/ContextMenu.tsx).
- [ ] Context menu behaviour: clamped to the window, one flyout level that flips left at the right edge with a diagonal grace period, closes on Escape and on window blur, `keepOpen` rows keep it up, a `trailing` control (the pin) has its own click (ContextMenu.tsx).
- [ ] Every menu row carries an icon from one glyph set (src/renderer/src/components/MenuIcon.tsx, menuGlyphs.tsx, glyphPaths.ts; e2e selectionEdit, pathLinks).
- [ ] Question dialog: title, one line of body, choices with the safe one first; primary wears the accent and Enter picks it; danger choices read as a warning; focus returns to the primary when the question changes under an open dialog (src/renderer/src/components/Dialog.tsx).
- [ ] Modal frame: Escape heard first and swallowed, cancels unless locked; only a plain Tab is trapped (Ctrl+Tab stays the host's); a press outside cancels unless locked and lifts the drag region; focus returns only if nothing else took it (core/renderer/components/Modal.tsx).
- [ ] "Copied" badge shows 1200 ms then fades 200 ms, exists in the page only while shown, a second copy restarts the clock, raised only by a successful copy, neutral grey with a hairline and no shadow (core/renderer/components/CopiedBadge.tsx; copyNotice.ts).
- [ ] Terminal edge is drawn once along the top under the bar; beside an open sidebar it also runs down the left and round the 10 px cut corner in the divider ink (src/renderer/src/components/TermEdge.tsx).
- [ ] The cut corner (CORNER_R = 10): the terminal mask and the corner piece in the sidebar colour are inverse masks of one path, no seam (src/renderer/src/lib/cornerShapes.ts).
- [ ] Main watches the pointer near the top and left edges only while a subscriber exists, because the page gets no move over the resize border or a drag region (src/main/edgeWatch.ts; edgeNear.ts; edgeWatch.test).
- [ ] One reveal-zone table read by main and the page: top 16, left 24, 8 px outside the window still counts (src/shared/revealZones.ts REVEAL_ZONE).
- [ ] Mark motion set covers seven motions: run, grow, breathe, flow (x, y, fill), spin, still (core/renderer/components/markClasses.ts MOTION_CLASS).
- [ ] Each session's look is applied only where it changed; link repaints coalesce to once per frame (core/renderer/lib/lookApply.ts).
- [ ] App facts (version, home folder) are asked of the backend once and cached (src/renderer/src/lib/appFacts.ts).
- [ ] Known-folder lookup for `%LOCALAPPDATA%` with the home fallback (src/main/appPaths.ts).
- [ ] Slint attribution "Made with Slint" is available to the About page (design spec; roadmap M7).
- [ ] Frame time, memory and cold-start numbers are recorded in every milestone PR (roadmap performance bar).

## M2: Pty, shells, terminal core

### Shells and command lines

- [ ] Shells are detected once per run and cached; a new shell is picked up on restart (core/main/shells.ts detectShells).
- [ ] Each probe (where, wsl) has an 8 s timeout, is hidden and async, never blocking the UI (shells.ts run()).
- [ ] PowerShell 7 is the first absolute path from `where pwsh.exe` not in the current directory; none means no pwsh entry (shells.ts pwshPath; shells.test).
- [ ] Shell list order and ids: pwsh "PowerShell 7", powershell "Windows PowerShell" (full System32 path), cmd "Command Prompt", then one `wsl-<name>` "WSL: <name>" per distro with args `-d <name> --cd .` (shells.ts).
- [ ] WSL list: `wsl -l -q` decoded UTF-16LE, NULs stripped, only lines matching `^[\w.-]+$` kept (shells.ts parseWslList; shells.test).
- [ ] Prism Terminal v0.55.6 does NOT detect Git Bash: shells.ts lists only pwsh, powershell, cmd and WSL distros (no `bash.exe` probe anywhere in core/main). The design spec's "Git Bash, as Prism Terminal detects them" is therefore not parity. DECISION: open decision O21.
- [ ] Windows tools (where, powershell, cmd, wsl) start by full System32 path, never a bare name (core/main/sysTools.ts SYS; shells.test).
- [ ] Saved shell id resolves via shellById: saved, else pwsh, else powershell, else first; the UI names a shell only by id (shells.ts shellById; shells.test).
- [ ] PowerShell 7 startup: `-NoLogo -NoExit -Command` with PredictionSource History, `EnableScreenReaderMode:$false` in try/catch, then file style and prompt hook (shells.ts).
- [ ] Windows PowerShell startup: `-NoLogo -NoExit -Command` with file style and prompt hook only (shells.ts).
- [ ] cmd has no arguments; its folder report rides the PROMPT environment variable (shells.ts; terminal.ts ptyEnv).
- [ ] PS prompt hook wraps the existing prompt once (captured in a global), falls back to `PS> `, prints OSC 9;9 only for the FileSystem provider (core/main/termPrompt.ts; termPrompt.test).
- [ ] cmd PROMPT is `ESC]9;9;$P ESC\` followed by the user's PROMPT or `$P$G` (termPrompt.ts cmdPrompt; termPrompt.test).
- [ ] PS file style (where `$PSStyle` exists): directories bold blue text, no background; executables, symlinks, extension colours, table header, custom header label and FormatAccent cleared (termPrompt.ts; termPrompt.test).
- [ ] parseOsc9 reads only payloads starting `9;`, strips one pair of quotes, accepts only absolute drive or UNC paths; ConEmu progress is not a report (core/shared/termCwd.ts; termCwd.test).
- [ ] decideFollow classifies a cwd against a root as same, inside or outside, case-insensitive, slash-normalised (termCwd.ts; termCwd.test).
- [ ] cdCommand: `Set-Location -LiteralPath '<path>'` plus CR for pwsh and powershell; `cd /d "<path>"` plus CR for cmd, refused if the path holds `%` or `"`; nothing for WSL or bash (termCwd.ts; termCwd.test).
- [ ] PowerShell path quoting doubles the single quote and the curly quotes U+2018, U+2019, U+201A, U+201B (termCwd.ts; termCwd.test #7).
- [ ] The cd line is written only at an idle prompt with nothing typed and no agent running (core/renderer/lib/termActivity.ts idleAtPrompt; TerminalPanel OSC 9 handler). DECISION: open decision O1.
- [ ] Environment for every pty: `TERM=xterm-256color`, `COLORTERM=truecolor`; NO_COLOR removed; FORCE_COLOR removed only when 0, false or none (core/main/terminal.ts ptyEnv; terminal.test).
- [ ] Session identity markers are removed: CLAUDECODE, CLAUDE_CODE_CHILD_SESSION, CLAUDE_CODE_SESSION_ID, CLAUDE_CODE_ENTRYPOINT, CLAUDE_CODE_EXECPATH, CLAUDE_CODE_MESSAGING_SOCKET, CLAUDE_CODE_MESSAGING_TOKEN, CLAUDE_PID, CLAUDE_PLUGIN_DATA, CLAUDE_EFFORT, CODEX_COMPANION_SESSION_ID, CODEX_COMPANION_TRANSCRIPT_PATH; other CLAUDE_CODE_* config is kept (terminal.ts SESSION_MARKERS; terminal.test).
- [ ] Everything else in the inherited environment passes through untouched (terminal.test).
- [ ] FORCE_HYPERLINK is never set; a user's own value passes through, 0 included (terminal.ts ptyEnv; terminal.test).
- [ ] Each shell gets its own random tab token `FLARE_TAB_ID`; an inherited value is never kept (terminal.ts ptyEnv; terminal.test #323).
- [ ] Spawn cwd is the tab's root; default size 80 x 24; a size asked for before the shell existed is applied at birth (terminal.ts spawnPending, desiredSize).
- [ ] A spawn for an id that is live or pending is refused, the first wins (terminal.ts spawnTerm; terminal.sessions.test).
- [ ] A tab closed while its shell starts: no shell starts, or the started one is killed and nothing registered (terminal.ts killedWhilePending; terminal.sessions.test).
- [ ] Warm shells: at most two, one per tab root (case-insensitive), started ahead for the active tab; adopted only when shell kind and plugin answer match; banner replay capped at 65536 chars; never adopted for a resume; removed by identity, every eviction kills (terminal.ts prewarmShell, spawnPending; terminal.sessions.test).
- [ ] Shell output is coalesced into batches every 8 ms per tab; a batch is flushed before the exit message (terminal.ts OutputBatcher; terminal.test).
- [ ] Exit: session removed, exit crumb logged, the renderer told (terminal.ts; TerminalPanel SessionRoute).
- [ ] A failed spawn writes red "Could not start the shell." into the tab and stops any resume skeleton (TerminalPanel.tsx createSession).
- [ ] After a spawn resolves the real cols and rows are sent once more (TerminalPanel createSession).
- [ ] Resume launch passes the id in the environment, never the command line: PowerShell appends `& { $r = $env:FLARE_RESUME_ID; Remove-Item Env:FLARE_RESUME_ID; claude --resume $r }`; cmd runs `/K set FLARE_RESUME_ID=& claude --resume %FLARE_RESUME_ID%`; codex runs `codex resume --last`; WSL and other shells are left alone (terminal.ts withResume; terminal.test).
- [ ] Every shell of a kind starts with the same command line (regression-rules same-command-line, #180).
- [ ] Pty options: xterm-256color, ConPTY on, bundled `conpty.dll` and `OpenConsole.exe` loaded first; a startup check fails loudly if missing (terminal.ts PTY_OPTS; regression-rules bundled-conpty).

### Pty, resize, lifetime

- [ ] ConPTY sends nothing on a resize, so the cursor's logical line is carried across it: only on the normal screen and only with no printed text below the cursor line; the line is rewritten at the new width and the cursor put back on the same character by cell count (TerminalPanel.tsx fitKeepingCursorLine; termCells.ts cellFrom; e2e promptLayout). DECISION: open decision O3, confirm still needed with alacritty_terminal reflow.
- [ ] Wrapped lines reflow the way ConPTY does (windowsPty conpty build 22621 in xterm); the prompt line returns without holes (TerminalPanel createSession).
- [ ] A refit skips hidden or zero-size panels; while the room animates the terminal keeps its columns, is clipped, and fits once at the end (TerminalPanel refit; core/renderer/lib/fitHold.ts; fitHold.test).
- [ ] A refit suppresses output scoring for 800 ms (termActivity.ts suppressActivity).
- [ ] pty resize rejects cols under 2, rows under 1, non-integers; a refused resize retries up to five times at 300 ms while still wanted (terminal.ts resizeTerm).
- [ ] A tab shown again gets one more fit and a full row repaint a frame later (TerminalPanel refit; e2e hiddenResize).
- [ ] Scrollback is 10000 lines per tab (TerminalPanel new Terminal options).
- [ ] A tab switch keeps each terminal alive (scrollback, selection, alternate screen); detached, never disposed; the shell runs on unseen and output keeps being parsed (TerminalPanel detach comment).
- [ ] Kill order: remove the session, dispose subscriptions, then kill the pty (terminal.ts killTerm).
- [ ] A kill is remembered until native exit lands: exit code polled every 20 ms for 5 s then once a second; a stuck kill is logged once; a late exit is still heard (terminal.ts killPty, pollExitCode; terminal.test).
- [ ] Quit waits for every killed shell to be gone with a 3 s cap, spares included (terminal.ts shellsGone, killAll; terminal.sessions.test; regression-rules quit-waits-for-shells).
- [ ] Closing a tab kills only that tab's shell; late output for a closed tab leaves nothing behind (termActivity.ts forgetSession; termActivity.test).
- [ ] Throughput: a runaway `cat` of 100 MB keeps the UI responsive and Ctrl+C reaches the shell (roadmap Review Focus).
- [ ] No blocking call on a live path: probes, stats and spawns run on workers; blocking calls are marked `sync-ok` and checked by a test (docs/build-and-conventions.md; CLAUDE.md "Main never waits on the disk").

### Cells and the grid model

- [ ] A row's text is built cell by cell: empty cells are spaces, wide-char spacers are nothing, an index maps cell to text position (core/renderer/lib/termCells.ts cellText; termCells.test).
- [ ] textEnd is the cell just past the last printed cell (termCells.ts).
- [ ] An emoji is two UTF-16 units in one cell pair and a link after it keeps its edges (termCells.test).
- [ ] cellFrom walks back over wrapped rows, a cursor past the text moves right with wrapping, never above row 0 (termCells.test).
- [ ] logicalLine joins a wrapped line from any of its rows; charAtCell clamps to the line (termCells.test).
- [ ] Reading a row into cells reuses one scratch cell and copies each value out (termCells.test).

### Activity, bell, bus

- [ ] Typing is heard from keys, never from the terminal's own replies; plain-text payloads (no leading ESC) such as an IME commit count as typing (TerminalPanel onKey/onData; termActivity.ts looksTyped; regression-rules typing-on-onkey).
- [ ] Input echo: output within 350 ms of a keystroke is echo, not agent work; typing keeps renewing; a session never typed into has no window (termActivity.ts inputEcho; termActivity.test).
- [ ] Suppression windows expire on schedule, default 800 ms (termActivity.test).
- [ ] Typing ends an agent's birth (termActivity.ts markTouched; termActivity.born.test).
- [ ] An agent's startup paint ends at the first silence over 1500 ms (QUIET_GAP); late detection costs nothing; a session with no agent is never suppressed (termActivity.born.test).
- [ ] idleAtPrompt is true only when a prompt report arrived after the last keystroke (termActivity.test).
- [ ] forgetSession clears touched, last input, suppression, resume, prompt, birth and output clocks (termActivity.test).
- [ ] Bell: at most one per 1000 ms per tab reaches the host; never a sound; the taskbar flashes only while unfocused (core/renderer/lib/termReplies.ts BELL_GAP_MS, bellGate; src/main/bellFlash.test; e2e bell).
- [ ] The bus publishes only: keys pressed (for the question indicator, nothing kept), the shell's folder, its title, the agent's hook signal, which tabs are resuming, and the visible bottom text rows (core/renderer/lib/termBus.ts; regression-rules replies-only-when-asked). Flare uses typed channels.
- [ ] The app answers a program only what it asked: OSC 52 reads are never answered; colour and version replies only on request (core/renderer/components/termReplyHandlers.ts; termReplies.ts; regression-rules replies-only-when-asked).
- [ ] XTVERSION: CSI > q answers the terminal's name, version and renderer, and the colours, only to the asker (PRIVACY.md; e2e XTVERSION; core/shared/termVersion.ts; termVersion.test). DECISION: confirm the Flare name string and format.
- [ ] Program-selection copy notice: a drag that changes the clipboard shows "Copied" once, a late copy is heard, a plain click or unchanged clipboard is silent, a double click counts, a mouse-owning terminal is not watched (core/renderer/lib/appCopyNotice.ts; appCopyNotice.test).
- [ ] Diagnostics never hold typed text, clipboard or audio (design spec Constraints).

## M3: Tabs, restore, single instance, window state

### Tab model and strip

- [ ] A tab is one shell plus its folder; two tabs may share a folder; a folder may be opened again as a new tab (src/renderer/src/lib/tabs.ts addTab; tabs.test).
- [ ] A new tab is appended and becomes active (tabs.ts addTab).
- [ ] Closing the active tab focuses the right neighbour, else the left, else nothing; closing a background tab keeps the active one (tabs.ts closeTab; tabs.test).
- [ ] Ctrl+Tab / Ctrl+Shift+Tab step next and previous with wrap (tabs.ts stepTab; tabs.test; App.tsx 'next-tab').
- [ ] Ctrl+1 to Ctrl+9 select tab 1 to 9; a number past the last does nothing and the key is not taken (keymap.ts 'jump-tab').
- [ ] Reorder by dragging the tab with the pointer, not HTML5 drag; a press must travel 4 px (DRAG_SLOP) before it is a drag, below that it is a click (src/renderer/src/lib/chromeLayout.ts; components/useTabReorder.ts).
- [ ] The carried tab is clamped to its row or column; neighbours slide 170 ms (CARRY_SLIDE); box geometry is frozen when the drag starts (useTabReorder.ts).
- [ ] The drop slot follows the carried tab's leading edge so first place is reachable even when the carried tab is wider (lib/tabDrop.ts dropSlot; tabDrop.test #125).
- [ ] reorderTabs corrects the index for the removed slot on a move right; a drop past the end appends (tabs.ts reorderTabs; tabs.test).
- [ ] Keyboard focus held before a drag is given back on the next frame after it ends (useTabReorder.ts endDrag).
- [ ] Middle-click closes a tab via auxclick button 1, not mousedown (components/tabList.ts auxClose).
- [ ] Right-click on a tab: "Close tab" (hint = close chord) and, for a folder tab only, "Copy folder path" (components/tabMenus.tsx useTabMenus).
- [ ] A tab's tooltip is the whole folder path; while Failed applies it adds "Failed: <kind>" (tabList.ts tabTitle).
- [ ] Tab label is the last path segment; two different folders sharing a name both get " - <parent>"; two tabs in the same folder keep the plain name; comparison ignores case, slash direction, trailing separators; a drive root is labelled by its letter (tabs.ts tabLabels, folderKey; tabs.test).
- [ ] Settings tab is labelled "Settings" (tabs.ts tabLabels).
- [ ] Tab name is centred; the name button has a minimum of 4 characters (components/TabStrip.tsx min-w-[4ch]).
- [ ] Tab width is fixed: flex basis 150 px, floor 84 px; all tabs one width, shrinking equally; no Dynamic/Fixed setting (TabStrip.tsx; stripPlan.ts TAB_FLOOR; two-apps.md EVERY TAB IS ONE WIDTH; e2e tabWidth).
- [ ] Overflow in three steps (lib/stripPlan.ts planStrip), only when room is short: step 0 all named if n x 84 fits; step 1 resting tabs become 40 px icons (ICON_TAB); step 2 tabs that need you become icons, newest first; step 3 resting tabs move to the +N list newest first, then tabs that need you. The front tab is never an icon or hidden; order never changes. +N button is 72 px (MORE_BUTTON). Room = strip width - 4 - plus width - 4 (stripPlan.test; e2e manyTabs).
- [ ] Icon tab: 40 px, 18 px agent icon, agent-state mark at the corner, name as aria-label and tooltip, closes by menu, middle-click or Ctrl+W (TabStrip.tsx).
- [ ] +N button reads "+N", aria-label and title "N more tabs", shows up to four dots in the needing tabs' mark colours, click toggles the list (TabStrip.tsx data-tab-more).
- [ ] +N list has a "Find a tab" input; tabs needing you first; Enter picks the first kept row; Escape closes; Ctrl+Shift+K opens it with every tab listed (components/TabOverflowList.tsx; TabStrip.tsx).
- [ ] Front tab carries a 2 px accent rule on its top edge and brighter name ink (TabStrip.tsx "The active mark").
- [ ] Every tab, the last too, has a faint 1 px divider on its right edge over full strip height; no line between tabs otherwise (TabStrip.tsx data-tab-sep; e2e tabLooks, edges).
- [ ] Close X: 10 px glyph in a 16 px box on the strip, 18 px in the sidebar; visible on the front tab and on hover, right of the ring; the ring stays on hover and the X slides in over a 150 ms width transition (components/tabParts.tsx TabClose, TabEnd).
- [ ] Tab icon: agent icon at the left, cog for Settings, 18 px, no box (tabParts.tsx TabIcon, TAB_ICON).
- [ ] A folder dropped from Explorer onto the tab row opens a new tab even if another tab shows it; a file opens the folder that holds it (tabList.ts folderDrop; regression-rules).
- [ ] A shell's folder change (OSC 9;9) updates tab cwd and label; an identical report returns the same state and saves nothing (tabs.ts setCwd; tabs.test).
- [ ] A shell that exits closes its tab without asking (App.tsx onTermExit; e2e exitClosesTab).
- [ ] Every tab shows its agent state through the shared tab-look rule: working outranks question outranks failed outranks done (components/tabLooks.ts resolveTabMark; colours belong to M6).
- [ ] Zero tabs: strip and sidebar draw nothing; the start screen carries the + path (TabStrip.tsx, TabSidebar.tsx).
- [ ] DECISION: roadmap M3 lists "rename"; Prism Terminal v0.55.6 has no rename (a tab name is always its folder name). Add as a new feature (record in docs/features) or drop from M3.
- [ ] Tabs are not imported from Prism Terminal; a fresh Flare starts with its own tabs.json (design spec Migration).

### Vertical tabs (sidebar, #162)

- [ ] Tab position setting: horizontal (default) or vertical; unknown reads horizontal and is never written back; label "Tab position", sub "Tabs along the top, or down the side."; own Flare key (lib/tabLayoutPrefs.ts; tabLayoutPrefs.test; settings/appOptions.ts).
- [ ] Sidebar mode open (default) or collapsed; unknown reads open (lib/sidebarPrefs.ts validSidebarMode).
- [ ] Sidebar width: initial 220 px, range 150 to 420, whole pixels, junk reads 220 (sidebarPrefs.ts SIDEBAR_WIDTH; sidebarPrefs.test).
- [ ] Ctrl+Shift+B toggles open and collapsed, only while tabs are vertical; no collapse button (keymap.ts 'sidebar'; keymap.test; e2e verticalTabs).
- [ ] With agent hotkeys on, Ctrl+S is a second sidebar key, vertical only (keymap.ts 'sidebar'.agent).
- [ ] Row is 46 px tall; square icon band a shade darker than the card; name above a dim folder line (components/TabSidebar.tsx ROW_H, data-icon-band, data-tab-folder).
- [ ] Folder line: parent folder of the tab's folder, home as "~", a drive root as the upper-case letter (lib/sidebarRows.ts folderLine; sidebarRows.test).
- [ ] Find a tab in the sidebar: a row is kept when every word appears in label or folder, any order, any case; Enter fronts the first kept row, clears and blurs; Escape clears; filtering hides without reordering; reorder is refused while a query is typed (sidebarRows.ts findKeeps; TabSidebar.tsx refuse).
- [ ] Collapsed: sidebar takes no room; slides over the terminal while the pointer is in the left zone (24 px, below the 36 px bar), stays while within 4 px of its right edge, opens after 60 ms, closes 100 ms after leaving (lib/revealZones.ts sideWants; hoverReveal.ts REVEAL; revealZones.test; hoverReveal.test).
- [ ] The slide takes 240 ms (cubic-bezier .16,1,.3,1); the terminal is not resized during it and fits once at the end (+40 ms) (sidebarPrefs.ts setSidebarMode; e2e verticalTabs).
- [ ] Pinning or unpinning slides sidebar and its room together (sidebarPrefs.ts sliding flag).
- [ ] Resize grip on the sidebar's right edge (6 px target, 1 px line turning accent while dragged), clamped live to 150-420, stored on release, works while slid out (TabSidebar.tsx resize).
- [ ] An open sidebar draws no edge of its own; the terminal draws the edge along the sidebar's side and into the bar (TabSidebar.tsx; e2e verticalTabs).
- [ ] Sidebar New tab button at the foot: label "New tab", title "New tab (Ctrl+T). Right-click for recent folders"; left click instant new tab, right click opens the places menu (TabSidebar.tsx data-sidebar-new).
- [ ] Vertical tabs: title-bar buttons sit in a bar beside the sidebar, the sidebar starts at the window top, only the terminal is below the bar (two-apps.md NO TITLE BAR OF ITS OWN; e2e titleBar, verticalTabs).
- [ ] Sidebar ground is one OKLab step darker than the card, black kept (lib/sidebarGround.ts; sidebarGround.test).
- [ ] fitHold: the terminal holds its size while the sidebar animates (core/renderer/lib/fitHold.ts; fitHold.test).

### New tab, places, recents

- [ ] Ctrl+T opens a new tab (keymap.ts 'new-tab'; the match does not exclude Shift, so Ctrl+Shift+T also opens one).
- [ ] Mode "folder" (default): opens in the chosen folder, else the user's folder, asking nothing (App.tsx newTab; lib/newTabPrefs.ts).
- [ ] Mode "ask": a native folder picker parented to the main window; cancel opens nothing; defaultPath is the given folder if it exists (App.tsx newTab; src/main/index.ts dialog.pickFolder).
- [ ] Settings "Folder for new tabs": segmented "Open in a folder" / "Ask where each time"; sub "Opens in your user folder." / "Opens in the folder you chose." / "Asks for a folder each time."; "Use my user folder" shows only when a folder is set; "Choose folder..." in folder mode (settings/TerminalPage.tsx; appOptions.ts newtab-mode).
- [ ] Choice stored as mode plus folder; empty folder means the user's folder, resolved by the backend (newTabPrefs.ts; newTabPrefs.test).
- [ ] Prewarm: in folder mode the next shell starts ahead of the click with the same shell kind and agent-hook setting (App.tsx prewarm; e2e newTabFolder).
- [ ] Right-click on + (or sidebar New tab) opens the places menu: pinned folders in pin order, then the 5 newest unpinned recents (RECENT_SHOWN); none gives a disabled "No recent folders" (core/renderer/lib/recentRoots.ts plusMenuList; components/tabMenus.tsx).
- [ ] Places rows show a folder glyph and label; same-name rows add "  ·  <parent>" (two spaces each side) (recentRoots.ts recentLabels; recentRoots.test).
- [ ] Each row has a pin toggle: title "Unpin" or "Pin to this menu"; toggling keeps the menu open and re-reads so the pinned row moves up (tabMenus.tsx trailing).
- [ ] History: at most 12 (CAP), newest first, case-insensitive dedupe, reuse moves to front; closing a tab does not forget; forgetting removes exactly that one; nonsense storage reads empty (recentRoots.ts; recentRoots.test).
- [ ] Pins: toggle appends or removes, case-insensitive; a pinned folder is never also a recent row; a pin outlives history (recentRoots.pins.test).
- [ ] Every folder a shell is spawned in is remembered; opening a place opens a tab and prewarms (App.tsx spawnSession, openRecent).
- [ ] Recents and pins live in Flare's settings store (PT used localStorage prism.recentRoots, prism.pinnedRoots).

### Start screen

- [ ] Shown with no tabs: after an empty restore and after the last tab closes; the window stays (components/EmptyState.tsx; e2e lastTab).
- [ ] Layout: app mark 84 px in the accent, name 22 px semibold, nav rows 36 px with 16 px glyphs and 13 px text, then a footer (EmptyState.tsx).
- [ ] Row "New terminal" with the new-tab chord hint; same code as the + button (EmptyState.tsx data-start-new).
- [ ] Places divider then one row per place (pins first): folder glyph, name, last two parent folders (ellipsis if more), "Pinned" tag, full path tooltip; a click opens a shell there (EmptyState.tsx whereabouts).
- [ ] Divider then "Settings" with the cog and hint Ctrl+, (data-start-settings).
- [ ] Footer: "GitHub" link opened via the http(s)-only opener, and "Version <x>" at the right (data-start-version).
- [ ] No greeting or explanation; it is not a first-run page (EmptyState.tsx).

### Tab restore

- [ ] tabs.json (profile) holds `{tabs:[{cwd, agent?}], active}`, written 400 ms after a tab-list report, atomically (temp then rename) (src/main/tabsStore.ts createTabsStore; atomicWrite).
- [ ] Every close, quit, session-end and window close flushes the pending save synchronously (tabsStore.ts flush; tabsStore.test).
- [ ] Reading keeps only entries with a non-empty cwd string; more than 50 is cut to the first 50 (MAX_TABS) (tabsStore.ts parseTabs).
- [ ] An agent other than claude or codex is dropped, the tab returns as a plain shell (core/shared/types.ts RESUMABLE_AGENTS).
- [ ] Active index kept only as an in-range integer, else 0 (tabsStore.test).
- [ ] Unreadable, non-JSON or wrong-shape files read as an empty strip, no crash (tabsStore.test).
- [ ] Loading inside the 400 ms window returns the pending state (tabsStore.ts load; tabsStore.test).
- [ ] Tab reports are ignored until the first restore has answered (src/main/index.ts awaitingRestore).
- [ ] planRestore: a missing folder is dropped silently; order kept; active index moves down for dropped earlier tabs; a dropped active tab makes the first survivor active; empty gives 0 (src/main/planRestore.ts; planRestore.test).
- [ ] Claude resume: newest session of the folder; two claude tabs in a folder take newest and second newest in strip order; lookup keyed by lower-cased folder; no session means a plain shell, never `--continue` (planRestore.ts; planRestore.test).
- [ ] A tab that hosted no agent never resumes (planRestore.test).
- [ ] Codex resume uses the marker `codex:last` and runs `codex resume --last`; the shell still starts in the tab's folder (core/main/agentResume.ts CODEX_RESUME; planRestore.test).
- [ ] Only interactive transcripts count: first 4096 bytes read; a queue-operation first line or an entrypoint other than cli is dropped (agentResume.ts HEAD_BYTES; two-apps.md IT RESUMES ONLY YOUR OWN CONVERSATIONS).
- [ ] The session lookup runs off the UI thread, 16 stats at a time, tabs settled as answers arrive (agentResume.ts; two-apps.md).
- [ ] The resume command is the only thing written into a shell, as the startup command; the id is shape-checked; the id rides in `FLARE_RESUME_ID`, not the command line (regression-rules).
- [ ] Peek at first frame: the saved strip is drawn as placeholders ('agent' with skeleton, 'shell' bare) before any check; settled in place by the answer (lib/restorePlan.ts peekState; restorePlan.test).
- [ ] settleRestore: restored tabs take over placeholders in place; a gone folder removes its placeholder; a placeholder closed before the answer stays closed and spawns nothing; meanwhile-opened tabs are kept; handed-over folders come after (restorePlan.test).
- [ ] Front tab after restore: a user-picked front that survives wins, else the restored front, else the last (restorePlan.ts).
- [ ] Every restored shell is spawned at restore time, front or not (App.tsx restore).
- [ ] Restore runs once per window (App.tsx restore guard).
- [ ] A restoring agent tab shows a ring at its end in the close's place (TabStrip.tsx loadingIds; tabParts.tsx TabLoading).
- [ ] Launch folders from argv join after the restored tabs and the last is in front (index.ts tabsRestore).
- [ ] A saved folder on a gone drive may take seconds to answer; the check runs in parallel off the UI thread and the reply is not held (index.ts tabsRestore comment).
- [ ] Hide and show within 400 ms restores the newest tab list (tabsStore.ts load).

### Single instance and command line

- [ ] A second launch hands its folders to the running app and exits 0; the window is shown and raised; the folders become new tabs even if shown already (index.ts requestSingleInstanceLock; e2e handoff).
- [ ] Every existing argv path is taken in the order typed, once each (case-insensitive), at most 32 (ARGV_MAX) (src/main/argv.ts pathsFromArgv; argv.test).
- [ ] Switches starting with `--` are skipped (argv.test).
- [ ] The app's own exe and, unpackaged, its entry script are skipped by identity, not position (argv.ts ownFiles; argv.test).
- [ ] Relative paths resolve against the folder the command was typed in and are stored absolute (argv.ts; argv.test; regression-rules).
- [ ] A drive root arriving as `C:"` becomes `C:\` (argv.ts unquoteArg; argv.test).
- [ ] A missing path is ignored; folders and files accepted; a file becomes its folder once per folder (argv.ts foldersFromArgv; argv.test).
- [ ] Before ready, a second launch's folders are queued; with no window one is created; queued folders are sent after the restore reply (index.ts second-instance, pendingFolders).
- [ ] Raise: restore from minimised, show, brief always-on-top toggle, focus, restore the previous value (index.ts raise()).
- [ ] Under the test flag the window is shown inactive, never raised, not focusable, not on the taskbar (index.ts reveal, createWindow).
- [ ] Single-instance lock is Flare's own and never collides with Prism Terminal's.

### Close flow

- [ ] Every route out ends at one close handler: title-bar X, Alt+F4, taskbar, a quit (index.ts win.on('close'); src/main/closeFlow.ts).
- [ ] Closing the window quits; window-all-closed quits; closing the last tab does not (regression-rules; e2e lastTab, quitManyShells).
- [ ] One close rule, not a setting: a plain shell closes unasked; a tab hosting an agent asks, working or idle (core/renderer/lib/agentClose.ts asksBeforeClosingTab; agentClose.test).
- [ ] The window close is held only while an agent is working (agentClose.ts holdsWindowClose; agentClose.test).
- [ ] Titles: "Close the tab and end the agent?", "Stop the agent and close?", "Stop the agent and close the window?", "Stop the agent and install the update?" (agentClose.ts closeQuestionTitle).
- [ ] Bodies: idle "<Agent> is running in <tab>. Closing the tab kills the shell, and the session with it."; working "<Agent> has been working for <duration> in <tab>" plus ", and N other tab(s) is/are working too" and ". Closing kills the shell, and the answer with it."; install "Installing restarts the app, which kills the shell, and the answer with it." (App.tsx Dialog body).
- [ ] Agent names Claude, Codex, "The agent"; working time is the longest-running agent's (agentClose.ts AGENT_NAMES; agentClose.test).
- [ ] Buttons: "Cancel" and a primary "Close tab", "Close window" or "Install and restart" (App.tsx choices).
- [ ] Window close with an agent working: veto, raise, ask; go-ahead closes; Cancel leaves open (index.ts; App.tsx onCloseRequest).
- [ ] A quit interrupted by the veto is finished by the go-ahead; after Cancel a later close is ordinary (closeFlow.test).
- [ ] An install answered in advance goes straight through once (closeFlow.test).
- [ ] One question at a time: while a question or update window is up, tab chords swallow the key except close-tab (keymap.ts underModal; regression-rules ONE QUESTION AT A TIME).
- [ ] Closing a tab kills its shell, disposes the session, forgets its state; Settings has no shell (App.tsx closeNow).
- [ ] On close every shell is killed and the app waits for all to be gone, 3 s cap, on the child's exit code, not the exit event; ten live shells quit three times with code 0 (index.ts will-quit, shellsGone; e2e quitManyShells).
- [ ] Windows shutdown or log-off flushes tabs.json and the diag log synchronously (index.ts session-end).
- [ ] Ctrl+W closes a tab (match also takes Ctrl+Shift+W). DECISION: PT's label says Ctrl+Shift+W; Flare label should read Ctrl+W, matching the stated rule (keymap.ts 'close-tab').

### Most recent tab switching

- [ ] "Tab switching": "In order" (default) or "Most recent"; unknown reads In order and is never written; sub "Which tab comes next when you switch." (lib/tabSwitchPrefs.ts; tabSwitchPrefs.test).
- [ ] Used list, front first; a tab moves to the top once (lib/tabMru.ts touchMru; tabMru.test).
- [ ] syncMru: closed tabs leave, new tabs join after known ones in strip order, empty starts from front then strip (tabMru.test).
- [ ] A walk starts at the first Ctrl+Tab of a hold from a snapshot; further presses go further back and wrap; Ctrl+Shift+Tab walks from the far end (tabMru.ts startWalk, stepWalk; tabMru.test).
- [ ] The list reorders only when the hold ends: Ctrl released, focus lost, another Ctrl chord, a click on a different tab, a tab opened or closed mid-hold (tabMru.test no ping-pong; walkHolds).
- [ ] Any other activation counts as a use at once; the list is not saved between launches (App.tsx commitWalk).
- [ ] Ctrl+PgDn / Ctrl+PgUp step tabs while agent hotkeys are on (keymap.ts 'next-tab'.agent).

### Settings tab

- [ ] One Settings tab, reused; id "settings", kind 'settings', empty folder, no shell; never persisted (tabs.ts openSettings; tabs.test).
- [ ] The cog, Ctrl+, and the start screen row toggle Settings; closing returns to the previous tab (most recent) else the close neighbour (tabs.ts toggleSettings; e2e cogToggle).
- [ ] shellTabs() excludes Settings so its close never asks and kills nothing (tabs.ts).

### Window state

- [ ] window.json holds width, height, x, y, maximised; written 400 ms after resize, move, maximise, unmaximise and synchronously on close (src/main/windowState.ts watchWindowState).
- [ ] A minimised window is not saved; the normal bounds are saved, not the maximised size (windowState.ts).
- [ ] Defaults 1180 x 780 for no, broken or foreign file (windowState.test).
- [ ] Minimum 560 x 400, enforced on read and as the OS minimum (MIN_WINDOW; windowState.test).
- [ ] Width and height rounded; non-finite falls back (windowState.test).
- [ ] Saved position kept only if it overlaps an attached display's work area; negative coordinates on a second monitor kept (windowState.test).

### Chords (the key table)

- [ ] One chord table feeds the key handler and the terminal's "owns this key" check (keymap.ts CHORDS, appOwnsKey; keymap.test).
- [ ] Matching uses the key name and, for letters, the physical key code (keymap.ts letter()).
- [ ] Tab chords are taken in the capture phase and never reach the shell (App.tsx onKey, hit()).
- [ ] Chord list: F1 help, F11 full screen, Ctrl+Tab, Ctrl+1..9, Ctrl+, , Ctrl+Shift+K, Ctrl+Shift+B, Ctrl+T, Ctrl+W, Ctrl+F (not in a full-screen program), Ctrl+Shift+F always (keymap.ts CHORDS).
- [ ] Agent hotkeys setting (default on): extra keys are the app's when on, the shell's when off (keymap.test).
- [ ] Displayed labels come from the table; no component types a shortcut by hand (keymap.test).
- [ ] Accepted cost: Ctrl+W and Ctrl+T take the shell's delete-word and Claude Code's task list; Ctrl+Backspace does delete-word (regression-rules).
- [ ] Tooltip placement and hidden-title rule: a tooltip only when text is cut or the label is hidden (core/renderer/lib/hiddenTitle.ts; hiddenTitle.test).

## M4: Themes, fonts, material

### Theme list (39 presets, source core/renderer/lib/termTheme.ts TERM_PRESETS)

Each line: id, name, background, foreground, cursor, accent, palette (curated = the sixteen are copied
verbatim from termTheme.ts; derived = from deriveAnsi(bg, fg)).

- [ ] pt-default, PT Default, #171717 #dedacf #bbbbbb accent #fe8f34, curated.
- [ ] prism, Prism, #0b0b0f #e7e7ee #7c7cf0 accent #5b5bd6, derived.
- [ ] pitch, Pitch, #000000 #efe7da #ec9448 accent #ec9448, derived.
- [ ] cinder, Cinder, #383c44 #f3eadc #ec9448 accent #ec9448, derived.
- [ ] graphite, Graphite, #1f1f1f #d6d6d6 #d6d6d6 accent #9a9a9a, derived.
- [ ] volt, Volt, #050706 #eef2e6 #d8ff26 accent #d8ff26, curated.
- [ ] paper, Paper, #f6f4ee #2a2620 #3a63c2 no accent (chrome takes palette blue), derived.
- [ ] mist, Mist, #e4e4e2 #26262a #4a4a52 accent #5c5c66, derived.
- [ ] umber, Umber, #2b211b #e8d8c0 #d9b38c accent #c4966a, derived.
- [ ] fawn, Fawn, #e6d8c0 #3d2a1f #7a4e35 accent #8a5a3c, derived.
- [ ] rosewood, Rosewood, #24161d #f2dce5 #ee85aa accent #e4719a, derived.
- [ ] blossom, Blossom, #fbeef2 #3b2230 #c64a78 accent #c64a78, derived.
- [ ] moss, Moss, #141c16 #d8e4d2 #8cc07a accent #78ad66, derived.
- [ ] sage, Sage, #e9efe5 #223024 #4c7d47 accent #4c7d47, derived.
- [ ] phosphor, Phosphor, #050d06 #7ef08a #7ef08a accent #3fcf5a, derived.
- [ ] amber, Amber, #110b02 #ffbf5a #ffbf5a accent #f0a030, derived.
- [ ] marine, Marine, #012456 #eeedf0 #ffd866 accent #ffd866, derived.
- [ ] retro, Retro, #352879 #b3adf0 #b3adf0 accent #9d93ea, derived.
- [ ] campbell, Campbell, #0c0c0c #cccccc #ffffff accent #3b78ff, curated.
- [ ] ocean, Ocean, #0a2630 #d3eef2 #5fd4e0 accent #3fb8c8, derived.
- [ ] garnet, Garnet, #1f0d10 #f3dadd #ff6b7d accent #e2475c, derived.
- [ ] plum, Plum, #1c1226 #e9ddf5 #c792ea accent #b07ce0, derived.
- [ ] lavender, Lavender, #f1edfa #2e2540 #7c5cc4 accent #7c5cc4, derived.
- [ ] sky, Sky, #eaf4fd #1c2b3a #2f7bd6 accent #2f7bd6, derived.
- [ ] peach, Peach, #fdeee3 #3e2a20 #c9632f accent #c05e2c, derived.
- [ ] butter, Butter, #fbf5d8 #3a3320 #9a7300 accent #8f6b00, derived.
- [ ] mint, Mint, #e5f6ef #1d3129 #1a8a64 accent #17805c, derived.
- [ ] solarized-dark, Solarized Dark, #002b36 #93a1a1 #93a1a1 accent #268bd2, curated.
- [ ] gruvbox-dark, Gruvbox Dark, #282828 #ebdbb2 #ebdbb2 accent #fe8019, curated.
- [ ] tokyonight, Tokyo Night, #1a1b26 #c0caf5 #c0caf5 accent #7aa2f7, curated.
- [ ] catppuccin-mocha, Catppuccin Mocha, #1e1e2e #cdd6f4 #f5e0dc accent #cba6f7, curated.
- [ ] catppuccin-latte, Catppuccin Latte, #eff1f5 #4c4f69 #8839ef accent #8839ef, curated (cursor is mauve on purpose).
- [ ] monokai, Monokai, #272822 #f8f8f2 #f8f8f0 accent #a6e22e, curated.
- [ ] kanagawa, Kanagawa, #1f1f28 #dcd7ba #c8c093 accent #7e9cd8, curated.
- [ ] cobalt, Cobalt, #193549 #ffffff #ffc600 accent #ffc600, curated.
- [ ] synthwave, Synthwave, #262335 #f0eff1 #ff7edb accent #ff7edb, curated.
- [ ] horizon, Horizon, #1c1e26 #d5d8da #e95678 accent #e95678, curated.
- [ ] nord, Nord, #2e3440 #d8dee9 #d8dee9 no accent, curated.
- [ ] dracula, Dracula, #1e1f29 #f8f8f2 #bbbbbb no accent, curated.
- [ ] Exactly 39 presets, ids and names unique (termThemeRetired.test "is thirty-nine").
- [ ] Wall order: Custom first when saved, then PT Default, then orderTermThemes: neutral grounds (OKLab chroma below 0.0125) first, then coloured, each black to white by OKLab L, ties keep input order (core/renderer/lib/themeOrder.ts; themeOrder.test snapshot; two-apps.md THE WALL'S ORDER IS MEASURED).
- [ ] Wall cards are one height, selected or not, in rows of two or more (e2e themeCards).
- [ ] Fresh profile wears PT Default (src/renderer/src/termHost.ts defaults; e2e).
- [ ] Core fallback id is 'prism'; unknown ids and legacy 'style' resolve to host default, then 'prism', then first preset (termTheme.ts resolveSavedTheme; termTheme.test).
- [ ] Retired ids map to a kept id at read time (bright-lights, high-contrast to pitch; molokai to monokai; jellybeans, hybrid, afterglow, materialdark to graphite; wombat to pt-default; onehalfdark, ayu, denim, argonaut, adventuretime, ink to prism; espresso to umber; zenburn to moss; full map core/renderer/lib/termThemeRetired.ts; termThemeRetired.test).

### Resolution, Custom, contrast

- [ ] Preset resolution: fg and cursor as written; ansi = curated or deriveAnsi(bg, fg); then legiblePalette(ansi, bg); selection = `${cursor}55` (termTheme.ts resolveSavedTheme).
- [ ] Custom resolution: bg opaque; fg legibleOn 4.5; cursor legibleOn 3; selection as set else floored cursor + 55; ansi composited then legiblePalette (termTheme.ts resolveCustomTheme).
- [ ] Alpha 1 changes nothing: opaque Custom is byte-identical to before (termTheme.snapshot.test, golden fixtures).
- [ ] Alpha never reduces legibility: every preset saved as Custom at alpha 0.1 to 0.9 keeps text at min(4.5, opaque) - 0.02, cursor min(3, ...), all sixteen 3:1 - 0.01 (termTheme.legible.test).
- [ ] Every value handed to the renderer is #rrggbb or #rrggbbaa, never rgba() (termTheme.test).
- [ ] Custom with nothing saved resolves like 'prism'; a hex8 cursor derives a 9-char selection from the opaque floored cursor; a chosen selection is kept (termTheme.test).
- [ ] Custom storage: one slot; corrupt JSON reads as none; bg or fg not a colour means none; bad cursor falls back to fg; bad selection absent; bad ANSI entries dropped; hex6 and hex8 accepted, stored lowercase (core/renderer/lib/termLook.ts customTermTheme; termLook.test).
- [ ] Custom keeps the accent of the theme it was made from; an older Custom without accent asks for none (themeDraft.test; termTheme.ts presetAccent).
- [ ] Custom Save also carries indicator, done and question colour choices, see-through level, accent, selection; never font or size (termLook.ts CustomTermTheme; e2e themeSwitch).
- [ ] Text floor 4.5:1; mark floor 3:1 (cursor, sixteen, accent line, icons, quiet ink); sidebar tab names 7:1; name flip under 2:1 (termTheme.ts; chromeTheme.ts TEXT_FLOOR, QUIET_FLOOR, SIDE_TEXT_FLOOR; termAnsi.ts ANSI_CONTRAST_FLOOR).
- [ ] WCAG luminance and contrast ratio exactly as termAnsi.ts luminance and contrastRatio.
- [ ] ensureContrast mixes toward white or black (whichever contrasts more) in 0.08 steps, up to 24, else the endpoint (termAnsi.ts; termAnsi.test).
- [ ] deriveAnsi: dark when luminance(bg) <= 0.35; black = mix(bg, fg, 0.18 dark / 0.25 light); brightBlack = ensure(mix(bg, fg, 0.55)); white = ensure(mix(fg, bg, 0.18)); brightWhite = ensure(fg); twelve hue seeds adapted at 3:1 (termAnsi.ts).
- [ ] Hue seeds: red #e05561, green #8cc265, yellow #d1a54b, blue #4aa5f0, magenta #c162de, cyan #42b3c2; bright #ff616e, #a5e075, #f0c366, #4dc4ff, #de73ff, #4cd1e0 (termAnsi.ts SEEDS).
- [ ] legiblePalette moves only failing colours, only as far as the floor (termTheme.legible.test; Dracula bright blue stays).
- [ ] Every preset: sixteen clear 3:1, text 4.5:1, cursor and accent 3:1 (termTheme.legible.test).
- [ ] normalizeColor accepts #rgb, #rrggbb, #rrggbbaa (alpha dropped), rgb(), rgba(); else falls back, never NaN (termAnsi.ts; termAnsi.test).
- [ ] nameInk and switchKnob flip only under NAME_FLIP = 2:1 (nameInk.ts, switchKnob.ts; their tests).
- [ ] Picked ground: text ensured 4.5, cursor and sixteen 3, derived selection follows the floored cursor, a chosen selection is kept; a ground equal to the theme's own returns the theme untouched (core/renderer/lib/termGround.ts; termGround.test; e2e pickedGround).
- [ ] Mode is measured: light when WCAG luminance of the ground is above 0.4; constants DEFAULT_GROUND #0b0b0f, ON_DARK_INK #0b0b0f, LIGHT_GROUND 0.4 (core/shared/ground.ts; ground.test; chromeTheme.test).
- [ ] DWM frost light/dark uses Rec.709 luma above 0.5, a different measure, kept as is (src/main/material.ts isLightBg; material.test).
- [ ] Colour maths: parseColour, format, composite, legibleOn, inkOn, selectionFor, colourCommit, alphaPct, snapAlphaStep, alphaHex (core/renderer/lib/colour.ts; colour.test).
- [ ] OKLab conversions toLab, fromLab with channel clipping (core/renderer/lib/oklab.ts).
- [ ] Theme card palette is cached once per preset; cards read five colours with fallbacks (core/renderer/settings/theme/palette.ts cardAnsi).

### Chrome tokens

- [ ] Window chrome derives only from the terminal theme (bg, fg, accent or blue or cursor, else INDIGO #5b5bd6) with every ink at a floor (src/renderer/src/lib/chromeTheme.ts; regression-rules theme-drives-chrome).
- [ ] Token set: the 32 CHROME_COLOUR_TOKENS (--p-bg, --p-bg-solid, --p-side, --p-side-flat, --p-secondary, --p-title, --p-tabs, --p-tab-active, --p-text, --p-text-soft, --p-dim, --p-dim2, --p-accent, --p-accent-solid, --p-accent-hi, --p-on-accent, --p-icon, --p-hover, --p-hover-hi, --p-divider, --p-line, --p-track, --p-control, --p-preview, --p-sel-bg, --p-tree-folder, --p-switch-knob, --p-pressed, --p-pressed-ink, --p-side-text, --p-side-dim), all #rrggbb or #rrggbbaa (chromeTheme.test).
- [ ] flat = mix(bg, ensure(fg,bg,4.5), 0.04); secondary = sidebarGround(bg); control = mix(bg, fg, light 0.09 / dark 0.035); fg floored against flat, secondary, control, bg (chromeTheme.ts).
- [ ] Accent order: chosen, theme.blue, theme.cursor, INDIGO; first visible (3:1 on bg and flat) wins else the first lifted to 3:1; a chosen accent that is not visible is only moved (liftToFloor, accentLift.test).
- [ ] accent-hi = floorOn(mix(solid, fg, 0.25), 3); accent-solid is the line; on-accent ink is the better of #ffffff and #0b0b0f (chromeTheme.ts).
- [ ] White on a strong mid colour: if white clears 3:1 and 1.5x white beats dark ink, deepen the accent by OKLab L steps of 0.005 (max 40) until white reaches 4.5:1 (chromeTheme.ts; e2e accentAsk).
- [ ] Selected-row fill (--p-sel-bg) = accent moved 4% toward ink's far end up to 15 steps until on-accent reaches 4.5:1 (chromeTheme.ts).
- [ ] dim, icon, text-soft, dim2, stage, track, hover, hover-hi, pressed segment and pressed-ink formulas as chromeTheme.ts (light/dark values per source).
- [ ] --p-switch-knob #0b0b0f on dark, #ffffff on light, flipped only under 2:1 on the ON track (switchKnob.ts; switchAccent.test).
- [ ] An ON switch track wears --p-sel-bg (regression-rules AN ON SWITCH WEARS THE THEME'S ACCENT).
- [ ] Sidebar inks: side-text held to 7:1 on sidebar ground and on the 18% accent pill; side-dim to 4.5:1 (chromeTheme.ts sideInks).
- [ ] Glass (see-through below 100%): --p-bg carries ground alpha, --p-bg-solid stays flat, --p-side and --p-tab-active are transparent, --p-secondary keeps the alpha, --p-title and --p-tabs are the sheet (chromeTheme.test; chromeTheme.opacity.test).
- [ ] A see-through accent is a fill with its alpha; text chosen by selectionFor over each ground; under glass the fill is flattened; --p-accent-solid stays opaque for lines (chromeTheme.test; e2e accent).
- [ ] Chosen rail page is a grey hover-hi fill, never the accent; row buttons, pressed segment, dictation buttons are neutral greys; only Save changes and on switches wear the accent (regression-rules SETTINGS CONTROLS ARE NEUTRAL; neutralControls.test).
- [ ] paintChrome applies once: background pick replaces the theme bg before measuring; sends main the solid ground, edges value and acrylic flag each paint (src/renderer/src/lib/paintChrome.ts).
- [ ] A theme change reaches running programs as a one-column resize nudge, never a key (regression-rules redraw-nudge; e2e termReplies).
- [ ] Mark colour floors live here: working = accent; finished = theme green floored 3:1 (fallback #22c55e if under 1.15 from working); question #3b82f6 floored; failed from [theme red, #ef4444, theme magenta, #d946ef] apart from both (src/renderer/src/lib/agentColors.ts; agentColors.test; consumed in M6).

### Window colours: Background, Accent, Secondary

- [ ] Three optional overrides, unset means the theme's own; stored #rrggbb or #rrggbbaa lowercase (src/renderer/src/lib/colourPref.ts, accentPrefs.ts, backgroundPrefs.ts, secondaryPrefs.ts and their tests).
- [ ] Each shows the theme's own colour until picked; then a plain "Reset" forgets it (e2e accent).
- [ ] Background is opaque in the store, no alpha; Accent has alpha; Secondary styles the sidebar and title bar beside it and sits above Accent (e2e opacityAlpha, windowColoursSave).
- [ ] Picking a theme forgets Background, Accent, Secondary (unless Custom, which restores what it kept) (regression-rules theme-switch-colours; e2e themeSwitch, windowColoursSave).
- [ ] Save changes keeps picked window colours with Custom; stored as prism.window.customColours, validated per colour, junk reads none (lib/customWindowColours.ts; customWindowColours.test).
- [ ] An unpicked working indicator wears the accent; a near-black red accent is lifted to a red, not grey (e2e accent, accentAsk).

### Theme switch, Save changes, editor draft

- [ ] Save changes lights only when settings deviate from the selected theme's stock; comparison by JSON of the extras in field order; a moved see-through slider lights it; a picked Background never does (core/renderer/settings/theme/useTermSetup.ts; useTermSetup.test).
- [ ] Save changes saves the whole setup as Custom (opaque ground, level in extras) then selects Custom (useTermSetup.ts saveAsCustom).
- [ ] A theme pick while dirty shows Save as Custom, Discard, Cancel; Escape and outside press cancel; a clean pick lands at once (regression-rules theme-switch-colours; e2e themeSwitch, accentAsk).
- [ ] A pick resets the extras: indicator to host default, agent colours to '', acrylic removed, see-through 0; rainbow, done, question, failed, hooks are not reset; font and size never (termLook.ts resetTermExtras; termLook.test).
- [ ] Applying a Custom writes the indicator only where drawn, valid colours only, see-through level or acrylic (termLook.ts applyCustomExtras; termLook.test).
- [ ] The editor draft: while open the theme in force is the draft for terminal and chrome; in memory only; Save makes it Custom; Cancel drops it; other cards and the preset cache never see it; it survives a tab switch with picker and focus (termLook.ts themeDraft; themeDraft.test; e2e themeEditorLive).
- [ ] Escape closes the picker and restores the opened colour; open and close with no change stores nothing (e2e colourPicker, themeEditorLive).
- [ ] Saving the editor keeps the origin theme's accent (e2e themeEditorLive).
- [ ] Appearance order: Window section (tab position, tab switching, edges), then Theme section (theme row with Save, wall, See-through row, Background, Accent, Secondary) (e2e themeEditorLive; two-apps.md WHAT NO THEME OWNS SITS ABOVE THE THEME WALL).

### Colour picker

- [ ] One picker everywhere with alpha on every colour; sliders named "Saturation and brightness", "Hue", "Alpha"; no native colour input (two-apps.md; core/renderer/settings/ColourPicker.tsx; e2e colourPicker).
- [ ] Formats cycle HEX, RGBA, HSLA then HEX; opens in HEX; junk reads HEX; choice remembered (core/renderer/lib/colourFormat.ts; colourFormat.test).
- [ ] RGBA shows `rgba(r, g, b, a)`, HSLA `hsla(h, s%, l%, a)`; a typed hsla stores the same hex8 (e2e colourPicker).
- [ ] Stored form is six lowercase digits when opaque, eight otherwise; alpha is round(a * 255) and every typed byte reads back (colour.test).
- [ ] Shift+Left walks alpha in steps; one press down from solid lands on 95; below the steps a press moves 1% (colour.test snapAlphaStep).
- [ ] colourCommit: nothing typed, same colour in another form, or junk commits nothing; no alpha in the code keeps the alpha in force (colour.test).
- [ ] Settings ask nothing nobody changed: tabbing through a following row never pins it (regression-rules settings-ask-nothing; fields.test).
- [ ] Eyedropper depends on Chromium EyeDropper; DECISION: open decision O7.

### Fonts and size

- [ ] 15 faces with fallback stacks, default Cascadia Mono: cascadia, cascadia-code, consolas, jetbrains, jetbrains-nf, fira, source, lucida, courier, ibm-plex, roboto-mono, ubuntu-mono, hack, iosevka, dejavu (termLook.ts TERM_FONTS; the inventory counts 14 or 15, the source has 15 ids). Unknown id reads cascadia (termFontId).
- [ ] Each font option is drawn in its own face (e2e term-font-family).
- [ ] Font and size belong to no theme (termLook.test).
- [ ] Base 13 px at 100%, px = round(13 * pct / 100); sizes 50 to 200 in tens, label "N%"; an off-list value reads the nearest step, out of range or junk reads 100 (termLook.ts FONT_PCTS, termFontPct; termLook.test).
- [ ] Ctrl+scroll zooms this session only, 7 to 64 px, one per notch, never saved, never sent to the program; a plain wheel stays with the program or scrollback (TerminalPanel wheel listener, zoomSession).
- [ ] Changing size or family refits every open session (TerminalPanel).
- [ ] A missing face falls through its stack quietly; theme-wall previews use installed monospace faces (two-apps.md).

### Acrylic, see-through, material

- [ ] Material exists only on Windows 11 build 22000+ (src/main/material.ts acrylicOk; material.test); on Windows 10 the row reads "Needs Windows 11." and is disabled.
- [ ] Acrylic is on only when wanted, supported, not full screen, and no full-screen change pending (material.ts apply).
- [ ] Material before colour: set the backdrop first, then the ground colour ('#00000000' for acrylic) (regression-rules material-before-colour).
- [ ] The material is set only on change per window (material.ts worn).
- [ ] Full screen: the window goes opaque before the OS is asked, back on settle (material.ts beforeFullscreen).
- [ ] See-through slider 0 to 70, 0 is solid, label "Off" at 0 else "N%", step 1; ground alpha = 1 - level/100; stored clamped, never stored reads 0 (termLook.ts SEE_THROUGH_TOP, seeThroughLevel; e2e seeThrough). Row "See-through window", sub "How much of the desktop shows behind every surface."
- [ ] Strip, top row, rail and page each show the desktop through one coat at the slider value; a picked colour wears the level (e2e seeThrough).
- [ ] Each theme's own see-through is 0; Custom keeps its own (termLook.ts termExtraDefaults).
- [ ] Legacy import from PT, once before first paint: switch + alpha becomes level round((1-alpha)*100) clamped 1-70; Opacity N becomes alpha byte, clamped 30-100; Custom folds its bg alpha into seeThrough; old keys removed (seeThroughMigration.ts, opacityMigration.ts; their tests; e2e opacityAlpha).
- [ ] Opaque ground under see-through: default dark 0xb9, light 0xd1 by luminance above 0.4; unparseable reads dark (core/renderer/lib/seeThrough.ts; seeThrough.test). Keep for import only.
- [ ] Glass fills are flattened onto the solid ground where text sits (chromeTheme.ts).
- [ ] Not ported: the Prism-style switch and style-owned glass (host specific).

### Window edges and DWM border

- [ ] Edges choice weakest to strongest: None, Faint, Hairline (default), Solid; unknown reads Hairline; nothing stored until chosen (src/shared/windowEdges.ts; windowEdges.test; edgesPrefs.test; e2e edges).
- [ ] Line alphas (ink percent): hairline divider 7 dark / 15 light, line 9 / 17; faint 2.2 / 5 and 3 / 6.5; solid 16 / 26 and 18 / 28; none 0 (windowEdges.ts EDGE_ALPHA; e2e edges).
- [ ] None keeps tokens as transparent colours so layout does not move; only the two line tokens change (windowEdges.test; chromeTheme.test).
- [ ] DWM border step off the ground: hairline 0.13 dark / 0.22 light; faint 0.045 / 0.075; solid 0.30 / 0.44; none no border; maximised or full screen no border (windowEdges.ts EDGE_BORDER_STEP; src/main/windowEdge.ts edgeFor; windowEdge.test).
- [ ] Border re-applied 250 ms after every material or ground change (windowEdge.ts).
- [ ] COLORREF is 0x00BBGGRR; DWMWA_BORDER_COLOR 34, none -2; failures are silent (src/main/dwmHelper.ts; dwmHelper.test).
- [ ] Edges are off under the test flag (windowEdge.ts header).
- [ ] The DWM helper is a direct in-process call in Flare, not a PowerShell helper (dropped launch test).

## M5: Selection, paste, drop, find, links, keyboard

### Selection, copy, click-to-caret

- [ ] Ctrl+C with a selection copies and clears it; without one it is the interrupt (core/renderer/lib/termKeys.ts termKeyAction; termKeys.test).
- [ ] Ctrl+C and Ctrl+V match by letter or physical key code, so Russian and Greek layouts still copy (termKeys.ts isKey; termKeys.test rule 11). DECISION: open decision O2.
- [ ] A copy shows the "Copied" badge at the bottom centre and leaves by itself (TerminalPanel copyText, announceCopied; e2e selectionEdit).
- [ ] A program's own selection (fullscreen Claude Code): a left press without Shift is followed; Ctrl+C is the program's copy and the copy is announced (TerminalPanel copyDown, watchAppCopies).
- [ ] Backspace/Delete over a selection on the edited line sends Left/Right to the selection end then one DEL per character, clears the selection, swallows the key; elsewhere the key is the shell's (core/renderer/lib/termSelectionEdit.ts selectionDeleteKeys; termKeys.test; e2e selectionEdit).
- [ ] selectionDeleteAllowed needs: mouse tracking none, normal screen, not scrolled back, cursor visible, nothing printed below, selection on the cursor's logical line (termSelectionEdit.test).
- [ ] A selection with nothing deletable clears and still swallows the key (TerminalPanel deleteSelection).
- [ ] selectionDeleteKeys counts non-spacer cells, stops at the text end, uses application cursor form when asked (termSelectionEdit.test).
- [ ] Plain left click on the prompt line moves the caret with Left/Right (ESC [ C/D, or ESC O C/D under application cursor) (core/renderer/lib/termClickCaret.ts; termClickCaret.test).
- [ ] A click is refused unless: button 0, click count under 2, no modifiers, no drag or selection, mouse tracking none, normal screen, not scrolled back, cursor not hidden, nothing printed below, on the line, not on a link (termClickCaret.ts caretClickAllowed; a test per refusal).
- [ ] The click is read after the terminal's own mouseup, in a timer, so a drag's selection is final (TerminalPanel attachClickCaret).
- [ ] Wide characters count once; a click on a wide char's second half lands on it; past the text goes to the end; wraps of one logical line are crossed (termClickCaret.test).
- [ ] Mouse reporting: when a program asks for the mouse it owns clicks, SGR press and release are sent, pointer is not a text caret (e2e mouseReport).
- [ ] Right-click menu fits what was clicked: Copy link leads over a link, Copy only with text marked, no Copy on plain text; path menu: Open, Show in Explorer, Copy path (TerminalPanel termContextAt; two-apps.md THE MENU FITS WHAT WAS CLICKED).
- [ ] Terminal right-click rows in order: path rows (Open folder / Open, Show in Explorer, Copy path), link rows (Copy link, Open link), Copy (Ctrl+C) with a selection, Paste (Ctrl+V), Find in scrollback, Command help last while on; no Close tab; bound to the session it opened on (src/renderer/src/components/termMenu.tsx; termMenu.test).

### Keyboard input

- [ ] Key decision order: non-keydown to the terminal; Ctrl+C with selection copies; Backspace/Delete with selection deletes; a host chord is taken by the host and never reaches the pty; Shift+Enter is a newline only where an agent runs; Ctrl+V pastes; Ctrl+Shift+V pastes plain; else the terminal (termKeys.ts; termKeys.test).
- [ ] Ctrl+V with Alt held is not a paste (termKeys.test).
- [ ] Ctrl+V is one paste: the key is consumed so the native paste does not run too; Ctrl+Shift+V and right-click Paste use the same single path (TerminalPanel case 'paste'; e2e paste).
- [ ] Ctrl+Shift+V pastes plain sanitised text even when an image rides along (TerminalPanel 'paste-plain').
- [ ] Shift+Enter where an agent runs sends the newline key (Claude: LF; Codex and others: backslash then CR) and is consumed (TerminalPanel 'newline'; termPaste.test; e2e imagePaste).
- [ ] Agent-armed state gates Shift+Enter: armed by Claude's title glyphs or Codex's Action Required before the poll, never by a bare braille spinner; a prompt report disarms at once (core/renderer/lib/agentArm.ts; M6 owns the arming).
- [ ] A live shell gets the keyboard when its tab is shown; focus returns after a strip click or drag (TerminalPanel focusTermSession).
- [ ] Caret hold for IME and screen readers: a key opens a 250 ms window; the lowest cursor position reached is the input line; while the program parks the cursor above on the normal screen the text input stays on the input line; released on alternate screen, resize, buffer change, scrollback (core/renderer/lib/termCaretHold.ts; termCaretHold.test; e2e caretHold).

### Find in scrollback

- [ ] Ctrl+F opens find, except when the focused shell is full screen (alternate screen or mouse tracking); Ctrl+Shift+F always finds (keymap.ts 'find'; two-apps.md FIND IS CTRL+F; e2e findKey).
- [ ] Accepted cost: at a readline prompt Ctrl+F no longer means cursor-right (two-apps.md).
- [ ] Find bar at top-right, input 180 px, placeholder and aria-label "Find in terminal", no shadow, 20 px buttons (core/renderer/components/TermFind.tsx).
- [ ] Incremental search keeps the current match while it still matches; Enter next, Shift+Enter previous, Escape closes; the bar stops keys reaching the terminal (TermFind.tsx).
- [ ] Previous and next disabled with no hits; readout: empty for empty query, "none", "N+" past the count threshold, "i of n" (TermFind.tsx).
- [ ] Highlight colours: match background #4a5160, border #6b7482, ruler #8a93a3, active match in accent-hi (else #5b5bd6) (TerminalPanel findOptions).
- [ ] Searches the whole scrollback, wrapped lines and the alternate screen; closing clears highlights (TerminalPanel).

### Links

- [ ] Left click opens a link; right click opens nothing (two-apps.md A RIGHT-CLICK ON A LINK OPENS NOTHING, #95).
- [ ] Link text is http/https only; runs to whitespace, quote, angle bracket or backtick; trailing `. , ; : ! ?` and an unopened closing bracket given back; a bare scheme is not a link (core/renderer/lib/termLinks.ts findLinks; termLinks.test).
- [ ] Link colour #79a7d8 moved only as far as needed to 4.5:1; where it cannot, tinted 35% toward blue; never the text colour (termLinks.ts linkColor; termLinks.test).
- [ ] Links are painted, not hover-only: each row gets link colour and a faint underline that never blocks a click; on the alternate screen per drawn row (core/renderer/lib/termLinkPaint.ts; regression-rules links-painted #10; e2e links).
- [ ] Scrollback links painted once and ride a marker; the live screen repainted from the last finished line down (termLinkPaint.test).
- [ ] Link colours repaint from scratch when ink or theme changes, once per frame at most (termLinkPaint.test; relinkSoon).
- [ ] A full pass never holds the UI: live screen first, then scrollback bottom-up in slices of at most 8 ms including painting; cancel and trim handled (core/renderer/lib/linkScanPlan.ts sliceRows; linkScanPlan.test; e2e linkPaintStall).
- [ ] A hidden tab's painter does nothing and catches up when attached (termLinkPaint.test).
- [ ] OSC 8 links open on left click, http(s) only, no confirm dialog (TerminalPanel linkHandler; e2e osc8Links).
- [ ] An OSC 8 label showing another host opens the address it shows; unparseable uri opens the label (core/renderer/lib/termOsc8.ts osc8Target; termOsc8.test).
- [ ] OSC 8 parse: uri after params, semicolons in the uri kept, empty uri closes, non-OSC-8 not handled; isWebLink http/https any case (termOsc8.test; core/shared/webUrl.test).
- [ ] OSC 8 spans follow the stream: 1 to 16 row labels, non-blank, survive a wrap, forgotten when the anchor is gone, newer replaces older over the same cells, cap drops oldest, alternate screen spans leave with it, reflow clears all (termOsc8.ts Osc8Spans; termOsc8.test).
- [ ] Path candidates: no whitespace/quote/`<>|*?`; leading `([{` and trailing `.,;:!)]}` and `:line[:col]` stripped; length 2 to 260; must contain a letter; leading `-` is a flag; needs a separator or `.ext` (1 to 8 alphanumerics); bare drive refused; URLs left to the link finder (core/renderer/lib/termPaths.ts findPaths; termPaths.test).
- [ ] A path is painted and clickable only when it exists; answers cached per folder; a "no" forgotten after 15 s (core/renderer/lib/termPathLinks.ts; regression-rules path-links #99).
- [ ] Relative paths resolve against the shell's folder (OSC 9;9) and folders the tab has shown; a bare name only in the shown folder; the folder itself is never a link (termPathLinks.test).
- [ ] Path answer cache: at most 5000 entries; a "no" lives 15 s (NO_TTL_MS); asks batched every 40 ms (BATCH_MS); a relative path tries at most 4 anchor folders (ANCHORS_MAX) and 4 ancestors (ANCESTORS); a tab teaches at most 256 folders (TAUGHT_MAX), each looked at again once in the tab's life; a folder holds the cwd case-insensitively (termPathLinks.ts holdsCwd, parentDir, anchorBases; termPathLinks.test).
- [ ] Paths asked in pieces of at most 300 (PATHS_MAX) one after another; a short answer is never read as missing past its end (core/shared/termLimits.ts; termPathLinks.test).
- [ ] A found path repaints only the asking tab and lines; a multi-folder tab settles after one wake per folder; a closed tab forgets what it taught (termPathLinks.test).
- [ ] The backend re-resolves the text and never trusts the UI: `~` is home, UNC refused, NUL refused, over 1024 chars refused, relative needs a drive-letter folder, `\foo` takes the shell's drive (core/main/termPathOpen.ts resolveTermPath; termPathOpen.test).
- [ ] Path checks are metered: at most 3 stats in flight; a stat over 750 ms marks its drive slow for 60 s; a click is one unmetered stat (termPathOpen.ts createStatGate; termPathOpen.test).
- [ ] Opening: a file opens in its own app; a folder in the user's chosen folder handler (Explorer default) via Start-Process with the path in the environment, falling back once to the shell opener; anything runnable is shown selected in Explorer, never run; Reveal always selects (termPathOpen.ts openTermPath, openFolderDefault, isRunnable; termPathOpen.test).
- [ ] Runnable extensions never run: exe com scr pif cpl msc msi msp msix msixbundle appx appxbundle appref-ms application gadget bat cmd ps1 psm1 psd1 ps1xml vbs vbe js jse wsf wsh ws wsc sct hta jar py pyw pyc pyz rb pl sh bash ahk au3 lnk url website reg inf ins isp scf shb shs library-ms settingcontent-ms search-ms diagcab xbap vsto (termPathOpen.ts RUNNABLE; one test per extension).
- [ ] Under the test flag nothing opens outside the app; opened paths are recorded (e2e pathLinks).

### OSC 52

- [ ] OSC 52 is write-only: Pc empty or "c"; a read query "?" is swallowed and never answered (core/renderer/lib/termOsc52.ts parseOsc52; termOsc52.test).
- [ ] Empty data (a clear) is ignored; non-base64 and bad length refused; invalid UTF-8 refused; valid UTF-8 round-trips (termOsc52.test).
- [ ] Cap 1,048,576 characters (OSC52_MAX): one over refused, far-over refused without decoding (termOsc52.test; core/shared/termLimits.ts).
- [ ] An accepted write goes to the clipboard through the backend and shows the Copied badge (TerminalPanel attachReplies; e2e termClipboard).
- [ ] Clipboard write refuses non-string, empty, or over 4000 chars (UI copies) and over OSC52_MAX (terminal), refusing not trimming (core/main/ipc.ts writeCapped; ipc.clipboard.test).
- [ ] Copies try the page clipboard first, then the backend's write (copyNotice.ts).

### Paste and drop

- [ ] decidePaste order: copied files become quoted paths typed as text; else an image sends the agent's image key only, beating text riding along; else text sanitised and pasted; else nothing; text of only controls pastes nothing (core/renderer/lib/termPaste.ts decidePaste; termPaste.test).
- [ ] Image key is exactly one key: ESC v (Alt+V) for Claude (including WSL once armed), ^V (0x16) for Codex, others and plain shells (termPaste.ts imagePasteKey; termPaste.test; e2e imagePaste).
- [ ] Text paste is one bracketed paste when the shell asked for it (TerminalPanel pasteFrom).
- [ ] sanitizePaste removes every C0 control except TAB, LF, CR, plus DEL (0x7f) and C1 CSI 0x9b, so no ESC and no early bracketed-paste end (termPaste.ts sanitizePaste; termPaste.test).
- [ ] quotePath: PowerShell single quotes with inner quotes doubled (curly included); bash single quotes with `'\''`; cmd and unknown keep double quotes; quotePaths joins with spaces (termPaste.ts; termPaste.test).
- [ ] Quoting follows the session's shell (core/shared/help/shells.ts shellOfShellId: cmd, bash for WSL or ids containing bash, else PowerShell).
- [ ] Dropping a file on a tab types its quoted path, never Enter, only over a shell (regression-rules file-drop-and-menu #16; e2e dropAndMenu).
- [ ] Paste reads the clipboard asynchronously (TerminalPanel pasteHere readClipboardAsync).
- [ ] Right-click Paste calls the same pasteHere as Ctrl+V (core/renderer/lib/termBus.ts registerPaste, pasteInto).
- [ ] Spoken text pastes into the shell in front when dictation started, newlines and U+2028/2029 become spaces, never Enter (TerminalPanel pasteSpokenText; termBus.ts).
- [ ] One Ctrl+V pastes once, one Ctrl+Shift+V once, right-click Paste once (e2e paste).
- [ ] The real clipboard is saved and restored; the test flag uses an in-memory stand-in (src/main/e2eClipboard.ts; e2eClipboard.test).
- [ ] Hostile text (control chars, bracketed-paste escapes, multi-line) and dropped paths with spaces or quotes are inert until the user acts (roadmap Review Focus).

## M6: Agent indicator and hooks

### Indicator model

- [ ] One state per tab: working, question, failed, done or none; priority working > question > failed > done (src/renderer/src/components/tabLooks.ts).
- [ ] Working shows on every tab, front included; question on any tab even the front, only when not working; failed only with its switch on and not working or question; done only with its switch on and none higher (tabLooks.ts; useAgentIndicator.ts).
- [ ] A tab is "present" (agent in it) when the poll reports one, a Claude/Codex title claims one, or a hook arrives (useAgentIndicator.ts).
- [ ] Only Ring and Bar are offered; stored `minimal` is Bar; `off`, `full` or anything else reads Ring (core/renderer/lib/termLook.ts readIndicator; termHost.ts). Flare has no Off and no Full.
- [ ] Ring (default): one mono mark at the tab's end in the close button's place, in the name's ink; spinner while working (1 s), breathing dot for question (2 s), check for finished, cross for failed (core/renderer/lib/tabMark.ts; components/TabMark.tsx; styles/marks.css).
- [ ] Ring stays on hover and close slides in after it; idle tab shows neither (e2e tabLooks).
- [ ] Bar: working is a running bar along the tab foot (1.25 s) in the accent; question, done, failed are a static 3 px line in their colour (tabMark.ts resolveTabMark; e2e indicator, attention).
- [ ] The front tab never gets a finished or failed mark while looked at; marks are news (done, failed) or state (question) (agentHookState.ts raisedWhileSeen).
- [ ] Motion: question breathes 2 s; finished-with-rainbow flows (line x 8 s, y 7 s; fill 14 s); working run 1.25 s (marks.css; regression-rules finished-and-question).
- [ ] Reduced motion stills every mark; a minimised window pauses every mark (marks.css; marks.test; src/renderer/src/lib/marksPause.ts; marksPause.test).
- [ ] 18 px tab icon: Claude tab = Clawd (#d77757, 18x6 quadrant grid), codex = OpenAI mark, plain shell or other agent = prompt icon, Settings = cog (core/renderer/lib/agentIcon.ts; agentIcon.test; e2e tabLooks).
- [ ] Failed tooltip: path newline "Failed: <kind words>" with the 12 kind words (rate limit, servers overloaded, sign-in failed, organisation not allowed, account on hold, billing, invalid request, model not found, server error, reply too long, cloud credentials, unknown error); unknown kind shows underscores as spaces; none shows "Failed" (tabList.ts tabTitle; agentHookSignal.ts failedLabel).

### Mark colours

- [ ] Working = accent unless picked; finished = theme green floored 3:1, fallback #22c55e if under 1.15 from working; question = #3b82f6 floored; failed = first of [theme red, #ef4444, theme magenta, #d946ef] floored and apart from working and finished (src/renderer/src/lib/agentColors.ts; agentColors.test).
- [ ] marksApart: saturation above 0.25 and hue 20 degrees or more apart, or RGB distance 80 or more (agentColors.ts).
- [ ] floorMark mixes toward black (light ground) or white (dark) in 2% steps up to 50, first step reaching 3:1 on every ground, else nearest (core/renderer/lib/markColours.ts; markColours.test).
- [ ] Rainbow finished mark (default on): #12cee5 #2179fa #945af5 #e84fd2 #fb7f6c #f0a934 #e8d021 flowing; off is plain finished colour, still (markColours.ts ICON_RAINBOW; rainbowGradient).
- [ ] Fill colours are opaque over the ground; name ink on a fill keeps text colour if contrast >= 2 else flips; rainbow ink uses the worst of seven at 25/50/75% (markColours.ts opaqueOver; nameInk.ts; markPalette.ts; their tests).
- [ ] Bar's run uses the name's ink on a working fill (markPalette.ts).
- [ ] Settings colour rows "Agent working colour" (sub "Follows the accent."), "Agent finished colour" ("Follows the theme green."), "Agent question colour" ("Follows the theme."); a picked colour shows "Your own colour." and a Reset link; Save changes in the section heading lights with the theme's (settings/MarkColoursSection.tsx; options.ts).
- [ ] Stored colour is canonical 6 or 8 hex, anything else removes the choice (termLook.ts writeChoice, agentColourChoice).

### Claude Code plugin and hook signal (core/claude-plugin; Flare name flare-status)

- [ ] Manifest `.claude-plugin/plugin.json` name flare-status, version 1.0.0, a description (agentHookSignal.test "has a manifest Claude Code accepts"; claudePlugin.ts PLUGIN_NAME).
- [ ] hooks.json has 25 command entries, each `cmd.exe /d /c call "${CLAUDE_PLUGIN_ROOT}/hook.cmd" <args>`, timeout 5 s, exec form: working (UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, SubagentStart, PreCompact), question (PermissionRequest, Elicitation, Notification with matchers permission_prompt, elicitation_dialog, elicitation_url_dialog), done (Stop), failed with kind (StopFailure per rate_limit, overloaded, authentication_failed, oauth_org_not_allowed, account_on_hold, billing_error, invalid_request, model_not_found, server_error, max_output_tokens, cloud_credential_error, unknown), failed without kind (StopFailure *, kind "failed"); SessionStart and SessionEnd are not hooked (agentHookSignal.ts header; agentHookSignal.test).
- [ ] hook.cmd is a static echo printing JSON `{"terminalSequence":"ESC]777;flare-agent;state=<state>[;kind=<kind>]BEL"}`, no node, no stdin; kind only on failure hooks.
- [ ] Only Claude's UI writes the sequence; `claude -p` and SDK runs never do (agentHookSignal.ts).
- [ ] The plugin is handed to a shell as `CLAUDE_CODE_PLUGIN_DIRS`: user folders kept in order, ours appended, an inherited copy of ours dropped (identified by plugin.json name), variable removed if empty, key spelling kept (terminal.ts withPluginDir, ptyEnv; terminal.test).
- [ ] Plugin dir shipped only if hooks/hooks.json exists in it (index.ts claudePluginDir); packaged under resources, dev under the repo.
- [ ] A warm shell is adopted only with the same plugin key (terminal.ts pluginKey).
- [ ] OSC 777 parser: payload max 200 chars, split on `;`, first part equals the prefix `flare-agent`, every later part contains `=` at index > 0; state in working|question|done|failed; unknown keys ignored; kind kept only with failed and matching `^[a-z][a-z0-9_]{0,39}$`; Flare must not accept `prism-agent`; non-Flare payloads leave other OSC 777 users alone (core/renderer/lib/agentHookSignal.ts; agentHookSignal.test).
- [ ] Per-tab channel: parser output to the tab's signal channel; the indicator subscribes (termBus.ts onAgentSignal).

### Hook state rules (core/renderer/lib/agentHookState.ts hookStep)

- [ ] Hook working: phase working, clears question, failed, finished (hookStep).
- [ ] Hook question: phase question, raises question, clears failed.
- [ ] Hook done: title still spinning means phase ending (still Working); background work on screen means phase background; else finish (phase done, raises finished, clears question and failed).
- [ ] Hook failed: phase failed keeping a prior kind if none, raises failed AND finished, clears question.
- [ ] Title spinner: while ending becomes background; while question or stopped becomes working; otherwise no change.
- [ ] Idle star: ending or background resolves to finish or background; working becomes stopped (an Esc, no Finished); otherwise no change.
- [ ] Settle: no title within STOP_SETTLE_MS = 1500 ms after a spinning Stop resolves to background or finish.
- [ ] bg-check every BACKGROUND_CHECK_MS = 3000 ms while background; finishes once neither screen nor spinner shows background work.
- [ ] While the question box is on screen the question stands; done raises finished under the question (screenDecides).
- [ ] After a permission Yes the spinner title returns Working; a spinner held by the box is re-read once after 250 ms (useAgentIndicator.ts titleRecheck).
- [ ] A hooked session's idle title after working with no Stop is Esc: idle, stopped, no Finished; a Stop landing after still finishes (agentHookState.timelines.test; e2e agentHooks).
- [ ] A hooked session is never scored from output; the screen is read only to hold a question the hooks raised (useAgentIndicator.ts onTermData).
- [ ] Background work on screen: `· N shell(s)|task(s)|command(s) still running`, or below the last box rule `· N shell(s)|task(s)` or a workflow row `N/M · [Xh][Ym]SSs`; only live marks count (core/renderer/lib/agentBackground.ts; agentBackground.test, screens.json; e2e backgroundWork).
- [ ] A new prompt or working signal clears failed and finished.
- [ ] A question raised on the front tab; looking never takes it down (raisedWhileSeen).

### Question detection and scoring fallback

- [ ] Unhooked sessions read the bottom 16 rows: both "Enter to select" and "Esc to cancel", or "Do you want to ...?" with a "1. Yes" line (core/renderer/lib/agentQuestion.ts looksLikeQuestion; agentQuestion.test).
- [ ] Read 200 ms after the title goes idle and as output arrives while idle; only titled unhooked sessions; a new question cancels pending answer reads (useAgentIndicator.ts checkQuestion).
- [ ] Walking choices is not an answer; answer keys Enter, Esc, Ctrl+C, digits 1 to 9, then the box is read at 250, 1000, 2500, 5000 ms and the question clears only when the box is gone (agentQuestion.ts questionAnswered; useAgentIndicator.ts onTermKey).
- [ ] A question clears on work again, an answer key with the box gone, agent gone, tab closed; looking does not clear it; it keeps its place when focus leaves (e2e attention, agentHooks).
- [ ] Output scoring only for a known agent that is neither titled nor hooked; ignores startup paint, resize suppression, input echo; a run with gaps <= 1500 ms longer than 1200 ms sets working; 2000 ms of silence clears it; event-driven (useAgentIndicator.ts; termActivity.ts).
- [ ] A poll report that finds an agent clears the output run, marks birth, clears working.
- [ ] Title dialects: Claude glyphs and Codex Action Required, titleAgent, titleArmsAgent (core/renderer/lib/agentTitle.ts; agentTitle.test).

### Process poll and detection

- [ ] Polls only if a pty printed since the last look or a session has no answer yet; timer 2500 ms (AGENT_POLL_MIN), wait doubles to 20000 ms while unchanged, resets on change (core/main/agentPoll.ts; agentPoll.test).
- [ ] Only changes are sent; an unchanged answer is re-sent only when asked (agentPoll.ts pollAgentAgain).
- [ ] Forced look (title or hook claims an agent the poll has not reported): at most once per 15 s per tab from the UI, main waits AGENT_FORCE_GAP = 5000 ms (useAgentIndicator.ts askLook; agentPoll.ts).
- [ ] A query with no answer doubles to AGENT_FAIL_MAX = 60000 ms; the third failure in a row is logged once; a failed query spends no tick (agentPoll.ts).
- [ ] Query: native process snapshot (pid, ppid, command line) with prefilter `claude|codex|aider|gemini` then strict match; 30 s timeout; 8 MB cap; off the UI thread (agentPoll.ts).
- [ ] Forget a shell's answer when it ends; warm poll 300 ms after a spawn that really started (agentPoll.ts; ipc.ts pollAgentsSoon).
- [ ] Signatures (agentDetect.ts AGENT_RE): `claude-code` anywhere; `claude`/`claude.exe|.cmd|.ps1` as a path segment followed by separator, quote, space or end; `codex`, `aider`, `gemini` likewise; `@openai/codex`; `gemini-cli`; case-insensitive; first token counts; a folder named Claude is not an agent; `git commit -m claude` is not (core/main/agentDetect.ts; agentDetect.test).
- [ ] Kind: claude wins anywhere under the shell, else codex, else other; BFS under the shell pid, cycles handled (agentDetect.ts treeAgentKind).
- [ ] Resumable kinds are claude and codex; others light the indicator but have nothing to resume (core/main/agentKinds.ts; agentKinds.test).

### Arm, resume, skeleton

- [ ] Armed at tab start if resumed, by the title before the poll, or by the poll; a prompt report disarms at once and asks the poll if it had seen an agent; a resumed agent's first poll "no" is doubted; a title-armed (WSL) agent stays until its title stops naming it; gone drops its theme reports (agentArm.ts; agentArm.test).
- [ ] Shift+Enter newline key and image-paste key per agent (see M5).
- [ ] Resume command only as the shell's startup command, only for a restored tab that had an agent, never for a warm shell; bash/WSL/Git Bash get no resume (terminal.ts withResume). DECISION: keep parity or add (open decision O5).
- [ ] Resume id shape: `codex:last` or `^[0-9a-f][0-9a-f-]{6,62}[0-9a-f]$` (agentResume.ts validResume; agentResume.test).
- [ ] Claude sessions: newest first in `~/.claude/projects/<cwd with non-alphanumerics replaced by '-'>`, up to 32, interactive only (first 4096 bytes) (agentResume.ts claudeSessions; agentResume.test).
- [ ] Saved tabs.json entry has agent claude|codex only when present at the last report; 'other' not saved.
- [ ] Skeleton over a restoring agent tab from the first frame (logo block, name lines, 12 paragraphs of 11 px lines, text colour 7%, pulse; still under reduced motion; 180 ms fade out) (components/ResumeSkeleton.tsx).
- [ ] Clear `ESC[2J ESC[3J ESC[H` written to the grid only, never the shell, just before the title naming the agent (core/renderer/lib/resumeReveal.ts; resumeReveal.test).
- [ ] Agent named by bare `claude` or `codex` or `claude.exe`; a folder called Claude is not the program (resumeReveal.ts namesAgent).
- [ ] Reveal 150 ms (SETTLE_MS) after the agent's own title or alternate screen, or at once on a key, exit or failed spawn, or at 12000 ms (CEILING_MS).
- [ ] A resume is not new work: no Finished, Question or Failed until a turn of its own ends; every restoring tab shows the ring (e2e launchSkeleton; regression-rules #151).
- [ ] Hooked and title-armed Claude question widget extraction: last tab row `←...✔ Submit→` is live; question text from lines after it until the first `\d+.` option, leading `│` stripped, joined with spaces; "Review your answers" gives review; no "Enter to select" gives none (core/renderer/questionWidget.ts; questionWidget.test).
- [ ] Question watch: 150 ms after output settles, rows from baseY minus rows to baseY plus rows, reports only a change, resets when the widget goes, stops on dispose (core/renderer/questionWatch.ts; questionWatch.test).
- [ ] Question payload check: `{text}` trimmed non-blank at most 2000 chars, or `{review: true}` (core/shared/questionShown.ts asShownQuestion).
- [ ] Sonara link: location `SONARA_HOME` else `%LOCALAPPDATA%\Sonara`; runtime.json `http_port` 1 to 65535 and non-empty `token`; POST `http://127.0.0.1:<port>/v1/question_shown` with bearer token and body `{"host_tab":<FLARE_TAB_ID>,"text":...}` or `{"host_tab":...,"review":true}`; 1500 ms timeout; success only on 200; failures quiet; nothing sent without Sonara (core/main/sonaraLink.ts; sonaraLink.test).
- [ ] DECISION: Sonara reads PRISM_TAB_ID today; bridge or wait (open decision O6).

### Close, clock, badge, keys, lifecycle

- [ ] Close question rule and wording: see M3 Close flow (core/renderer/lib/agentClose.ts).
- [ ] Agent clock starts the first tick a session is seen working, forgotten when it stops, exits, closes or the agent leaves; humanFor rounds down: under 10 s "a few seconds", under 60 s "N seconds", under 60 min "N minutes", hours "N hour(s)" or "N hour(s) M minute(s)"; negative clamps 0; never-working is null (core/renderer/lib/agentClock.ts; agentClock.test).
- [ ] Taskbar count: unique tabs carrying a question, failed or done (each if its switch is on and not working); "1 tab needs a look" / "N tabs need a look"; above 9 shows "9+"; zero or setting off clears (src/renderer/src/lib/taskbarBadge.ts attentionCount, badgeText; taskbarBadge.test).
- [ ] Badge drawn at physical size 16 logical px x scale (capped 4, nonsense reads 1), fill #25242c, white number, font 0.8x for one digit and 0.56x for two, nudged down 4%; an overlay only, never a window icon (taskbarBadge.ts badgePixels, drawBadge).
- [ ] Keep agent sessions open (default on): with an agent in the front tab Ctrl+D never reaches it; Ctrl+C is taken unless the agent works or holds a selection; never in a plain shell (keymap.ts 'agent-guard'; agentGuardPrefs.ts).
- [ ] Agent hotkeys (default on): Ctrl+PgUp/PgDn switch tabs; Ctrl+S toggles the sidebar only when vertical; off gives them to the shell (keymap.ts; agentKeysPrefs.ts; e2e agentKeys).
- [ ] Looking = active tab and window focused; a pointer move over an unfocused window counts as the owner back for 1000 ms (MOVE_LOOK_MS) and takes down the front tab's finished or failed mark at most every 500 ms; coming back is a look (useAgentIndicator.ts; e2e agentHooks).
- [ ] Finished-while-away for non-hooked sessions: working stops with nobody looking gives done; hooked sessions get done only from hooks.
- [ ] Done and failed clear on working again, looking, agent leaving; question clears on answer, agent leaving; tab closed drops every mark, timer and record; agent left drops working, question, failed, hooked and title records (useAgentIndicator.ts forget).
- [ ] Per-tab state in one owner struct, forgotten on close; indicator rules live in flare-agent with no UI types (useAgentIndicator.ts; design rules).
- [ ] Diagnostics kinds agent-hook, agent-title, agent-mark, agent-restore: first of a run at once, repeats counted and written on change, close or every 30 s (HOOK_FLUSH_MS), 60 s gap (HOOK_GAP_MS) starts a new line; title none only after 2000 ms; 40 lines per tab per 10 s then `dropped`; rule note within 1 s (WHY_MS); never typed, screen, title text or conversation (core/renderer/lib/agentDiag.ts; agentDiag.test; e2e agentDiag).
- [ ] Settings rows for this area: see M7 Agents page.

## M7: Settings pages

### Frame and navigation

- [ ] Settings is a tab filling the pane; opened from the cog; the page chosen is kept while the tab stays open, resets to Appearance when closed and reopened; memory in the window only, never saved (src/renderer/src/components/settings/Settings.tsx; core/renderer/settings/layout/SettingsFrame.tsx; e2e settingsPage).
- [ ] Pages in rail order: Appearance, Terminal, Agents, Dictation, Diagnostics, About (settingsIndex.ts SETTINGS_PAGES); About sits at the bottom after a spacer.
- [ ] Rail is a nav "Settings pages"; each button has the page id, aria-label, and aria-current on the chosen one (SettingsFrame.tsx railButton).
- [ ] Rail 244 px wide, padding 18 top and 12 sides/bottom, 1 px divider on the side ground; title "Settings" 15 px bold; item 36 px, 10 px padding, 6 px radius, 11 px gap, 17 px icon, 13 px label weight 500; hover --p-hover; chosen --p-hover-hi grey, weight 600, no accent bar (SettingsFrame.tsx; e2e settingsLook).
- [ ] Rail keys: Tab from the find field lands on the first page; Up/Down walk; Home/End jump; Enter opens; Tab then enters the page (SettingsFrame onRailKey; e2e settingsKeys).
- [ ] Page header 24 px bold, letter-spacing -0.02em, line-height 1.15; pane scrolls with hidden scrollbar; inner column max 960 px, padding 26 top, 44 sides, 64 bottom; first section 18 px under the header.
- [ ] Settings font is the system UI stack (Segoe UI Variable Text, Segoe UI, system-ui) whatever the theme (SETTINGS_FONT).
- [ ] Below 760 px of the frame's own width the rail becomes 56 px icons with hover titles and the find field becomes a 40 px magnifier opening a 300 px field; nothing scrolls sideways at 900 or 1600 px (SettingsFrame NARROW/COMPACT; e2e settingsLook).

### Find a setting

- [ ] Field: placeholder and aria-label "Find a setting", 32 px tall, 6 px radius, 14 px magnifier; clear button "Clear the search" only with text; `data-owns-escape` only while text is present (SettingsFrame.tsx; e2e settingsSearch).
- [ ] Index built from rows drawn on this PC (GPU row only with NVIDIA; tab-mark rows where the strip draws them; help row) in page order (coreIndex.ts; settingsIndex.ts ROW_ORDER).
- [ ] Matching: lower-case, accent-folded, query split into words, every word prefix-matches a word in label, subtext, section, page or keywords; rank 0 exact label, 1 all words in label, 2 others; ties keep index order; blank query returns nothing (core/renderer/settings/layout/search.ts; search.test).
- [ ] With text the pane shows header "Results" and one panel of rows: 32 px icon tile, 13 px semibold label, one-line sub, dim "Page, Section" at the right (SettingsFrame results block).
- [ ] Status line role=status "N result(s)" / "No results" exists only while text is typed (PT #58).
- [ ] No match shows "Nothing matches <query>. Try a shorter word."; no rail item is chosen while results show.
- [ ] Field keys: Down marks the first result; Enter chooses it; Escape with text clears. List keys: Down/Up move (Up from first returns to the field); Enter/Space choose; Escape clears and returns to the field (SettingsFrame onFieldKey, onListKey). Hover marks, click chooses.
- [ ] Choosing a result clears the field, switches page, scrolls the row to the pane centre, flashes it (16% accent-hi fading 1.4 s; none under reduced motion) and focuses its first control; a late-drawn row is polled every 60 ms up to 3 s then the section is flashed (layout/flash.ts WAIT_MS).
- [ ] Every indexed row is found by its label, opened, flashed and focused (e2e settingsSearch).

### Layout primitives

- [ ] Section heading 13 px semibold soft ink, sentence case, 8 px above the panel, optional right action; sections 26 px apart; landmark per section with a section id (SettingsSection.tsx).
- [ ] Panel fill 3.6% of the text ink, 1 px line, radius max(4, radius + 3) (PANEL_RADIUS).
- [ ] Row: columns 32 px / 1fr / auto, gap 14, min height 58, padding 10 top/bottom, 16 right, 14 left; hairline starts after the icon column (SettingRow.tsx).
- [ ] Icon tile 32x32, radius max(4, radius - 1), fill 6%, 16 px stroke 1.7 icon (SettingRow.tsx IconTile).
- [ ] Label 13 px weight 600; subtext 11.5 px dim, 2 px under, one line truncated with full text on title (SettingRow.tsx Subtext).
- [ ] Warning subtext amber (#e0a84b mixed 85% with text) with an 11 px glyph; off row 45% opacity, not clickable, aria-disabled; a switch row is clickable as a whole (hover 2.5% tint) and the switch stays the one focusable control.
- [ ] Block rows for walls, swatch grids, model lists, About card (SettingBlock.tsx).
- [ ] Every control is named by its row label and described by its subtext (design spec 1.3).
- [ ] Contrast measured on a dark and a light theme: label 4.5:1, subtext 4.5:1, warning 4.5:1, icon 3:1 (e2e settingsLook).
- [ ] Only Save changes and ON switches take the accent (neutralControls.test; e2e settingsLook).
- [ ] Layout is measured, not only present: row at least 58 px, tile 32, hairline starts 60 px in, no overlap, no sideways scroll (regression-rules page-looks-right #20; e2e options).
- [ ] Switch: role switch, aria-checked, ON wears the accent fill with the theme knob, OFF grey track white knob (core/renderer/settings/fields.tsx Switch).
- [ ] Segmented: buttons with aria-pressed; pressed one step lighter (darker on light) (fields.tsx Segmented).
- [ ] Select: aria-haspopup listbox, aria-expanded, options with aria-selected and a check, closes on Escape (consumed), outside press, blur; no shadow (fields.tsx Select).
- [ ] Save button: accent when dirty, grey "Nothing to save yet" when clean; row button is the grey dropdown look; Reset word is accent-hi 11 px semibold, underline on hover (fields.tsx).
- [ ] Colour field: code field (HEX/RGBA/HSLA) plus swatch opening the popover (square, hue, alpha, eyedropper, opened vs new colour, format toggle) (ColourPicker.tsx).
- [ ] Escape is handled once: find field with text clears, empty reaches the host, a colour popover reverts (SettingsFrame; ColourPicker onRevert).
- [ ] Copy rules: subtexts use letters, digits, spaces, commas and full stops only; no key names (Ctrl, Control, Shift, Alt, Enter, Escape, Esc, Tab, Backspace, Win, F1-F99 as whole words); at most 8 words; labels pass the symbol rule only (core/shared/settingsCopy.ts; settingsCopy.test; options.test). DECISION: See-through sub has 9 words; verify the rule applies (open decision O8).
- [ ] Option lists and row ids are closed lists: one flat line per entry, known section, icon and subtext, unique ids, storage keys exist in their modules (core/renderer/settings/options.ts; options.test; optionKeys.test; appOptions.test). No Opacity row, no confirm-close row, no Tab width, no Tab style row.
- [ ] The Terminal option order per section: term-shell; term-font-family, term-font; agent-indicator, agent-done-on, agent-failed-on, agent-hooks; term-theme, term-acrylic; agent-color, agent-done-color, agent-question-color (options.ts TERMINAL_OPTIONS). App rows: tab-layout, tab-switch, agent-guard, agent-keys, window-edges, window-background, window-secondary, window-accent, newtab-mode, explorer-verb, taskbar-badge, app-version (appOptions.ts).

### Appearance page

- [ ] Window section: Tab position (tab-layout, Horizontal | Vertical), Tab switching (tab-switch, In order | Most recent; sub "The next tab along the strip." / "Back to the tab you used last."), Panel edges (window-edges, None | Faint | Hairline | Solid; sub "Lines between panels and around the window.") (AppearancePage.tsx; appOptions.ts; e2e tabSwitch, verticalTabs, edges).
- [ ] Theme section below: Terminal theme row ("Colours of the terminal and the window.") with Save changes (title "Nothing to save yet" clean / "Saves the theme, agent colours and see-through window as Custom" dirty), the wall, See-through row, then Background ("Behind the text in window and terminal.", no alpha), Secondary ("The sidebar and title bar beside the terminal."), Accent ("Highlights, the active tab and selection.", alpha min 0.1), each with Reset while picked (TerminalThemeSection.tsx; AppearancePage.tsx).
- [ ] Wall: cards 196 px wide in a wrapping grid, each a miniature terminal in its own palette; Custom first then PT Default; "Show all N themes" / "Show fewer" with a collapsed default; selected card has a pencil "Edit <name>" ("Edit colours, saved as Custom") (ThemeWall.tsx).
- [ ] Theme editor is a modal "Edit colours" ("Edit terminal colours") with every colour, live preview, Save as Custom and Cancel; draft seeded from the selected theme; presets never change (ThemeWall.tsx TermThemeEditor).
- [ ] Theme switch ask: Save as Custom (wears the accent), Discard, Cancel; Escape and outside press cancel (ThemeSwitchAsk).
- [ ] See-through slider per M4; Windows 10 shows "Needs Windows 11." disabled.

### Terminal page

- [ ] Default shell (term-shell): dropdown of detected shells, sub "New terminals start with this shell."; renders nothing until the list arrives; a missing saved id shows the real first shell (ShellSection.tsx; core/renderer/lib/termPrefs.ts; termPrefs.test).
- [ ] Opening terminals: Folder for new tabs then Add to the Explorer menu (explorer-verb: switch reads the registry, disabled with "Checking with Windows." while busy; sub "Open terminal here, on every folder." DECISION: label per open decision O4).
- [ ] Keys: Agent hotkeys (agent-keys, default on, "Spare shell keys open the sidebar, switch tabs.") and Keep agent sessions open (agent-guard, default on, "Keys never clear or close a running agent.", tooltip "Ctrl+C and Ctrl+D never reach an agent. Ctrl+C still stops one while it works, and copies a selection.").
- [ ] Text: Terminal font (15 faces, each in its face, sub "The typeface inside every terminal.") and Terminal text size (50% to 200%, sub "Text size for every terminal.").
- [ ] Command help row (help-enabled, default on, "Find a command by describing it."); off means off: no title-bar ?, F1 does nothing, no menu item, the key goes back to the shell (HelpSection.tsx; e2e helpPanel).

### Agents page

- [ ] Tab marks rows in order: Agent working indicator (Bar | Ring, default Ring; subs "A line under the tab while it works." / "One small mark at the end of the tab, in its ink."), Mark tabs when an agent finishes (on; "Stays until you open the tab."), Rainbow finished mark (on; "The colours of the app icon, flowing."), Mark tabs when an agent asks (on; "Stays until you answer or open it."), Mark tabs when an agent fails (on; "When it stops on an error."), Count marked tabs on the taskbar (on; "A number on the taskbar button.") (AgentMarksSection.tsx; options.ts; markOptions.ts; AgentsPage.tsx).
- [ ] The indicator belongs to no theme (a pick never resets it) unless Custom saved it (termLook.ts).
- [ ] Claude Code section: Exact status from Claude Code (agent-hooks, on, "Applies to terminals opened after a change."); off hands new shells no plugin (ClaudeCodeSection.tsx; termLook.ts agentHooksOn).
- [ ] Mark colours section with Save changes in its heading, lighting with the theme's (MarkColoursSection.tsx).
- [ ] Every switch stores '0' or '1' and reads on unless '0' (termLook.ts onSwitch).

### Dictation page (rows only; the engine is M8)

- [ ] First section row Use dictation (dictation-enabled, "Speech to text on this PC, never sent."; warning "Needs a speech model before it works." when none installed) (core/renderer/settings/sections/DictationPage.tsx; dictationOptions.ts).
- [ ] Listening section: Key behaviour (Hold | Toggle), Dictation key, Microphone, Spoken language; rows dimmed and disabled while off (see M8 for texts).
- [ ] While dictating: Pause media, Play start and stop sounds. Speech models card tagged "Shared by both apps". GPU acceleration card only with NVIDIA.

### Diagnostics page

- [ ] Detailed logging (diag-verbose, off by default, "Records every call, for tracking down a problem."); the flag is kept by the backend in diag.json and the switch shows the saved answer (core/renderer/settings/sections/DiagnosticsPage.tsx; diagnosticsOptions.ts).
- [ ] Log files (diag-folder, sub the folder path else "Kept on this PC, never sent."; button "Open folder", disabled until known).
- [ ] Mark a problem (diag-mark, "Stamps this moment in the log."; button "Mark" then "Marked" for 1200 ms, both words in one grid cell, no width change).
- [ ] Quiet level: an idle 3 s writes nothing (e2e diagLog).

### About page

- [ ] App icon 56 px, name as an 18 px bold heading, one line of description; Version row (app-version, "The version you are running.", mono pill) (AboutPage.tsx).
- [ ] "Made with Slint" (AboutSlint) shown (design spec).
- [ ] No update row on About; the update chip lives in the title bar; no "Prism Terminal" footer word on the rail.
- [ ] DECISION: Flare's About description line is the owner's to choose (open decision O9).

### Storage and import

- [ ] Every setting is one store with get, set, change listener; a blocked store loses the choice, never the app; unknown enum words read the default and are never written back; default-on bools read on unless '0', default-off read on only for '1'; never set is not zero (core/renderer/lib/prefStore.ts; prefStore.test; helpPrefs.test).
- [ ] Settings write immediately and notify listeners; the window follows store changes (prefStore.ts).
- [ ] Settings are written atomically (temp file then rename) in `%APPDATA%\Flare` (core/main/atomicWrite.ts; atomicWrite.test).
- [ ] Persisted keys keep their PT names and meanings inside the settings file: prism.term.theme, font, fontPct, custom, acrylic (legacy), seeThrough, opacity (legacy, import only), agentIndicator, agentColor, agentDoneColor, agentQuestionColor, agentDoneOn, agentQuestionOn, agentRainbow, agentFailedOn, agentHooks, colourFormat, shell; prism.window.tabLayout, tabSwitch, edges, background, secondary, accent, customColours, taskbarBadge, agentGuard, agentKeys; prism.newtab.mode, folder; prism.help.enabled, shell, panelWidth; prism.recentRoots, pinnedRoots; prism.dictation.* (the inventory notes: keep names, import by name; open decision O10).
- [ ] First-run import reads PT's Stable profile (`%APPDATA%\PrismTerminalStable`) else `%APPDATA%\PrismTerminal`, from LevelDB local storage; a value that cannot be read reliably is skipped, never guessed; settings, themes, custom theme, fonts and shells import once; tabs do not (design spec Migration).
- [ ] Indicator `off`/`full` import as Ring, `minimal` as Bar (termLook.ts).
- [ ] Legacy keys prism.term.opacity and prism.term.acrylic are read once and removed (seeThroughMigration.test, opacityMigration.test).
- [ ] Explorer verb state lives in the registry, not the settings file.

### Command help panel (core/renderer/components/HelpPanel.tsx, core/shared/help)

- [ ] Sheet flush to the right edge under the title bar, full height, opaque `--p-side-flat`, one left divider, soft shadow; not a dialog (no scrim, no trap) (HelpPanel.tsx; docs/superpowers/specs/2026-10-13-help-panel-design.md).
- [ ] Width default 620, clamped 420 to 1100 on the way in and out, never wider than the column less 120; stored prism.help.panelWidth (helpPrefs.ts; helpPrefs.test); left edge resizes by drag and keys (Left grows 20, Right shrinks 20, Shift x4); role separator "Panel width".
- [ ] Slide in 460 ms exponential ease-out (.16,1,.3,1), out 220 ms (.4,0,.6,1); none under reduced motion. DECISION: the spec says 260 ms out, the code 220; use 220 (open decision O11).
- [ ] Opens with F1 even over a focused shell, the title-bar ? and "Command help" in the terminal menu; Escape or the X closes; F1 over Settings opens it; stays across a click into the shell and a tab switch; opening starts on the front tab's shell with an empty question; shares the top right with find, last opened wins (e2e helpPanel).
- [ ] Shell chips PowerShell, Command Prompt, Bash (aria-label "Shell"); a hand-picked chip is remembered (prism.help.shell) (helpPrefs.ts helpShell).
- [ ] Search field is a combobox with the aria-label "Search commands. Up and Down move through the results, Enter copies the highlighted command." and placeholder "Try: find big files, what is using port 3000" (HelpPanel.tsx).
- [ ] Count text "N commands" / "N results" / "No results" / "50+" at the cap (data-help-count).
- [ ] Browse groups in order: Folders, Files, Text and search, Processes and ports, Network, System, Git, Claude Code and Codex, Installing things, WSL (Linux on Windows), The shell itself; headings not selectable (core/shared/help/index.ts HELP_CATEGORIES).
- [ ] Paging: first 40 rows, then 60 more when the end is within 600 px; search returns at most 50 (HelpPanel.tsx).
- [ ] Rows two columns (task, command in the terminal face), 34 to 44 px, one or two lines, truncate with full text on hover, square corners, zebra with the first plain and stripes across headings; no sub text; danger is an amber mark read by screen readers as "Careful." (HelpPanel.tsx Row; e2e helpPanel).
- [ ] Nothing marked on open; first Down marks the row; Up/Down/PageUp/PageDown (5) move; the mouse over the list clears the mark.
- [ ] Enter copies the marked row; a row click copies; a click in the command text copies nothing; the text is selectable.
- [ ] Copy hands the exact text on screen (trailing placeholders dropped, trailing space kept) to the clipboard callback and nothing else; the icon becomes a green drawn check for 1400 ms; no "Copied" badge; screen reader hears "Copied <command>"; failure says "Could not copy to the clipboard"; row height unchanged (HelpPanel.tsx COPIED_MS; catalogue.test blankless).
- [ ] A key to press (Ctrl+C, Esc, Shift+Tab) draws key caps with no copy button; Enter says "<key> is a key to press, not text to copy" (shared/help/index.ts isKeyPress).
- [ ] Ctrl/Alt/Meta chords pass to the host; footer keys line only: Up, Down "move", Enter "copy", Esc "close".
- [ ] Empty text: "Nothing matches. Say it another way (what you want to DO, such as "delete a folder"), or try another shell."
- [ ] The panel never inserts, runs or types into the shell; it takes no session handle; its only effect is the clipboard callback; PTY write count is zero across search, copy and Enter.
- [ ] Search (core/shared/help/search.ts): query cut at 400 chars, 24 meaningful words scored, accents folded, apostrophes closed up; stop words drop question scaffolding but keep small meaning words (all, not, no, up, down, out, back, off, kill, who, where, new, last, first, set, run, use); "get" is a stop word; stems for plurals, -ing, -ed, -er/-est on a short list, irregular forms, short words untouched; 27 synonym groups plus the one-way running to process; prefix match while typing; one slip allowed in words of five letters or more, scored below exact; tier scores 12000/10000/6000/2000/1000 and field weights task 10, keywords 6, summary 3, command 2, rest 1; phrase bonuses up to 30; ties by shorter task then id; empty query is the catalogue in order; no match is an empty list; a number is never a required word (search.test).
- [ ] Shell filtering: an entry shows in its shell, in any-shell entries, and in windows-only entries for PowerShell and cmd, never Bash; Bash gets python3 (shared/help/shells.ts shownIn; catalogue.test).
- [ ] Catalogue: 7 files (powershell, cmd, bash, git, agents, packages, wsl) as one array, ported verbatim; search index built once; a keystroke answers in under 20 ms, the first under 150 ms (search.test).
- [ ] Catalogue gate: unique kebab ids prefixed ps-, cmd-, sh- or by tool; known category; sentence-case task under about 60 chars without a full stop; summary present; one-line ASCII command, no angle-bracket placeholders; UPPER_SNAKE placeholders declared both ways; 6 to 16 unique lower-case keywords; a danger sentence on anything that deletes, kills or overwrites; row names stand alone (at most 8 words, 54 chars); variant hidden words 3 to 12; no em dash (catalogue.test).

## M8: Dictation

Scope is v0.55.6 dictation: Parakeet, warm-up and the GPU pack are in; dictate-anywhere is out.

### Off, on, master switch

- [ ] Dictation is off by default; no key listener, no process, no download until switched on (core/renderer/lib/dictationPrefs.ts dictationEnabled; dictation.ts "Off means off").
- [ ] Master row dictation-enabled "Use dictation", "Speech to text on this PC, never sent."; with no model installed the sub becomes the warning "Needs a speech model before it works." (dictationOptions.ts; DictationPage.tsx noModelYet).
- [ ] Switching off during a recording cancels it (nothing pasted) and stops the speech server and media helper (dictation.ts armDictation.sync; dictationIpc.ts DCH.stop).
- [ ] With dictation off the key does nothing and no whisper-server or parakeet-cli process exists (e2e dictation, dictationParakeet).
- [ ] On arm the engine is warmed after 3000 ms the first time, 500 ms on re-syncs; asked again at each press and when the model or language changes; warm is a no-op with no installed model (dictation.ts warmEngine, begin; dictationIpc.ts DCH.warm).
- [ ] Quit kills every dictation child (a Job Object with kill-on-close) (dictationIpc.ts disposer; index.ts stopDictation).

### Hotkey reducer (core/renderer/lib/dictationKey.ts)

- [ ] Default key bare Right Alt (code AltRight), mode Hold (dictationKey.ts DEFAULT_HOTKEY; dictationPrefs.ts).
- [ ] Hold: recording starts after the bare key is held alone for 200 ms (DEFAULT_HOLD_MS, host tick 210 ms); release stops and transcribes; a release before 200 ms is only a release (dictationKey.test).
- [ ] A late tick still starts if the next event arrives at or after holdMs (reduceKey).
- [ ] AltGr typing: Ctrl then AltRight then a key during arming cancels silently and that key is not swallowed; the preceding ControlLeft within ALTGR_PAIR_MS = 50 ms is forgiven; AltGr held alone still dictates; a real Left Ctrl well into arming cancels; only AltRight is forgiven a Ctrl (dictationKey.test).
- [ ] Auto-repeat is swallowed, never re-arms after a cancel, can carry the start if the tick is late.
- [ ] Hotkey up is always swallowed so an unswallowed Alt up never opens the system menu (dictationKey.ts release).
- [ ] Toggle: a solo tap (within TAP_MAX_MS = 1000 ms, no other key) starts, the next stops; polluted taps do nothing; holding never starts; Escape mid-tap does not let the tap start (dictationKey.test).
- [ ] Escape while listening cancels, nothing pasted, down and up swallowed; idle Escape is ordinary; Escape during arming goes idle polluted.
- [ ] Another key while listening does not cancel and is not swallowed; window blur while listening cancels, while arming goes idle.
- [ ] Chords and plain keys (Ctrl+Shift+D, F9) start on the down at once, stop on the main key's up (hold) or toggle on each non-repeat down; after an Escape the held key's up stops nothing; an up whose down was not seen is not swallowed; Shift already down means a chord (dictationKey.test).
- [ ] isHotkeyEvent: up matches on code alone; bare-modifier down requires every other modifier up; chord down requires exact modifiers.
- [ ] usableHotkey refuses letters, digits, Enter, Space, Shift+letter without Ctrl/Alt/Win; allows a bare modifier, F1 to F24, Pause, Scroll Lock, chords with Ctrl/Alt/Win except Ctrl+C, V, W, T, F, Tab, Ctrl+1-9, Ctrl+Comma and the Flare app keys; checked on capture and on read; unusable stored key reads default (dictationKey.ts; dictationPrefs.ts).
- [ ] Display names "Right Alt", "Left Ctrl", "Ctrl+Shift+D", "Win" for Meta, US-legend punctuation (formatHotkey, keyName; dictationKey.test).
- [ ] Capture field (dictation-hotkey): button text "Press a key…" with a capture marker; bare modifier commits on release, chord on down; Escape, outside press, blur cancel; unusable key keeps capture open; "Reset" only when not default; AltGr is read as Right Alt (core/renderer/settings/dictation/parts.tsx HotkeyField; dictationKey.test parseHotkeyFromEvent).
- [ ] Key listener is capture-phase, only while armed, only over a shell with a live session (dictation.ts armDictation.feed).
- [ ] Physical key codes are required; DECISION: open decision O2/O12 (winit KeyCode vs Win32 raw events).

### Recording and text (core/renderer/lib/dictation.ts)

- [ ] Recording starts only when a model id is stored; else "Choose a model in Settings".
- [ ] The paste target is the session in front when recording started; if that tab is gone: "That tab has closed".
- [ ] Mic failure messages: no device "No microphone found"; refused "Microphone access was refused"; cannot open "The microphone could not be opened" (micCapture.ts captureFailure; dictation.ts FAILURE_TEXT).
- [ ] Capped at MAX_SECONDS = 300; partials every PARTIAL_EVERY_MS = 350 only when none in flight, at least 1 s recorded, and new audio >= 0.6 s (Whisper) or 1.0 s (Parakeet); each partial sends the newest 28 s as WAV with final false (dictation.ts; wav.ts tailSamples).
- [ ] Partial text is cleaned and shown only while the same recording listens; "warming" shows "Starting speech engine…"; partials are never pasted.
- [ ] A clip under MIN_SECONDS = 0.35 s is dropped silently.
- [ ] Final request is the whole clip, final true, with `languageFor(model, stored)`; a final is never dropped.
- [ ] Pasted text = cleaned text + one trailing space, never a newline or Enter, as one bracketed paste through the sanitiser; empty after cleaning shows "Heard nothing".
- [ ] Engine errors: "Choose a model in Settings", "The speech engine is missing. Reinstall the app.", "The speech engine stopped. Try again."
- [ ] Stop and cancel are heard once (no double paste); Escape while transcribing drops the clip; Escape while the mic opens, or a release before it opened, is honoured on arrival (dictation.test).
- [ ] Phase crumbs only, never text; paused media is resumed on every path (dictation.ts setView, resumeMedia).

### Microphone, cues, cleaning

- [ ] Capture 16 kHz mono float, PCM16 WAV in memory, never written to disk; constraints: 1 channel, echo cancellation on, noise suppression off, auto gain on, exact device when chosen; plane 0 of stereo (micCapture.ts; core/renderer/lib/wav.ts encodeWav).
- [ ] Meter level = min(1, rms * 6) per frame; pill bars smoothing `shown*0.82 + level*0.18` down, bars `max(0.12, min(1, shown * BARS[i] * 1.25))` with BARS [0.55, 0.85, 1, 0.8, 0.5], 70 ms linear (DictationPill.tsx).
- [ ] Mic picker: '' = "System default"; 'default' and 'communications' filtered; unlabelled "Microphone N"; Test button shows a meter for at most 10 s; refused "Microphone access was refused." else "No working microphone was found."; opening the page never opens the mic (micCapture.ts listMics; parts.tsx MicField).
- [ ] Only the microphone is granted; camera refused (src/main/index.ts permission handlers).
- [ ] Downsampling: integer ratio box filter, fractional ratio 2/3-sample slots computed from index, below 16 kHz linear up (wav.ts downsample; wav.test).
- [ ] Cues synthesised: start 587.33 then 880 Hz (90 ms, then 110 ms starting 70 ms later), stop reversed, cancel one 392 Hz 120 ms, sine, peak gain 0.12, 12 ms attack; they do not delay capture; sounds default on, '0' off (dictationSounds.ts; dictationPrefs.ts). DECISION: open decision O13 (cue leaking into the clip without AEC).
- [ ] Cleaning (core/renderer/lib/dictationClean.ts; dictationClean.test): controls become spaces; artifact brackets removed ([ ], ( ), * * around blank audio, silence, no speech, no audio, inaudible, unintelligible, indistinct, crosstalk, music, music playing, applause, clapping, cheering, laughter, laughs, laughing, sighs, sigh, coughs, cough, coughing, sniffs, gasps, groans, breathing, clears throat, noise, background noise, static, beep, click, clicking, typing, keyboard clicking, wind, footsteps, pause); bracket content without letters removed; music cues and note glyphs removed; stage directions of 1 to 3 words between asterisks removed but "2 * 3 * 4" kept; fillers um umm uh uhh uhm erm hm hmm removed as whole words with a following comma ("uh-huh" stays); a leading filler hands its capital on; doubled words collapse except that had bye no very really yeah ha knock night there; stranded punctuation fixed (full stop beats comma); space before marks removed only where the mark ends something (dotfiles and `cd ..` kept); runs to a standstill in up to 8 passes; output has no control, no line break, no ESC, cannot close a bracketed paste.

### Pill and tab mark

- [ ] Pill: bottom centre 16 px up, max width min(680 px, 100%), full radius, divider border, `--p-side-flat`, shadow `0 6px 24px rgba(0,0,0,0.35)`, padding 6/12/6/16, gap 10, absolutely positioned (never changes shell rows), pointer-events none, mounts and unmounts (DictationPill.tsx).
- [ ] Faces "Listening…" (five 3 px bars 16 px tall), "Transcribing…" (14 px spinner), "Starting speech engine…"; transient messages for MESSAGE_MS = 2600 shown over whichever terminal is in front; live text 12.5 px dim right-to-left clipped so the newest words show; role status polite (DictationPill.tsx).
- [ ] Tab mic mark: 13 px in accent-hi on the paste target tab while not idle, even when another tab is in front; aria-label "Listening" or "Transcribing" (DictationTabMark.tsx).

### Models and store

- [ ] Engine pin: whisper.cpp tag b5130, asset `whisper-bin-x64.zip`, 8573270 bytes, sha256 f9ec6c52a2e949b62ab51fa21d0d497958f9e41c3010c157c4e42932d5316f3c; files copied flat (15): whisper-server.exe, whisper.dll, ggml.dll, ggml-base.dll, nine ggml-cpu-*.dll (alderlake, cannonlake, cascadelake, haswell, icelake, sandybridge, skylakex, sse42, x64), parakeet-cli.exe, parakeet.dll; all nine CPU variants kept (core/shared/dictationCatalog.ts).
- [ ] Models pinned to a Hugging Face commit, never `main`: Whisper 5359861c739e955e79d9a303bcbc70fb988958b1 (ggerganov/whisper.cpp), Parakeet 35156454d1a39de06863303dd209fd2bed6ee079 (ggml-org/parakeet-GGUF); a test enforces it (dictationCatalog.test).
- [ ] base "Whisper Base" 147951465 B sha 60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe, Recommended on CPU, "Fast live text on any PC."
- [ ] small "Whisper Small" 487601967 B sha 1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b, "More accurate, about three times slower."
- [ ] parakeet-v3 "Parakeet v3" (q8_0) 668757119 B sha 4d64e9e96c2792186d072fde0034df0ad670cf680a2f53069052ead827fd600e, engine parakeet, limited languages, "Fast and accurate, in fewer languages."
- [ ] large-v3-turbo "Whisper Large v3 Turbo" 1624555275 B sha 1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69, needs GPU, Recommended on GPU, "Close to Large v3 at half the download."
- [ ] large-v3 "Whisper Large v3" 3095033483 B sha 64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2, needs GPU, "The most accurate."
- [ ] tiny (test only) 77691713 B sha be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21; parakeet-v3-q4 (test only) 355615679 B sha aa7fe2f5fb47d863ca23e8b1d490632d63a2599f515268b6d6bd656158dad45e.
- [ ] gpu-pack "NVIDIA GPU acceleration" `whisper-cublas-12.4.0-bin-x64.zip` from b5130, 674539285 B sha af520ddd034d985b55dfeea3e465ed93653ba2aee1a55e865033edc548c272a7, "Large models answer in under a second."
- [ ] Manager list order: Base, Small, Parakeet v3, Turbo, Large v3; exactly one Recommended CPU pick (Base) and one GPU pick (Turbo), chosen by whether the pack is installed (dictationCatalog.ts visibleModels, recommendedModel; dictationCatalog.test).
- [ ] Notes are at most eight plain words and never repeat a badge.
- [ ] Limited-language models: the picker shows Auto-detect disabled with a globe icon titled "This model doesn't support all languages."; stored language unchanged and returns on a Whisper model; sub "This model detects the language itself."; downloading one first asks in a one-line dialog with Download and Cancel (Escape cancels; Retry does not ask again) (dictationCatalog.ts languageFor; DictationPage.tsx ModelDownloadAsk; e2e dictationPage).
- [ ] Languages: auto plus en, no, sv, da, fi, ar, zh, cs, nl, fr, de, el, he, hi, hu, id, it, ja, ko, pl, pt, ro, ru, es, tr, uk, vi; values `[a-z]{2,4}` or auto, else auto (dictationCatalog.ts LANGUAGES; dictationPrefs.ts; dictationIpc.ts validLanguage).
- [ ] Shared store root, layout `models/<id>.bin`, `.part` while downloading, `engines/<id>.zip` then `engines/<id>-<12 hex>/`. DECISION: open decision O14 (folder and marker names).
- [ ] Nothing unverified is loaded: stream to `.part`, hash and count, rename only if size and sha256 match; more than the catalogued size aborts with checksum; failures network, checksum, disk, cancelled, unpack with texts "The download failed.", "The file failed its check and was deleted.", "It could not be written to disk.", "It could not be unpacked."; no auto retry; progress at most every 100 ms; a throwing listener does not break the download; two asks share one promise; cancel then retry starts afresh; cancel removes `.part` (core/main/dictationStore.ts; dictationStore.test).
- [ ] Stale `.part` (60 s) removed on status; installed memo 30 s, a "no" never memoised; installed = exact catalogued byte count; ids match `^[A-Za-z0-9][A-Za-z0-9._-]*$` (dictationStore.ts).
- [ ] Pack install: verify, unzip, delete zip, find whisper-server.exe at top or up to two levels, write marker `prism-installed.json` {id, bytes, sha256} last; no marker means not installed; failure is undone and reports unpack; older pack folders swept (dictationStore.ts; DECISION marker name per O14).
- [ ] Remove cancels a running download, removes files, stops the engine first; removing the active model clears the stored id; after a download the engine stops; the first downloaded model becomes active unless one already is; download ids come only from the catalogue (dictationIpc.ts; dictationState).
- [ ] Row UI: name, size (GB with one decimal from 1e9, else MB), Recommended badge (outlined), Active/Enabled badge (filled), sub, faint fill when Active; downloading shows a 6 px bar 112 px wide with percentage and Cancel; installed non-active shows Use and Uninstall; "Slow without GPU acceleration." warn on large models without the pack; a failure replaces the note; NVIDIA mark for pack and Parakeet, OpenAI-style for Whisper (DictationPage.tsx ModelRow).
- [ ] Uninstall title names the apps that share the files; reworded for Flare (parts.tsx Uninstall; O14).

### GPU pack and engine

- [ ] GPU row (dictation-gpu, "GPU acceleration", "Large models answer in under a second.") only with an NVIDIA adapter (vendor 0x10de, answered once, no process spawned) (dictationOptions.ts onlyWhere; dictationEngine.ts hasNvidia).
- [ ] Engine choice is automatic: GPU pack when installed and NVIDIA present, else CPU (dictationEngine.ts pick); Enable downloads; when on shows "Enabled" and a "Disable" button titled "Turns GPU acceleration off and frees 675 MB of disk space." that removes the pack.
- [ ] GPU failure at start or mid-pass: one attempt then CPU for the session (gpuFellBack) with the note "It could not start, the CPU is used."; a timeout or bad answer does not set it.
- [ ] CPU runtime folder is first on the child's PATH; GPU engine gets `CUDA_CACHE_PATH` and `CUDA_CACHE_MAXSIZE` 4294967296; the CPU engine neither (dictationEngine.ts childEnv).
- [ ] A GPU pass must never queue partials behind a cold compile (31.8 s cold vs 0.26 s cached on the owner's RTX 5090).
- [ ] whisper-server: started on first transcribe or warm; one server; replaced on model, language or engine change; command `whisper-server.exe -m <model> --host 127.0.0.1 --port <free> -l <lang|auto>`, cwd engine folder, hidden; ready when GET / answers below 500 polled every 100 ms, failing after 60000 ms; a server that exits returns the last 2000 chars of stderr (dictationEngine.ts launch; dictationEngine.test).
- [ ] Pass: POST /inference multipart with `file` audio.wav and `response_format` json, random boundary, HTTP 200 with JSON `text`; timeout 120000 ms kills the server; socket error waits 1000 ms for the exit report.
- [ ] Idle stand-down at 300000 ms after the last request (not under a longer pass); the next press restarts.
- [ ] Passes serialised; a final jumps ahead of a waiting partial; a newer partial replaces a waiting one (`superseded`); a partial on a cold engine answers `warming` at once and starts a warm-up; a final waits for the warm server.
- [ ] Warm-up is one silent 0.5 s 16 kHz WAV (44-byte header + 16000 zero samples); repeated asks are one server and one pass; after a failed warm-up partials go the ordinary way.
- [ ] After a server would not start, partials for the same engine/model/language answer that failure for LAUNCH_FAIL_MS = 30000; a final still tries; stop() clears it.
- [ ] stop() bumps an epoch, kills the server, fails in-flight and waiting with `engine-failed` "dictation was stopped", clears memos; a stop during GPU start is not a GPU fault; info() reports running, last engine kind, gpuFellBack.
- [ ] Parakeet: `parakeet-cli.exe -m <model> -f - -np -t <threads>` plus `-ng` on CPU; WAV on stdin, text on stdout UTF-8; threads = clamp(floor(cpus/2), 1, 16); language sent as 'auto'; 120 s timeout; `error:` on stderr with exit 0 is a failure, not a GPU fault; any resident Whisper server is killed first; warmed once per model path; stop() ends a pass at once; GPU failure falls back to CPU for good (dictationEngine.ts parakeetOnce, runParakeet; dictationEngine.test).
- [ ] Timing targets: Parakeet 0.72 to 0.86 s a pass on CPU, 0.76 to 0.78 s on GPU; Whisper Base about 0.9 s (docs/two-apps.md).

### Pause media

- [ ] Off by default; "Pause media while dictating", "Resumes when you stop."; pause requested at recording start (dictationPrefs.ts; dictation.ts mediaToken).
- [ ] Never sends the play/pause key; asks Windows' media session manager for Playing sessions; pause counted only when TryPauseAsync said true; token = SourceAppUserModelId list; empty token starts nothing; resume plays only the token's ids still Paused; ids with spaces work; a newline in a token cannot send a second request (core/main/mediaPause.ts; mediaPause.test).
- [ ] Requests numbered and matched by number; per-request timeout 3000 ms; a late pause answer is undone by resuming those ids; two timeouts in a row kill the helper and the next starts fresh; helper lazy on first pause; death, failed start and a throwing spawn never throw to the caller; dispose answers waiters with nothing; quit kills it; each await bounded at 2000 ms (mediaPause.ts; mediaPause.test). Flare uses a native call, not PowerShell.

### Settings rows and privacy

- [ ] Row ids and keys: dictation-enabled, dictation-mode (Hold | Toggle; "Key behaviour"; subs "Runs while the key is held." / "One press starts, the next stops."), dictation-hotkey ("Dictation key", "Text is pasted at the cursor."), dictation-mic ("Microphone", "Test it to see the level."), dictation-language ("Spoken language", "The language you speak."), dictation-pause-media, dictation-sounds ("Play start and stop sounds", "A short sound as the microphone opens."), dictation-model ("Speech models", "Shared by both apps on this PC."), dictation-gpu (dictationOptions.ts; DictationPage.tsx). Keys prism.dictation.enabled (off), mode (hold), hotkey (AltRight), mic (''), language (auto), pauseMedia (off), sounds (on), model ('').
- [ ] Page cards in order: master, listening, while, models, gpu; the Dictation page is in the rail after Agents for every PC.
- [ ] Values are per app, only big files are shared; a damaged hotkey JSON reads the default; preferences notify subscribers on write.
- [ ] Audio never touches disk and never leaves the machine (loopback 127.0.0.1 or engine stdin); diag records phases only; child stderr kept in memory only, last 2000 chars.
- [ ] Engine binaries are official whisper.cpp builds, pinned; the C++ runtime DLLs (MSVCP140, VCRUNTIME140, VCRUNTIME140_1, VCOMP140) copied only when Authenticode is Valid from Microsoft Corporation; the MIT notice shipped as LICENSE-whisper.cpp.txt; entries added to THIRD-PARTY-NOTICES.md (tools/fetch-whisper.mjs).
- [ ] The shared model folder survives an uninstall.
- [ ] Not machine-testable, carried to the owner's hands-on list: how it sounds, AltGr on a physical Norwegian keyboard, pause-media against a real player (docs/two-apps.md "Testing is REAL").

## M9: Updates, installer, Windows integration, diagnostics

### Update check

- [ ] The check runs only in a packaged build; dev and test builds never touch the network; the unpackaged build shows the preview (src/main/update.ts watchForUpdates; core/main/updatePreview.ts). The whole updater stays off until the repo is public (design spec).
- [ ] First check at launch then every 4 hours (CHECK_EVERY_MS), timer does not keep the process alive, offline is silent; each check writes a crumb `update-check` with offered, fetchMs, offerMs, heapMb, rssMb (update.ts; docs/diagnostics.md).
- [ ] No offer unless the version is strictly newer (newerVersion, non-numeric parts count 0); release body kept raw, cut to 20,000 chars; the installer asset is found by an anchored name pattern that the sibling app's name cannot match (update.ts latestUpdate, isInstallerName; update.test). DECISION: Velopack feed replaces the GitHub asset scan (open decision O15).
- [ ] A download URL is accepted only if https, host github.com, under `/<repo>/releases/download/`, ends `.exe`, no credentials, query or hash (update.ts isReleaseAssetUrl). DECISION: re-expressed as "only the package Velopack offered".
- [ ] Offer shape `{version, url, notes, mock?}` (core/shared/updateTypes.ts).

### Install gate and handoff

- [ ] Install touches only the current offer; any other url is refused with no fetch; no offer returns false; a mock offer is previewed never downloaded; one run at a time; cancel aborts the current run (src/main/updateGate.ts; updateGate.test).
- [ ] The close question is pre-answered at the quit, not at the start of the download (update.ts installUpdate beforeQuit; two-apps.md INSTALL IS HELD BY THE WINDOW'S OWN RULE).
- [ ] Download to a fresh temp folder; progress whole percent capped at 99 while downloading, 100 on the last byte; cancel aborts fetch and write, wins even after the last byte, leaves nothing, answers false; a failed download removes its temp and returns false, reported as "The update could not be downloaded. Nothing was installed." (update.ts; core/renderer/lib/updateFlow.ts INSTALL_FAILED).
- [ ] Handoff is a detached process that waits for the installer then restarts the app at the same path, with the profile arg only if the running copy had one, then removes the temp folder; if it cannot start the download counts as failed and the app does not quit; the app quits 400 ms after a successful handoff (src/main/updateHandoff.ts; updateHandoff.test). DECISION: Velopack apply/restart replaces the script (O15).
- [ ] The stable-copy branch of the handoff is not ported (no stable copy in Flare).

### Preview

- [ ] `--preview-update` exact match, works in the installed app; a second launch with the flag previews in the running app (core/main/updatePreview.ts wantsPreview; e2e updateQuiet).
- [ ] Preview offer: next minor version, empty url, mock true, SAMPLE_NOTES; the real watcher never starts when previewing; no network, file, process or quit (updatePreview.ts previewVersion, previewUpdate).
- [ ] Preview install: 20 steps to 100% over 80% of 3000 ms then a 600 ms hold then false; cancel honoured during the steps only; no close question (updatePreview.ts runPreviewInstall; core/renderer/lib/useUpdateFlow.ts).
- [ ] Without the flag under the test flag: no chip, no network (e2e updateQuiet).

### Update chip

- [ ] Exists only while an offer exists; label exactly `Update {version}`; aria-label `Update to {version}`; aria-haspopup dialog; busy while installing (core/renderer/components/UpdateChip.tsx).
- [ ] Tooltip: `Updating to {v}` while not idle; `Preview: see what an update to {v} would look like`; else `See what is new in {v}`.
- [ ] Height 24, padding 10, gap 6, radius 6, 11.5 px semibold label, 11x11 up-arrow icon (path `M12 4v11m0 0l-4.5-4.5M12 15l4.5-4.5M5 20h14`, stroke 2.4, round caps).
- [ ] Filled with `--p-sel-bg` (not the raw accent), label `--p-on-accent` at 4.5:1 or more, not grey (channel spread 40+); width and label never change across phases; leftmost control of the right group (e2e updateWindow).
- [ ] Hover brightness 110%; focus ring 2 px accent-hi offset 1 px; a click opens the window and never installs.
- [ ] Notice under the chip when the window is not up: role status, 8 px below, right aligned, flat surface, 1 px border, 11.5 px soft text, shadow `0 10px 30px rgba(0,0,0,.45)`, click dismisses, auto-dismiss 8000 ms (NOTICE_MS).

### Update window

- [ ] Modal: scrim, max width 460, padding 20; Escape and outside press close only when idle; Tab trapped; focus to Install; title `Update to {v}` / `Updating to {v}`; sub `You have {current}` (core/renderer/components/UpdateDialog.tsx).
- [ ] Notes list scrolls inside min(44vh, 300px), aria-label "What is new", no box; sections h3 12.5 px; entries with a 4 px accent-hi dot; `+ N more` when capped; text only, no HTML, markdown, anchor, image or handler; empty shows the sentence `No notes were published with this release.` (UpdateDialog.tsx; core/shared/releaseNotes.ts NO_NOTES; e2e updateNotes).
- [ ] Status line: `Cancelling…`, `Downloading the update`, `Installing. The app will close and reopen.`, or `Installing (preview: nothing is installed)`; percentage at the right (tabular) only while running.
- [ ] Progress track always in layout (6 px, rounded), fades 200 ms, fill accent-solid with a 300 ms ease-out only while running, role progressbar; the box never changes size (e2e samples every 25 ms).
- [ ] Buttons right-aligned gap 8: Cancel (reads "Close" when idle with a notice) and primary "Install"; Cancel disabled at `installing` (title "The installer has started") or when the download cannot be cancelled ("This download cannot be cancelled"); Install disabled while running; focus moves to the box when Install is disabled.
- [ ] The window stays open through the install and draws the bar; fits at the smallest window (innerHeight 420).
- [ ] State machine (core/renderer/lib/updateFlow.ts; updateFlow.test, 28 tests): `available` swaps the offer only while idle; `open` no-op without an offer, clears an old notice when fresh; `cancel` refused while running; `hide` closes in any phase and the window returns after the question; `install` only from idle; `progress` ignored when idle, whole, clamped 0..100, NaN is 0, 100 becomes installing; `abort` only while downloading; `settled ok` changes nothing; `settled` failure while aborting returns idle silently; otherwise idle with notice "Preview only: nothing was installed" or the download failure sentence.
- [ ] Release notes parser: caps 20 entries and 160 chars (ellipsis, never through a surrogate pair), rest counted; controls, zero-width, bidi, BOM stripped; HTML comments, script/style blocks and tags removed (four passes), lone `<` kept; markdown link keeps text, emphasis stripped, headings/rules/bullets/checkboxes handled; generated `by @user in <url>` tail removed only from a title's end; PR number kept as `(#N)` only if absent; "New Contributors" skipped, "Full Changelog" dropped; body cut at 20,000 back to a whole line; wrapped prose joins (releaseNotes.ts; releaseNotes.test, 31).
- [ ] Grouping: New features, Improvements, Bug fixes, Under the hood; prefix mapping feat/feature, fix/bugfix/hotfix, perf/refactor/style/ui, docs/doc/chore/build/ci/test/tests; unmarked is a feature; `(#N)` removed; capitalised (core/shared/releaseGroups.ts; releaseGroups.test).
- [ ] The close question for an install: "Stop the agent and install the update?" with "Install and restart", asked before download-to-quit (see M3 close flow).

### Installer (Velopack)

- [ ] Per-user install, no admin, no install-folder choice, no desktop shortcut, Start menu shortcut, uninstall keeps user data (electron-builder.yml nsis block). DECISION: Velopack options must match or the choice is recorded (O15, one-click vs wizard).
- [ ] x64 only; Flare names for artifact, exe and app id; installer name cannot collide with Prism Terminal's.
- [ ] The installer ends the running app before touching files, and the same before uninstall (build/installer.nsh customInit, customUnInit).
- [ ] Uninstall deletes the two HKCU verb keys (installer.nsh customUnInstall; shellVerbParity.test).
- [ ] Shipped beside the app: icon, whisper-server.exe and parakeet-cli.exe with DLLs, the flare-status plugin, conpty.dll and OpenConsole.exe (electron-builder.yml extraResources).
- [ ] Release workflow: a push to main with a new version publishes `v<version>`; an existing version publishes nothing and does not fail; a same-version push replaces that release's installer; gates first (.github/workflows/release.yml). DECISION: Flare CI per spec and tag scheme.
- [ ] Both apps installed side by side: no shared lock, profile, verb, plugin or signal (e2e in M9).

### Explorer verbs

- [ ] Two HKCU entries via reg.exe (or the registry API) with argument vectors only: `Software\Classes\Directory\shell\<key>` and `Directory\Background\shell\<key>`; no `*` entry (src/main/shellVerb.ts verbKeys, addArgs; shellVerb.test).
- [ ] Folder command `"<exe>" "%1"`, background `"<exe>" "%V"`; Icon `<exe>,0` (shellVerb.ts verbSpec; shellVerb.test).
- [ ] Label: PT writes "Open terminal here" and the label does not name the app. DECISION: Flare brief says "Open in Flare" (open decision O4). Key name is Flare's own, never PrismTerminal or OpenWithPrism.
- [ ] Each reg call has a 10 s timeout and runs by full path (shellVerb.ts reg).
- [ ] The label is read by matching REG_SZ, never "(Default)", which is translated (shellVerb.ts readLabel; shellVerb.test).
- [ ] pointsAt: a verb is ours only when its value contains this exe path, case-insensitive; the Settings switch reads the registry; a verb for another copy reads off (verbSwitch.ts status).
- [ ] Default on: shouldWriteVerb = !saidNo && !installed; the one stored fact is the NO, a marker file in the profile written on off, removed on on (verbSwitch.ts set, reconcile; verbSwitch.test).
- [ ] Reconcile at launch only when allowed (packaged, not test): no marker and verb missing or foreign gets written; a stale label on a present verb for this exe is relabelled, label value only, key by key; keys outside the two are refused (shellVerb.ts staleLabelKeys, relabelVerb; shellVerbRelabel.test).
- [ ] A non-boolean set is refused; writes are counted and the test asserts zero in dev and test builds (verbSwitch.ts writes()).
- [ ] Windows 11 shows the classic verb under "Show more options"; say so in the docs.
- [ ] The uninstaller removes every key the app writes, checked by a test that reads the installer script (shellVerbParity.test).

### Diagnostics log (writer) and reader

- [ ] Log at `%APPDATA%\Flare\logs\diag.jsonl`; verbose state in `diag.json` `{"verbose":true}` (core/main/diagLog.ts stateFile).
- [ ] One JSON object per line; batched every 250 ms through one queue, order preserved; rotation at 2 MB (`.1` newest to `.4`), at most 10 MB; a failed rotation keeps writing and retries after 30 s; queue cap 2000 drops the newest and writes `logger-dropped` n; a logger never throws; first failure writes one `logger-error`; quit writes the queue synchronously (the one allowed sync write) (diagLog.ts; diagLog.test).
- [ ] Every line has `t` (ISO UTC), `up` (monotonic ms), `src` (main|page), `k`; a field cannot overwrite them (diagLog.ts LINE_KEYS).
- [ ] Field cleaning: strings 300 chars with `...(+n)`, `stack` 2000, lists 10 items, objects 12 keys one level, top level 40 keys, functions dropped, non-finite null; arg summary first 4, arrays `[n]`, bytes `bytes:n`; opaque channels by size only (core/shared/diagSummary.ts; diagSummary.test; ipcTiming.ts OPAQUE).
- [ ] UI kinds allowlist: page-stall, page-task, page-error, page-rejection, crumb, agent-hook, agent-title, agent-mark, agent-restore, any `*-slow`; kind matches `^[a-z][a-z-]{1,30}$`; batch at most 200; UI time kept only within one day; `often` crumbs and page-task only in verbose (core/main/diagIpc.ts; diagIpc.test).
- [ ] Session line: app, version, runtime, graphics backend, windows, pid, verbose, cpus, cpu, ramGb, `e2e:true` under test; `quit` is the last line; no folder means nothing logged (diagnostics.ts; diagnostics.test).
- [ ] Kinds: session, quit, session-end, main-lag (50 ms tick 100 ms late, never for a sleep), fs-slow (5 s canary, 500 ms), mem-high (page 2048, main 1024, GPU 2048 MB, again at each doubling), memory crumb every 5 min, ipc-slow (500 ms; sync body 100 ms) and ipc-error, page-stall (frame of 200 ms+, last 5 crumbs), page-stack (missed 2 s heartbeat, kept 60 s), page-error, page-rejection, main-error, main-rejection (observe only), gone, unresponsive, responsive, crumb, mark, verbose, logger-error, logger-dropped (docs/diagnostics.md; stallWatch.ts; memWatch.ts; crashHooks.ts; their tests). DECISION: native equivalents of the page-stack watchdog and ipc timing (O16).
- [ ] Error gate: the same error once per 10 s with `repeats`; 10 lines per kind per 10 s; 200 distinct keys tracked, a quiet key forgotten after 6 windows (core/shared/diagGate.ts; diagGate.test).
- [ ] Crumb ring of the last 50; page queue 200 (diag.ts; diag.test).
- [ ] Reader tool: default app log, newest 20 problems with up to 5 crumbs within 120 s each; flags `--since`, `--all`, `--kinds`, `--agent`, `--agent --tab`, `--dir`, `--n`; sorted by `t` (tools/diag.mjs).
- [ ] Diagnostics settings rows: see M7.

### Privacy statement (PRIVACY.md)

- [ ] No accounts, analytics, telemetry or crash reporting; one automatic request, the update check, and it is off until the repo is public (state this when it is switched on).
- [ ] Only on request: installing an update downloads the release; dictation (off by default) downloads engine and model from pinned sources with SHA-256 checks and sends voice only to loopback or the engine's input; links open only on click; Sonara question and tab id go only to 127.0.0.1; the Claude plugin makes no network call and is passed by environment, never written into the user's Claude Code settings; OSC 52 copy up to 1 MB with no read answered; XTVERSION answers only the asker; the log holds no typed text, clipboard, audio, screen text, title text or conversation (PRIVACY.md).
- [ ] PRIVACY.md changes in the same PR whenever a network request is added.

### Icon and brand

- [ ] One .ico for app, installer, uninstaller and taskbar, generated from square artwork on a rounded black square (inset round(8*n/256), corner radius 56/256) with a #202020 one-pixel outline, 8x supersampling, LANCZOS; frames 16, 20, 24, 28, 30, 32, 36, 40, 42, 48, 54, 56, 60, 64, 72, 80, 96, 128, 256 (tools/make-icon.py).
- [ ] The window icon is the exe's own; the badge stays an overlay.
- [ ] DECISION: Flare artwork reuse or new (O17).

## M10: Parity sweep, e2e, performance

### Harness rules

- [ ] The e2e drives the built release exe and exits 1 with "run build first" when missing; `FLARE_E2E_PACKAGED=1` drives the installed layout (tools/e2e/run.mjs:24-33).
- [ ] Filter words are case-insensitive substrings of scenario names (run.mjs:4, 26, 8207).
- [ ] Launch: `flare.exe --user-data-dir=<profile> --e2e`; waits for the test hook; records agent poll verdicts per tab (run.mjs:102-124).
- [ ] Offscreen, never hidden: park at (-4000,-4000), alpha 0; unfocusable, shown without activation; never hide() (run.mjs:9-11, 38-43).
- [ ] Throwaway world per scenario (`flare-e2e-profile-*` with profile, alpha, beta), removed after with retries; the single-instance lock released between scenarios; leaves nothing in %TEMP% (run.mjs:83-100).
- [ ] Reap after every scenario and once before the first: kill every process whose command line contains the profile name, tree and all; never the owner's own Flare (run.mjs:52-80).
- [ ] closeApp races a 15 s timer then kills (run.mjs:135-148).
- [ ] Per-scenario limit 180000 ms (SLOW overrides: verticalTabs 360000, linkPaintStall 300000, dictation 360000, dictationParakeet 360000, helpPanel 300000, updateWindow 300000, indicatorStyles 360000); a timeout is a FAIL, strays reaped, late assertions silenced; each scenario has its own try/catch (run.mjs:8203-8236).
- [ ] Reporting: `pass`/`FAIL` per assertion, a final table with scenario, checks, fails, secs; exit 1 on any failure (run.mjs:8200-8265).
- [ ] Stall report after each scenario from each profile's diag log (not a gate): stalls of 1000 ms or more and error kinds listed; only the diagLog scenario's own stalls are expected (run.mjs:8142-8193).
- [ ] Clipboard guard: the clipboard sequence number is read before the first and after the last scenario; any change fails the run (run.mjs:8195-8262). The app's clipboard is an in-memory stand-in under the test flag (e2eClipboard.ts).
- [ ] Wait for the first agent verdict per tab before standing a shell in for an agent (run.mjs:126-133); typing waits for a PowerShell prompt (45 s); `until` is the only wait primitive with an explicit timeout; no retry loops (run.mjs:231-250).
- [ ] Mute PSReadLine paste chords before any scenario presses Ctrl+V (run.mjs:215-229, 2026-10-15).
- [ ] Settings rows are reached by id through a row-to-page table, never by index (run.mjs:166-202).
- [ ] Screenshots to `.e2e-shots/`, strip animations frozen at the mockup moments, failures ignored (run.mjs:453-468).
- [ ] Fixtures in `.e2e-cache/` fetched once and SHA-256 checked every run, from Flare's own catalogue (run.mjs:265-298).
- [ ] Stand-in agents speak the real protocol: OSC 777;flare-agent hook lines, a fake `claude.cmd` with a temp HOME and transcripts, an HTTP Sonara stand-in with runtime.json and a bearer key (run.mjs:343-345, 3275-3310, 3750-3815).
- [ ] Test hooks exist only under the test flag: opened-paths record, flash count, clipboard, registry-write counter, window edges, slow-IPC, update calls; a unit test asserts each registration is behind the flag (run.mjs hooks; src/main/e2eOnly.test.ts).
- [ ] Env seams use the FLARE_ prefix: FLARE_E2E_PACKAGED, FLARE_E2E_PICK, FLARE_E2E_MIC, FLARE_E2E_NVIDIA, FLARE_E2E_UPDATE_OFFER, FLARE_E2E_UPDATE_NOTES, FLARE_DICTATION_ROOT, FLARE_WHISPER_DIR, plus the Sonara home.
- [ ] Machine-dependent features are faked (NVIDIA card, bun, the owner's whisper-server and clipboard); process checks match only binaries under the build's own folder (run.mjs:300-329, 1296-1299, 5093, 8048).
- [ ] The e2e is the PR gate and nothing else; an install runs none (CLAUDE.md).
- [ ] Performance bar (blocks M10 only): cold start to a usable prompt under 300 ms; idle memory at most a third of Prism Terminal's; heavy-output throughput at least twice with no frame over 33 ms (GPU); software renderer at least matches Prism Terminal on all three (roadmap).
- [ ] Side-by-side screenshots against Prism Terminal on the same theme in every UI milestone's PR (roadmap).

## E2E scenarios (73 named in the inventory, which counted 74; in run order, source tools/e2e/run.mjs line numbers)

- [ ] spawn (473): empty launch shows the start screen with the mark in the accent; New terminal opens one tab in the home folder with the full path as tooltip; `echo pt-42-ok` round-trips; Ctrl+T opens a second tab over a focused shell; no registry write.
- [ ] cwdLabel (510): a launch folder opens as a tab named for it; `cd sub` renames the tab and the tooltip.
- [ ] indicator (522): two launch folders give two tabs; a working title lights the tab; Ring is the default; Bar is stored `minimal` with a line in the accent; no brain icon; a background answer leaves the finished mark; visiting clears it.
- [ ] attention (609): names centred; one-letter tab at least 4 chars wide; three working tabs each draw their own bar; a background question is a 3 px breathing line; badge "1 tab needs a look"; opening does not clear a question; answering clears line and badge; Finished is a flowing rainbow by default, plain when off; Finished off counts nothing.
- [ ] agentHooks (769): a new shell gets the plugin; working, question, failed, done hooks light and mark background tabs; Question line and badge; walking choices is not an answer; a working hook answers a question; permission prompt raises Question then Working; long tool with no hook holds Working; subagent question; Stop with spinner stays Working; "Failed: rate limit" tooltip; Failed on by default, off shows Finished.
- [ ] agentDiag (999): agent-mark moves with rules, one per move; agent-title only on meaning change; no screen or title text in the log.
- [ ] theme (1082): dark default; strip, frame and rows paint the ground; a light preset turns chrome light (luminance > 0.4); main told the ground.
- [ ] newTabFolder (1146): title bar has the cog and no menu; default mode opens in a folder; + opens at once in the chosen folder and does not ask again.
- [ ] caretHold (1200): the text input stays on the input line while a program streams above it; returns to the cursor at a prompt.
- [ ] mouseReport (1274): a program that asks gets mouse reporting; key and click reach it; the bun variant skipped if bun is absent.
- [ ] clickCaret (1327): a click on the typed line moves the caret in real pwsh.
- [ ] selectionEdit (1399): right-click on a link offers Copy link first then Open link, no Close tab; every row has its icon; Copy link copies the whole URL; Open link recorded; Copied badge bottom centre leaves by itself; plain text offers no Copy; marked text copies exactly; Ctrl+C over a selection shows the badge.
- [ ] findKey (1587): Ctrl+F at a prompt opens find; no shadow; Escape closes; Ctrl+F in a full-screen program opens nothing; Ctrl+Shift+F still finds.
- [ ] scrollbar (1625): no native gutter or steppers; slider 6 px; thumb lighter than ground by > 25 on dark, darker on light.
- [ ] paste (1687): in-memory clipboard; Ctrl+V, Ctrl+Shift+V, right-click Paste each deliver exactly once.
- [ ] tabDragFirst (1767): dragging the first tab moves it and not the window.
- [ ] tabWidth (1796): no Tab width setting; every tab one width of 140 to 152 px; long name truncated; picking moves no tab.
- [ ] tabLooks (1840): no full border; faint separator at every right edge incl. last; 18 px icons; name centred; ring at the end; Ring only; close waits behind the ring; idle shows neither; hover shows ring plus sliding close; no Tab style row; indicator segments Ring|Bar default Ring; Settings tab icon.
- [ ] tabSwitch (1955): Tab switching right after Tab position; In order default, Most recent stored `recent`; Ctrl+W closes the front tab; Ctrl+Tab goes to the other open tab.
- [ ] indicatorStyles (2051): each style's front-tab rule; ring alone on marked tabs in the name ink; Bar line per marked tab; rainbow flows; reduced motion stills; minimise pauses.
- [ ] verticalTabs (2152): default Horizontal, choices, first row of Window; Vertical stored; strip gone and a six-row sidebar on the left; square icon band tinted when front; terminal cut where bar meets sidebar; line follows Panel edges; slide keeps the cut within 1 px; resize grip on a slid-out sidebar; narrow Settings keeps its frame; rows show folders; Ctrl+Shift+K, Escape, typing, Enter, drag swap; no collapse button; Ctrl+Shift+B stores `collapsed`; pointer at the left edge reveals it.
- [ ] windowColoursSave (2820): Secondary above Accent; a pick lights Save; Save keeps it with Custom; another theme forgets it; Custom brings it back.
- [ ] tooltips (2870): a tip waits over half a second; centred and 6 px below; native title removed while shown and restored; cog tip inside the edge; a tab tip centred under the whole tab. (Source SHOW_MS is 1000 ms, stricter than the assertion.)
- [ ] manyTabs (2932): 16 tabs; strip never scrolls; resting tabs icons; front keeps its name; "+N" with drawn + listed = 16; Find field has focus; picking fronts that tab; Ctrl+Shift+K lists all; typing narrows; Enter fronts; Escape closes.
- [ ] backgroundWork (3005): working hook lights a background tab; a Stop with "1 shell" stays Working; without it the work is over.
- [ ] systemMenu (3033): the system menu is refused.
- [ ] agentKeys (3057): hotkeys on by default (Ctrl+PgUp/PgDn, Ctrl+S sidebar); off stored '0' and Ctrl+S leaves the sidebar; Ctrl+Shift+B still works.
- [ ] cogToggle (3121): the cog opens Settings as a tab; again closes it; Ctrl+, the same.
- [ ] accentAsk (3145): red accent lights Save with white text; theme switch asks and its Save as Custom wears the accent; near-black red lifted.
- [ ] titleBar (3199): tabs in the top row with buttons at the right end; one row above the terminal; empty space drags; no Show title bar setting; old stored "shown" changes nothing; no-tab top row holds the buttons.
- [ ] settingsPage (3249): Settings opens on Appearance; Dictation stays picked across tab switches; reopening starts on Appearance.
- [ ] launchSkeleton (3279): first frame has the tabs; skeleton on the front agent tab; ring on every tab; lifts when the agent appears; earlier shell text never shown uncovered; a tab behind shows its agent at once; restored tabs carry no done, question or failed.
- [ ] restore (3378): two saved tabs return in their folders in order.
- [ ] handoff (3396): the second instance exits 0; its folder is a new tab; a held folder still gets its own tab.
- [ ] quitManyShells (3419): ten shells quit by themselves within the timeout with code 0, repeated.
- [ ] lastTab (3452): closing the last tab lands on the start screen; window stays; two folders offered; one press opens a shell; X ends the process; next launch restores the tab open at quit.
- [ ] closeAsk (3482): an idle agent still asks; names Claude; Tab and Shift+Tab stay inside; a press over the title bar cancels; Cancel keeps the tab; Escape cancels; a working agent asks and says "working for"; confirming closes.
- [ ] pathLinks (3545): a printed path is a link only if it exists; clicking a .bat opens it (recorded, not launched); right-click opens nothing; every row has its icon; Show in Explorer recorded; Copy path whole.
- [ ] linkPaintStall (3669): find, theme switch and 10,000 lines never hold the window 200 ms.
- [ ] sonaraQuestion (3756): Sonara hears the shown question with the bearer key and `FLARE_TAB_ID`; a redraw posts nothing; the next question and the review are posted; same tab id.
- [ ] termReplies (3831): DA1 `ESC [?1;2c` last in order; OSC 11 answers the ground, OSC 10 the text; `CSI ? 996 n` dark on PT Default; DECRQM 2031 set; version answer exactly twice before DA1; Fawn pick tells light once; one-column nudge once per switch; back to PT Default tells dark.
- [ ] termClipboard (3932): OSC 52 write lands with UTF-8 intact and shows Copied; the sequence is not drawn; a READ gets no reply; clipboard left as it was; "p" changes nothing.
- [ ] imagePaste (4016): image on Ctrl+V sends ^V to a plain program, ESC v to an armed Claude; Ctrl+D and Ctrl+C never reach an idle agent; Shift+Enter sends Ctrl+J with an agent, CR without; becomes Ctrl+J within 1 s once the title reads "✳ Claude Code".
- [ ] bell (4125): no flash before a bell; one bell flashes once; ten in a burst flash once.
- [ ] hiddenResize (4153): after a hidden redraw the scroll slider is hidden or fills its track.
- [ ] links (4215): printed link wears the link blue; full stop excluded; wrapped link inked on both lines; alternate screen link blue and stays blue; light theme link still reads.
- [ ] osc8Links (4339): FORCE_HYPERLINK unset; label wears the link colour; right-click Copy link opens nothing; left click opens once; file:// label not a link; alternate-screen label coloured; no dialog.
- [ ] reviewKeys (4458): a drag in flight takes the strip off window-drag, next move gives it back; Shift+Enter at a plain prompt runs the line, no backslash; Ctrl+C by physical key copies; Ctrl+F finds while a command runs; cd lands with find open; typing stays in the find bar; the menu is put away on tab change; closing an agent's tab asks; Ctrl+T and Ctrl+Tab do nothing under the question.
- [ ] dropAndMenu (4551): a dropped file types its quoted path (space quoted), nothing runs; right-click offers Paste and Find in scrollback, no Close tab; Find opens the bar.
- [ ] helpPanel (4614): ? in the title bar; click opens; F1 over a focused shell; field focused; chip is the front tab's shell; browse by category; 20 to 200 rows; placeholder; "find big files" finds the PowerShell answer first and Bash changes it; 620 px wide, terminal height, opaque, 12 px row padding, copy button 26x26 at the right, rows 34 to 44 px, aligned columns, square corners, zebra, nothing marked, no sub text, terminal face, list scrolls not the panel; edge draws nothing; drag widens by 100; Right narrows 20; clearing restores default; Copy shows a check and no Copied badge; screen reader told "Copied"; row height unchanged.
- [ ] options (5089): core list at least 21 entries with sections; every terminal option on the page; each core section drawn once in one panel in order; app rows listed (at least 9) and all drawn; nothing else claims to be a setting; no confirm-close row; no GPU row without NVIDIA; model manager laid out.
- [ ] diagLog (5231): session line fields; restored shell crumb from main; Ctrl+T crumb from the page; an idle 3 s writes nothing; a busy page gives page-stall with scripts and prior crumbs; main takes the page stack (2000 ms+); a slow IPC call logs ipc-slow with its channel; thrown error page-error with stack; rejection page-rejection; opening Diagnostics is a crumb; Log files shows the folder; Open folder recorded; Mark says "Marked" and stamps the log; verbose off by default, on writes a verbose line and logs every call, persists; quit is the last line; every line one JSON object.
- [ ] settingsLook (5376): at 1600 px dark and light: label and sub 4.5:1, icon 3:1, warning 4.5:1; chosen rail page hover-hi not accent; only Save and on-switches accent-filled; no sideways scroll; agent colour Save and theme Save light together; with acrylic text still 4.5:1; at 900 px rail has names (200 px+); under 760 px icons (60 px or less); magnifier opens the field.
- [ ] settingsSearch (5511): index covers at least 30 rows; no status line when empty; empty field does not claim Escape; each row found by label and opened; "N results"; Down to the first; no rail page current while results show; Escape clears; unmatched shows "No results" and "Nothing matches zebra".
- [ ] settingsKeys (5608): Tab from the field to the first page; Down walks; End last (About); Home first; Enter opens with aria-current; one page at a time; nav landmark "Settings pages"; Tab enters the page.
- [ ] themeEditorLive (5667): a picked Background paints at once; Ctrl+Tab to the shell keeps it; editor reopens holding the pick; picker not on screen on another tab and open with focus on return; Escape closes and restores; a user-closed picker stays closed; saved as Custom with PT Default's accent; Cancel restores.
- [ ] themeSwitch (5746): Appearance order; font size 50% to 200% in tens; appearance rows do not light Save; theme switch after them asks nothing; Custom first then PT Default; Save as Custom carries no font; background and accent light Save and a switch asks; Discard drops them; Cancel changes nothing; Save as Custom keeps then switches.
- [ ] reviewSettings (5894): tabbing through a following colour field stores nothing and offers no Reset; Tab goes into each swatch's editor; Escape closes the editor and returns focus to the pencil; Escape closes only the picker; saving keeps the agent colour and forgets the picked background; a typed letter while capturing the dictation key is accepted, a press elsewhere ends capture.
- [ ] colourPicker (6014): working colour follows the theme until changed; swatch named "Pick Agent working colour", no native colour input; popover named "Agent working colour" with sliders in order and HEX; Escape stores nothing; focus returns to the swatch; Shift+Left walks alpha to 50; stores hex8 ending 80; the working line opaque; Escape on a changed picker returns to following; formats cycle; toggle remembered; typed hsla stores the same hex8.
- [ ] pickedGround (6289): theme text reads at 4.5:1 on its ground; working indicator follows a picked accent; picked light background keeps text at 4.5:1.
- [ ] opacityAlpha (6350): saved Opacity 60 paints 0x99 and moves to see-through 40; Background opaque; old keys removed; no Opacity row; slider 40; Background picker has no Alpha; 50 and back; Save lights; theme pick asks; Cancel keeps; 0 is solid reading Off; a live Custom at 60 paints it and Save keeps 20; Custom again restores the saved 80.
- [ ] seeThrough (6460): row under the wall; slider "See-through window" with its sentence; no switch; starts at 0 solid; at 30 each surface paints at 70% alpha; "30%"; Right steps 31; stops at 70; no Alpha on Background; Save lights; theme pick asks; Save as Custom keeps 45 and another theme is 0; Custom brings back 45.
- [ ] themeCards (6580): at least two rows; every card one height; order matches the snapshot.
- [ ] edges (6683): at launch tokens set, nothing stored; title rule 7%; no line between tabs; main holds the hairline; panel and row rule 9%; None, Hairline, Solid each stored and applied; None transparent; dark Solid white ink, light Solid black at 26%; relaunch keeps Solid before Settings opens.
- [ ] accent (6864): active tab rule is the accent; row buttons, pressed segment unchanged by accent; on-switch track the accent fill; swatch shows the theme accent; a pick stores and offers Reset; hex8 gives a see-through fill with an opaque rule and line token; spinner solid; Save over an opaque window see-through at alpha 128/255 with label 4.5:1; under see-through Save is opaque.
- [ ] updateWindow (7222): preview chip even under the test flag; next minor; accent-filled at 4.5:1; click opens and installs nothing; title "Update to <next>" and "You have <current>"; at least 5 sample entries; sections in order New features, Bug fixes, Under the hood; no PR number, prefix, "by @", URL, Full Changelog, New Contributors or What's Changed; no anchor; "Preview only" line.
- [ ] updateGuard (7556): real-shaped offer shows the chip; install for another url refused with no fetch; empty notes say "No notes were published"; a real offer is not called a preview; Ctrl+W over the update window still asks about the agent; Escape backs out; working agent: Install asks "Stop the agent and install the update?" naming Claude, "working for" and "Install and restart"; Cancel starts nothing; go-ahead starts; a failed download shows "could not be downloaded" once in the window; the chip offers again.
- [ ] updateNotes (7654): hostile body becomes no element and runs no handler; words survive; list capped at 20 with "+ 10 more"; scrolls inside; buttons stay on screen.
- [ ] updateQuiet (7721): no flag means no chip, zero checks and installs; a second launch with the flag previews in the running app and exits 0 without opening a tab or asking GitHub.
- [ ] dictation (7752): engine present with the runtime DLL and notice; off means no key and no server; AltGr typing starts nothing; on starts exactly one resident server before any press; holding R with the fake mic shows the pill and tab mic mark; rows unmoved; meter moves; live text; release shows transcribing then the sentence "ask not what your country" on the prompt, no Enter; Escape pastes nothing; switching off kills the server; none outlives the app.
- [ ] dictationPage (7880): row order; Active model offers Uninstall; Download buttons match Uninstall; no "Delete" or "Remove"; GPU row Enabled/Disable; with GPU Turbo is Recommended, without Base; Parakeet Download asks first with Cancel and Download and nothing downloads while asking; a Whisper model downloads without a question; Cancel leaves no file; language follows the model and the stored value is untouched.
- [ ] dictationParakeet (8017): Parakeet runner bundled; language picker disabled and marked limited; no Parakeet or whisper server stays running when idle; holding R gives pill, live text and the sentence; no process outlives its pass (4 s); no whisper server ever started; off means no speech process.
- [ ] exitClosesTab (8086): Ctrl+W closes the focused shell's tab; `exit` ends its tab; the other tab comes to the front.
- [ ] promptLayout (8107): after a narrow then wide resize the prompt is whole and typing lands where the prompt is.
- [ ] Installer side by side (M9): both apps installed; no shared lock, profile, verb, plugin or signal.

## Open decisions for the owner

- [ ] O1: the cd line. The spec says the only command Flare writes is the agent resume, but Prism Terminal also writes an idle-prompt cd line (core/shared/termCwd.ts cdCommand, #99). Keep for parity (recommended) or drop.
- [ ] O2: Slint key events may not expose the physical key; Ctrl+C/V by key code and the dictation key need a scan-code path (winit KeyCode or Win32 raw input) or an accepted limitation.
- [ ] O3: alacritty_terminal reflows the whole grid on resize; confirm whether the cursor-line carry is still needed.
- [ ] O4: Explorer verb label. Prism Terminal's rule is that it does not name the app ("Open terminal here"); the Flare brief says "Open in Flare". Owner chooses.
- [ ] O5: resume in Git Bash, WSL and other shells is absent in Prism Terminal; keep parity or add.
- [ ] O6: Sonara reads PRISM_TAB_ID; Flare exports FLARE_TAB_ID. Bridge with PRISM_TAB_ID or wait for Sonara's own change.
- [ ] O7: the colour picker eyedropper depends on Chromium; drop at first (recommended) or add a Win32 screen pick.
- [ ] O8: the See-through sub is 9 words against the 8-word copy rule; check whether the rule applies to that row.
- [ ] O9: Flare's About line.
- [ ] O10: keep the `prism.*` key names inside Flare's settings file (recommended, simplest import) or rename to `flare.*` with an import map.
- [ ] O11: help panel close slide 220 ms (code) vs 260 ms (spec); use the code.
- [ ] O12: whether Settings is a tab (as in Prism Terminal, recommended) or a page inside the window.
- [ ] O13: dictation start cue leaking into the clip without echo cancellation: trim the first ~200 ms, delay capture (forbidden by the owner's rule), or use a communications capture path.
- [ ] O14: shared dictation model folder and marker (`%LOCALAPPDATA%\PrismDictation`, `prism-installed.json`): share so a Prism user does not re-download about 3 GB, or a new `FlareDictation` folder.
- [ ] O15: Velopack replaces the GitHub asset scan and the handoff script; one-click vs wizard install; release feed and tag scheme.
- [ ] O16: native equivalents of the page-stack watchdog and IPC timing in the diagnostics.
- [ ] O17: Flare icon artwork and the XTVERSION name string.
- [ ] O18: Prism Terminal's spec lists Tab width and Show title bar rows that the shipped 0.55.x build does not have; port the shipped build.
- [ ] O19: the media pause mechanism (native GSMTC through the `windows` crate) and the unzip step (PowerShell Expand-Archive or a Rust zip crate) each add a recorded dependency decision.
- [ ] O20: `rename` appears in roadmap M3 but not in Prism Terminal (see M3); Explorer verbs sit in M9, not M3; schedule once.
- [ ] O21: Git Bash. Prism Terminal v0.55.6 detects no Git Bash (core/main/shells.ts), but the design spec lists it. Port parity (no Git Bash, recommended for the parity sweep) or add it as a new feature with its own detection rule (for example `where bash.exe` filtered to a Git for Windows install, or the Git install path from the registry), quoting as bash and no resume (O5).
