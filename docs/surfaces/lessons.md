# Surfaces lessons

Holes hit while building, what fixed them, and what the Prism Terminal port should do differently.
Newest last.

## Stage 1: panels

- **Slint 1.18: `Flickable.viewport-*` is deprecated** (the build fails on the warning); use `content-x`,
  `content-y`, `content-width`, `content-height`. `MouseCursor` has no zoom values; `pointer` stands for "click
  to zoom". Electron: not relevant (CSS `zoom-in` exists).
- **An element declared under an `if` cannot be reached from outside it.** A `FocusScope` that must be focused
  from the window's code cannot live under `if panel-open`. The viewers are always in the tree and drawn only
  while open; the controller keeps their picture and rows empty while closed, so a closed panel holds no memory.
  Same shape in any retained-mode UI: make the focus holder permanent, make the content conditional.
- **A zero-sized FocusScope is the key holder**; clicks do not focus it by themselves, so every interactive part
  of the panel calls `focus-panel()` on a pointer press. The window's key path already routes "non-chord key,
  Slint control has focus" to Slint (`chrome-has-focus`), so the panel only adds one more term there.
  Electron: a focused DOM element does this for free; the rule to keep is "a key that is no chord goes to the
  panel, not the pty, while it has focus".
- **Keys must return to the terminal when the panel goes away.** The zero-sized scope keeps Slint focus after
  its panel is hidden (the tab changed, the panel closed), and the window would keep treating keys as the
  panel's. The controller remembers that the panel had the keys and moves them back when the panel is no longer
  shown. A test covers close, toggle and tab change.
- **Existing tests that click a `.txt` expecting "open in Windows" changed meaning.** Three did (`appctl`,
  `term_ui` and the headless scene `pathLinks`). They now click a `.pdf` for the "allowlist opens" case, and
  there are new tests for the preview. The e2e `pathLinks` clicks a pdf, a folder and a script, so it is
  unchanged. Prism Terminal's `pathLinks` scenario clicks `.txt` too and will need the same change.
- **A settings row touches many snapshot tests**: the row count (40 to 41), the app table, the find index (39 to
  40, and the `ui_tests` copy), the terminal page's section list, and the section table. Expect the same in PT's
  `options.ts` snapshot tests. The Links section is new; no page held link behaviour.
- **Every stroked glyph in `ui/*.slint` needs a size test** (`every_stroked_path_has_a_size_test` counts
  `stroke-width:` lines per file). The panel's close glyph has one; the scan's `SIZED` table lists the file.
- **The overlay-order scan reads `app.slint`**: components in the overlay layer sit at 8 spaces, none may use
  `z:`. The panel is in the page, before the help panel and the find bar, so it needs neither.
- **A bare folder name is not a path link** (`sub` has no separator and no dot); `sub/` is. A test that printed
  `sub` and waited for a folder open waited forever. Print the trailing slash.
- **`cargo test` and `cargo nextest` disagree on three tests** (`a_right_click_on_the_strip_calls_no_system_menu`
  and two in `diag_ctl`): they share process-wide state and fail when run in one process. Nextest runs a process
  per test and is the gate; do not read a `cargo test` failure of those three as a regression.
- **Build tooling**: a PowerShell script that forwards `-p` as a parameter fails ("parameter name 'p' is
  ambiguous"); pass the cargo line as one string. Setting the script's own process priority to Idle makes every
  child inherit it, which is how the gentle builds run.
- **What this means for Prism Terminal (Electron)**: the viewers are `<img>` and `<pre>`; the decode, the caps
  and the checkerboard (CSS) are free, but the limits (64 MB, 100 megapixels, 4 MB of text) should be the same, and
  the worker is a main-process read over IPC (rule 34). Copy the layout numbers, the chords, `panels.json` and its
  matching rule, the previews rule (both names must agree on a kind), and the test list. The single fit while
  dragging is the same idea (keep the xterm columns, call `fit()` on the release). The overlay flag is not needed
  until the web panel; a `WebContentsView` has the same z-order problem with native menus.
- **Regenerating `THIRD-PARTY-NOTICES.md` writes an em-dash** into the Unicode licence text (the ICU4X line). The no-em-dash
  scan fails on it; replace that one line with `-` after `cargo about generate about.hbs -o THIRD-PARTY-NOTICES.md` (use `-o`: PowerShell
  redirection is refused). A regenerated file also carries the version bumps the lock had gathered since the last time.
- **Flush every store you add (rule 36).** A new debounced store is easy to create inline and forget at logoff: the
  session-end closure and the end of `run` both need a handle. Keep one function that flushes all of them and test it.
  Electron: `app.on('before-quit')` and the session-end event both need to flush the new file.
- **A cache key that says "same" must have a way to say "again".** The controller skipped the load when the tab, kind
  and target matched, so a click on the same path never refreshed. Keep an explicit reload request next to the
  "already shown" check.
- **A superseded worker must be stoppable, not only ignorable.** Dropping the answer by generation number is not
  enough for work that holds hundreds of megabytes: share the newest generation with the worker and check it between
  stages, and serialise the heavy stage. Electron: abort the read with an `AbortController` and decode in one worker.
- **`TextDoc.bytes` was the bytes read, not the file size.** Capped reads make a header lie; take the length from the
  metadata and keep a reason for each cap.
- **Windows PowerShell 5.1 writes UTF-16LE with a BOM** for redirection and `Out-File`; a "NUL means binary" test refuses
  those logs. Look for a BOM first. Node: check the first two bytes before `toString('utf8')`.
- **A target read back from a file is untrusted input, so it gets the click's checks**, not only string checks:
  canonical form, allowlist on both names, same kind. Do it in the one place that reads, so no caller can skip it.
- **The harness had no way to change the scale after the window was made.** `Harness::set_scale` sends the scale
  change and resizes to the same css size; a layout that only works at the scale it was built at fails there.
- **Overlay lists drift.** The F12 diagnostics card and the tab-drag ghost were drawn over the page but not counted.
  The test that reads the property's line counts terms against `Overlay::ALL`, so adding to both keeps them together.
- **Clippy 1.99 wants `as_chunks::<2>()`** over `chunks_exact(2)` with a constant size.
## Stage 2, library part (flare-ctl, flare-mcp), 2026-10-21

- **A closed pipe can eat the last line.** The server writes a refusal (`too_many_clients`) and closes
  at once. The client's `hello` write then fails with a broken pipe before it ever reads, and the
  refusal is lost: the test for the 65th client failed about one run in three. Two fixes, both needed:
  the reader treats "peer closed" as "read what is left, then end" (a peek reports a broken pipe even
  though a read may still return buffered bytes), and a client whose write fails reads one more line
  (up to 500 ms) before it says "closed". Node's `net.Socket` shows the same race as `EPIPE` on write
  versus `data` on read: handle `error` after `data`, not instead of it.
- **Synchronous handles serialise.** A parked `ReadFile` on a handle blocks every `WriteFile` on the
  same handle. Probe with `PeekNamedPipe` and sleep, or use overlapped I/O. Node does not have this
  problem (libuv is overlapped), so the port needs no polling.
- **Node cannot easily set a pipe's DACL.** Flare creates the pipe with `D:P(A;;GA;;;<SID>)`. The
  default descriptor libuv gives a named pipe may let other local users connect. The Electron port
  should not rely on the ACL alone: create the pipe through a tiny native helper, or add a per-run
  secret (a random key in an environment variable the app puts in its shells, sent in `hello`). Open
  question for the port; Flare's ACL is enforced and its SDDL is pinned (`dacl_sddl`), and the client
  also refuses a server that runs as another user.
- **Pipe names are global to the machine.** Another user can create `\\.\pipe\flare-ctl-<hash>` first.
  The server uses `FILE_FLAG_FIRST_PIPE_INSTANCE` (it fails to start instead of serving a squatter's
  name) and the client checks the server process's user before sending anything. Both are cheap; keep
  both.
- **The pipe tests use one name per test** (`pid` plus a counter), never the real name, so they run in
  parallel and can never meet a running Flare.
- **A SID helper was copied, not shared.** `flare-ctl/src/winid.rs` repeats about 60 lines of
  `flare-win`'s token code, so the MCP server and mods' helpers link nothing of the app. Fold the two
  together the next time `flare-win` is edited (a parallel stage-2 builder owns that area now).
- **JSON lines over stdio needs no length framing**, but stdout must carry protocol messages only. The
  MCP binary logs nothing and prints to stderr only on a fatal error; a stray `println!` would corrupt
  the stream silently. The stdio test parses every stdout line as JSON to pin this.
- **Notifications never get a reply**, including ones a client invents (`notifications/cancelled`). A
  reply to a notification is a protocol error that some clients treat as fatal.
- **A line reader must remember where it already looked.** Searching the whole buffer for a newline
  after every chunk is quadratic: a 16 MB screenshot answer in 4 KB pieces took 44 s in a debug build.
  Keep a `scanned` offset, reuse one read scratch buffer instead of zero-filling a chunk per idle poll,
  and pin it with a 16 MB trickle test. (The first fix attempt silently did nothing because the edit
  script's pattern did not match; the test caught it.)
- **A blocked write is not interrupted by a stop flag.** The worker was parked in `WriteFile` to a client
  that never reads, so `stop()` waited its full 2 s and abandoned the thread. `CancelIoEx` on the
  handle from the stopping thread fails the write at once; clear the hook before the handle closes so
  it can never hit a recycled handle value. Node: destroy the socket.
- **Count what you cap.** "Stuck handlers" was incremented for every call, so the cap measured
  concurrency, not timeouts. Increment on the timeout edge, decrement on the finish edge, and order
  the two atomically (count first, then compare-and-swap the state, undo if it lost).
- **Do not write events while a request is half-read.** Both sides can block on 64 KB pipe buffers.
  Flush the outbox only between whole messages.
- **A known-answer test beats an equality test for anything a port must reproduce.** The pipe-name
  test only compared names with each other; a Node FNV with a different truncation would have passed
  its own tests and connected to nothing. The vectors are now in protocol.md section 1.
- **Read the live object back, not the string you built it from.** The DACL test now opens the running
  pipe and reads its descriptor (`GetSecurityInfo`). `GA` reads back as `FA`.
- **Tooling note**: when scripting edits, assert that every replacement matched; a silent no-op
  hides behind a green compile.

## Stage 2, the channel inside Flare, 2026-10-21

- **A name is not an identity.** The first version let `hello` carry a free-form `tab`; the stage 2 security review found that
  any process of the user could claim any tab, and `tabs.list` handed out the ids to do it. The fix is a secret per tab that
  only the tab's shell is given. **What Electron must do:** make the token in the main process when it spawns the pty
  (`crypto.randomBytes(32).toString('hex')`), put it in the pty environment as `FLARE_TAB_TOKEN` (the same `ptyEnv` function and
  its test list that drops an inherited copy), keep a `Map<token, tabId>` that is checked on every request, never log it or put
  it on a command line (`node-pty` env is the way), refuse a `tab` param, and compare with `crypto.timingSafeEqual`. Keep the
  public tab id separate (hook lines and Sonara carry it).
- **The same variable name can come from outside.** A Flare started from a Flare tab inherits that tab's `FLARE_TAB_TOKEN`; if
  the builder only overwrote the variable when a channel was present, a Flare without one would pass the outer secret to its
  shells. `pty_env` drops all three variables unconditionally and then sets them. Electron: strip them in `ptyEnv` first.
- **A one-shot channel in the message is the whole bridge.** The worker posts `UiMsg::Ctl` carrying an `mpsc::Sender`, the UI
  thread answers on it, the worker waits with a deadline and polls a stop flag in 50 ms slices. Three ways out all end the wait:
  an answer, the deadline (`timeout`), a dropped sender (the message was thrown away because the window is gone, `unavailable`).
  The third is free: dropping the message drops the sender. Electron: `ipcMain.handle` already is this; add the deadline and
  answer `unavailable` once `before-quit` has run.
- **`Debug` on a message must not print its data.** `UiMsg` is `Debug`; the request type prints its kind only, because a path or
  a 4 MB text must never reach a log through `{:?}`. A test pins it.
- **A scan that counts blocking calls caught the wait.** The `sync_calls` scan flags `recv_timeout`; the channel's wait is
  correct (it is a worker thread), so it carries a `sync-ok` reason. Check the scan's list when adding a new waiting thread.
- **Moving the test that reads source order.** `the_import_runs_before_the_look_and_the_first_paint` searches `window.rs` for the
  text `TabHost::new(`; the constructor changed name and the test failed with "window.rs has ...". A test that reads source text
  should name the function it protects in its failure.
- **Do not rely on Claude Code expanding `${VAR}` in `plugin.json`.** The first draft named the server `${FLARE_MCP_EXE}` and passed
  the secrets as `${VAR:-}`; the review found that expansion unverified for a plugin manifest (documented for `.mcp.json`), and that
  a build with no `flare-mcp.exe` left the variable unset, which makes Claude Code report an error for the `flare` server in every
  tab. Fixed the way `hooks.json` already is: a template, with the absolute path written in at install and the block left out when
  the file is missing. Electron: do the same (render `plugin.json` when the plugin is copied, `process.execPath`'s folder as the
  place of the exe), never a variable. **Merge gate, by hand (not machine-verified, the safe rules forbid starting Claude against
  the owner's settings):** open a Flare tab, run `claude`, `/mcp` must list `flare` connected with four tools (five since stage 3, with `open_page`), then ask Claude to
  "show README.md in a panel". If a stdio server turns out not to inherit the shell's environment, put `FLARE_TAB_TOKEN`,
  `FLARE_CTL_PIPE` and `FLARE_CTL_PID` in the manifest's `env` by rendering them at install; the secret must then never be written to
  disk, so prefer fixing the inheritance (a wrapper) over writing it.
- **Hands-on list for the owner (not machine-tested):** Claude Code lists the `flare` server in `/mcp`; the tools appear to Claude
  and `show_text` fills a panel; the agent in a tab that is behind shows its panel without the tab switching; the Velopack
  install path finds `flare-mcp.exe` beside `Flare.exe` (the portable zip is checked by `verify-pack.ps1`, an install is not run
  here).
- **A relative path is the tab's, and the e2e's front tab is not the first folder.** The scenario wrote its picture into the first
  folder and the front tab was the second (the last opened is in front). The channel was right and the test wrong; the picture is
  now written into both folders. Electron: resolve a relative `path` against the tab's cwd, not the main process's.
- **Merge leftovers hide in docs.** `docs/features.md` still held the conflict markers of the stage 2 library merge (two sections
  between `<<<<<<<` and `>>>>>>>`), found only because this stage edited the same file. Grep for `^<<<<<<<` before a PR.

## Stage 2 review fixes, 2026-10-21

- **A click must reset what a call set.** The tab's title and handed-over text were set by the channel and only cleared by the next
  channel call, so a user's click showed a real file under the agent's title. Any path that puts a file in a panel clears both.
  Electron: keep the title in the panel record and write it on every show.
- **Tell the shells about a channel only after it runs.** Building the environment before starting the server meant a failed start
  (the name taken by another process) still handed every shell a secret and a pipe name. The environment is read at spawn time from a
  flag the start sets. Electron: set the variables in the pty options only after `server.listen` succeeds.
- **A squatted pipe name is the one gap a per-user ACL cannot close.** The name is predictable from the SID, and a process of the same
  user (low integrity, restricted token) can create it first. The server fails to start (`FILE_FLAG_FIRST_PIPE_INSTANCE`) and the
  client trusted whoever answered. Give the shell the server's process id and have the client compare it with the pipe's server
  (`GetNamedPipeServerProcessId` on Windows). Electron: `net.createServer` cannot say who connected; if the port keeps a pipe, pass
  the pid and check it with the same API from a small native helper, or use a random pipe name that only this app and its shells know
  (no predictable name at all), which is simpler and also closes it.
- **A refusal's text is a prompt-injection path.** Whatever answers on the pipe, `message` ended in the model's tool result. The
  client cleans it (one line, no control characters, 200 characters).
- **One deadline per call, carried by the request.** Two UI round trips of 10 s each plus a disk read can pass the server's 15 s, and
  a request still queued then acts after the caller was told it timed out. Share one deadline, skip the folder ask for an absolute
  path and let the UI thread drop a request that arrives late. Electron: an IPC handler checks `Date.now() > deadline` before it acts.
- **A test that reads a file with `unwrap_or_default()` can pass with no file.** The panels.json test did exactly that for text that
  was never written. Make the store write something real first (a file panel on another tab), assert the file exists, then assert the
  text is absent and that a restore brings back only the file.
- **Identical fixtures hide a wrong folder.** The relative-path e2e wrote the same picture into both folders. Different sizes now say
  which folder was read, from each tab's own connection, and `mcpRoundTrip` starts `flare-mcp.exe` in a folder that is not the tab's
  to pin its own rule.
- **The design spec is a source too.** It still said `hello` carries `tab` from `FLARE_TAB_ID`; a later stage reading it first would
  rebuild the hole stage 2 closed. Fixed in the spec; protocol.md and decisions.md are the truth.

## Stage 3: the web panel

- **Airspace is the port's problem too.** The README of this folder said an Electron `WebContentsView` has no airspace problem for
  the renderer's own overlays. It does: the view is a native child of the window composited ABOVE the main page's web contents, so
  every HTML menu, dialog and tooltip the main renderer draws is covered by it, exactly as a child HWND covers Slint. The fix is
  the same: when any overlay opens, take `webContents.capturePage()` of the view BEFORE hiding it (`setVisible(false)`), show the
  picture in an `<img>` where the view was, and put the view back when the overlay closes. The table in `README.md` is corrected.
  One property (`overlayOpen`) computed from a closed list of overlays, and a test that opens each one, is the contract.
- **Take the still before the window is hidden.** A hidden WebView2 window has no frame to capture. The window stays up until
  the picture arrives (a few tens of milliseconds, during which the menu is under the page), then hides. Each capture carries a
  token; an answer for an older one, or for a tab that is no longer in front, is dropped. Decode the picture off the UI thread.
  Electron: `capturePage` returns a `NativeImage`; send `toDataURL()` to the renderer, and ignore it if the overlay is already
  closed.
- **A child window takes the Windows keyboard focus, and the host stops hearing keys.** Once the user clicks into the page, key
  events go to the page's window; winit and so Slint hear nothing, including the app's own chords. WebView2 offers one place to
  take a key first, `AcceleratorKeyPressed`, and it must answer inside the callback, so the decision has to be a pure, quick
  function over state the callback can reach without borrowing the window controller (the chord table plus a copy of the key
  context, refreshed at every sync). **Electron:** `webContents.on('before-input-event', (e, input) => ...)` on the view's
  contents is the same hook, also synchronous (`e.preventDefault()`), and a chord table shared with the main renderer gives
  the same set. Keep the same exceptions (a page's Ctrl+F, Ctrl+C, Ctrl+D) and the double Escape.
- **Taking the keyboard back is a separate job.** A click on the terminal does not move Windows focus out of the page's child
  window; without a `SetFocus` on the host window the terminal would show a caret and type into the page. Do it on every
  pointer press on the host window, and when the chord or Escape hands the keys back. **Electron:**
  `mainWindow.webContents.focus()` for the same moments, and `view.webContents.focus()` to give the page the keys.
- **The page taking the keyboard sends the host a kill-focus.** winit reports `Focused(false)` for the parent when focus moves
  into its child, which would close menus, mark the window unfocused and (with agents) look like the user leaving. Ignore it
  while the page has the keyboard, and treat the page's own got-focus and lost-focus as the host's (a lost-focus
  with another foreground window IS the user leaving, and winit does not report it because the parent never had the focus).
  **Do not decide on the foreground window at the instant of the kill-focus:** during a real switch to another app it can still
  be the host's, and the blur (dictation, the Ctrl+Tab walk, tips, menus) was lost. When the window is still foreground and the
  page does not yet claim the keys, look again one message later.
  **Electron:** the BrowserWindow's `focus` and `blur` events already include its views; nothing to do, but check that a blur is
  reported when the page held the focus.
- **COM completions arrive inside the message loop, possibly re-entrantly.** An outgoing COM call in a single-threaded apartment
  can dispatch other callbacks while it waits, so a handler that touches the same state the caller is using borrows it twice.
  Every handler here only posts a value (an inbox plus a wake message, or a `UiMsg`). Electron has no re-entrancy, but the
  habit of answering from an event rather than from the call carries over.
- **Answer a navigation call with the navigation it started.** Answering on the first `NavigationCompleted` after the call
  answered a `back` with the page that was still loading, and everything after raced the real navigation (found when two
  scenario steps passed or failed depending on timing). Use the browser's navigation id: bind the call to the first
  `NavigationStarting` after it (a redirect starts a new one), settle on the completion of that id, and answer a call whose
  navigation is cancelled. **Electron:** `did-start-navigation` gives `navigationId`-like data via the event details
  (`isMainFrame`, the URL); `did-finish-load` and `did-fail-load` end it. Same rule: do not settle on a load that began before
  the call.
- **Created at birth, the rectangle was wrong by the bar.** The page's box depends on the panel being a web panel (the bar adds
  32 px to the head), and the property that says so was set after the controller was made, so the first rectangle was 32 px
  too tall and too high until the next sync. A unit test that compares the creation call with what Slint draws caught it.
  Set the kind before reading the box.
- **Layout is read after a window event, not during.** A winit pre-filter sees a resize before Slint has laid out, so reading the
  box there reads the old size. `changed` handlers on the box's numbers run after the layout and follow every case (resize,
  splitter, sidebar slide, DPI, monitor move) with one code path. **Electron:** `view.setBounds` takes DIPs and Electron does
  the scale; call it from the renderer's `ResizeObserver` on the placeholder element's rectangle (sent over IPC), which has the
  same property: it fires after layout.
- **WebView2 works in a parked, never-activated window.** Navigation, scripts, `CapturePreview`, permission denial and video
  playback all behaved in the e2e window at (-32000, -32000) with no foreground and no focus. The page's `visibilityState` is
  `visible`. 7 frames of the test clip were dropped in the first 2.5 seconds (while the window settles); the steady
  rate was 30.6 frames per second. A scenario can therefore assert behaviour, geometry and clocks; it cannot see pixels of the
  page (the snapshot probe sees Slint, not the child window).
- **A script `click()` is not a user gesture.** `target=_blank` and a link to another page worked from a scripted click;
  `window.open` from the same click was blocked by the browser's own popup blocker before Flare heard of it, so the first test of
  it could never fail. The run now starts the app with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--disable-popup-blocking` (an
  environment variable of the child only), the browser asks Flare for the window, the handler navigates the panel, and the test
  counts top-level windows of both processes. **Electron:** `app.commandLine.appendSwitch('disable-popup-blocking')` under the
  test seam only, and the same count.
- **A world shared by two legs restores the first leg's tabs.** The 225 percent leg started with three tabs because the 100
  percent leg had saved two, and `Ctrl+1` then jumped to a different tab. Count tabs instead of assuming, and jump to the tab by
  its place.
- **A failed start must leave nothing behind, and must not stick.** A view whose controller could not be made is removed from the
  host (the count of controllers is honest, the e2e asserts zero) and the environment is not remembered as failed, so closing the
  panel and opening it again asks the runtime question again.
- **The static loader leaves no DLL to ship, but it is a licence line.** `webview2-com-sys` links `WebView2LoaderStatic.lib`; the
  exe was searched for the DLL's name to be sure. The loader is Microsoft's BSD 3-Clause, which `cargo about` cannot see, so it
  is a hand entry in `about.hbs`. **Electron:** nothing to bundle; Chromium is the app.
- **Adding a crate re-resolved an unrelated one.** `Cargo.lock` moved `tempfile` from `getrandom 0.3.4` to `0.4.3` (already in the
  lock for another crate) when the new package was added, dropping `getrandom 0.3.4` from the notices. No version was chosen by
  hand; check the lock diff for this in every dependency PR.
- **`gen` is a reserved word in edition 2024.** A field called `gen` for a generation counter does not compile; `serial` does.
- **The repository's own guards fired, and they are the checklist for a new surface.** Every one of these failed on the first full
  run and is a thing the port's tests should have the equivalent of: the list of test seams in `tests/names.rs` (Appendix F: two new
  `FLARE_E2E_*` names), `PRIVACY.md` and its network inventory in `tests/privacy.rs` (a web panel is the first place a page the user
  or an agent names is fetched, so it has its own paragraph and keyword; three files with an address literal got rows), the
  blocking-call scan (`sync_calls`: a test's `sleep` needs a marker; the runtime check moved to a worker), the e2e runner's ban on
  `loop` and `sleep` outside `until` (the pages now report through their title and the scenario waits with `until`), the probe
  scan that forbids `borrow_mut` and `.set_` in a probe (a script in a page is a hook request, not a probe), the list of stroked
  glyph paths that each need a size test (four bar glyphs, drawn at eight scales), the no-em-dash scan over the generated notices
  (the ICU text carries one that the committed file had replaced by a hyphen), the rule that nothing reads `LOCALAPPDATA` outside
  `flare_win::paths`, and the stage 1 tests that used the web kind as "a kind this build cannot draw" (they use `board` now).
- **Two existing scenarios fail on this machine with or without this stage.** `dpiMatrix` (15 checks, all the `[gpu]` leg's "no frame gap
  of 100 ms" with gaps of 107 to 266 ms) and `monitorMove` (3: the same gap check, and "Ctrl+C brings the prompt back within 5 s" on
  both legs) fail identically on a build of the stage 2 head (`f804dd7`), run the same way the same hour, so they are the machine
  (`docs/e2e.md` names the sleeping display) and not this change. The scenarios that touch what this stage changed pass: `panelImage`,
  `ctlChannel`, `mcpRoundTrip`, `findKey`, `systemMenu`, `closeAsk`, `helpPanel`, `resizeEdges`, `settingsLive`, `tooltips`,
  `dropAndMenu`, `tabSwitch` and `restore`.
- **WebView2 does not ask before it saves a download.** It saves straight into Downloads. Take the deferral in
  `DownloadStarting`, ask in the app's own dialog, and cancel on anything but a yes (a closed panel, a closing window, a failed
  registration). The first reading of "WebView2's own prompt" was wrong and was only caught in review. **Electron:**
  `session.on('will-download', (e, item) => ...)`; `item.pause()` (or `e.preventDefault()` and restart with `downloadURL` after the
  answer), the SAME question dialog, then `item.setSavePath(path.join(app.getPath('downloads'), item.getFilename()))` and
  `item.resume()`, or `item.cancel()`. A file name is page text: cut and escape it in the dialog.
- **Close the browser before the window.** Live controller and environment objects with registered handlers outlived the window and
  were released whenever the controller dropped, after the COM apartment's teardown (rule 28 is this class of bug). Close every view,
  answer every pending call, drop the environment, and refuse to open again, all before the window hides. **Electron:** close
  every `WebContentsView` in `before-quit` and answer pending IPC calls first.
- **A capture needs a deadline.** `CapturePreview` never answers for a hung page; the page then stays over every menu. Race the
  capture against a 250 ms timer and hide without a picture. **Electron:** the same race around `capturePage()`.
- **Closing the focused child leaves the focus nowhere.** Destroying the window that has the keyboard makes it NULL; keys then reach
  the active window as system keys. `SetFocus(host)` and take the child off the screen before closing it, on every path that
  closes or hides a focused page (tab close, panel close, tab switch, a failed page).
- **A process failure has kinds.** The GPU, a utility process and a frame's renderer are restarted by the browser; only the main
  renderer and the browser process are the page's end. After a browser process exit the environment is dead: close the
  controllers and drop it, or every later open fails. **Electron:** `render-process-gone` carries `reason`; `child-process-gone`
  is for the GPU and utility processes, which Electron restarts.
- **Autorepeat is a key down.** A double-Escape rule counted a held Escape as two. Read the key's `WasKeyDown`
  (`PhysicalKeyStatus`); **Electron:** `input.isAutoRepeat` in `before-input-event`.
- **A navigation call needs the address it waits for, and same-document changes raise no navigation.** Bind to the first
  navigation for the requested address, and settle on `SourceChanged` (`IsNewDocument` false) for `#fragment` changes.
  **Electron:** `did-navigate-in-page` for the second case; match `did-start-navigation`'s URL for the first.
- **The environment and controller creation are not free on the UI thread** (registry, library load, data folder, browser launch).
  They have to stay there; mark them and list them in the blocking-call scan. Cold first `browser.open` on the gate machine: 2.3 s
  end to end (see "Stage 3 review fixes" in `decisions.md`).
- **Hands-on list for the owner (not machine-tested here, the safe rules forbid real input, the network and the user's windows):**
  play a YouTube video in the panel (sound, the full screen button, seeking, a 1080p stream) and check it keeps its frame rate
  while the splitter is dragged; Widevine content (Netflix, Spotify web) if it matters; type in the address bar with a real
  keyboard, then Enter, then Escape twice; click into a page and press Ctrl+Shift+P, Ctrl+T, F1, Ctrl+Tab; click the terminal
  after the page and type; scripted `alert`, `prompt` and file pickers (WebView2's own dialogs, parented to the window); a
  download (Flare's "Save this file?" question, then Downloads; also close the panel and the window while it is up); kill the
  browser's `msedgewebview2.exe` browser process from Task Manager with a page open and look for the note; switch to another app
  with a page focused and check dictation and the Ctrl+Tab walk let go; log in to a site, close the panel, reopen it, and close and reopen Flare (the cookie
  folder is the profile's); move the window between two monitors of different scale with a page open; open a menu over a
  playing video and look for a flash at the swap; the text caret and the Windows IME in the address bar; High Contrast mode.

## Stage 4: Claude can see and use the browser panel

Found while building it (2026-10-22, WebView2 Runtime 155.0.4283.62). What each means for Electron is in the last line of each item.

- **The DOM agent refuses node calls until `DOM.enable`.** `DOM.getContentQuads`, `DOM.scrollIntoViewIfNeeded` and `DOM.focus` by
  backend node number need it; `Accessibility.getFullAXTree`, `Page.getLayoutMetrics` and `Input.*` do not. Every job that touches a
  node sends `DOM.enable` first, in the same batch as the first independent calls (the session keeps order). *Electron:* the same
  protocol, `debugger.sendCommand('DOM.enable')`.
- **The browser draws at the monitor's resolution, not at the window's scale.** The e2e forces Flare's own scale to 100 percent on a
  225 percent machine; the page still rendered at `devicePixelRatio` 2.25 (the controller's rasterization scale follows the real
  monitor). So a "100 percent" run shows a CSS viewport of 177 px in a 399 px controller, and a picture of 367 device pixels. Nothing in
  the agent path may assume Flare's scale equals the page's. A look reports `viewport.device_scale` and the picture's real
  `scale` (read from the PNG header, `width / viewport.width`); the assertions are written in terms of the page's own ratio.
  *Electron:* `webContents.getZoomFactor()` and `window.devicePixelRatio` are the page's; `screen.getDisplayMatching` is the window's.
  Ask the page.
- **`Page.captureScreenshot` with a `clip` is in document space, and its `scale` multiplies the DEVICE pixel size.** A scrolled page
  needs `pageX`/`pageY` of the visual viewport added to the clip, and a 400 css px wide page on a 2.25 display comes out 900 px wide
  at `scale` 1. To keep the picture under 1600 pixels the scale is `1600 / (css width x ratio)`; a page that fits is not scaled at all
  (the sharpest picture is the browser's own). The scale of the picture is measured, not computed. *Electron:* the same call over
  `debugger.sendCommand`; read the PNG's IHDR (`png_size_b64`).
- **The visual viewport leaves out the page's vertical scroll bar.** The look's `viewport.width` is 14 css px (a 32 px bar at 2.25)
  less than the controller when the page is taller than the panel. The picture and the element coordinates use the same viewport, so
  they agree with each other; only a comparison with the controller's width must allow the bar. *Electron:* the same.
- **A screenshot works in the parked window.** The e2e window is off the screen (-32000, -32000), unfocusable and never the foreground
  window, and `Page.captureScreenshot` returns a real picture of the page there (decoded and checked for colours at 100 and 225
  percent). Nothing needs the window to be seen or active. *Electron:* an offscreen `BrowserWindow`/`WebContentsView` that is shown
  (not `hide()`n) behaves the same; a hidden one may not paint, so keep the parked window "shown".
- **A DevTools key event did not reach the key hook.** The page's `AcceleratorKeyPressed` is how Flare hears the person's keys and takes
  its chords. During the agent's own `Enter`, `Control+a` and `Control+Shift+P` presses the hook was asked about 0 keys (counted per
  press in the e2e: `web.state` `agent.keys_heard`), so an agent cannot press an app chord by accident and a press does not look like
  the person. The guard that ignores keys while an agent press is in flight stays, for a runtime that behaves differently. *Electron:*
  verify `before-input-event` for DevTools-dispatched keys; if it fires, mark the agent's presses by a flag set around `sendCommand`.
- **Real mouse buttons cannot be a hook, but can be polled, and a 100 ms poll misses quick clicks.** The agent's mouse events are
  DevTools events and never move a real button or the pointer, so "a real button is down with the pointer over the page" is the
  person. A click lasts 50 to 120 ms and the timer jitters by 15 ms, so a plain "is it down now" at 100 ms lost many clicks: it polls
  every 16 ms and also reads the "pressed since the last call" bit of `GetAsyncKeyState` (cleared when a call starts). The page
  taking the window's keyboard focus (`ControllerGotFocus`) while a call runs is a takeover too, whatever the poll saw; the agent's
  DevTools events never move the window's focus (the e2e proves it by clicking and typing with that rule on). It needs the window
  under the pointer to be Flare's own (`WindowFromPoint` then `GetAncestor(GA_ROOT)`), otherwise a click in another window that covers
  the page would end the agent's call. No `SetWindowsHookEx`, no `SendInput`. The e2e cannot press a real button (rule: never input
  into real windows), so it uses a request that stands in for the person (`WebUserInput`) and the real-button path is on the by-hand
  list. *Electron:* see the port table (`input-event` / `before-mouse-event`).
- **`interrupted` has to leave the page alone, except for a button or key the agent was holding.** A takeover ends the job and
  forgets the refs, but it sends no "cancel": the half-done click or key is whatever the person's input made of it. The one thing it
  does send is the release of a press that was cut short (`mouseReleased`, `keyUp`, no answer awaited), because the renderer would
  otherwise think the button is still down and turn the person's next mouse moves into a drag or a selection. *Electron:* the same,
  in the `finally` of the job.
- **The ribbon cannot take layout from the page.** A row above the page would resize the controller when it came and went, and the page
  would reflow under an agent that is reading its coordinates. It sits in the panel's head over the title instead. The e2e compares the
  controller's rectangle with and without it.
- **Wait polling needs a clock the job does not own.** A job is told the time (`on_tick(now_ms)`) and a timer exists only while a call or
  the ribbon's hold lasts; with none, no timer is registered at all (the idle-cost rule). A wait that runs out answers `met: false`, and a
  wait's check that never answers cannot hold the call past its end plus 3 s.
- **600 DevTools calls at once are fine.** Each listed element asks for its boxes in the same batch (`DOM.getContentQuads`), joined by
  call id, and nothing on the UI thread waits for them. The cap (600) keeps the answer small for a model, not the cost of the calls.
- **Names from a page are the model's input.** The first line of defence is Flare's (`clean_text` before a name leaves the app), the
  second is `flare-mcp`'s (cleaned again, quoted as a JSON string, one element per line, between two marker lines): the e2e page has an
  element whose accessible name contains a line break and a forged `[e99]` line, and a paragraph that reads `END PAGE CONTENT`.
  A name is never put in a line unquoted, so no page text can start a line with `[e` or equal the end marker. *Electron:* port
  `clean_text` and the wrapper with their tests; they are the contract.
- **A browser zoom (Ctrl+wheel in the panel) does not move a click.** Verified in WebView2 155 by setting the controller's
  `ZoomFactor` to 1.5 in the e2e: `window.devicePixelRatio` and the look's `viewport.device_scale` rise by 1.5, and a click by ref and
  a click by point at the centre of a look (CSS pixels) both land on the button. No scaling by the zoom is needed or done. A look
  taken the instant the zoom changes can still hold the old layout's boxes, so the step looks again before it clicks by point.
  *Electron:* `webContents.setZoomFactor` is the same kind of zoom; re-run the step (zoom, look, click by ref and point) there before
  assuming it.
- **An agent in a tab that is behind needs a browser nobody sees.** Flare makes a browser only for the front tab with a shown panel, so
  the agent's first call there used to be told "no web page is open", `open_page` succeeded without loading and the next call said the
  same: a loop. The agent's call now makes the tab's browser hidden. *Electron:* create the `WebContentsView` for the tab's page when
  an agent call needs it, not only when its tab is shown; keep it unattached or hidden until the tab is in front.
- **A look that straddles a navigation must not keep the old page's names.** The tree is read, the page begins a new document, then the
  look finishes: its refs belong to the document that is gone. A counter of documents (raised at every navigation start) recorded when
  the call begins decides. *Electron:* the same counter on `did-start-navigation` for the main frame.
- **Hidden characters beyond zero-width ones.** Unicode tag characters (U+E0000 to U+E007F) spell ASCII words that a person never sees
  and a model reads ("ASCII smuggling"); variation selectors (FE00 to FE0F, E0100 to E01EF), the Mongolian vowel separator, the
  combining grapheme joiner, the Hangul fillers and private use code points can carry text the same way. One shared list now drops
  them (`flare_ctl::text::invisible`, with its tests); U+2028 and U+2029 count as white space. A title is cleaned and quoted in a
  navigation answer like any other page text. *Electron:* port the list and its tests; do not write a second copy.
- **A page's own getter can hang the pixel-ratio question.** `window.devicePixelRatio` runs in the page's main world; a page that
  defines a getter that never returns would stall the look to its 20 s budget. `Runtime.evaluate` takes a `timeout`, so the question
  has one (2 s) and falls back to 1.0. The wait checker runs in the main world too (accepted, `decisions.md`).
- **Stage 3's `if` rule again: keep the permanent thing permanent.** The ribbon property is always in the tree (empty means hidden),
  like the viewers, so the head's text does not jump when it appears.
- **A page that is not on screen paints nothing, so `Page.captureScreenshot` never answers; its tree and boxes still do.** The first
  version looked at a page whose tab was behind (its controller hidden) and ran the whole 20 s budget into `timeout`. The accessibility
  tree, the boxes and the layout metrics of a hidden page answer at once (16 ms at 100 percent and 263 ms at 225 percent in the e2e,
  with the right elements). So a look of a page that is not on screen (its tab behind, or a Flare menu open over it, the phases the
  window calls hidden and still) skips the picture and says why in `image_error`, and a picture that is asked for and does not come in
  6 s is given up on the same way: the list is what the agent most needs, and it is never lost to the picture. Clicks and keys to a
  hidden page are answered. *Electron (not verified there):* a `WebContentsView` that is `setVisible(false)` probably behaves the
  same, so give a screenshot of it its own short timer and fall back to the list.
- **Frames the browser puts in another process are not in the list.** The accessibility tree of the page and the boxes by node number
  belong to the main frame's process; a cross-origin iframe's nodes either do not appear or fail `DOM.getContentQuads` and are left out
  (an element with no box is never listed). An agent that needs one navigates the panel to the frame's own address. Not handled, written
  down. *Electron:* the same limit with `webContents.debugger` on the main frame; attaching to the frame's target is a later step.

### By hand, not machine-tested (Stage 4)

1. A REAL mouse click in the page while a call runs: the call answers `interrupted` and the ribbon goes (the polling of the real
   buttons; the e2e stands in for it with `WebUserInput`). Include a QUICK tap (a few tens of ms, a touchpad tap) and a click on a
   page that did not have the keyboard focus: both must end the call.
2. A REAL key typed in the page while a call runs: the same.
3. A click in ANOTHER window that sits over the page while a call runs must not end the call (`WindowFromPoint` is Flare's window or the
   click is not the person's).
4. The ribbon at the themes (light and dark): the accent tint over the head reads, the words fit at 100 and 225 percent, the page's
   title does NOT show through under the words (the base is opaque), and the title comes back when it goes.
5. A monitor change between a look and a click: the click still lands (CSS pixels), and the next look reports the new `device_scale`.
6. A real agent: in a Flare tab ask Claude to open a page with `open_page`, look, click a link and read the new heading; watch the ribbon
   and take over by clicking in the page.
7. A page with a cross-origin iframe, a canvas and a shadow-DOM component: what the list shows (known limits above).
8. Switch "Let agents run scripts in pages" on and off in Settings with a Claude session open: `browser_eval` is refused, then allowed,
   then refused again with no restart.
9. A REAL takeover in the middle of an agent click: press and hold the real mouse in the page while the agent clicks; afterwards a
   plain mouse move over the page must not select text or drag (the agent's button was let go of).
10. An agent in a tab that is behind: ask Claude to `open_page` and `browser_look` while another tab is in front; the look must work
    (the list, no picture) and the page must show when the tab is brought to the front.
11. A very large page (thousands of nodes): time a look by hand; the tree is parsed on the UI thread, so watch for a hitch.
12. Ctrl+wheel the panel to zoom while Claude is asked to click a link: it must land (the e2e proves it at 150 percent).
