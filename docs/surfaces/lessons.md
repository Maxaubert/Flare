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
  the owner's settings):** open a Flare tab, run `claude`, `/mcp` must list `flare` connected with four tools, then ask Claude to
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
