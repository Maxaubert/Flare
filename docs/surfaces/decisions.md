# Surfaces decisions

Each entry: the date, the stage, the choice, the reason. Owner decisions are marked (owner).

- 2026-10-21, plan: build in Flare first, document for Prism Terminal, port after (owner).
- 2026-10-21, plan: the agent must be able to USE surfaces, not only show them, modelled on the iOS
  simulator MCP (describe, tap, type, screenshot) (owner).
- 2026-10-21, plan: video must play at full frame rate, so web content is a real WebView2 panel, not
  frames drawn into the grid (owner: "it can't just be a frame here and there").
- 2026-10-21, plan: one panel per tab in v1; more later.

## Stage 1: panels (`surfaces-1-panels`, issues #15 and #16)

- 2026-10-21, stage 1, chords: **Ctrl+Shift+P** shows or hides the front tab's panel, **Ctrl+Shift+O** moves the
  keys between the terminal and the panel. Why: a program cannot tell Ctrl+Shift+letter from Ctrl+letter in the
  legacy key encoding, so no shell or agent loses a key it could have used; plain Ctrl+P and Ctrl+O stay the
  shell's. P is "panel"; O is "the other side". Both are always the app's (rule 7: a host chord never reaches the
  pty), so with no panel to bring back, or none open, they do nothing and the shell does not hear them (a ^P would
  walk the shell's history). A chord with Alt too (AltGr on some layouts) is the shell's.
  Both live in the one chord table (`flare-tabs` `keymap.rs`, rule 7), and the table's own tests cover them.
- 2026-10-21, stage 1, focus: a panel never takes the keys by itself (a click on a link leaves them in the
  terminal). A click in the panel, or Ctrl+Shift+O, gives it the keys; Esc, or the chord again, gives them back.
  While the panel has the keys a key that is no chord goes to Slint (the panel scrolls with the arrows, Page
  keys, Home and End; Enter or Space zooms a picture), never to the shell. Closing the panel, or leaving its tab,
  while it has the keys returns them to the terminal at once. Why: typing into the wrong place is the worst
  thing a split view can do.
- 2026-10-21, stage 1, state file: **`panels.json` beside `tabs.json`**, not inside it. Why: `tabs.json`'s key
  order is a shape other code depends on, and a panel is not a tab property that must survive a downgrade. A tab
  has no id across a restart, so an entry carries its folder and is matched to a restored tab by folder, in
  order (`match_restored`); a tab dropped from the restore only skips its entry. The file has a `version`. It is
  written the way `tabs.json` is (temp file, rename, 400 ms debounce, synchronous flush at close; rules 36 and
  41). The writer thread is made on the first save, and a session that never opens a panel writes no file.
- 2026-10-21, stage 1, what is stored: kind, target, open, width (the share of the page). A closed panel keeps
  its kind and target so the toggle chord can bring it back; it holds no picture and no rows. Unknown kinds are
  dropped on load; a kind this build cannot draw yet (web, changes, tasks, board, mod) is kept in memory and in
  the file and is not shown. A width that is not a fraction reads as "not set" and the default applies, it is never
  written back as a number (rule 40). A stored target is re-checked like a click: a local drive path, no NUL, no
  network path (it would make Windows reach out), a name of the stored kind.
- 2026-10-21, stage 1, layout numbers (all in `flare-surfaces` `layout.rs`, handed to the window once):
  default share 40 percent of the page; the slot is at least 240 css px unless the page cannot give it so much;
  the terminal always keeps 320 css px; the slot is at most 70 percent; under 120 css px of room the slot is not
  shown (the panel stays open in the model, and the terminal keeps the whole page); the splitter's hit area is 8
  css px centred on the slot's left edge; the header is 32 css px. Why 320: about 40 columns, the least a prompt
  stays readable in. The `.slint` file evaluates the same formulas in logical pixels with `Px.snap`; a test pins
  the two to each other at all eight scales, five widths and four shares, and a probe reports both.
- 2026-10-21, stage 1, one fit (rule 24): while the splitter moves, the slot follows the pointer and the grid
  keeps its width (the slot is drawn over it, opaque); the share is stored on the release and the grid fits
  once. The held-width trick of the sidebar's slide is not reused: it freezes the whole page, and here only the
  slot moves.
- 2026-10-21, stage 1, previews: a plain left click on a path link asks a worker to resolve the path again (rule
  13), and the app shows it in the panel only when the file is on the click allowlist (`may_open`) as named AND as
  the file system names it, and both names are of the same previewable kind (`classify_resolved`). So a link
  called `a.png` that points at `run.exe` is revealed, not shown. Pictures: png, jpg, jpeg, gif, bmp, webp, ico.
  Text: txt, md, markdown, rst, log, json, jsonc, yaml, yml, toml, csv, tsv, ini, cfg, conf, xml, lock, diff, patch,
  rs, ts, tsx, c, h, cpp, hpp, cc, cs, go, java, kt, swift, css, scss, sql, slint, vue, svelte. Not svg (a vector
  file can carry script and is not in the picture list), not pdf, not any script (rule 14). Folders and
  everything else keep today's behaviour. The right-click menu's Open still hands the file to Windows: that is an
  explicit ask. The setting is **Open previews in Flare** (`flare.window.openPreviews`, default on) in a new
  **Links** section of Settings > Terminal; no page held link behaviour before.
- 2026-10-21, stage 1, pictures: decoded on a worker with the `image` crate (png, jpeg, gif, bmp, webp, ico; the
  rest of its formats are off). Limits: a 64 MB file, 100 megapixels, and a side over 8192 px is scaled down
  (a GPU texture limit). The decoded bytes go into a Slint pixel buffer on the worker, so the UI thread copies
  nothing. A GIF shows its first frame. Fitted: never enlarged, kept in shape, centred; a click shows one image
  pixel per device pixel with scrolling; a click again fits it. A small picture is shown as it is and a click does
  nothing. A picture with any transparent pixel sits on a checkerboard (a tiled 16 px tile, two neutral greys).
- 2026-10-21, stage 1, text: read on a worker, first 4 MB, 100 000 lines, 2000 characters a line (a longer line is cut
  with a mark); tabs are four spaces; bytes that are not UTF-8 show as replacement marks; a NUL in the first 8 KB
  means "This file is not text." and a missing, denied or unreadable file shows its reason in the panel, never a
  blank. Monospace (Cascadia Mono), a line-number gutter, only the rows in view are built (100 000 lines draw in
  milliseconds), no wrapping, a horizontal scroll, no selection and no copy yet (v1 is a viewer). A cut file says
  so in a line under the text.
- 2026-10-21, stage 1, one panel per tab; only the front tab's is loaded. A tab behind keeps its path and reads it
  again when it comes to the front (no cache: a file may have changed, and memory stays flat). A newer click
  replaces the panel; an answer for an older one is dropped by a generation number.
- 2026-10-21, stage 1, the overlay flag: one `out` property of the window, `overlay-open`, the OR of every overlay:
  menu, tab list, dialog, update window, update notice, the Settings page in front, Settings dropdown, theme
  editor, theme question, Dictation question, colour picker, Command help, find bar, the F12 diagnostics card and a
  tab being dragged (its ghost crosses the page; added after the stage 1 review). `flare-surfaces`
  `overlay::Overlay::ALL` names them; a test opens each and checks the flag; the match in that test has no
  catch-all arm, so a new overlay cannot be added without being counted; another test reads the property's line
  from `app.slint` and counts its terms. Native panels ignore the flag (Slint orders them with the rest); stage 3's
  web panel hides while it is true. The find bar spans the terminal only, so it never overlaps the slot, but it
  counts (it is one of the overlays the task names). The Copied badge and tooltips are not counted: they are small
  and brief; stage 3 should check them against a child window.

### Stage 1 review fixes (2026-10-21)

- 2026-10-21, stage 1, rule 36: `panels.json` is flushed with the other stores at logoff, shutdown and quit. The
  window keeps a handle to the `PanelStore` and one function (`flush_stores`) writes all four, for the session-end
  closure and for the end of `run`; a test holds that a change still inside its debounce lands on the disk.
- 2026-10-21, stage 1, a click on the file already on show reads it again. A panel that never re-read would show a
  log the agent has since rewritten, and a file that first failed could never be retried. The old content stays on
  show until the new read answers (no flash of "Opening").
- 2026-10-21, stage 1, one picture decodes at a time, and a decode the user has left stops. A shared "newest
  generation" number is checked before the decode, between decode and resize, and before the answer is posted; a
  mutex lets one decode run at once. A decode at the 100 megapixel limit is about 400 MB, so several at once was
  the real danger. The decoded image is turned into RGBA by value (no clone) and dropped as soon as the pixel buffer
  has its copy.
- 2026-10-21, stage 1, every load is vetted the way a click is, including a target restored from `panels.json`: the
  worker canonicalises it, both names must be on the click allowlist and of the same kind, and the real name must be
  a local drive path. A mapped drive (which canonicalises to a network name) or a link that now points at another
  kind is refused with "not allowed". A missing file still says "The file is gone."
- 2026-10-21, stage 1, text files: a UTF-16 file with a BOM (what Windows PowerShell 5.1 writes for redirection) is
  decoded as UTF-16, a UTF-8 BOM is dropped, and only then does a NUL in the first 8 KB mean "not text". The header
  shows the file's real length, and the foot names the cap that cut it ("Only the first 4.0 MB are shown." or "Only
  the first 100000 lines are shown.").
- 2026-10-21, stage 1, the rows and the widest line are built on the worker; the UI thread wraps them in a model.
- 2026-10-21, stage 1, Ctrl+Shift+P on a panel of a kind this build cannot draw (web, changes, tasks, board, mod,
  restored from the file) does nothing and leaves the stored entry alone, so the chord never flips an invisible state.

- 2026-10-21, stage 2 library: the channel is two crates, `flare-ctl` (protocol, grants, framing, the
  connection state machine, the pipe, the client) and `flare-mcp` (a client binary). Neither depends on
  anything of the app, so the same contract ports to Prism Terminal and the MCP binary can be shared.
- 2026-10-21, stage 2 library: the connection logic (`session.rs`) is written against a `Wire` trait and
  the Windows pipe is one implementation, so every error path is tested in memory and the real pipe has
  its own round-trip tests. A port implements the same state machine over `net.Socket`.
- 2026-10-21, stage 2 library: reads are non-blocking probes plus a 1 to 10 ms sleep, never a parked
  `ReadFile`. A synchronous Windows handle serialises its calls, so a parked read would hold out every
  writer on the same handle (events, replies). Cost: an idle client wakes about 100 times a second on
  its own worker thread; the UI thread never waits (rule 34).
- 2026-10-21, stage 2 library: only the connection's own worker thread writes to its pipe. Events the
  app publishes wait in a per-connection outbox (256, drop oldest) and are written between requests and
  while idle. A client that never reads can only stall its own worker, never the app.
- 2026-10-21, stage 2 library: the dispatcher runs on a short-lived thread per request so a hung handler
  cannot hold a connection past its timeout (15 s, 60 s for `browser.*`). Only handlers past their
  timeout count as stuck (a three-state flag per call); past 32 new calls answer `unavailable`.
  (Review fix: the first version counted every in-flight call, which would have refused 33 legitimate
  concurrent `browser.wait`s.)
- 2026-10-21, stage 2 library: the grant is re-read on every request, not only at hello. This makes
  "a disabled mod's token is dead" (S5) true for open connections, and makes the user's eval switch live.
- 2026-10-21, stage 2 library: grants are a pure function (`Grant::may_call`) over an exhaustive method
  enum. Adding a method is a compile error until its namespace, timeout and grant are decided. The `ui`
  grant is in-process only and is never issued over the pipe.
- 2026-10-21, stage 2 library: `shell.input`, `settings.set` and `mods.enable` exist as reserved method
  names that every pipe grant is refused, only so the S3 refusal is a pinned test rather than an absence.
- 2026-10-21, stage 2 library: the pipe name is `flare-ctl-<FNV-1a of SID>` with an optional scope hash,
  and `FLARE_CTL_PIPE` overrides it for clients. A scope keeps an e2e run (own profile) off the owner's
  running Flare, as the single-instance pipe already does.
- 2026-10-21, stage 2 library: framing errors that mean the peer is broken (not JSON, not a request,
  over 16 MB, bad token) close that connection after one error line; mistakes a well-formed client can
  make (unknown method, forbidden, bad params, rate limit) leave it open.
- 2026-10-21, stage 2 library: `flare-mcp` is hand-rolled on `serde_json` (four JSON-RPC methods, about
  300 lines). No MCP crate: nothing to audit, no new dependency, nothing to keep in step with the spec.
  Argument mistakes are tool errors (`isError`) so the model sees them; unknown tools are JSON-RPC errors.
- 2026-10-21, stage 2 library: relative paths are made absolute in `flare-mcp` (the agent's cwd), then
  re-resolved by Flare (S2). Flare's own cwd for a tab is not the agent's.
- 2026-10-21, stage 2 library: not yet wired. flare.exe does not start the server, the shells do not get
  `FLARE_TAB_ID`/`FLARE_CTL_PIPE` from this crate, the `flare-status` plugin manifest does not list
  `flare-mcp`, and the pack script does not ship it. That is stage 2 proper, after the panels land.
- 2026-10-21, stage 2 library review fixes: the library refuses a `ui` grant from the app over the
  pipe (token: `unauthorized`, agent: `internal`) and refuses `ui`-namespace methods whatever the grant.
  The app is trusted to decide grants, but a single app-side bug must not open `shell.input` to a
  socket, so the wire layer does not rely on it.
- 2026-10-21, stage 2 library review fixes: every request takes a rate token before the app is asked
  anything (including `unknown_method`, `forbidden` and `hello` retries). Cost: `hello` spends one of
  the 200 burst tokens. Gain: a spammer cannot drive unlimited grant callbacks into the app.
- 2026-10-21, stage 2 library review fixes: stopping the server cancels blocked pipe writes
  (`CancelIoEx` from the stopping thread, repeated until the workers are out), makes waiting calls
  answer `unavailable` within 50 ms, and returns how many handler threads are still running. Rule 28:
  nothing that calls into the app may outlive teardown unnoticed; the app waits on the count inside
  its quit cap and the handlers are told (in the `Dispatcher` docs) to return quickly after stop.
- 2026-10-21, stage 2 library review fixes: queued events are written only between whole messages
  (never while half a request line is buffered), otherwise a 4 MB `view.text` request against more
  than 64 KB of queued events deadlocks both sides on full pipe buffers.
- 2026-10-21, stage 2 library review fixes: the tab of a connection is advisory and grants are not a
  sandbox against the user's own processes (the agent has Bash as that user). Methods act on
  `ctx.tab` only. Written into protocol.md section 4 so a port does not read S3 as enforced by the
  channel.
- 2026-10-21, stage 2 library review fixes: `flare-mcp` waits for a call as long as the method's server
  timeout plus 5 s; a fixed 20 s dropped the connection on a legitimate 60 s `browser.wait`.

## Stage 2: the channel inside Flare (`surfaces-2-channel`, issues #18 and #19)

- 2026-10-21, stage 2, tab identity (security review finding, fixed): a connection is a tab only by presenting the
  tab's secret. Flare makes one per tab when it starts the shell (two random UUIDs, 64 hex digits, the OS generator),
  puts it in the shell's environment as `FLARE_TAB_TOKEN` and nowhere else (not on a command line, not in a log, not in
  Sonara's post, not in a hook line). `hello` takes `tab_token` and refuses a `tab` name outright (`bad_params`, so an old
  client fails loudly instead of silently acting as nobody). Why not reuse `FLARE_TAB_ID`: that id is public by design (hook
  lines carry it, Sonara is told it, `tabs.list` hands the same string out); a secret must be a different value that is never
  shown. The session manager answers token to tab (`tab_for_ctl_token`) under its own lock, without early exit, and the
  channel asks again on every request, so a closed tab's token is dead at once and the connection falls back to no tab
  (`no_tab`). A restored tab starts a new shell and so gets a new secret each launch; nothing about a secret is stored.
- 2026-10-21, stage 2, an unknown tab token closes the connection (`unauthorized`), like an unknown mod token. A client that
  presents none still connects as the agent with no tab (it can call `tabs.list`, which is empty for it, and nothing that
  needs a tab). Why: both are the same signal ("you are not who you say"); and a tool run outside Flare must not be able to
  tell a wrong secret from no secret by timing or by a different failure.
- 2026-10-21, stage 2, `tabs.list` for the agent grant gives its own tab only. A mod holding `panel` sees every tab's id and
  title; a mod without it sees its own. Why: the agent has no use for the other tabs, and a list of titles is the one
  place the channel could leak what the user is working on.
- 2026-10-21, stage 2, the pipe is per instance: the default profile (`%APPDATA%\Flare`) uses the user's plain name; any
  other profile folder is hashed into the name (`flare-ctl-<sid hash>-<profile hash>`). Every shell is told the whole name in
  `FLARE_CTL_PIPE`, so the right Flare is found with no computation, and the e2e runs (own profile) never touch the
  installed Flare's pipe. A name already taken fails the start (first-instance flag) and Flare runs without the channel; the
  shells then get no channel variables at all, and the plugin's `mcpServers` entry degrades to "not running".
- 2026-10-21, stage 2, three variables, one function: `pty_env` (rule 12) sets `FLARE_TAB_TOKEN`, `FLARE_CTL_PIPE` and
  `FLARE_MCP_EXE` and drops an inherited copy of each, whatever its case. The channel's own data (pipe name, MCP path) is the
  `SessionOptions.ctl` of the session manager, so every shell of the instance gets it from one place.
- 2026-10-21, stage 2, the server starts at launch from `run_window` (the pipe is made on the UI thread at startup, which is the
  allowed exception: one `CreateNamedPipe` and a thread) and every connection runs on its own worker. A call that needs the
  window posts `UiMsg::Ctl` with a one-shot reply channel and waits at most 10 s, looking at the stop flag every 50 ms. The
  UI thread answers from the model at once and never waits on a client. A file the agent names is resolved on the call's own
  thread, so the UI thread is only ever given a path that passed.
- 2026-10-21, stage 2, quit: the first thing after the event loop ends is `CtlServer::stop` (flag set, waiting calls answer
  `unavailable` within 50 ms, pipe closed, workers joined, at most 2 s), before the shells are killed and the stores flushed.
  A message that reaches the controller after `end()` is dropped, which drops its reply sender, which wakes the waiting call
  with `unavailable`. Rule 28: nothing of the channel outlives the quit unnoticed (a count of still-running handlers is
  logged).
- 2026-10-21, stage 2, a view call for a tab that is not in front stores that tab's panel (the target and the title) and does
  not switch tabs; the panel shows when the tab comes to the front. Why: switching tabs is the user's act; an agent in a
  background tab finishing a task must not pull the user away from what they are typing. The user sees the tab's own signals
  (the agent mark) and finds the panel when they get there. `panel.changed` goes to that tab's connections either way.
- 2026-10-21, stage 2, an explicit ask is not a click: `view.*` ignores the "Open previews in Flare" switch (that setting is for
  clicks on paths), but applies every other check a click gets (rules 13 and 14, the same kind on both names).
- 2026-10-21, stage 2, `view.text` is shown in the same text viewer, from memory: the target is `inline:<n>` (a new number for
  every call, so the panel reads it afresh), the text is held on the controller (at most 4 MB per tab, let go when the tab
  closes), shaped on a worker like a file, never written to `panels.json` and never restored (a restored `inline:` target fails
  the stored-target check and is dropped). The title defaults to "Text". `language` is validated and ignored (no syntax
  colouring in the viewer yet). `view.file`'s `line` is validated and ignored (the viewer cannot scroll to a line yet).
- 2026-10-21, stage 2, `view.file` shows text and code only (the stage 1 list); it does not show a runnable file as text, it
  refuses it as `not_found` like any path off the allowlist. The protocol had said "as text or refuses"; refusing is the
  stricter of the two and matches rule 14.
- 2026-10-21, stage 2, `panel.open` is implemented as the refusal (`web` is `unavailable`, anything else `bad_params`) and is
  not in `capabilities` until stage 3, so a client that offers tools from the list never offers it. `browser.*`, `convo.state`
  and `tools.respond` answer `unavailable` ("not built in this version of Flare").
- 2026-10-21, stage 2, events: `panel.changed` is published for every change of a tab's panel (a channel call, a click on a path,
  the close button, the toggle chord); `tabs.changed` is not sent yet (the tab strip has no single place that says "a tab was
  retitled"; it comes with the board in stage 7).
- 2026-10-21, stage 2, MCP in the plugin (changed by the review, see "Stage 2 review fixes" below): the shipped manifest is a
  template and the install step writes the absolute path of `flare-mcp.exe` into the installed copy. First draft: the manifest
  named the server by `${FLARE_MCP_EXE}` and passed the secret in `env` with an empty default; that relied on Claude Code
  expanding variables in `plugin.json`, which nobody had verified. Why a path written in at install and not a path in the plugin
  folder: the plugin is installed as a copy under `%LOCALAPPDATA%` and refreshed at every start, but the exe lives with the app
  (Velopack moves it on every update); the running app knows where its own exe is and rewrites the copy when it changes, with no
  binary copied and no bare name for `PATH` to answer. The plugin version is 1.1.0.
- 2026-10-21, stage 2, packing: `flare-mcp.exe` is built by its own step in the three workflows that pack (release,
  installer-e2e, update-e2e), copied beside `Flare.exe` by `pack.ps1` and required in the portable zip by `verify-pack.ps1`;
  a release test reads the three files so none of them can lose it.
- 2026-10-21, stage 2, e2e: the shell's secret is read through the `ctl.info` probe (only answered under `--e2e`), which also
  reports the pipe and the MCP path; the scenario plays the shell. The first scenario also reads the three variables from
  inside a real shell and compares them. `flare-mcp.exe` is run with `CREATE_NO_WINDOW` and killed on every way out.

### Stage 2 review fixes (2026-10-21)

- Plugin manifest: `plugin.json` is a template with `@@FLARE_MCP_EXE@@`; `install_plugin_with` renders it with the absolute path of
  `flare-mcp.exe` beside the running exe, or drops the whole `mcpServers` block when the file is not there (a development build,
  a package without it). No `env` block: a stdio server inherits Claude Code's environment, and a literal `${VAR:-}` that is not
  expanded would have overwritten the real value. A manifest problem must never take down the status hooks of the same plugin.
- A tab's own panel title ends with its call: a click on a path (and a restore) clears the title and the handed-over text an agent
  gave an earlier call, so a real file is never shown under a title the agent chose.
- The shells hear about the channel only once its server runs (`SessionOptions.ctl_up`, set after a successful start). A server that
  could not start (the name is taken, no SID) means no shell gets a secret or a pipe name, so nothing is ever sent to whoever owns
  the name, and the agent is told "not running" and not "unauthorized".
- A call has one deadline, shared by every step (the UI is asked for the tab's folder only for a relative path; an absolute path never
  asks), and the request carries it: the UI thread answers `timeout` and does nothing for a request that arrives after it. A call that
  was told it timed out never changes the window later. The wait on the UI is 10 s, inside the 15 s of the channel.
- Squatter: the shell is given the app's process id (`FLARE_CTL_PID`, rule 12 drops an inherited copy) and `flare-mcp` connects only
  to a pipe served by that process. We chose the pid over comparing integrity levels because it is exact and cheap (one call the
  client already makes), where an integrity comparison still trusts any same-integrity process. `flare-mcp` also cleans a refusal's
  words (one line, no control characters, at most 200 characters) before the model reads them. Without a pid `flare-mcp` does not
  connect at all (it only works in a Flare tab).
- Relative paths from the agent: kept absolute in `flare-mcp`, against the folder the agent was started in (the process's cwd), and
  the tool text says so. The channel's own rule (a relative path is the tab's cwd) stays for other clients. Why not send the path
  as it is: Claude's idea of "the current folder" is where it started, which is not the tab's tracked cwd once the shell has moved
  on; the model reasons in its own folder.
- Rule 28 (no detached thread outlives main) has one recorded exception: the control channel's accept thread, a call that timed out
  and a handler still running when the 2 s stop wait ends may outlive the stop. They touch only the `post` function (which fails once
  the loop has ended), the stop flag and their own pipe. A call that arrives after the stop is never posted (pinned by
  `an_unanswered_call_times_out_and_a_stop_ends_the_wait_within_a_slice`).

## Stage 3: the web panel (`surfaces-3-web`, issues #20 and #21)

- 2026-10-21, stage 3, engine: WebView2 through `webview2-com` 0.39.1 (released 2026-03-11), whose `-sys` crate links
  Microsoft's STATIC loader, so there is no `WebView2Loader.dll` to ship and none is imported (the exe is checked for the name).
  The `windows` crate stays at the workspace's `=0.62.2`. The loader is BSD 3-Clause and has its own entry under "Bundled
  binaries" in `about.hbs`. Why not `wry`: it brings its own event loop and window model; Flare already has winit through Slint
  and only needs a child controller. Flare never installs or updates the runtime (safe rule and owner's machine): it asks
  `GetAvailableCoreWebView2BrowserVersionString` at every open and shows a message when that fails.
- 2026-10-21, stage 3, threading: everything runs on the UI thread, which winit has already made a single-threaded COM apartment
  with the message loop WebView2 needs (`CoInitializeEx` is called anyway and its answer ignored). Nothing waits for the browser:
  the environment and each controller are created with completion handlers; a handler only parks its result in an inbox and
  posts `WebEvent::Pump`; `pump()` takes the inbox in. No WebView2 callback touches the host's state, so a callback that fires
  while a call into the browser is still on the stack (COM can re-enter in an STA) cannot re-enter the host. Every other
  callback only posts a `WebEvent` (`UiMsg::Web`); a message is `Debug` by kind, never by address or title.
- 2026-10-21, stage 3, rule 34: the one slow question, whether the runtime is installed (a registry and disk read), is asked on a
  worker thread when a browser is first needed and answered as a `WebEvent::Runtime`; the environment's folder is made by WebView2
  itself, not by a `create_dir_all` on the UI thread. A scan like the one in `flare-surfaces` runs over `flare-web/src`.
- 2026-10-21, stage 3, life: one controller per tab with an OPEN web panel, made when its tab is in front and the slot has room
  (a restored or channel-opened panel of a tab behind waits for its tab). A tab that goes behind keeps its controller, hidden
  (the page stays loaded, its video keeps its place); a closed panel, a closed tab or `Ctrl+Shift+P` frees the controller
  (`Close()`), and the browser environment goes with the last one, so with no web panel open there is no browser object, no
  process and no handler. Reopening makes a new controller at the last address; back history and scroll are not kept (the
  owner's spec: closing frees it). The e2e watches the browser's own process end.
- 2026-10-21, stage 3, address of a restored panel: `panels.json` keeps the page's CURRENT address (updated on every
  `SourceChanged` and `NavigationCompleted`, in normal form), and a stored address is accepted on load only if
  `normalize_url(target) == target`, so a hand-edited file cannot make a web panel open `file:` or a login URL.
- 2026-10-21, stage 3, profile: ONE WebView2 user data folder per Flare profile, shared by all tabs (so a login persists and one
  browser process group serves every controller): `<profile>\WebView2` when the run was given `--user-data-dir` (every e2e
  world), else `%LOCALAPPDATA%\Flare\WebView2`. Never the user's Edge profile (S4). Mini-apps and mods get their own folders in
  stages 5 and 8; the agent's page is the user's page in v1.
- 2026-10-21, stage 3, where the page sits: the controller's rectangle is the `web-box` Slint lays out inside the panel body
  (the body less the slot's one-pixel hairline, so the splitter's visible line stays drawn), converted by rounding both
  edges (`phys_rect`), and it follows on `changed` callbacks of the box's four numbers and of the scale, so a window resize, a
  sidebar slide, a splitter move at every pointer step, a DPI change and a monitor move all move it. Why Slint's box and not
  the layout maths alone: it is exactly what is drawn, including mid-drag. The maths (`web_rect`) is held equal to it by a
  unit test at four scales, three widths and three shares and by the e2e at 100 and 225 percent, where the controller's own
  bounds, the child window's rectangle, Slint's box and the maths must all agree.
- 2026-10-21, stage 3, head of the panel: a web panel's head is two rows, the 32 px header (title, close) and a 32 px bar
  (back, forward, reload or stop while loading, the address). The title is the page's; a line that says why something did not
  happen ("Not opened here: ...", an address the field refused) goes where the size goes for a picture. Back and forward are
  dimmed until the page has that history, as the browser reports it.
- 2026-10-21, stage 3, airspace: `web_show(front, shown, overlay-open)` and the phase machine `step` in `flare_surfaces::web`.
  An overlay opening asks for a still (`CapturePreview`, PNG into a memory stream) and only when it arrives hides the window;
  the PNG is decoded on a worker and drawn in the web box. The overlay closing shows the window and lets the picture go. A
  capture that comes back after the overlay closed (or for another tab) is dropped by its token. A capture that fails still
  hides the window (an empty panel is better than a menu under a browser). A window made while an overlay is open starts
  hidden with no picture. A tab behind and a slot with no room are `Hidden`, not `Still`.
- 2026-10-21, stage 3, keys (record of the question the plan left open): while the page has the keyboard WebView2's
  `AcceleratorKeyPressed` is asked, before the page sees a key, whether the app wants it, and answers at once. The app takes
  the chord table (`flare_tabs::keymap`) less two chords whose key a page has its own use for: Ctrl+F (the page's find;
  Ctrl+Shift+F is still the terminal's) and the agent guard's Ctrl+D and Ctrl+C. So F1, F11, Ctrl+Tab, Ctrl+1..9, Ctrl+,,
  Ctrl+Shift+K and B, Ctrl+T, Ctrl+W, Alt+Space, Ctrl+Shift+P and Ctrl+Shift+O are the app's even inside a page. **Escape: the
  first is the page's, a second within 600 ms (with no other key between) gives the keys back to the terminal.** A page that
  uses Escape keeps it; the cost is two presses to leave. Ctrl+Shift+O moves the keys both ways as for any panel. Enter in the
  address field sends the page there and gives the page the keys, as a browser does. A press on Flare's own window takes the
  Windows focus back from the page (`SetFocus`), or the terminal would type into the page; and the page taking the keyboard
  is not the window losing it (the foreground window is still Flare's), so "focus lost" is ignored while it is.
- 2026-10-21, stage 3, what a page may not do: every permission request (camera, microphone, location, notifications,
  clipboard, sensors) is denied without asking; `window.open` and `target=_blank` go to the same panel (no second window,
  ever); `window.close()` and full screen of the whole window do nothing; the page-to-app message channel and host objects are
  off; a navigation to anything but http, https and about:blank is cancelled whoever started it (a link, a redirect, a
  script). Downloads: see the review fixes below (S6; WebView2's own behaviour is NOT a prompt). Script dialogs (`alert`)
  keep WebView2's own.
- 2026-10-21, stage 3, the channel: `browser.open`, `browser.navigate` and `panel.open` (kind web) are real and listed in
  `capabilities`; the rest of `browser.*` stays `unavailable` until stage 4. They wait for the page: the call is bound to the
  first navigation the browser starts after it (rebinding on a redirect) and answered on that navigation's completion, up to
  30 s (`BROWSER_WAIT`, inside the channel's 60 s). First version answered on the first completion of ANY navigation; the first
  e2e run showed a `back` issued while the previous page was still loading being answered by that page's completion and the
  next steps racing the real navigation, so the browser's navigation id now decides. A tab behind answers at once.
- 2026-10-21, stage 3, MCP: one new tool, `open_page {url}`. The title it reports comes from a web page, so it is cleaned like a
  refusal's words. The `browser_*` tools are stage 4's.
- 2026-10-21, stage 3, no runtime: the panel keeps its place and shows the reason and the download address
  (`https://go.microsoft.com/fwlink/p/?LinkId=2124703`) with a button that opens that page in the user's own browser; the
  channel answers `unavailable` with the link. Nothing is remembered: the next open asks again, so a runtime installed meanwhile
  is found without a restart (a first draft kept the failure and would have asked the user to restart).
- 2026-10-21, stage 3, tests that need a browser: two seams read only under `--e2e`: `WEB_ROOT` (a folder served as
  `https://flare-test.local/` by a WebView2 virtual host mapping, so no page is a `file:` address and no network is touched) and
  `WEBVIEW_RUNTIME=missing` (the runtime check fails). The probe `web.state` reports the controller's own numbers; a script in the front
  page is a test-hook REQUEST (`WebScript`), not a probe, because a probe only looks (a scan holds that), and its answer is the probe
  `web.script.result`. The pages tell the scenario what they did through their own title (`geo:err1`, `pop:blocked`, the video's
  clock as JSON), so a scenario waits with `until` and never loops.
- 2026-10-21, stage 3, video: nothing special is done for it. The e2e plays a 6 second clip made with `ffmpeg` and reads the
  clock and the frame counter: 1.00 times real time and 30.6 frames per second in the parked window. YouTube and other
  streaming sites need the network and are by hand (below); they need no code of ours.

### Stage 3 review fixes (2026-10-21)

- 2026-10-21, stage 3, downloads (S6, corrects the first reading above): WebView2 does NOT prompt by default; it saves straight
  into the Downloads folder and shows a flyout. So Flare registers `ICoreWebView2_4::add_DownloadStarting`, takes a deferral and
  posts `WebEvent::Download {id, file, host}`; the window asks in its own dialog (`download_ask.rs`: "Save this file?", the file
  name and the host, Save or Cancel, the names cut at 80 characters and escaped so a file name cannot add markup). A yes leaves
  the browser's proposed path (a unique name in Downloads) and completes the deferral; anything else cancels. Nothing is saved
  when the question is dropped for any reason: the panel closed (the host cancels that tab's downloads, and the window forgets
  its questions), the window ends (`close_all`, and `Drop` of the host), the deferral cannot be taken, or the runtime cannot
  hold a download (a view that cannot register is a failed view, never a silent saver). A second download queues behind the
  first. The close question has the dialog first; a waiting download is asked after it. Electron must ask the same thing
  (`docs/surfaces/README.md`). The default flyout stays on for a download that was allowed (progress, open file).
- 2026-10-21, stage 3, rule 28 for the web views: `WebCtl::close_all` runs in `finish()` before the window hides. It answers every
  pending `browser.*` call `unavailable` ("Flare is closing"), closes each controller, drops the environment and forbids a later
  open (a refresh after the end of life would otherwise make a view again). `WebHost` also cancels downloads when dropped.
- 2026-10-21, stage 3, airspace has a timeout: `Act::Capture` starts the timer `web.capture` (`timer_registry`, 250 ms,
  `CAPTURE_TIMEOUT_MS`); when it fires it posts a `Still` with no picture for the same token, which `captured` turns into
  "hidden, no still". A page that is busy or hung can therefore stay over a menu or dialog for 250 ms at most. The real answer, if
  it comes later, finds a different phase or token and is dropped.
- 2026-10-21, stage 3, focus when a page is closed: the host window keeps its handle from every sync. Closing the tab, the panel,
  or sending the page away while it has the keyboard calls `SetFocus(host)` first and takes the window off the screen before
  `Close()`; Windows would otherwise leave the focus nowhere, the keys would arrive as system keys and the terminal would type
  nothing until a click. The same runs for a page that failed (its window is taken off the screen, so the note is visible and the
  crash face is not on top) and when the front tab changes while the page has the keys.
- 2026-10-21, stage 3, process failures: `failure_of_kind` (`flare_surfaces::web`) decides by `COREWEBVIEW2_PROCESS_FAILED_KIND`.
  Browser process exited: the host closes every controller, drops the environment (`Env::None`, so the next open starts fresh),
  forgets the views and says `Gone` for each tab. Render process exited: `Gone` for that tab. GPU, utility, a frame's renderer,
  the sandbox helper, plug-ins and an unresponsive renderer (WebView2 asks the person itself and the page can recover): only
  logged. The first reading ("every kind is fatal") put the "could not be shown" note up for a GPU restart. A failed page's panel
  stays until it is closed; closing it lets the next open make a new browser.
- 2026-10-21, stage 3, blur while the page holds the keyboard: `focus_loss(has_backend, page_has_focus, window_is_foreground)`
  decides. The page has the keys: skip. The window is still the foreground window but the page is not known to have the keys
  yet: that is either the keyboard moving into the page or a switch to another app that Windows has not finished, so the
  decision is asked again one turn later (`UiMsg::FocusLostCheck`: blur unless the window is foreground or the page has the keys
  by then). Deciding on the foreground window at the instant of the kill-focus lost real blurs (dictation, the Ctrl+Tab walk,
  tips and menus). By hand only, see `lessons.md`.
- 2026-10-21, stage 3, Escape: WebView2 reports autorepeats as key downs, and the default repeat delay (about 500 ms) is shorter
  than the 600 ms window, so one Escape held a moment counted twice. The host reads `PhysicalKeyStatus().WasKeyDown` into
  `KeyPress.repeat`; the filter ignores a repeated Escape (it neither counts nor resets).
- 2026-10-21, stage 3, waiting for a page: `Pending` records the address the call sent the page to. The first `Loading` that
  counts is the one whose address matches it (`url_matches`: parsed, fragment ignored), so a `Loading` left in the queue from an
  earlier navigation cannot become the call's. A same-document change (a `#fragment`, `history.pushState`) raises no navigation;
  the host reports `SourceChanged` with `IsNewDocument` false as `WebEvent::SameDocument`, and a call whose address equals it
  (fragment included) is answered then.
- 2026-10-21, stage 3, rule 34: `CreateCoreWebView2EnvironmentWithOptions` and `CreateCoreWebView2Controller` run on the UI
  thread and do synchronous work inside (registry reads, loading the runtime's library, making the data folder, launching the
  browser process). They cannot move to a worker: the objects are thread-affine to the UI thread's apartment. They are marked
  `// sync-ok:` and are in the `sync_calls` scan's list, so a third such call cannot slip in. Measured on the gate machine
  (2026-10-21, release build, the e2e's fresh profile, process at idle priority): the first `browser.open` of a run, which is the
  cold browser start plus the page load and the whole round trip through the channel, answers in 2.3 s (2309 ms at 100 percent,
  2297 ms at 225); the window stays responsive meanwhile because the runtime question is on a worker and the environment and
  controller completions come back through the inbox.
- 2026-10-21, stage 3, video (closes the plan item "a local mp4 plays"): the first leg used a VP8 webm because it needs no
  codec the Windows edition might lack (N editions without the Media Feature Pack have no H.264). `webVideo` now also plays an H.264
  mp4 made with `libx264` and holds the same clock and frame-rate checks, so both are verified on the gate machine.
- 2026-10-21, stage 3, e2e honesty: "the browser's process ends" is its own check (the pid must be known) and also runs after
  the window quits; `window.open` is tested with the popup blocker off (the run sets `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`
  for the app it starts, nothing else) so the handler runs, and the check counts visible top-level windows of Flare's and the
  browser's processes before and after.

## Stage 8: mods, the library part (`surfaces-8-mods`, issue #31)

- 2026-10-22, stage 8, no new dependency: `crates/flare-mods` uses `flare-ctl` (the grant types), `flare-settings` (the atomic write and the startup read), `serde_json`, `log` and `windows-sys` (already in the workspace). Tokens come from `BCryptGenRandom` with the system-preferred generator, the call `flare-dictation` already makes. Not `uuid` v4, which carries 122 random bits, and not a new `getrandom` entry.
- 2026-10-22, stage 8, the manifest is strict: unknown keys are refused at every level (a typo is never a missing permission), every refusal is one sentence that names the field, and the list of checks is `docs/surfaces/mods.md` in the same order.
- 2026-10-22, stage 8, permissions: a manifest may ask for `panel`, `view`, `browser`, `convo` and `tools`. `eval` is refused as "the user's own switch" (S4). The `panel` permission and a `panel` entry go together, and so do `tools` and the `tools` permission. A mod with no panel and no permission is valid.
- 2026-10-22, stage 8, ids: 3 to 40 characters of `a-z`, `0-9`, `-`, equal to the folder name, and not a Windows device name (a folder `con` cannot exist). Folder names are compared as written, so the lower-case rule keeps two mods from sharing one folder on a case-insensitive disk.
- 2026-10-22, stage 8, caps (chosen here, not in the spec): 32 tools per mod, 40 characters per tool name, 500 characters per description, 80 per name, 200 per entry path, 8 KB per `input_schema`, 64 KB per `mod.json`. Discovery reads at most 64 folders (sorted by name, the rest are one error) and at most 4096 directory entries.
- 2026-10-22, stage 8, paths are checked twice: the syntax when the manifest is read (no `:`, no leading `/` or `\`, no `..`, no empty or `.` part, `.html` or `.htm`), then the canonical location on disk (`canonicalize`, so a junction or a file link that points out is refused). The file-link tests skip on an account without symlink rights.
- 2026-10-22, stage 8, semver is parsed by hand (no crate): numbers without leading zeros, optional `-pre` and `+build` identifiers.
- 2026-10-22, stage 8, the enabled store is `%APPDATA%\Flare\mods.json`, `{"version":1,"mods":{"id":true}}`. A mod with no entry is off. `set` changes memory only after the write lands, and puts the old value back when the write fails. A file with a newer version is read as empty AND left alone (read-only for the session), so a later Flare's file is not overwritten. Tokens are never in this file.
- 2026-10-22, stage 8, tokens: 256 bits, 64 hex digits, held in memory. Enabling a mod that has a token replaces it (the old one is dead). The lookup compares every live token in constant time with no early exit. `Tokens` has a `Debug` that prints mod ids only, so a token cannot reach a log by accident.
- 2026-10-22, stage 8, tool names: `mod_<id with - as _>_<tool>`. The mapping is not reversible, so two mods can clash (`ab-c`/`d_e` and `ab-c-d`/`e`). BOTH sides are left out of the agent's tools and reported. The table is built from the enabled mods only, so a disabled mod's tools do not exist for the agent.
- 2026-10-22, stage 8, the bridge: `window.flare.on('tool', fn)`, `window.flare.respond(callId, result, error?)` and `window.flare.call(method, params)`. The page never holds the token; the host adds it. `respond` takes the error as a third argument rather than a shape inside `result`, so a mod's real data can never look like a failure. The `tools.respond` params are fixed in `protocol.md`; the app does not answer it until the stage 8 UI lands.
- 2026-10-22, stage 8, scope: this is the library only. The Settings > Mods page, the bridge injection, the `tool` events, the `tools.respond` answer and the `flare-mcp` routing are the next part of stage 8. `crates/flare` is not edited here.

### Stage 8, library review fixes (Opus review, 2026-10-22)

- 2026-10-22, stage 8, SERVING: a mod's page is served per request, never by a folder mapping. A WebView2 virtual-host mapping of the mod folder follows the junctions inside it, so `data -> C:\Users\<me>` would expose the user's files to the page. `paths::serve(folder, url_path)` checks syntax, canonicalises the joined path, requires it under the canonical folder and a file, and runs on every request (a file swapped for a junction after the scan is refused the next time). The header CSP and the navigation lock are the app's, written in `mods.md` ("Serving rule"). Why not a meta CSP: the page's author writes it.
- 2026-10-22, stage 8, TAB SCOPE: a mod is bound to the tab whose panel shows it; no mod call names a tab; its `view.*` and `browser.*` calls are `unavailable` while no panel is shown. Why: the agent's "its own tab" rule (stage 2) applies to a mod too, so a mod cannot read another tab's page.
- 2026-10-22, stage 8, MOD BROWSER PROFILE: a mod's `browser.*` drives its own WebView2 profile folder under `%LOCALAPPDATA%\Flare\mods\<id>\profile`, never the shared web profile that holds the user's logged-in tabs. Why: the shared profile holds the user's sessions, and a mod with `browser` must not reach them. This is a v1 choice; the owner can revisit it before the app part.
- 2026-10-22, stage 8, AGENT NAME LENGTH: the agent sees `mcp__flare__<name>`, and the model API and MCP clients cap a tool name at 64 characters, so `<name>` is at most 52 (`tools::MAX_AGENT_NAME`). A longer name is left out and reported as `TooLong`, like a clash. Why: a client that rejects one name can fail the whole flare tool list, the built-in `show_*` and `browser_*` tools included.
- 2026-10-22, stage 8, EVAL CANNOT BE MINTED: `Tokens::mint` refuses `eval` and `mod_grant` drops it. Why: S4 says `eval` is the user's switch, and a later host path (a settings toggle, a helper) must not be able to grant it by passing a list.
- 2026-10-22, stage 8, DAMAGED VERSUS NEWER: `mods.json` has three outcomes. Damaged (not JSON, no `version`, no `mods` object) is moved to `mods.json.bad` and the switches start empty, so a user's switches are not silently overwritten. A `version` present and not exactly 1 (a newer Flare, `"2"`, `2.0`) is read as empty and left alone, read-only. Why: the old code overwrote a damaged file on the next save.
- 2026-10-22, stage 8, MANIFEST: `input_schema.type` is REQUIRED and must be `"object"` (MCP needs an object schema). A missing nested field names its place (`panel.entry is missing`, `tools[0].input_schema is missing`); the top level keeps `mod.json is missing "key"`. Hidden Unicode format characters (Cf: zero-width, bidi overrides, BOM, soft hyphen) are refused in every text field a person reads.
- 2026-10-22, stage 8, DISCOVERY: only folder names that are legal mod ids count toward the 64-folder cap; each other folder is still reported by name. A directory entry past 4096 is one error ("the rest were not looked at") instead of a silent cut.
- 2026-10-22, stage 8, `respond` WIRE FORM: a string error sends `{call_id, error}`; otherwise `{call_id, result}` with `result` null when the page gave nothing. A present `null` result is a success. Written in `mods.md` and `protocol.md`.
- 2026-10-22, stage 8, NOT BUILT, and the findings the review did not close: the app part (Settings > Mods, the bridge, the `tool` events, the `tools.respond` answer, `flare-mcp` routing, the sample-mod e2e, the serving of pages through the rule above). The issue stays open with that list.
