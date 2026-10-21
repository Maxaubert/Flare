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
- **An env var in a plugin manifest is expanded by Claude Code, not by Flare.** `${FLARE_MCP_EXE}` as the server's `command` and
  `${VAR:-}` for the secrets rely on Claude Code expanding them when it reads `plugin.json` (documented for `.mcp.json`; the
  plugin manifest uses the same server format). **Not machine-verified:** the e2e starts `flare-mcp.exe` the way Claude Code is
  documented to (stdio, the tab's environment) but cannot start Claude itself without touching the owner's Claude settings and
  plugins, which the safe rules forbid. Do this by hand once: open a Flare tab, run `claude`, `/mcp` should list `flare` as
  connected with four tools, then ask Claude to "show README.md in a panel". If the expansion does not happen, the fallback is
  to ship `.mcp.json` in the plugin folder with the same content, or to have Flare write the absolute path into the installed
  copy at install time (`install_plugin` already rewrites `hooks.json` that way).
- **Hands-on list for the owner (not machine-tested):** Claude Code lists the `flare` server in `/mcp`; the tools appear to Claude
  and `show_text` fills a panel; the agent in a tab that is behind shows its panel without the tab switching; the Velopack
  install path finds `flare-mcp.exe` beside `Flare.exe` (the portable zip is checked by `verify-pack.ps1`, an install is not run
  here).
- **A relative path is the tab's, and the e2e's front tab is not the first folder.** The scenario wrote its picture into the first
  folder and the front tab was the second (the last opened is in front). The channel was right and the test wrong; the picture is
  now written into both folders. Electron: resolve a relative `path` against the tab's cwd, not the main process's.
- **Merge leftovers hide in docs.** `docs/features.md` still held the conflict markers of the stage 2 library merge (two sections
  between `<<<<<<<` and `>>>>>>>`), found only because this stage edited the same file. Grep for `^<<<<<<<` before a PR.
