# Remote sessions: implementation plan (2026-11-12)

Spec: `docs/superpowers/specs/2026-11-12-remote-sessions-design.md`.
Status: approved 2026-11-12; building. PR 1 built on `feat/remote-1` (2026-11-12, not merged); PR 2 built on `feat/remote-2` (2026-11-12, stacked on PR 1, not merged); PR 3 next.

Seven PRs, in order. Each one ships alone and passes its own tests. Only PR 7 needs an approved mockup.
Rule numbers below are "R-new-N". Number them after the last rule in `docs/regression-rules.md` when you write them.

## Shared decisions (apply to every PR)

- **Transport.** `ssh <host> <flare> --serve-remote`. The ssh command line never carries a token. Host and user go as separate argv items, never through a shell string. No listener.
- **Serve process is a bridge.** `flare --serve-remote` is a headless process. It does not own tabs. The tabs live in the running GUI Flare. The bridge ends its own `remote.hello`, then relays to the local Flare control channel (`connect_default`). It never takes the single-instance lock, never opens a window, and is parsed before `e2e::init` and the instance code in `main.rs`.
- **New grant, not Ui and not Agent.** Add `Grant::Remote` and `Namespace::Remote` in `flare-ctl`. It is issued only to a connection that came in on the serve stdio path. It is never issued on the app pipe, never from `FLARE_TAB_TOKEN`, never from "no token". It gets no `shell.input`, `settings.set`, `mods.enable`, browser or eval methods.
- **Output is polled, not pushed.** Events are lossy (256 queued, oldest dropped). So the viewer calls `remote.output {tab, since}` in short polls. Each chunk has a host sequence number. One request at a time stays true. No long polls. A second connection is allowed for polling.
- **Keystrokes are batched.** One `remote.input` call carries a typed intent list (key, text, paste). Resizes are coalesced. This keeps the 100/s rate limit safe.
- **Agent state: option A plus a small status call.** The viewer feeds the replayed bytes to its own emulator and hub. A small `remote.status` carries state word, session id, folder, title, working_for and question text. Replay never raises Finished or Question marks (rules 35, 38).
- **Seam in the tab stack.** A `RemotePty` implements `PtyProcess` and a routing spawner picks it per `SpawnSpec.remote`. This reuses the batcher, writer queue, resize retry, emulator, scrollback and OSC parsing. Remote tab = `TabKind::Shell` plus a `remote: Option<MachineTag>` field on `Tab`. This avoids editing the ten `kind == Shell` filters. Those sites are audited in PR 3.
- **Conversation transport.** The host sends `Entry` values (`said::read_last` backfill plus a `Follower`), with a host sequence number. No host paths cross the link.
- **All blocking work on worker threads** (rule 34). ssh spawn, transcript reads, listings, fetches.
- **Tests never take the screen.** Every e2e is offscreen on the hidden desktop. Test-only env vars go through `E2eFlag`.

---

## PR 1: Stdio wire, Remote grant, `remote.hello` (flare-ctl only)

**Goal.** The protocol base. A stdio `Wire`, a Remote grant, and a version handshake. No app change yet.

**Files.**
- `crates/flare-ctl/src/stdio.rs` (new), `lib.rs`
- `crates/flare-ctl/src/grant.rs`, `method.rs`, `proto.rs`, `session.rs`, `framing.rs`
- `docs/surfaces/protocol.md` (grant table, new methods, version s8), `docs/regression-rules.md`

**New types and methods.**
- `pub struct StdioWire` implements `Wire`. A reader thread does blocking `read` into a `Mutex<VecDeque<u8>>` plus a flag. `poll_read` drains it (`Some(0)` empty, `None` after EOF and drained). `write_all` locks, writes, flushes. Stdout carries only protocol lines. Logs go to stderr.
- `StdioWire::from_child(stdin: ChildStdin, stdout: ChildStdout) -> StdioWire` for the viewer side, and `StdioWire::from_process() -> StdioWire` for serve mode. On Windows, set binary mode on both handles.
- Make `testwire::{pair, MemWire}` available to other crates behind a `testing` cargo feature.
- `Grant::Remote` with arms in `kind()`, `permission_names()`, `may_act_on_any_tab()` (false), `may_enter()` (only `Namespace::Remote`), `may_call()`.
- `Namespace::Remote` (`ALL` 18 to 19). `Permission::Remote` is not askable by a mod.json.
- `Method::RemoteHello` (`remote.hello`). Params `{remote_protocol: u32, machine: String, flare_version: String}`. Result is the same shape plus `capabilities`. `pub const REMOTE_PROTOCOL: u32 = 1;` in `proto.rs`.
- `Dispatcher::remote_grant(&self, origin: ConnOrigin) -> Option<Grant>` (default `None`). `ServerConfig.origin: ConnOrigin::{Pipe, Stdio}`. Only `Stdio` can yield `Grant::Remote`. `pipe_grant` keeps refusing it.
- Version refusal. On mismatch, answer `unsupported_protocol` with one line: `remote protocol N here, M there`. Both versions named.
- `Method::timeout()` gets a `Namespace::Remote` arm. Default 15 s. Long ones (`remote.open`, `remote.ls`, `remote.fetch`) get a new `REMOTE_CALL` constant (60 s). Add the prefix to `names_follow_their_namespace`.
- Hello timeout. Add `ServerConfig.hello_timeout` override for Stdio origin (30 s). The viewer starts hello only after ssh is up.

**Tests.**
- Unit: `StdioWire` over OS pipes (poll is non-blocking, EOF gives `None`, partial lines, CRLF stripped, 16 MB cap).
- Unit: grant tests extended. `agent_enters_no_plugin_namespace`, `nothing_over_the_pipe_reaches_the_ui_namespace`, `permission_names_agree`, `Method::ALL` count, new test `remote_grant_never_on_the_pipe`, `remote_grant_cannot_reach_ui_or_browser`.
- Unit over `MemWire`: hello ok, version mismatch line names both versions, a banner line before the first JSON line is skipped (resync on first valid `remote.hello`).
- `tests/pipe.rs`: a live pipe client cannot get `Grant::Remote`.
- ui_tests: none.
- e2e: none yet.

**Rules to add.**
- R-new-1: Remote grant exists only on the serve stdio path and never reaches the Ui namespace (extends S3, rules 186 to 188).
- R-new-2: stdout of the serve process carries only protocol lines.
- R-new-3: remote protocol mismatch refuses with one line naming both versions.

**Built (2026-11-12, `feat/remote-1`).** Rules R-new-1 to R-new-3 are rules 276 to 278. Where the build differs from
the lines above:
- The origin lives on `Shared`, not on `ServerConfig`: `Shared::new` (every pipe server) is always `ConnOrigin::Pipe`
  and only `Shared::new_stdio` (with `serve_stdio`) is `Stdio`, so no pipe server can be configured into the remote
  grant. `ServerConfig::stdio()` holds the stdio limits (one client, the 30 s hello) and `ServerConfig.machine` the name
  `remote.hello` answers.
- There is no `Permission::Remote`: `Namespace::Remote.permission()` is `None`, so no `mod.json` can ask for it, and
  the remote grant's `permissions` is `["remote"]`.
- On the pipe `remote.hello` is `forbidden` before and after `hello`; on a remote link `hello` is `forbidden`. A token or
  agent grant that resolves to `remote` is refused like `ui`.
- `StdioWire::from_process()` and `from_child()` return `io::Result` (the reader thread can fail to start, and a GUI
  process may have no stdio handles). "Binary mode" is a duplicate of the std handle read and written as a plain file:
  Rust never puts a handle in CRT text mode, so there is no `_setmode` to call.
- Banner resync is on both ends: the serve side skips lines that are no request until `remote.hello` is answered, and
  `Client::remote_hello` skips up to 200 lines that are no message.
- `REMOTE_CALL` (60 s) is defined; `remote.hello` takes the default 15 s and the long methods take `REMOTE_CALL` when
  PR 2 adds them.

**Depends on.** Nothing. **UI.** No.

---

## PR 2: Serve mode and host bridge (`--serve-remote`, `remote.open/ls/attach/input/resize/output`)

**Goal.** A host Flare that can run, list, attach to, type into and stream a Claude tab for a Remote peer. Testable with a plain loopback client.

**Files.**
- `crates/flare/src/config.rs` (`--serve-remote`), `main.rs` (early branch), `crates/flare/src/serve_remote.rs` (new, the bridge)
- `crates/flare/src/ctl_app.rs` (`CtlOp::Remote*`, `AppDispatcher::call` arms, `capabilities`), `appctl.rs` (UI-thread handlers)
- `crates/flare-term/src/session.rs` (output tee and ring), `crates/flare-term/src/handle.rs` (typed remote intents)
- `crates/flare/src/tab_host.rs` (size owner)
- `docs/surfaces/protocol.md`, `docs/regression-rules.md`, `PRIVACY.md` (only in PR 4, where ssh is first used)

**New types and methods.**
- `Args.serve_remote: bool`. In `main.rs` branch before `e2e::init`. Attach or redirect stdio on Windows (`attach_parent_console` is not enough in a release build; inherit the handles from sshd).
- `fn run_serve_remote(timeout) -> ExitCode`. It builds a `StdioWire`, runs `serve_connection` with a `BridgeDispatcher`. The dispatcher holds a `flare_ctl::Client` to the local pipe (`connect_default`). It does not run a `SessionManager`.
- Methods, all in `Namespace::Remote`:
  - `remote.open {folder, resume?: String} -> {tab, session?}`. Folder is a host path, validated on the host. Resume id is checked with `valid_resume`.
  - `remote.ls {path?} -> {entries: [{name, dir: bool}], truncated: bool}`. Bounded (500 entries), host paths only.
  - `remote.tabs {} -> {tabs: [{tab, cwd, title, state, session?}]}`.
  - `remote.attach {tab} -> {cols, rows, seq}` and `remote.detach {tab}`. Detach never kills the session.
  - `remote.input {tab, intents: [Intent]}`. `enum Intent { Key{...}, Text(String), Paste(String) }`. Paste goes through `sanitize_paste`. Limits: 64 KB per call, control chars refused in `Text`.
  - `remote.resize {tab, cols, rows}`. Clamped. Sets the size owner to remote.
  - `remote.output {tab, since: u64, max: u32} -> {from: u64, to: u64, data: base64, gap: bool}`. If `since` is older than the ring, `gap: true` and the viewer asks for a snapshot (`remote.snapshot {tab}`) instead.
  - `remote.close {tab}`. Goes through `request_close` (rule 48).
- `SessionManager::subscribe_tail(id) -> TailHandle`. A per-session ring of the last N MiB of output bytes with a running `u64` sequence. It is fed from `Session::deliver` after the existing sink, so batch order is kept and the UI sink is never blocked by a slow remote. Cap 4 MiB.
- `SessionManager::write_remote(id, intents)`. It calls the same typed path as `plugin_input`. It also marks the tab as typed-by-remote and clears the Question state.
- `TabHost::size_owner: HashMap<id, Owner::{Local, Remote}>`. `refit_all`, `set_dims` and `activate` skip remote-owned tabs until the next local key. The last typist wins.
- `remote.snapshot` sends a rendered screen plus scrollback tail, for reconnects that fell out of the ring.

**Tests.**
- Unit: intent validation (empty, over 64 KB, control chars, rate), `sanitize_paste` on paste, ring wrap and `gap`, sequence monotonic, size-owner switching, detach does not kill.
- Unit over `MemWire` plus `FakeSpawner` (`flare-term/src/fake.rs`): attach, type, output, resize, close.
- `tests/pipe.rs`-style: a live `AppDispatcher` behind a loopback stdio pair answers `remote.*` and refuses everything else.
- Headless e2e scenario `remote_loopback` (first one): two profiles (`--user-data-dir`), host Flare offscreen, a test client speaks over an in-process stdio pipe to `--serve-remote`. Steps: open a tab in a temp folder with a fake shell, attach, send `echo hi`, read the output, resize, detach, reattach with `since`, confirm no duplicate bytes. Waits for events, no fixed counts (rule 259). Runs on the hidden desktop.

**Rules to add.**
- R-new-4: `remote.input` is a typed intent path. It is a deliberate, documented exception to rule 2, called from one place. Paste is sanitized (rule 1). Held by `remote_input_*` tests.
- R-new-5: a remote detach or a dead link never kills a host session (rules 27, 28 stay for quit).
- R-new-6: the output ring has a cap and never blocks the UI sink.
- R-new-7: remote resize sets the size owner. Window refit skips remote-owned tabs until a local key.
- R-new-8: serve mode takes no single-instance lock and opens no window.

**Built (2026-11-12, `feat/remote-2`, stacked on `feat/remote-1`).** Rules R-new-4 to R-new-8 are rules 279 to 283; the
bridge grant below is rule 284. Where the build differs from the lines above:
- **The bridge needs a pipe grant.** Rule 276 keeps `remote.*` off the pipe for every pipe grant, but the serve process
  relays to the running Flare over its pipe. So the pipe has one new grant, `Grant::Bridge` (kind `bridge`), issued
  only for the **bridge key**: a fresh 256-bit secret the running Flare writes into its profile at every start
  (`remote-bridge.key`, by a rename, owner-only), compared in constant time, never for "no token". It enters `hello` and
  `remote.*` but `remote.hello`. The remote grant itself still never rides the pipe. A same-user process that can read
  the key can also run `flare --serve-remote`, so the key adds no reach (written into rule 284).
- **The pipe is the profile's**, not `connect_default`: the serve process takes `--user-data-dir` and finds the pipe and
  the key the way a launch does (`ctl_app::pipe_for`), so an e2e profile is served too. It opens its pipe connection at
  the first relayed call and again after one broke; a call that changes nothing is sent once more, typing, opening and
  closing never.
- **No `SessionManager::write_remote`.** Typing goes `remote.input` -> `CtlOp::RemoteInput` -> `TermHandle::remote_input`
  on the UI thread, the same `TermHandle` paths a local key, text and paste take (so a question is answered, rule 279).
  "Typed by remote" is the handle's `last_typist` (`Typist::Remote`); the hub sees the same `Typed` and `AnswerKey`
  events as for a local key.
- **The tail is per session, made at the first attach and kept for its life** (`SessionManager::subscribe_tail`, 4 MiB),
  fed after the sink under the delivery lock. The followed tabs live in `remote_host::RemoteHost` (handle and tail per
  tab, followers per connection), so `remote.output` and `remote.snapshot` run on the channel's worker without the UI
  thread. `remote.output` needs an attach (`no_tab` otherwise); a detach or a dropped link (`client_gone`) only removes
  that follower, and the last one gone gives the size back (`CtlOp::RemoteRelease`).
- **Size owner** is `TabHost::remote_sized` (`SizeOwner`): `remote_resize`, `take_size_back`; a local key is seen in
  `drain_events` (a `Typed` event with `last_typist() == Local`).
- **`remote.snapshot`** is plain text (no colours): clear, home, up to 1000 lines joined by CR LF, read under the
  delivery lock with the tail's `seq` (`with_tail_still`), so it meets `remote.output {since: seq}` exactly.
- `remote.close` answers `{closed}`: false while the host's window asks (rule 48). `remote.tabs` answers `{tab, cwd,
  title, state, session}` from `tab_facts`. `remote.input` is limited to 50 calls a second a tab.
- **Tests.** Unit: `flare_ctl::remote` (intents), `grant` and `session` (bridge), `flare_term::tail`, `handle`
  (`remote_input`, tail and snapshot), `tab_host` (size owner), `ctl_app` (`remote_*`), `remote_host`, `serve_remote`
  (the bridge in front of a live `AppDispatcher` over in-memory pipes, and over OS pipes), `config`, `main` (branch
  order). Process: `crates/flare/tests/serve_remote.rs` runs the built binary with piped stdio (no Flare running: the
  hello answers, the relay is `unavailable`, exit 0 when the link closes; an `--instance-probe` beside it is the first
  instance). E2E: `remoteLoopback` (one host profile; the test client plays the viewer, so the second profile waits for
  PR 3).
**Depends on.** PR 1. **UI.** No.

---

## PR 3: Viewer tab (`RemotePty`, remote tab field, reconnect)

**Goal.** A tab on the viewing Flare that draws a host session like a local PTY, types into it, resizes it and reconnects without duplicate lines.

**Files.**
- `crates/flare-term/src/pty.rs` (`SpawnSpec.remote`), new `crates/flare-term/src/remote_pty.rs`, `session.rs` (route by spec, skip adoption and prewarm for remote)
- `crates/flare/src/remote_link.rs` (new: ssh child, `Client`, poll loop, reconnect)
- `crates/flare-tabs/src/tabs.rs`, `restore.rs` (`Tab.remote`, `SavedTab`)
- `crates/flare/src/tab_host.rs`, `appctl.rs` (audit of every `kind == Shell` site: cwd tracking, drop, Ask agent, link open, dev-server chips)
- `crates/flare/src/agent_hub.rs`, `crates/flare-agent/src/arm.rs` (remote tab: poll blind)
- docs and rules

**New types and methods.**
- `pub struct MachineTag { id: String, name: String, kind: Os::{Mac, Pc} }`. Persisted in `tabs.json` as `remote: {machine, host_tab, last_seq}`. Unknown values read as local (rule 40). A restored remote tab reconnects and is not new work (rule 38).
- `pub struct RemotePty` implements `PtyProcess`. `take_reader` yields bytes pulled by the poll loop. `take_writer` batches intents into `remote.input`. `resize` coalesces into `remote.resize`. `kill` sends `remote.close` (closing ends the session everywhere). `pid()` is a fake. `foreground_pid()` is `None`.
- `pub struct RoutingSpawner` implements `PtySpawner`. It delegates on `SpawnSpec.remote`.
- `pub trait Link: Send` with `fn call(&self, Method, Value, Duration) -> Result<Value, LinkError>`. Real impl `SshLink` spawns `ssh -o BatchMode=yes <host> <flare> --serve-remote` through `StdioWire::from_child`. A fake `LoopLink` joins `MemWire` or an in-process serve for tests. The remote command string is built per host OS so the Windows `cmd` login shell works (quote the exe path, no `&&`).
- Reconnect loop. Back-off 1 s to 30 s. It calls `remote.attach` then `remote.output {since: last_seq}`. On `gap` it calls `remote.snapshot`. The replay is marked "replayed": it must not raise agent marks.
- Agent hub entry `AgentHub::on_remote_tab(tab)` marks the tab poll blind and turns off the question screen-read for it. The question text arrives in `remote.status` (PR 4).

**Tests.**
- Unit: `RemotePty` against `FakeState` style tests (feed, written, size). Reconnect with `since` gives no duplicate bytes. Gap gives a snapshot. Back-off. A dead link does not block `kill` or quit (rule 28, 3 s).
- Unit: `tabs.json` round trip with and without `remote`. Unknown machine kind is not rewritten.
- Unit: hub timeline test (in the `Rig`): a replay does not raise Finished or Question.
- ui_tests: a remote tab with a fake link shows output and takes typing (headless scene).
- e2e `remote_viewer`: Flare A (viewer) and Flare B (host), joined by a stdio pipe. A opens a tab on B, types, sees output, A is killed and restarted, the tab restores and reattaches with no duplicate lines. Both offscreen.

**Rules to add.**
- R-new-9: a remote tab is `TabKind::Shell` with a `remote` field. It owns a `RemotePty`, never a local pty. Amends rule 45.
- R-new-10: a viewer disconnect or restart never ends the host session. Restore reattaches and is not new work (rules 37, 38).
- R-new-11: replayed output never raises agent marks.
- R-new-12: no tokens or env cross the link. The host builds its own env with `pty_env` (rule 12).
- R-new-13: remote spawn never uses a warm spare and never mints a `FLARE_TAB_TOKEN`.

**Depends on.** PR 2. **UI.** Almost none (no new screens). The Mac/PC tag is deferred to PR 7.

---

## PR 4: Status, conversation and keep-awake

**Goal.** The host forwards state, the viewer shows marks, badge and notices once, the conversation view works, and the host stays awake while sessions work.

**Files.**
- `crates/flare/src/ctl_app.rs`, `appctl.rs` (`remote.status`, `remote.convo`)
- `crates/flare/src/remote_convo.rs` (new, host side), `crates/flare-convo/src/said.rs` (host sequence on entries)
- `crates/flare/src/remote_status.rs` (new, viewer side), `agent_hub.rs`
- `crates/flare-win/src/keep_awake.rs` (new, Windows), `crates/flare-win/src/macos/keep_awake.rs` (new)
- `crates/flare/src/notify` glue, `PRIVACY.md`, `docs/features.md`

**New types and methods.**
- `remote.status {} -> {tabs: [{tab, state: "working|question|done|failed|idle", kind, session?, folder, title, working_for?, question_text?}], seq}`. Built from `AgentHub::send_tabs`, `failed_kind`, `question_text`. Short poll, 1 to 2 s, plus on demand.
- `remote.convo {tab, since: u64} -> {entries: [{seq, role, text, tool, at}], reset: bool}`. Host: `said::read_last` (200 entries, 4 MiB) for backfill, a `Follower` for deltas, `claude_dir(CLAUDE_CONFIG_DIR, home)` resolved on the host. Entries get a host `seq`. The viewer dedupes by `seq`.
- Viewer: synthetic "remote override" entry into the Indicator for question and working. Done and failed are held and cleared by the viewer's own looking state (rule 61), not copied.
- One notifier rule. A question toast fires on the viewer for a remote tab. The host suppresses its own toast for tabs that have a live remote attach (rule 206).
- `pub trait KeepAwake { fn hold(&self) -> Hold; }`. Windows: one long-lived thread calls `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`. macOS: `IOPMAssertionCreateWithName` (`PreventUserIdleSystemSleep`), released on every exit path. Driven by `hub.working_count()` of remote-attached tabs. A fake for tests.
- PRIVACY.md lists the SSH request (rule 113).

**Tests.**
- Unit: status mapping per state, `seq` dedupe, reconnect with `since` gives no duplicate entries, `reset` on truncated transcript, Windows and Mac transcript slugs resolved only on the host.
- Unit: keep-awake fake: held while working count above zero, released at zero, released on drop and on link loss.
- Unit: notifier: question toast once per episode, host suppressed while attached.
- ui_tests: none new (no new screens).
- e2e `remote_status`: B runs a fake agent that prints OSC 777 `working` then `done`. A shows the mark, the badge count and one toast. A reconnect does not re-raise `done`.

**Rules to add.**
- R-new-14: remote status is host-derived words only. Done and failed hold on the viewer's looking state.
- R-new-15: one toast per question across both machines.
- R-new-16: the host holds sleep only while a remote-attached session works. Release on every exit path.
- R-new-17: conversation entries cross the link as `Entry` values with a sequence. No host paths.

**Depends on.** PR 3. **UI.** No new screens. The fitted conversation view is deferred to PR 7 (new UI).

---

## PR 5: Starting sessions (command, agent tool, hidden start, resume list)

**Goal.** Start a remote session by command or by agent. The host Flare starts hidden when it is not running. List past sessions to resume.

**Files.**
- `crates/flare/src/config.rs` (`flare open --on <machine> <folder>`), `main.rs`
- `crates/flare/src/cli_open.rs` (new), `startup.rs` (`--background`), `crates/flare-win/src/window_park.rs`, `crates/flare-win/src/macos/window_park.rs` (reuse for non-e2e hidden start)
- `crates/flare-ctl/src/method.rs`, `grant.rs` (agent method)
- `crates/flare-mcp/src/lib.rs` (`open_remote_session`, `server_instructions`)
- `crates/flare-agent/src/resume.rs` (`list_sessions`)
- `crates/flare/src/machines.rs` (registry, port trait)
- docs, rules

**New types and methods.**
- Hidden start. `--background` reuses the e2e park (offscreen, `WS_EX_NOACTIVATE`, `SWP_NOACTIVATE`) and `hold_off_app_nap`, without e2e fakes. The lock, ctl pipe, tab restore and live shells stay normal. The serve bridge starts `<flare> --background` when `connect_default` fails, then waits for the pipe (60 s). It never takes the foreground. The window can be raised later by a normal launch. Spike first on the real Mac: start from an SSH session needs a GUI login session. If none, `remote.hello` answers `host_not_ready` with one line.
- `flare open --on <machine> <folder> [--resume <id>]`. It runs in the viewer. `--on` and its value are parsed in `config.rs`, so they never reach `paths_from_argv`. It finds the running local Flare (`connect_default`) and calls `Method::RemoteOpenLocal` (a UI-grant method that the CLI reaches only through the same-user pipe with a new `cli` permission). If no local Flare runs, it starts one with `--background`.
- `Method::OpenRemoteSession` (agent grant, namespace `Namespace::Show` family or a new `Namespace::Open`). Params `{machine, folder, resume?}`. The agent may start a session without a prompt (owner decision). The opened tab takes no agent input: the agent grant still has no `remote.input` and no `shell.input`.
- MCP tool `open_remote_session {machine, folder, resume?}` with a one-line mention in `server_instructions()`.
- `ClaudeSessionSummary { id, first_prompt: String(<=200), modified: SystemTime }` and `fn claude_session_list(root, cwd, max) -> Vec<ClaudeSessionSummary>`. It reads only the head of each transcript (rule 4). Host method `remote.sessions {folder} -> [...]`. Worker thread.
- `trait MachinesPort { fn list(&self) -> Vec<Machine>; fn get(&self, id) -> Option<Machine> }` with a store file `machines.json` (atomic write, rule 36). For PR 5 the list can be edited by hand. PR 7 adds the page.

**Tests.**
- Unit: `--on` parsing (value is never a path, `--user-data-dir` untouched), unknown machine gives a clear error.
- Unit: grant: the agent grant has `open_remote_session` and still no `remote.input`, `shell.input`. Update `agent_can_show_and_drive_but_not_eval` and the protocol.md table in this PR.
- Unit: MCP tool schema and arg errors with the fake `Channel`.
- Unit: session list reads at most the head, ignores non-interactive transcripts, sorted by time.
- Unit: hidden start decision with a fake launcher (already running, not running, pipe never appears, no GUI session).
- e2e `remote_open`: `flare open --on loop <folder>` with machine `loop` mapped to a stdio loopback command. Host started with `--background`, offscreen, no focus change (asserted). Tab appears on both profiles.

**Rules to add.**
- R-new-18: an agent may open a remote session. It may not type into it. The opened tab takes no agent input (amends S3, rule 188).
- R-new-19: `--background` never raises or takes focus. Same park code as e2e.
- R-new-20: `--on` and other flag values never reach path parsing.
- R-new-21: session listing reads only the head of each transcript on a worker.

**Depends on.** PR 4. **UI.** No (the folder picker UI is in PR 7). The agent and command paths pass a folder string.

---

## PR 6: Edges (file links, image paste and drop, Mac to PC)

**Goal.** File links fetched over the link, image paste and drop copied to the host, and the PC as host (Windows checks). It must still work in both directions.

**Files.**
- `crates/flare/src/remote_files.rs` (new), `ctl_app.rs`, `appctl.rs` (side panel, `open_term_path` routing for remote tabs)
- `crates/flare-term/src/handle.rs` (paste of images and drops for remote tabs)
- `crates/flare/src/remote_link.rs` (Windows command quoting)
- docs: `docs/features.md`, a by-hand check list `docs/remote-sessions-manual-checks.md`

**New types and methods.**
- `remote.fetch {tab, path, max_bytes} -> {name, mime, size, chunk: u32, chunks: u32, data: base64}`. Chunked well under the 16 MB line cap (256 KB per chunk). The path is checked on the host against the same allowlist as S2 (rule 187): the tab's folder, the temp folder, and nothing else. Symlink escape refused.
- `remote.put {tab, name, chunk, chunks, data} -> {host_path}`. Writes into a per-session temp folder on the host. Size cap. Name sanitized. The viewer then pastes `quote_path(host_path)` for the host shell, so rule 1 holds.
- Viewer: a link on a remote tab is resolved by the host, never as a local path (rules 157, 266 to 275). The fetched copy shows in the side panel. "Open" saves a copy to the local temp folder and opens it.
- Windows host command: `ssh <host> "<exe>" --serve-remote` quoted for `cmd` (no `&&`, path with spaces in double quotes). Test the builder with a table of paths and both host OS kinds.
- Mac host: confirm `LOCAL_PEERPID` and the per-user socket path work for a process started by sshd. The serve child has no `FLARE_CTL_PID`, so it uses the plain-name path (protocol.md line 70).

**Tests.**
- Unit: allowlist (`..`, symlink, absolute outside, device names on Windows), chunking and reassembly, size caps, temp folder cleanup on session end.
- Unit: command builder table (spaces, quotes, Windows and Mac).
- Unit: paste of a dropped file gives a quoted host path, never raw bytes.
- ui_tests: side panel shows a fetched file for a fake remote tab.
- e2e `remote_files`: B has a file in the tab folder. A clicks the link (scripted), the panel shows the bytes. A drop of a small image reaches B's temp folder.
- Both directions: run all remote e2e scenarios with the roles swapped (A and B profiles swapped). Mac to PC is mostly this plus the by-hand list on real hardware.

**Rules to add.**
- R-new-22: a remote path is never opened as a local path. Files come through `remote.fetch` with the host allowlist (extends S2).
- R-new-23: paste and drop of files and images copy to a host temp folder and paste a quoted host path.
- R-new-24: the ssh command line is built per host OS from separate argv items and has no secrets.

**Depends on.** PR 5. **UI.** Reuses the existing side panel. No new screens.

---

## PR 7: UI (Settings > Machines, new-tab menu, folder picker, Mac/PC tag, fitted conversation view)

**Goal.** All the new screens. Mockup first. Build only after the owner approves the mockup.

**Owner picks (2026-11-12, mockups in Zen). These replace the matching lines below.**
- No subtext anywhere on the Machines page. Logos, never text tags: Apple for a Mac, Windows for a PC.
- Linked machines: a table with columns Machine (logo and name), SSH host, Flare (version), Status (green dot and "Linked"), and a ⋯ menu (Test, Remove).
- Adding: a "+ Add a machine" row that opens an inline form: SSH host drop-down, Name field, Cancel, Add. A failed check is one red line under the form, for example "Flare there is 0.0.9, here 0.1.0. Update it there."
- The + menu: one row per linked machine, its logo and name (for example the Apple logo and "Mac Mini"). It opens a shell tab on that machine. No submenu, no folder browser, no past-sessions list in the menu: the owner starts or resumes Claude in that shell. Drop the `RemoteFolder` browser and R-new-26.
- Vertical tabs (Flare's cards with the Clawd pane): Clawd stays in the pane; the machine's logo sits before the folder path on the second line.
- Horizontal tabs: Clawd stays first; the machine's logo sits at the right, after the status spinner and before the ×.
- The mockups: `docs/superpowers/specs/remote-sessions-mockups/` (pick.html, tabs.html).

**Gate.** Before any code: ask which design skill and which image tool to use for mockups (with a recommendation), make HTML mockups, open them in Zen, and get a yes. Everything in PRs 1 to 6 works without this PR (command, agent tool, hand-edited `machines.json`).

**Files.**
- `crates/flare-settings/src/pages.rs` (`PAGES` 7 to 8, `machines`), `page/machines.rs` (new), `page/mod.rs` (`build_page`), `find.rs`, `find_state.rs`, `icons.rs`, `sections.rs`, `rows.rs`
- `crates/flare/src/settings_ctl.rs` (`MachinesPort`, `set_machines`), `config.rs` (`--settings-page` validation)
- `crates/flare/src/menus_ui.rs` (`plus_menu_rows`), `appctl.rs` (`menu_picked`, new `MenuKind::RemoteFolder`)
- Slint: rail, Machines page, tab tag, folder browser, fitted conversation view
- `ui_tests/settings_*`, `ui_tests/remote_*`

**New types and methods.**
- `MachinesInfo { machines: Vec<MachineRow>, ssh_hosts: Vec<SshHost> }`, `PageEnv.machines: Option<&MachinesInfo>`.
- `trait MachinesPort { list_ssh_hosts, save, test_link, remove }`. `~/.ssh/config` parser on a worker. It handles `Include`, `Host` patterns and skips wildcards and `Match`. Host names go as argv items only.
- `test_link` runs `remote.hello` and shows the result or the one-line version refusal.
- New-tab (+) menu: rows "New tab on Mac" and "New tab on PC" (one per machine that is up). A pick opens the `RemoteFolder` browser: a Flare-drawn list backed by `remote.ls`, with Back, Home, a "Past sessions" tab from `remote.sessions`, and loading and error states.
- Tab strip and list: a small Mac or PC tag from `Tab.remote.kind`. Right-click adds "Hide here only" (closes the viewer, keeps the host session) next to "Close" (ends everywhere, rule 48 asks first).
- Fitted conversation view: a Slint list of `Entry` rows (role, text, tool) fitted to the window width, shown for a remote tab that is idle on this screen. A click or a key switches to the live terminal and sets the size owner (last typist).

**Tests.**
- Unit: `build_page("machines", ...)` rows, search index hits, ssh config parser fixtures (Include, patterns, Match, quotes, CRLF), `--settings-page machines` accepted.
- ui_tests and scene screenshots (headless): `--scene settings --settings-page machines`, the + menu with machines, the folder browser (loading, listing, error), the tab tag, the conversation view fitted to three widths.
- Unit: "Hide here only" does not call `remote.close`. "Close" does.
- e2e `remote_ui`: scripted pick of the + menu entry opens the folder browser against the loopback host, picks a folder, a tab appears with the tag.

**Rules to add.**
- R-new-25: "Close" on a remote tab ends the session everywhere and asks as any tab (rule 48). "Hide here only" never ends it.
- R-new-26: the folder browser lists host paths only and never touches the OS dialog.
- R-new-27: machines are stored atomically. Unknown values read as default (rules 36, 40).

**Depends on.** PRs 5 and 6. **UI.** Yes, all of it. Needs the approved mockup first.

---

## Order, ship gates and risks

| PR | Title | UI | Ships alone because |
|---|---|---|---|
| 1 | Stdio wire, Remote grant, `remote.hello` | no | Library and docs only. No behavior change. |
| 2 | Serve mode and host bridge | no | Host side works with a test client. |
| 3 | Viewer tab (`RemotePty`, reconnect) | no | Needs `machines.json` by hand. Works through the e2e. |
| 4 | Status, conversation, keep-awake | no | Adds marks and sleep hold on top of PR 3. |
| 5 | Starting sessions (command, agent, hidden start, resume) | no | The usable v1 without screens. |
| 6 | Edges (files, paste and drop, both directions) | no | Completes the feature set. |
| 7 | UI (Machines, menu, picker, tag, conversation view) | yes | Needs the approved mockup. |

Gate for every PR: `cargo test` on Windows and the macOS CI job (shared code compiles on macOS, rule 227), the headless e2e, protocol.md and regression-rules.md updated in the same PR, then install the branch on this PC and confirm the version before the merge question (global rule). Ask before each install. Never take the screen. Real two-machine checks (PC to Mac, Mac to PC) are by hand after PR 6, listed in `docs/remote-sessions-manual-checks.md`.

Early spikes (do in PR 1 and 2 before more code):
1. A release build on Windows with `windows_subsystem="windows"`: does stdio inherited from sshd work?
2. Mac: can Flare start from an SSH session of the logged-in user?
3. `LOCAL_PEERPID` for a child of sshd.
If a spike fails, change PR 5 (hidden start) and write down the limit.

Spike status after PR 1 (2026-11-12): none of the three can be proven without a real ssh session, and the PR 1 work ran
no ssh to any real machine (tests use OS pipes and in-memory wires only). What PR 1 does prove: the protocol, grant and
banner handling over real OS pipes (`a_remote_hello_round_trips_over_os_pipes_after_a_banner`). Spike 1 is the first
step of PR 2: `StdioWire::from_process` returns an `io::Result`, and with no stdin or stdout handle (what a
`windows_subsystem = "windows"` build would see if sshd's handles were not inherited) duplicating the handle is
expected to fail rather than hang. That is not tested yet: PR 2 tests it with a child it starts itself, and a by-hand
run over real ssh confirms it. Spikes 2 and 3 stay
for the by-hand list (`docs/remote-sessions-manual-checks.md`, PR 6).

Spike status after PR 2 (2026-11-12): still no ssh to any real machine. Spike 1 is proven as far as it can be without
sshd: the serve process is a `windows_subsystem = "windows"` binary in a release build, and `remoteLoopback` starts that
build with piped stdin and stdout (`CREATE_NO_WINDOW`) and speaks the whole protocol over them; the debug build does
the same in `tests/serve_remote.rs`. With no stdin or stdout handle at all `from_process` fails and `--serve-remote` exits 2 with one stderr line, never a hang. Whether sshd hands a GUI-subsystem child its pipe handles the same way is
the one part left for the by-hand list. Spikes 2 and 3 are unchanged.

Open risks tracked across PRs: closed enum arrays (`Method::ALL`, `Namespace::ALL`, `Permission::ALL`) and their exhaustive tests; event lossiness (solved by polling and sequence numbers); stuck-handler cap of 32 on slow links (short polls only); two writers on one session; version skew between machines (refuse with one line); `tabs.changed` is reserved and unsent, so the viewer polls `remote.tabs` and `remote.status`.
