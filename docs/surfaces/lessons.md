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

## Stage 5: mini-apps and diagrams

Found while building it (2026-10-22, WebView2 Runtime 155.0.4283.62, Mermaid 11.17.2). What each means for Electron is in the last
line of each item.

- **A profile is enough; a second environment is not needed, and costs a browser restart.** `CreateCoreWebView2ControllerOptions`
  then `SetProfileName("flare-app")` and `SetIsInPrivateModeEnabled(true)` then `CreateCoreWebView2ControllerWithOptions` gives a
  view with cookies, storage and cache of its own that keeps nothing on disk, inside the environment the web panel already runs
  (`ICoreWebView2_13::Profile` reads the name and the private flag back; the e2e asserts both). It needs the runtime's
  `ICoreWebView2Environment10` (1.0.1245 or later); an older runtime fails the view rather than falling back to the shared profile.
  *Electron:* `session.fromPartition('flare-app')` with no `persist:` prefix is in memory and separate from the web panel's
  `persist:flare-web`; create the mini-app's `WebContentsView` with `webPreferences.partition` set to it (never `defaultSession`).
- **Origins are the isolation between apps, so each app gets its own host name.** One shared host would make two apps share
  `localStorage`, cookies and IndexedDB inside the one private profile. `flare-app-<n>.local` per app; the e2e writes storage and a
  cookie in the first and finds neither in the second. *Electron:* serve each app from its own origin the same way (a custom
  protocol handler on that session answering `https://flare-app-<n>.local/...`, or `app-<n>://`), with the page held in memory:
  there is then no folder to write or clean at all, and the Mermaid file is answered from the app's resources instead of being copied.
- **A meta policy written first is a real wall, and removing the tag removes nothing.** The e2e's page deletes its own
  `<meta http-equiv="Content-Security-Policy">` and fetches again: the second fetch is still a `connect-src` violation. The
  document's policy is fixed when the parser meets the tag; Flare's tag is the first element, before anything the agent wrote.
  The violation EVENT (`securitypolicyviolation` on `document`, with `effectiveDirective` and `blockedURI`) is the proof that the
  policy, and not a closed port, stopped a request: a fetch to `http://127.0.0.1:9/` that merely failed to connect gives no event.
  Script elements report `script-src-elem`, images `img-src`, fetch `connect-src`. *Electron:* when the page is answered by your own
  protocol handler, send the policy as a real response header as well (`Content-Security-Policy` in the `Response` headers); keep
  the first-element meta too, so the wall is the same wherever the page came from.
- **Mermaid draws under `default-src 'self' 'unsafe-inline' data: blob:` with no `'unsafe-eval'`.** It is a classic script
  (`<script src="mermaid.min.js">`, an IIFE that sets `globalThis.mermaid`), needs `style` inline (allowed), uses DOMPurify for
  `securityLevel: 'strict'`, and renders into an `svg` string that is put in the page with `innerHTML`. Version 11.17.2 is 3.5 MB; the
  copy into each diagram's folder is a plain file copy on the call's thread. `mermaid.parse(text)` throws a real `Parse error on line 2: ... Expecting
  ...` with a pointer line; call it before `render`, because a `render` that fails can leave an error graphic in the page.
  *Electron:* the same bundle, the same two calls, the same strict level, no `unsafe-eval`; answer the script from the app's
  resources through the protocol handler.
- **Asking the page whether it drew is one `Runtime.evaluate` with `awaitPromise`.** The page exposes `window.__flareDiagram` (a
  promise of `{ok}` or `{ok: false, error}`); after `NavigationCompleted` Flare evaluates that name with `awaitPromise: true,
  returnByValue: true` and the CDP answer is `{"result": {"type": "object", "value": {...}}}` (an `exceptionDetails` instead when the
  page threw outside its own catch). `app::diagram_result` reads both, and any other shape is an error, never a quiet yes. *Electron:*
  `webContents.debugger.sendCommand('Runtime.evaluate', ...)`, the same parameters and the same two shapes.
- **Replace a view in place; do not close the last one.** Stage 3 frees the browser environment with the last view (the anti-goal
  "panels cost nothing while closed"). A mini-app is typically updated again and again by an agent, so closing the old view before
  opening the new one would have restarted the whole browser each time (a new browser process). The host now swaps the *view*
  (`close_view`) and keeps the environment; the window forgets the old page's state (`replace_tab`) without closing the view. The
  bug was found because a unit test's stand-in browser was remade by the factory when the last view went and lost its record.
  *Electron:* `view.webContents.close()` and a new `WebContentsView` in the same session; never destroy the session or the
  `BrowserWindow` for a replacement.
- **A page must not rename the panel or move its address.** `DocumentTitleChanged` and `SourceChanged` flow into the panel's title and
  stored address for a web page; for a mini-app both are ignored (the agent's title is the name, and the address is Flare's own and
  never stored). A script that sets `document.title` or `history.pushState` cannot make the panel say "Your bank". *Electron:* ignore
  `page-title-updated` and `did-navigate-in-page` for these views.
- **The second wall is testable with a `no-cors` fetch.** With the policy switched off by the e2e seam, a request to
  `http://127.0.0.1:9/` made with `mode: 'no-cors'` resolves as an opaque response if Flare answered it (a synthetic 403) and rejects
  if the connection was really tried and refused. The e2e holds `answered:opaque`. *Electron:* `session.webRequest.onBeforeRequest`
  with a filter of `<all_urls>` that cancels any URL that is not the app's own origin, `data:`, `blob:` or an allowed `https:`; a
  cancelled request rejects in the page, so count cancels in the main process (or use `onErrorOccurred`) instead of the opaque probe.
- **Navigation, links and new windows.** `NavigationStarting` cancels everything but the app's own address (the same address with a
  different `#fragment` passes); `NewWindowRequested` is handled and does nothing for a mini-app (the web panel sends it to the panel).
  A page's `location.href`, a link click, a form post (`form-action 'none'` as well) and `window.open` all went nowhere in the e2e, the
  header said "Not opened here" and the view stayed on its address. *Electron:* `will-navigate` and `will-redirect` with the same
  check, `setWindowOpenHandler(() => ({action: 'deny'}))`.
- **A diagram's error is worth its length.** Mermaid's message is 3 to 6 lines with a pointer at the offending column and the list
  of tokens it expected; the model needs all of it to fix the text. The usual 200-character cleaning of a refusal cut it mid-sentence,
  so the diagram tool keeps up to 900 characters and the line breaks (control characters still become spaces). *Electron:* the
  same: do not squeeze this one error into a line.
- **`ExecuteScript` answers JSON, so a string answer is JSON inside the test's JSON.** The e2e decodes the page's answer once with a JSON
  parse before comparing text; comparing the quoted form passed by accident for ASCII and failed for the error text with a line break.
- **A control that is not drawn must give its space back.** `bar-h` is 0 for a mini-app (no address bar), so the page starts directly
  under the header and the layout maths (`web_rect` with the header height only) equals the browser window's bounds at 100 and 225
  percent; the e2e holds the three-way equality. *Electron:* the same: the placeholder rectangle has no bar row for these views.
- **Not verified, written down:** whether `WebResourceRequested` is raised for the files a virtual-host mapping serves (the wall lets the
  app's own host through either way, so it did not matter); how a very large inline page (the 4 MB cap) loads (the e2e uses a
  small one; the cap itself is checked); SVG with script inside a page (the policy is the same, a script in it is the page's own);
  print and save-as of a mini-app (the browser's own, not handled).

### Stage 5 review fixes

- **Check the site, not the origin, when you want two pages apart.** Browsers draw the cookie boundary at the registrable domain
  ("site"), and a made-up `.local` name has no public suffix, so every subdomain of one parent is one site. Use a different
  registrable name per app (`flare-app-<n>.local`), and test with a `Domain=` cookie, not a host-only one. What Electron must do:
  one scheme or one unrelated host per app, never subdomains of one parent.
- **A Content-Security-Policy does not stop WebRTC, and the request hook does not see it.** Three walls, because each alone leaks:
  remove the API before the page runs (a document-created script in every frame, and AWAIT its completion before the first
  navigation, failing the view on an error); give mini-apps a browser environment of their own started with the IP handling policy
  and, with no network, a dead proxy; and shut child frames in the policy. **A browser switch is not a wall until a socket says so.**
  `--force-webrtc-ip-handling-policy` is documented for Chromium and did nothing in WebView2 155; `--webrtc-ip-handling-policy` worked.
  The test that settled it: a UDP socket on loopback as the STUN server, counting packets from a page with the script off, plus the
  same probe in an ordinary panel as the control (it must hear packets, or silence proves nothing). *Electron:* check the policy with the
  same kind of socket, not by reading the setting back.
- **Two environments cannot share a user data folder if their arguments differ, and an environment is a whole browser.** One folder per
  kind (`WebView2-app-sealed`, `WebView2-app-net`), made lazily, closed with the last view of that kind. A swap to the same kind must
  keep the browser (the e2e compares the process id); a swap to another kind drops the idle one.
- **A request filter that is not told the source kinds sees only the document's requests.** Use the all-sources variant where
  the runtime has it, and test a srcdoc frame and a blob worker, not one main-frame fetch.
- **Never write a backslash-letter into a comment through a tool that treats it as an escape.** `` became a BEL byte once; the
  comment now says `<profile>pps` by hand. A recursive delete at start is renamed aside and swept on a worker instead.
- **Error text quoting the agent's own source passes the shared hidden-character filter** (`flare_ctl::text::invisible`) like
  every other outside word, line breaks kept.

### By hand, not machine-tested (Stage 5)

1. A mini-app in a tab behind, then bringing the tab to the front: the page is there, sized to the slot, with no flash of a blank panel.
2. An overlay (a Flare menu or dialog) over a mini-app: the still of the page stays in its place and the page comes back live.
3. The ribbon over a mini-app while Claude uses it (`browser_look` then `browser_click`), and the real mouse taking it over.
4. Light and dark system themes: a diagram follows the system setting, not Flare's theme; check both read on the panel's background.
5. A real Claude session: ask for a chart as a page, then ask for a flowchart with an error in it; it should fix the Mermaid from the
   error text alone.
6. "Let mini apps use the network" on, a page that loads a script from a CDN over https: it loads; with the setting off it does not and
   the agent is told why.
7. A page that tries to read the clipboard, the camera, the location: denied (the web panel's permission handler is the same).
8. Pressing Escape twice in a mini-app, and Ctrl+Shift+O: the keys go back to the terminal and into the page, as for a web page.
9. A very large chart (a 4 MB page of data): load time and memory.

- **Electron note, 2026-10-22: WebView2 browser arguments are not the last word.** The environment variable and the policy key of the same name
  REPLACE the options value (from the WebView2 documentation, not re-measured here), so a sandbox that rests on arguments must remove the
  variable and refuse on the policy. Port 9 is not privileged on Windows: never rest a wall on "nothing listens there".
## Stage 6 library, the conversation reader (`flare-convo`), 2026-10-21

- **NTFS tunnelling defeats a creation-time replacement check.** A file deleted and created again within about 15 seconds keeps the old creation time, so `MetadataExt::creation_time` said "same file" for a real replacement. The test that caught it failed with `restarted == false`. The file index and volume serial, read from the open handle with `GetFileInformationByHandle`, changed. Electron: Node's `fs.stat` gives `ino` and `dev` (on Windows, the file index and the volume serial); use those, never `birthtime`, for "is this the same file". Test it with a delete-and-write, not only a truncate, because a truncate keeps both.
- **A shrink is not the only way a file restarts.** A truncate is caught by `len < offset`. A replacement that is longer than the old offset passes that check, so the identity check is what catches it. Keep both checks.
- **Read through the open handle, not a separate stat of the path.** The first draft stat'ed the path and then opened it, so a replacement between the two could give a length and an identity from different files. Opening once and taking the length and the identity from the handle closes the gap. Electron: `fs.open` then `fh.stat()`, not `fs.stat(path)` followed by a read by name.
- **A test expectation was counted by hand and was wrong.** The fixture has 21 lines; the first draft asserted 23 and failed. Count with `wc -l`, or compare against the file's own `lines()` count, before trusting a number.
- **Clippy 1.99 wants let-chains for nested `if let`.** `collapsible_if` fails `-D warnings` on the nested form. Edition 2024 writes it as `if let A = x && let B = y { }`. Electron: no equivalent lint applies; the nested form is fine in JavaScript.
- **A "last value" read after a loop is clearer as the loop's value.** The `unused_assignments` warning came from a `let mut` set inside a loop and read after it. `let x = loop { ...; break value; }` says what is meant.
- **What this means for Prism Terminal (Electron).** The transcript reader is a main-process worker (rule 34: no `*Sync` on a live path). `fs.open` once per poll, read at most 4 MB at an offset, split on `\n` only, carry the partial line, drop a line over 8 MB while it is read. Poll every 500 ms only while a panel is subscribed, and end the subscription on the panel's `destroyed`. The fold is pure JSON handling and ports line for line; `docs/surfaces/convo.md` says which shapes are guesses. The slug is the folder's non-alphanumeric UTF-16 units replaced by `-` (`encode_cwd`); `path.join` does not do that for you.

### Stage 6 library, review fixes (2026-10-21)

- **A fixture that follows the author's model can hide a wrong layout.** The synthetic fixture put subagent lines inline in the session file, so every test passed on a layout that does not exist, and the reader tailed only that file. A fixture has to copy the real file layout, and one structure-only check of a real session is the test of it. Electron: the subagent folder is `<slug>/<session-id>/subagents`; list it with `fs.readdir` on each poll (asynchronously), and open each `agent-*.jsonl` by name.
- **Rust's regex has no lookbehind; JavaScript does.** The path rule guards the drive letter with a leading character class, which made the Rust version capture the character before the path. Electron: `(?<![A-Za-z0-9_/\\:])[A-Za-z]:[\\/]...` says the same thing directly.
- **A backslash in a raw test string is an escape in the JSON.** A JSON line written with one backslash in a Rust raw string reads `\r` and `\b` as control characters, and the test failed for the wrong reason. Put `\\` in test JSON. A regex character class with `[\/]` matches only a slash; the path test caught that when the backslash went missing in a patch.
- **A stop test must observe the stop.** The first version slept after the drop and asserted nothing, so a worker that never stopped would pass. Electron: keep a flag the worker sets when its loop ends, and assert it within a bound after the subscription is dropped.
- **Backpressure, not a bigger buffer.** A reader that queues every batch holds the whole file when the consumer is slow. Rust: a `sync_channel` of two, with the worker waiting while it is full. Electron: stop scheduling the next `fs.read` until the consumer has taken the last batch; never push into an unbounded array.
- **Cap the model, not only the read.** A 4 MB read cap says nothing about memory when every edit's text is kept forever. Cap each stored text, the edit count and the total text bytes, and drop the oldest first.

## Stage 6: the Changes and Tasks panels, 2026-10-22

Holes hit while wiring the library into the window, with what each means for Prism Terminal (Electron).

- **A Slint element with no x and y is CENTRED in its parent.** The diff's "Files / path" bar was declared with a width and a height and no position, so it appeared in the middle of the panel and the click on "Files" hit nothing. A screenshot (`Frame::save_png`) found it in one look. Give every non-layout child an explicit `x` and `y`. Electron: nothing to port (CSS flows from the top left), but a test that clicks the bar's real rectangle is how this was caught.
- **`accessible-role` must be a constant in Slint.** A row that is a button for a header and text for a task cannot say so with `kind == 0 ? button : text`. Two touch areas, one per kind, each with its own constant role. Electron: `role="button"` on the header and none on a task, set in the markup, no problem.
- **`Flickable` says `content-*` now, not `viewport-*` (Slint 1.18).** The old names compile with a deprecation warning that is a build message, not an error; the `-D warnings` gate does not see it. Use the new names in new files.
- **`preferred-height` of a layout reads 0 before the first layout pass, and a layout under an `if` reports nothing to its parent.** A test that read the content height right after setting the rows got 0, and rows whose height came from a conditional layout had none. Each row now states its own height (a header 30 px, a task as tall as its words plus padding), and a test calls `render()` before it reads. Electron: nothing to port, but the same rule holds for a virtual list: give every row a height it can compute, never one it must measure after drawing.
- **The task list is the one view that is not virtualised, on purpose.** Tasks are tens, wrap their words, and have variable heights; a virtual list wants fixed ones. The file list (44 px) and the diff (18 px) are fixed and are virtualised. Electron: a windowed list (`react-window` or a hand-rolled one) for the file list and the diff with the same two row heights; plain DOM for tasks; cap the diff at 5000 rows and the tasks at 500 as Flare does (`flare_convo::view`).
- **The worker, not the window, folds and diffs.** The tracker keeps the `Convo` and builds the snapshot; the window gets finished rows. A diff is cached by a hash of the file's edit ids and outcomes, so a snapshot made for a task update does not diff again. Electron: do the same in a worker thread of the main process (`worker_threads`) or at least off the renderer's event loop; send the renderer rows, not the transcript. A 100 ms coalescing window keeps a burst of lines to one update.
- **A restart re-reads every file, not one.** The library asks the consumer to clear "that file's part" on a restart; clearing one source's lists needs per-source bookkeeping the model does not keep. Resubscribing from 0 and starting a new model is a few lines and runs once. Electron: same; on a stat identity change (`ino` plus `dev`) of any tailed file, close every tail and start over.
- **The session is known only after the first prompt, and a panel opened before that must say so.** The hook that names the session fires on the first turn; a Changes panel opened on a fresh Claude tab has nothing to track yet. The panel shows its own words and the tracker starts when the hub learns the session (the same host wake that applies the hook). Electron: the session lives in the agent hub per tab; subscribe the panel to "session changed", not to a timer.
- **`convo.state` needs the panel, because the panel is what pays for the reading.** The alternative, a read on demand, would mean a bounded transcript read on the UI path or a short-lived worker for each call. The honest answer for a tab nobody is looking at is "not reading", with the one call that makes it read (`panel.open`). Electron: the same rule; do not start a tailer for a method call.
- **A panel on a tab that is behind keeps reading.** It is cheap (one thread, one wake per 100 ms and one `stat` per 400 ms until the file exists) and it makes the list current when the person switches. If that cost matters, drop the tracker when the tab leaves the front and start it again on return; `ConvoCtl::sync` would only need the front tab in `wants`.
- **The diff's path click replaces the panel.** The slot holds one panel, so the preview of stage 1 takes the place of the list. The person comes back with "Show changes". A second slot (two panels per tab) is on the later list in the design.
- **Colours from the theme need a global the theme code writes.** `ConvoInk` is set where the chrome tokens are (`apply_chrome`), from the terminal palette. The global's defaults are theme tokens, replaced at the first `apply_chrome`. Electron: CSS variables on the panel root set from the same two ANSI slots (green, red) whenever the theme changes.
- **The path rule for CLAUDE_CONFIG_DIR is "absolute or ignored".** A relative value would point the reader at whatever folder the tab happens to be in. Electron: `path.isAbsolute` before use.
- **What the e2e proved at 100 and 225 percent, and what it cannot:** the panels open through the channel, list the file, show the diff rows by a real click on the row's rectangle, follow appended lines, switch with a new session and stop on close. The look (the tints, the dot, the tick) is held by pixel tests in `ui_tests/convo.rs` at 100 and 225 percent and by the glyph size tests (`icons_misc.rs`, all eight scalings).

### By hand, not machine-tested (Stage 6)

- A real Claude Code session: the shapes of its real `tool_result` lines for a failed edit (`is_error` as a boolean), and `TaskCreate` and `TaskUpdate` (guessed in `convo.md`), against a live transcript. The fixtures are synthetic by rule.
- The look of the diff and the task list in all 18 themes (the tints are alpha over the ground; a light theme and a glass theme were not checked by eye).
- A very long real session (a transcript of hundreds of megabytes): the first read is bounded at 4 MB a poll and the model at 2000 edits, but the time to the first full list was not measured.
- `CLAUDE_CONFIG_DIR` set by a shell profile after Flare starts (not seen), and a profile that moves `.claude` with a junction.

### Stage 6, review fixes (2026-10-22)

- **Never hand a long line to a no-wrap text.** Shaping is per character and happens on the UI thread; one 64 KB line costs milliseconds each time its row is built. Cut the text where the rows are made (here 400 characters and an ellipsis) and say so. Electron: truncate in the main process before the IPC, not in CSS (`text-overflow` still lays out the whole string).
- **A snapshot is several pieces; track them separately.** One counter for "something changed" makes every update rebuild the diff and the task list even when only the file list moved. Keep a change count per piece (or give the renderer a keyed store) so unchanged lists keep their DOM/model. Electron: separate state slices per list, not one object replaced whole.
- **A worker's answer can outlive the question.** A path check that comes back after the person switched panels must be dropped, not applied: carry what the click was about (here the picked file) and compare on arrival. Electron: an `AbortController` or a request id per click.
- **A "not there yet" search needs a back-off.** A fresh Claude session has no transcript until its first prompt, and that state can last as long as the panel is open. Electron: the same doubling timer up to 30 s.
- **Wrapped text has a fractional height; snap the sum, not only the constants.** At 125, 175 and 225 percent every row after a wrapped one sits off the pixel grid unless the text's own height is rounded (up). The software renderer rounds rectangles anyway, so only a test of the layout's total (`wrapped_task_rows_sit_on_whole_device_pixels_at_fractional_scales`) catches it.
- **`panelImage` at 225 percent failed 10 checks once (a click on a path timed out) and was not reproduced.** After the review fixes it ran 10 times in a row on this branch's release exe (66 checks, 0 failures each, 100 and 225 percent). The conversation views are `visible: false` while an image or text panel shows, so they take no pointer events; the one failure came while another builder was compiling on the same machine, and the scenario's click waits are timed. Treated as load, not as the branch, but it is NOT proven unrelated: if it comes back, make `ChangesView` and `TasksView` conditional (`if root.kind == 4`) and re-measure. The gate is run alone, with the display awake (see `docs/e2e.md`).
- **Full e2e after the review fixes (2026-10-22):** everything green except the four dictation scenarios (the model and audio fixtures are not in the fixture cache on this machine; no download was made) and `monitorMove`, whose two "Ctrl+C brings the prompt back within 5 s" checks fail the same way on an older exe (the 2026-10-21 build in another target folder), so they are not from this branch. `panelImage`, `convoPanels`, `webPanel`, `agentBrowser` and `mcpBrowser` pass.

## Stage 7: live preview, questions as notifications (the board section below was removed in round 9)

Holes hit, what fixed them, and what Prism Terminal's Electron build must do. The decisions are in `decisions.md` (Stage 7).

- **A viewport link scanner cannot see a server in a tab the person left, a byte reader cannot follow ConPTY, and a history suggestion is not output.** Three traps, in the order they were hit. (1) The link scanner scans only the tab in front. (2) A reader of the pty's bytes (a small escape-sequence state machine) looked right in unit tests and read PSReadLine's inline prediction as output: ConPTY does not forward the program's bytes, it repaints the console buffer with absolute cursor moves (`ESC[r;cH`), so the bytes never say what the row ended up holding. (3) Scanning the emulator's rows is right, and the traps there are the HISTORY (alacritty keeps a cleared screen in it, so "is the address still on the screen" cannot be answered from history rows) and the INPUT ROW (the row the cursor is on holds what is typed, and the shell's suggestion after it). Fix: read only the live screen's logical lines on demand (any tab, at most every 40 ms, off the alternate screen), leave out the cursor's row, call an address news only when it is a different one on a row no older than the held one, and listen to the bytes only for ED 2, ED 3 and RIS. Electron: do this in the MAIN process over a headless xterm (`@xterm/headless`) fed with the pty's data for every tab, so the buffer is the same truth; read `buffer.active` lines from `baseY` (the live screen), skip the line of `cursorY`, join wrapped lines with `isWrapped`, and hear clears with `parser.registerCsiHandler({final: 'J'}, ...)` and `registerEscHandler({final: 'c'})` returning false. Do not scan the scrollback for the address. Port `dev_server.rs` (the rule and `ClearWatch`) and the emulator tests `a_suggestion_painted_after_the_cursor_and_typed_over_is_not_read` and `the_row_the_cursor_is_on_is_input_and_never_read`.
- **Compare what is on the screen, never absolute row numbers, and let the chip hear a reprint.** Absolute rows (history plus row) shift with every line once the scrollback is full, so any "is this older" test on them goes stale; keep the addresses and screen rows seen at the last reading and compare against those. A server restarted on the same port prints the SAME address, which is not news for the held address but is the only sign the server is back: expose a second counter for it and act on it only when no chip is live. In Electron: with `@xterm/headless`, read `buffer.active` rows relative to `viewportY`, keep the previous reading's list of `[url, row]`, and bump `reprint` when the held url has more occurrences or a lower row than before. A retry timer is per tab (a `Map<tabId, deadline>` that keeps the earliest), never one shared deadline. Re-check a shown chip only in the tab in front and not while `BrowserWindow.isMinimized()`, and look once on `tab-activated` and `restore`. Ignore ED 2 / ED 3 while the alternate screen is on (follow `?47`, `?1047`, `?1049` with `parser.registerCsiHandler({prefix: '?', final: 'h'|'l'})`).
- **`localhost` is `::1` on a modern Node.** Vite and others print `http://localhost:5173/` and listen on IPv6 only, so a listener check that reads only the IPv4 table says "nothing there". Flare reads both tables (`GetExtendedTcpTable`, `AF_INET` and `AF_INET6`). Electron has no listening-table API: run `%SystemRoot%\System32\netstat.exe -ano -p TCP` (absolute path, `execFile`, never a shell; and the IPv6 table with `-p TCPv6`) on a worker, parse the `LISTENING` rows' local port, at the same schedule. Do NOT use `net.connect` to look: it makes a client the dev server sees (and some servers do work on a connection). If a native module is ever acceptable, the call is the same two `GetExtendedTcpTable` calls.
- **"The process is gone" is a property of the port, not of the shell.** A shell that returned to its prompt may still have a backgrounded server, and a short command (`echo http://localhost:3000`, which the e2e uses) returns to its prompt at once; the port's listener is the only signal that is both cheap and right. Two failed looks in a row end the chip (a restart under a file watcher is a blink, not an end).
- **The chip must never open a remote address.** The rule is a whole-host comparison on a closed list plus "port required, no user part, plain ASCII", applied at detection and again at the click, and the address is then normalised by the same function the address bar uses. A prefix test (`startsWith('http://localhost')`) is the classic hole (`localhost.evil.com`, `localhost@evil.com`). Port `parse_local` with its tests as they are; the `URL` class parses but does not decide.
- **A notification needs the app's identity, and Windows is silent when it is missing.** An unpackaged app's toast shows only when the AppUserModelID is set on the process AND a Start menu shortcut carries the same id; otherwise `Show` succeeds and nothing appears. Flare's installer gives the shortcut `com.flare.terminal`. Electron: `app.setAppUserModelId('<the id the installer gives the shortcut>')` before any `Notification`; electron-builder's NSIS target is documented to give the Start menu shortcut the `appId` as its AUMID (confirm that on the installed build, it is the one thing that decides whether a toast shows), and an `npm start` run has no such shortcut, so a dev run shows no toast. Use `new Notification({ title, body })` in the main process (not the renderer's `Notification`), `silent: false` is the default, give NO `actions`, `toastXml` or `urgency`, and handle `notification.on('click', ...)` to show and focus the window and switch to the tab. A toast has no tag in Electron: keep a `Map` from tab id to the live `Notification` and `close()` the older one when a newer one for the same tab is shown, and drop every entry when the app quits. Under the e2e, inject the notification factory (an object with `show(title, body, onClick)`) so the suite records and never shows one.
- **A toast clicked after the app quit must do nothing.** Flare registers no COM activator, on purpose. Electron: do not register `toast activator` / `app.setAsDefaultProtocolClient` for the notification; the click handler exists only on the live `Notification` object.
- **Decide "looking" once, in a pure function.** The rule (tab in front AND window has the keyboard; a question seen is never announced; grace for the text; one per episode) is `flare_surfaces::notify::Notifier`, ported with its tests. In Electron the inputs are `BrowserWindow.isFocused()` (false when minimised) and the renderer's active tab; call it on every agent-state change, every tab switch, every focus event and on a timer for the grace.
- **A closed list of fixed row heights makes the board testable.** The rows are 56 css px (92 with a question), so the probe computes the same rectangles without asking Slint. Electron: fixed CSS heights for both and a Playwright check that the rectangles match the model.
- **The panel is per tab, the board is not.** The board is the panel of the tab it was opened in, and lists every tab. That tab keeps it (stored in `panels.json`) when the person goes to another tab; the other tab shows its own panel. A click on a row goes to that tab and leaves the board where it is. Electron: the same (a per-tab panel slot whose content for kind `board` is derived from the whole agent store).
- **Layout is lazy in headless tests.** A Slint `out property` that reads a layout's `preferred-height` is 0 until the window has been drawn once; the test renders before reading it. Not an app problem (the app is always drawn), but a trap for a test.
- **Line endings.** The working tree is CRLF with `core.autocrlf=true`; an editing script that reads and writes text through Python normalises a file to LF. Git commits LF either way and `rustfmt` does not mind, but a script that writes a Rust string with `\r\n` through a shell heredoc can put real carriage returns into a literal (the compiler says "bare CR not allowed"): write such tests with an editor tool, not a shell.

- **A test that compares two walks of the process's own memory needs absolute slack.** `memory::tests::the_regions_add_up_to_the_private_working_set` allowed a tenth of the private set between `regions()` and `private_working_set()`. A test process's private set is about 2 MB, so a tenth is 200 KB, less than what the two walks themselves allocate between them (the page list, a name map): it failed about half the time by a few hundred KB either way (regions 2.7 MB against 2.19 MB, then 2.06 MB against 2.33 MB), more often as the test binary changed shape, and it failed on every run of the full suite once this stage's code was in. The slack is now a tenth or 1 MiB, whichever is more. Not the app's code, but the lesson holds: a measured-against-itself check is a flaky check unless its tolerance is in bytes.

### By hand, not machine-tested (Stage 7)

- A real Windows toast on the installed build: that it appears, with the tab's name and the question, that a click brings the window up and fronts the tab, that a second question replaces the first in the action centre, that Focus Assist / Do Not Disturb silences it as the person set. The suite never shows one (the recorder stands in) and `release_files.rs` only holds that the shortcut carries the AUMID. Also: what happens when a toast is clicked after Flare has quit (expected: the app starts, no tab change).
- That the toast does not appear from a build run from a folder (no shortcut with the AUMID): expected, and the taskbar flash fires only when Windows refuses the request, which it does not in that case. A dev run therefore shows neither; the installed build is the check.
- A real Vite, Next.js, webpack-dev-server, Rails and `python -m http.server` printing their banners (the e2e uses `echo` and a listener of its own). Python prints `0.0.0.0`, which gets no chip by decision.
- The chip over real content in all 18 themes (it uses the tab strip's fill, the accent outline and the text colour) and at 150 percent.
- A dev server on IPv6 only in a real shell (the unit test binds `[::1]` when the machine allows it).
## Stage 8, library part (`flare-mods`), 2026-10-22

- **A junction needs no privilege, a file symlink does.** This account cannot make file symlinks, so the two file-link tests skip here (they print `skipped`). The directory junction test runs everywhere and covers the same containment rule for a folder. **Electron:** `fs.symlink` on Windows needs Developer Mode or admin for files; use `fs.realpath` in the code under test, and a junction (`fs.symlink(target, path, 'junction')`) in the tests, so the check runs on a normal account.
- **The name mapping loses information.** `-` and `_` both become `_`, so `mod_ab_c_d_e` is ambiguous. The fix is in the table, not the name: build the table, find the clashes, drop both sides. Do not parse the name back into an id (you cannot). **Electron:** the same table, built in the main process when the mod list changes; never a `split('_')`.
- **Read the file size before the file.** A 64 KB cap is only a cap if the read stops at it: read `take(limit + 1)` and refuse when the length is over. A metadata check alone can race a growing file. **Electron:** `fs.promises.open` plus a bounded `read`, and a length check on the bytes read.
- **A flat list of checks is easier to keep honest than a schema library.** Each refusal names its field in the same words across the Rust and the Electron port, so the test vectors in `mods.md` can be pasted into both suites. No schema crate was needed; the check order is written once in `manifest::parse`.
- **Constant-time compare needs equal lengths first.** `crypto.timingSafeEqual` throws on unequal lengths, so compare `Buffer.byteLength` first and return false, as `constant_time_eq` does. Scan every live token; do not stop at the first match.
- **The page must never hold the token.** In Electron the bridge is a preload `contextBridge` object with `on`, `call` and `respond` functions that send over IPC; the token stays in the main process, and the page only ever sees a call id. Do not expose the token as a property of `window.flare`.
- **A CSP meta can trip a naive scan.** The sample page's `http-equiv` attribute contains the substring `http`, so the network check looks for `http://`, `https://` and `//cdn` instead. The same check belongs in the Electron port's test.

### Stage 8, library review (Opus), 2026-10-22

- **A folder mapping is a file server with the junctions left in.** A virtual host that maps the mod folder serves every file under it, and follows a junction inside it to the user's own folders. The manifest's check ran once, at the scan, so the check did not cover the page's later requests either. **Electron:** do not use `protocol.registerFileProtocol` or a static root for a mod. Use `protocol.handle` and answer each request with the `serve` rule: `fs.realpath` of the joined path, `startsWith` of the canonical folder, a file check, and a header CSP. Test it with a junction (`fs.symlink(target, path, 'junction')`) out of the folder and a file swapped for one after the scan, both on the request. See `mods.md`, "Serving rule".
- **A CSP in the page is the page author's.** A meta CSP only says what the page's author wants. The host's policy is the response header; the navigation lock (`will-navigate`, `setWindowOpenHandler` deny) is what stops a link from carrying the content out. **Electron:** `session.webRequest.onHeadersReceived` for the header on the mod's partition, and `will-navigate` / `setWindowOpenHandler` on its `webContents`.
- **A limit that the agent sees is a limit on the name the agent sees.** The MCP name is `mcp__flare__<name>` and the model API caps it at 64 characters; the 85-character names our own limits allowed could make a strict client reject the whole tool list (not tested against a real client yet). The cap is in the table, not the manifest, so the message names the mod and the tool. **Electron:** the same 52-character cap on `mod_<id>_<tool>`, in the table builder.
- **Default-allow in a helper is a grant.** `mint` copied whatever permissions it was given, so `eval` could reach a token through a later path. The fix is at the mint and in the grant builder, and a test that passes for the right reason (a grant WITH browser, refused `browser.eval`). **Electron:** the same two checks in `tokens.mint` and the grant builder, and a test that a mod's token cannot call `browser.eval` even when the caller passes `eval`.
- **A store that cannot read its file must not overwrite it.** The first version treated a non-JSON file as an empty store and the next save wrote over it, so the user's switches were lost. The fix: three outcomes (damaged, newer, entries). Damaged is moved to `.bad` first; newer is read-only. **Electron:** `fs.rename(file, file + '.bad')` on a parse failure, then start empty; `read_only` for a newer version.
- **Count only what you mean to count.** A junk folder sorted before a real mod used up the 64-folder cap, and the entry bound dropped entries silently. The cap now counts legal ids only, and an entry past the bound is an error. **Electron:** the same two rules in the scanner; the entry bound is a parameter so the test does not make 4096 folders.
- **A bridge call that says `undefined` must not look like a failure.** `respond(callId)` with no result sent neither field, and `respond(callId, null)` sent `null` with no error. The wire form is fixed in `mods.md`: a string error is `{call_id, error}`, everything else is `{call_id, result}` with `null` for nothing. **Electron:** the preload's `respond` maps `undefined` to `null` before the IPC send, and `tools.respond` in the main process treats a present `null` as a success.
- **The bridge is bound to a tab, not to a name.** A mod's calls carry no tab, so a call cannot aim at another tab's panel; the host binds the mod to the tab whose panel shows it. **Electron:** inject the preload into the mod's `WebContentsView` only, and bind the mod id to that view's tab at `show`.
- **BLOCKER (2026-10-22): the full workspace nextest run takes many hours on this PC.** The `flare` crate's suite is 5743 tests; the run got through 1127 with 0 failures and was stopped by hand, because the rate was about 10 tests a minute (render scenes take 5 to 50 seconds each, and two builders share the machine). The review fix was verified by `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and the `flare-mods` suite (86 of 86). Nothing outside `flare-mods` depends on it. **Electron:** no equivalent; keep render tests few and cheap, and split the suite by group so a library change can run its own crate only. The full run still has to finish green before the stage PR is merged.

## Stage 8, the app part (mods), 2026-10-22

- **A page served for a host that does not exist works, even inside the sealed browser.** The mini-apps' browser has a proxy that is not there and resolves no name, and a `WebResourceRequested` answer for `https://flare-mod-notes.local/` still loads (measured on WebView2 155.0.4283.62, the e2e `modsSample`). The handler runs before the network stack, so nothing needs a resolver. **Electron:** `protocol.handle` on the mod's session answers the same way; give the session `setProxy({mode: 'direct'})` or nothing, and refuse every other request in `webRequest.onBeforeRequest`, as for the mini-apps.
- **A deferral is how a request waits for a worker.** `args.GetDeferral()` holds the browser's request; the UI thread must still be the one that sets the response and completes it (COM objects belong to the UI apartment), so the worker pushes the bytes into a queue, wakes the UI thread (`Pump`) and the UI thread builds the response from a memory stream. A view closed meanwhile answers its held requests 503 and completes them, or the browser keeps waiting. **Electron:** the handler is async already; await the `fs` read and return a `Response`.
- **`default-src 'none'` is also `connect-src 'none'`.** The contract's header policy stops a page from fetching its own address, which the first scenario assumed it could. A page loads scripts, styles and images from its folder and keeps data in `localStorage`. The page's own meta CSP is applied as well as the header, so the page gets the intersection (the sample's meta leaves it no images). **Electron:** `session.webRequest.onHeadersReceived` adds the header; the same two consequences; test them with an `Image`, not a `fetch`.
- **A named profile persists; a private one does not.** The sample keeps its notes in `localStorage`. The mini-apps' profile is private (nothing kept), so a mod's profile is a different, named, non-private profile inside the same sealed browser, and the notes survive a restart (the e2e: the third note after a restart is `count 3`). **Electron:** `session.fromPartition('persist:flare-mod-<id>')`, never `persist:flare-web` and never the mini-apps' in-memory `flare-app`.
- **Only a mod's view gets web messages, and only from its own address.** `SetIsWebMessageEnabled(false)` stays for every other view; the message handler checks the message's source against the mod's host and drops a message over 4 MB before it is copied. **Electron:** `ipcMain.on` checks `event.senderFrame.url` against the mod's origin and that `event.sender` is the mod's `webContents`; a `contextBridge` object (`on`, `respond`, `call`) is the bridge, frozen.
- **The page never needs the token, so there is none to leak.** The host looks up the mod's live grant for every call (`grant_of_mod`), so turning a mod off needs no message to the page: its next call is `unauthorized`. The first design handed the page a token; the test that a disabled mod's page is refused at once made the lookup the only sensible design. **Electron:** the main process keeps `Map<modId, token>`; the preload sends `{method, params}` and the main process adds the grant.
- **One code path for the pipe and the page.** The page's call goes through the control channel's `Dispatcher::call` on a worker with `Grant::Mod`; the grant table is the only place a permission is decided. The test for "a mod without browser is refused browser.*" checks the page path AND `Grant::check` for every `browser.*` method. **Electron:** call the same dispatcher object from the IPC handler; do not re-implement permission checks in the handler.
- **Do not make the UI thread wait for the dispatcher.** The dispatcher's calls wait for the UI thread (`ask`); a page's call is therefore started on a worker and answered by a `UiMsg`, never run inline. The call, the answer and the post to the page are three steps on three threads, and the tests play all three (`AppDispatcher` over the fixture's queue, a thread per call, `pump_until`). **Electron:** the main process is one thread, so run the dispatcher call as a promise and post the answer on resolve.
- **A tool call to a tab that is behind needs the page made hidden, and the event queued.** `ensure_hidden` makes the view; the page is not ready for a second or two; the `tool` event is held until the browser reports the page loaded (`Loaded` ok) and sent in order. A page that reloads (a link inside the mod) is not ready again until it loads. **Electron:** create the `WebContentsView` without adding it to the window, `loadURL`, and flush the queue on `did-finish-load`.
- **Every way a call can lose its page must fail the call.** The panel closed or replaced, the browser's process ending, the mod turned off, a rescan that loses the mod, Flare closing: each is a test (`closing_the_panel_the_page_dying_or_the_mod_going_off_fails_the_waiting_call`). The sync that notices a replaced panel runs with every window sync, because several paths replace a panel. **Electron:** the same list; wire it from `render-process-gone`, the panel store's change event and `before-quit`.
- **Two connections for `flare-mcp`.** A watcher cannot share the call connection (a call blocks it), so the server opens a second, says hello as the same tab, and turns `mods.changed` into `notifications/tools/list_changed`. Locking stdout for the whole run would deadlock the watcher's write, so the handle is not locked and each message is one `writeln!`. Tested with the real binary and a real pipe (`a_mods_tools_are_listed_called_and_announced_when_they_change`). **Electron:** the same MCP server binary is reused; nothing to port.
- **The rail reference test was pinned to Prism Terminal's six tabs.** A seventh page moves Diagnostics down one item; the test now says why (DIFF-25). A page that is not in the closed row tables needs the "every row of every page is in the tables" tests to see an empty page when no port is wired, or they list the Mods rows as unknown. **Electron:** an extra page is data; keep the same empty-without-a-port rule.
- **Interning ids is the price of `&'static str` row ids.** The layout takes `&'static str`; a mod's id is run-time data, so each distinct row id is leaked once (bounded by 64 mods per scan and a few rows each). **Electron:** not a problem; ids are strings.
- **A leftover test helper process blocks the next build.** `fake-parakeet-cli.exe` from a dictation test run in the shared target folder was still alive (its parent gone) and cargo could not replace the file ("Access is denied"). Killed by process id; it is a stand-in the build made. A runner that leaves helpers behind is its own bug, not fixed here.
- **E2E facts learned:** with Settings in front no shell tab is "active", so a scenario that needs the shell tab's secret takes the first shell tab; a relaunch restores the earlier tab AND opens the launch folder as a new one, so after a restart the active tab is the one to use; `Ctrl+Tab` leaves Settings.

### Review fixes (Stage 8)

- **A permission that opens a panel is a permission to reach the network, if the panel kinds are not narrowed.** `panel` allowed `panel.open` of any kind, including `web` (the user's shared profile and the network), and `view.html` took `allow_network` from the caller. Both are narrowed for a mod in the dispatcher (so the page and the pipe agree). **Electron:** in the single permission check, allow a mod `panel.open` only for its own `mod:<id>` and refuse `allow_network` for a mod grant; test with the user's network switch ON.
- **A page whose process ended must be replaceable.** Keep the tab bound to the mod while its panel still names it, mark the page not ready and drop queued messages; the next call replaces the view. **Electron:** on `render-process-gone` keep the panel record, destroy the view, and create it again on the next tool call.
- **The bridge's call ids restart in every document.** A reply for call 1 of the old page must not resolve call 1 of the new one. Give each load a generation, carry it with the worker's reply, drop a mismatch, and fail calls already handed to the old document. **Electron:** a counter bumped on `did-start-navigation` (main frame), captured when the call starts.
- **Text from a mod is untrusted all the way to the terminal.** Strip escape sequences (CSI, OSC, DCS and the like), control characters and invisible or direction characters from a tool result, not only from names. **Electron:** the same filter in the MCP server.
- **The event watcher costs a connection.** Start it only when the first `mods.tools` answered, and give up after ten failed connects (the environment names the old Flare process). **Electron:** nothing to port if the MCP server stays the same binary.
- **One deadline for `mods.call`.** The wait for the UI thread (8 s) and the page's wait share the channel's 40 s minus a 2 s margin, so the answer is always Flare's own.

### By hand, not machine-tested (Stage 8)

- A mod whose folder is a junction to somewhere else inside the mods folder, on the installed build (the unit tests cover the junction; the e2e does not make one).
- The look of the Mods page in every theme, and with a mod whose description is far longer than a row (the elision tooltip).
- A real Claude Code session listing and calling a mod's tool, and whether it re-lists on `notifications/tools/list_changed` (no real client was used; the real `flare-mcp.exe` was driven over stdio and a real pipe).
- A mod page with video, audio or a very large file near the 16 MB cap; fonts from the folder.
- Two tabs showing the same mod at once (the second tab's panel is a separate view in the same profile; calls go to the tab of the caller).
- A strict MCP client that rejects names near 64 characters (the 52-character cap is from the documented limit, not from a client).

- 2026-10-22, stage 7: a chip that trusts "something listens on that port" is trusting the whole machine. The TCP listener table names the owning pid, and the tab's shell pid plus a Toolhelp32 parent walk names the tree it may believe. The Electron app never had a chip like this, so there is no Electron note; the rule is Flare's own.

- 2026-10-22, stage 8 review: a consent is a fact about a moment, so store what was agreed (the permission list), not only that something was agreed (a boolean), and compare on every load. The Electron app had no mods; the rule is Flare's own.
- 2026-10-22, stage 7 review: a boolean probe hides the case that matters. "Is something listening?" folded "a stranger took the port" into "nobody", so the grace meant for a restarting server also protected a hijack. Return what you saw (nobody, ours, someone else) and let each caller decide; and an unreadable source is a fourth answer, never an empty list. A click on something that was true a moment ago re-proves it first. **Electron:** the same shape applies to a link that opens a dev server: check the listener owner at the moment of the click, not when the badge was drawn.
- 2026-10-22, stage 8 review: an asynchronous message must name its sender, not just the slot it came through. A page message was tied to a mod by the TAB it arrived on, so one queued before the tab switched to another mod ran with the new mod's grant. Carry the origin (here the view's host, which names the mod) with the message and compare it with what the slot holds when the message is HANDLED, not when it was sent. **Electron:** `ipcMain` handlers should compare `event.senderFrame.url` and `webContents.id` with the mod the window shows now, and never trust "the active tab" as the sender.
- 2026-10-22, stage 8 review: a shared list needs the same consent as a private one. Listing every tab title to a mod approved as "Show its panel" widened what the words said. Give a mod only its own tab, and when a wider list is wanted give it its own permission and its own words in Settings.

## Round 9: menu words, Show in side panel, wrapping, the splitter, 2026-10-22

- **What the splitter did wrong (found by dragging it the way a hand does).** Headless (`splitter_tests`, through the real controller) and end to end (`panelSplitter`, release exe, GPU renderer, real pointer events and window pictures) checked limits, edge, pointer following, grid hold, one fit on the release, content on every frame, the gap and the pointer leaving the window, at 100 and 225 percent over a picture and text. All of those held. Two things did not. (1) **A drag could outlive its slot.** The splitter's touch area is hidden with the slot; when the slot went away mid-drag (Settings or another page brought to the front by key, a window too small to hold a slot) nobody heard the release, `panel-splitting` stayed true and the slot came back drawn at the stale dragged width while the grid kept the settled one: a gap or an overlap, and the share never stored. Fixed with `changed panel-shown => panel-splitting = false` in the window file; held by `the_slot_going_away_during_a_drag_lets_go_of_the_grab` (fails without it). Electron: end the drag on `pointercancel`, on `blur` and when the panel unmounts, not only on `pointerup`; use `setPointerCapture` and release it in all three. (2) **Text looked cut off during a drag.** The rows are wrapped once when the width settles (rule 217), so while the splitter is held a narrowing slot used to clip rows that were wrapped for the old width at the right edge, which matches the owner's "disappears". Review (2026-10-22) said this should not be called by design: the text view now keeps the width its rows need while the button is down (`wrapped-cols` on the view; `content-width` is at least the gutter plus those columns), so a narrowing drag gives a temporary sideways overflow and nothing is cut; the release re-wraps once and the width and scroll settle (held by `narrowing_the_slot_mid_drag_cuts_no_wrapped_text_it_overflows_and_the_release_settles`). **The owner's reported glitch ("bug out when growing or shrinking") was NOT reproduced**; the bugs found are real but may not be it. Ask the owner for a short screen recording of the glitch before calling the splitter item closed.
- **A re-wrap must keep the reader's place (review, 2026-10-22).** Swapping the row model left the Flickable's offset as a row offset, so after a width change the view showed another part of the file, could sit past the end, and unwrapped text scrolled right stayed shifted after Wrap was turned on. Now Rust reads the line at the top before the swap (`top_line`), puts it back after (`restore_top` through the view's `scroll-to-row`), the view clamps its offset when the row count or the wrapped columns change, and wrapping on resets the sideways offset. Held by `a_rewrap_keeps_the_line_the_reader_was_on_at_the_top` and `turning_wrap_on_brings_a_sideways_scrolled_view_back_to_the_left_edge`. Electron: save the first visible line number before re-rendering the rows and `scrollTop` to its row after.
- **A drag can outlive a tab switch too.** `changed panel-shown` only fires when the slot goes away; a chord to another tab that also has a panel keeps it shown, so the grab carried on over the other tab's slot and the release stored the share on the wrong tab. The panel sync now lets go when the front tab changes (`switching_tabs_by_key_during_a_drag_lets_go_and_stores_nothing_on_the_other_tab`).
- **Count columns by display width.** `wrap_line` counted every char as one column, so rows of CJK or emoji drew twice as wide as the slot and were cut with no way to scroll to them. It uses `unicode-width` now (wide = 2); a_long_cjk_line_wraps_by_display_width_and_draws_nothing_past_the_right_edge. The widest-line measure for the unwrapped sideways scroll still counts chars.
- **Pointer leaving the window ends the drag in Slint.** A `PointerExited` while a button is held reaches the touch area as a cancel and the existing handler stores the share dragged so far and fits once (`leaving_the_window_during_a_drag_lets_go_of_the_splitter`, and the e2e `Leave`). The test harness needed `Harness::leave`.
- **The fixture binds no `grid-resized`.** Counting grid fits in a headless controller test means installing a counter on `on_grid_resized` yourself; the fake pty's resize count stays 0 because the window glue, not `AppCtl::new`, wires the grid to the terminal. The e2e reads `grid_fits` from `panel.state` (`AppCtl::on_grid_dims` counts).
- **A pixel probe needs enough samples.** The e2e's first "content is on the frame" check sampled 160 points across the text body and failed at 225 percent and at the widest slot because glyphs cover a few percent of the area; 1152 points in the body's left part were stable. Sample densely where the ink is, never across a wide empty area.
- **Wrapping a virtual list: one row one line tall, wrap in Rust, let the view report the columns.** The view exposes `wrap-cols` (width less gutter and margin over the monospace cell it measures itself) so Rust never guesses at fonts or scales; the gutter's width depends on the line count, so the count is handed over BEFORE the columns are read. Strings are shared (a line that fits is the same `SharedString`), so the wrapped model costs about one pointer per row. Electron: a hidden monospace span for the cell width, the same order (count, then columns), and a single `ResizeObserver` callback that re-wraps only when the column count changed and not while the splitter is down.
- **Keep the old rows on show while a bigger wrap is made.** A re-read of the file on show keeps the old content until the new answer (stage 1 rule); a wrap follows the same rule, and an answer carries the request number so a superseded one is dropped.
- **A menu group is a flag on the row.** The menu draws a hairline between every row already; a group needed a wider gap, so the row has a `group` flag and the list stays flat (no separator rows to skip in picks and indexes). Electron: a separator item before the first Surfaces row.
- **An explicit ask is not a click.** "Open previews in Flare" decides what a plain click does; the menu's "Show in side panel" and the agent's `view.file` ignore it. The result message carries `asked` so the one place that checks the setting can tell them apart.
- **Removing a feature means reading what else it carried.** The board's `board.rs` also held `one_line`, the text cleaner the notifications use, and the two test files held the Preview chip's and the notifications' tests: they were moved, not deleted (`notify::one_line`, `ui_tests/chip.rs`, `agent_flows/stage7_flows.rs`); the question-notification e2e read the board's rows to know an agent was working and now reads `agent.state`.
- **Line endings, again.** The working tree is CRLF; edit scripts that read with Python's default newline mode and write back are fine, but a Rust string literal written by a script that contains `

` must be written as the two characters backslash and r, not an escape that becomes a real newline.

### By hand, not machine-tested (Round 9)

- The look of the new menu group, the three glyphs and the wrap button in all 18 themes.
- A real mouse leaving the window and a real Alt-Tab during a drag (the e2e injects the exit event; Windows' own capture loss was not exercised).
- A 4 MB text file wrapped at a very narrow slot (the worker path is tested with a 1.3 MB file).

## Round 10: a verified review of the panel, web, apps and channel work, 2026-10-23

A review by many agents (flag, verify, then fix) of the newest Surfaces work: the side panel and its splitter, the web panel, mini-apps and mods, the conversation panels, and the control channel. Each finding below was re-read in the code by a second pass before it was fixed. The ids are the review's.

- **The splitter is the web page's neighbour, and the browser is a child window (panel-2, web-5, ui-wiring-4).** The splitter's 8 px grab zone is centred on the slot's left edge, and the browser window started one hairline right of that edge, so about 4 px of the zone lay under it. A child window takes the mouse above anything the window draws: a press there went into the page, and the cursor flickered between the splitter's and the page's. The page now starts past the zone's whole half (`web_inset_px`), held by a layout test at every scale. Not fixed: the page is still resized at every pointer step of a drag (holding it would show the page over the terminal; a still-and-settle phase like the overlays' is the real fix). **Electron:** an `iframe` or `webview` swallows pointer events as well; put a transparent shield over it while a drag runs and keep the grab zone outside it.
- **A drag must survive the pointer leaving the window (panel-1, panel-8).** winit sends `CursorLeft` when a captured move crosses the client edge. Slint turned it into an exit, the touch area's grab was cancelled and the drag stopped part way. Every native splitter keeps tracking under the OS capture, so the window's hook now holds back `CursorLeft` while a drag runs (the leave is owed once the button is up), and the left button going up ends the drag from the hook (`AppWindow.end-splitter`), because Slint drops its grab on ANY button's release. Rule 219 was rewritten: it had written the old behaviour down as intended. **Electron:** `setPointerCapture` on the splitter and end the drag on `pointerup` of the primary button only.
- **Whatever closes a tab must forget the same things (apps-1, ui-wiring-1).** `close_now` forgot the panel, the tracker, the dev chip, the notifier row and the browser; the path for a shell that exited on its own forgot only two of those, so a hidden browser (its audio, a pending download, a mod's grant) outlived its tab until quit. Both paths now call one `forget_tab_surfaces`. Two copies of a cleanup list drift; keep one.
- **A pipe that nobody reads fills (channel-1, channel-5).** The server wrote every event to every connection, and the MCP server's call connection reads only while a call waits. About 64 KB of unread events later the server's worker blocked in `WriteFile`, and a request bigger than the pipe then blocked the client: both ends waiting for good. The call connection now says `events: false` in its hello and gets none; a client also caps the events it keeps. Rule: a connection that does not read continuously must not be sent anything unasked. Not fixed: the client's write has no deadline.
- **One shared install folder is a shared fate (channel-4).** Every Flare rewrote the same `claude-plugin` copy, whose manifest now holds the absolute path of THAT Flare's `flare-mcp.exe`: the last started (a dev build, a worktree about to be removed) decided what every Flare's Claude sessions run. The copy is per exe folder now (`claude-plugin-<tag>`).
- **A cancelled navigation still completes (channel-2).** WebView2 raises `NavigationCompleted` with a failure for a navigation the host cancelled in `NavigationStarting`, with a number the page never started. Treating every completed load as the page's own made a live mod page unready and failed its calls. Only the load of a navigation the host allowed (its `Loading` came first) counts. **Electron:** `did-fail-load` with `ERR_ABORTED` after `will-navigate` was prevented is the same event.
- **A queue of work needs the caller's lifetime (channel-3).** Tool calls queued for a page that was not ready outlived their callers: after the agent's call timed out the page loaded and ran it anyway. The queue is checked against the hub's pending calls when the page becomes ready, and capped.
- **Plain typing never reaches WebView2's accelerator hook (web-1).** Only chords and non-character keys do, so the takeover from the agent was never triggered by typing a letter. The agent's timer now reads the keyboard (the mouse already had this) while the page has the keys. By hand: type a letter in the page while an agent call waits.
- **A history step inside one document raises no navigation (web-2).** `browser_navigate` back over a `pushState` or `#fragment` entry waited the full 30 s. The same-document event now answers a back, forward or reload that named no address.
- **Giving a field the keys is not enough when the page holds the OS focus (web-3).** A chord that moves the keys into a window field (find, the tab list) left the Win32 focus in the page, so the first letters went into the page. The window takes the focus back before such a chord (not for the ones that decide about the page themselves).
- **A browser needs to be told its parent moved (web-4).** `NotifyParentWindowPositionChanged` was never called, so select lists, the IME's candidates and menus kept the old screen position after a window move. It is called on every geometry change. By hand: move the window, then open a select.
- **A dead renderer is not a dead tab (web-6).** After `Gone` the view stayed, hidden behind its note, still driven by Reload, the address bar and the agent. A lost page is now let go and made again by the next address, Reload or open; a history step on it says so.
- **A failed release must still be sent (web-7).** If the release of a click or key failed the job ended without queuing it, and the renderer believed the button held.
- **A process-wide `remove_var` is a global side effect (apps-5).** The mini-app browser takes three `WEBVIEW2_*` variables out of Flare's environment for good (its walls), and every shell was spawned from that environment. Shells now get the environment as Flare was started with. The variables are not put back: the browser process may read them after the call returns.
- **Keep what the person was reading (apps-2, panel-7, ui-wiring-7, apps-3).** The tracker was dropped the moment Claude exited, which is when its changes are most wanted; it now lives while the panel is open. One reader view shared by every tab carried the scroll offset of the last file or tab; it is reset when the tab, kind, session or picked file changes. A tab that comes back finds its text at the line and its picture at the size it was left (a few tabs are remembered; the picture's pan is not).
- **Wrap once when the width settles, whatever moves it (panel-4, ui-wiring-3).** The sidebar's slide and a window edge change the width at every frame; the wrap ran at each. It waits for the slide to end, and for the width to hold still 120 ms.
- **Smaller ones.** A refused "Show in side panel" no longer replaces an open panel (panel-3); a click on a file in a window too narrow for a slot opens it in Windows instead of an invisible panel (panel-5; the "Show in side panel" row in that case still opens the panel); the widest line is counted in screen columns (panel-6); the keys return to the terminal when the tab changes to one that also has a panel (ui-wiring-2); a file dropped on the panel is not typed into the shell (ui-wiring-5); the Copied badge is centred over the terminal (ui-wiring-6); a subagent's `TaskUpdate` finds the task in the shared list (apps-4); a mod with tools and no panel is refused (channel-6); the Preview chip stays away while the panel shows any page of that server, and a click's port look no longer counts as a periodic look (channel-7, channel-8).
- **Not fixed.** channel-9 (`flare-mcp` reads stdin one message at a time, so a `notifications/cancelled` is not seen while a call runs and the Flare-side job runs on): a real fix reads stdin on its own thread, aborts the call connection on a cancel, and has Flare end the job when its connection drops; that is a protocol change across three crates and wants its own round. The page still reflows at every step of a splitter drag (see above).

### By hand, not machine-tested (Round 10)

- A real winit drag past the window edge and a release outside the window (the headless tests cover the Slint end and the hook's decision, not Windows' capture).
- A real key typed into a web page while an agent call waits (the tick reads `GetAsyncKeyState`, which the tests cannot press).
- Moving the window with a `<select>` open on the page, and the IME's candidate window after a move.
- A WebView2 renderer killed from Task Manager, then Reload and an address typed.

## Round 10 recheck

- A held-back `CursorLeft` needs its undo in `CursorEntered`, or a drag that goes out and back in still owes a fake leave at the release.
- Slint's TouchArea drops its grab on ANY button's release: the hook swallows other buttons' press and release while a splitter drag runs.
- A key that is not text (Tab, Escape, F-keys, media keys, a chord with Alt or Win) is not "the person typing at the page"; a same-document entry answers an address-less call only when it moved the page off the address the call began on.
