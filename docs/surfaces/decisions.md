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

## Stage 6 library: the conversation reader (`flare-convo`, issues #25, #26, #27), 2026-10-21

The pure-Rust part only: no panel, no channel method, no change to `crates/flare`. The format it relies on is in `convo.md`.

- **A new crate, `flare-convo`, depends on `flare-agent` for the slug.** `flare_agent::resume::encode_cwd` already encodes a folder the way Claude Code does, and the resume lookup uses it. The convo crate reuses it and does not copy it. The slug belongs with the transcript store it names, and `flare-agent` already owns that store's lookup.
- **Session ids are checked before they become a path.** `valid_session_id` allows ASCII letters, digits, `-` and `_`, up to 64 bytes. A `..` or a separator is refused with `None`, so the lookup never leaves `<home>\.claude\projects`. The fallback search uses the same check.
- **`transcript_path` takes the user's profile folder, not the `.claude` folder.** It takes the same `home` that `resume::claude_sessions` takes, so a caller passes one value to both.
- **The tail is a plain struct, and the worker is a helper around it.** `Tail::poll` does one bounded read and returns a `Batch`. The worker (`subscribe`) loops it and sleeps 500 ms in 25 ms slices. The worker exists only while a `Subscription` does; dropping one sets a flag. Why not a permanent thread per tab: most tabs never open the panel, and a worker that polls for nothing costs a wake-up every half second per tab.
- **Limits: 4 MB per read, 8 MB per line, a skipped line is counted.** The 4 MB read keeps one poll's memory and its time bounded when a session file has grown by hundreds of megabytes. The 8 MB line cap drops a runaway line (a pasted image as base64, for one) while it is being read, not after. Both are constants (`MAX_READ`, `MAX_LINE`).
- **Replacement is detected by the file index and volume serial, not the creation time.** The first version used the creation time (`MetadataExt::creation_time`). A test that deleted and recreated a file at once read no restart, because NTFS tunnelling keeps the old creation time. `GetFileInformationByHandle` on the open handle gives the index; a reused record gets a new sequence number, so the index changes. `windows-sys` (already in the workspace) is the one new use of an existing crate; no new crate was added.
- **Lenient fold: a bad line counts, an unknown thing is ignored.** A line that is not a JSON object increments `bad_lines` and is skipped. An unknown `type`, an unknown tool or a field of the wrong shape only means that item is absent. Nothing panics on a transcript a newer Claude Code wrote.
- **TodoWrite replaces the list; the task tools add to it.** TodoWrite sends the whole list each time, so each call replaces the main list. TaskCreate and TaskUpdate (a newer family, shape guessed) add to and change the same list, and a later TodoWrite replaces them too. It is one list, because the panel shows one list.
- **A subagent's task list never replaces the main one.** Sidechain TodoWrite and task-tool calls go to `side_tasks`. Edits and files from a subagent stay in their lists, tagged `sidechain`, so the Changes panel can show or hide them.
- **A failed edit stays in the list, marked `Failed`.** The agent tried it; hiding it would make the Changes panel say something other than what the agent did. The panel can dim it.
- **Write and NotebookEdit diff as all added.** The transcript does not hold the text a Write replaced, so the diff says so by showing the whole new file and nothing removed.
- **File names come from the assistant's words only: absolute Windows paths with an extension.** The user's own words are not scanned. A folder path with no extension is not a file.
- **The diff is a hand-written LCS, not a crate.** Line LCS on a few thousand lines is small code and no dependency. The output is capped at 2000 lines; a pair too big for the table falls back to all removed then all added. `similar` was not added.
- **The tests use a synthetic fixture, and no real content.** The shapes come from the author's knowledge of Claude Code's transcript format. No real transcript was opened in this stage, so every shape marked known in `convo.md` is pinned by the synthetic fixture, not checked against a live file. The first step of the next stage is a structure-only check (field names, no text) against a real session.
