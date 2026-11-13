# Remote sessions: implementation plan (2026-11-12)

Spec: `docs/superpowers/specs/2026-11-12-remote-sessions-design.md`.
Status: approved 2026-11-12; building. PR 1 built on `feat/remote-1` (2026-11-12, not merged); PR 2 built on `feat/remote-2` (2026-11-12, stacked on PR 1, not merged); PR 3 built on `feat/remote-3` (2026-11-12, stacked on PR 2, not merged); PR 4 built on `feat/remote-4` (2026-11-12, stacked on PR 3, not merged); PR 5 built on `feat/remote-5` (2026-11-12, stacked on PR 4, not merged); PR 6 built on `feat/remote-6` (2026-11-12, stacked on PR 5, not merged); PR 7 built on `feat/remote-7` (2026-11-12, stacked on PR 6, not merged). All seven PRs are built; none is merged.

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

**Built (2026-11-12, `feat/remote-3`, stacked on `feat/remote-2`).** Rules R-new-9 to R-new-13 are rules 287 to 291.
Where the build differs from the lines above:
- **No `Link` in `flare-term`.** `flare-term` has no `flare-ctl`, so the pty and the link meet in a `RemoteIo`
  (`remote_pty.rs`): the pty's writer puts typing there, `resize` the last size, `kill` and `leave` the way it goes; the
  link's worker (`flare/src/remote_link.rs`) takes them and pushes output back. `Link::call` takes `&mut self` (one call
  at a time on a link). The fake link is an in-process host (`remote_link_tests.rs`), plus one test over a `MemWire`
  to a served dispatcher; `SshConnector` is the real one.
- **Typing is read back into intents.** The tab's emulator and key encoder stay as they are; `decode_typing` reads
  what the tab wrote back into keys, text and pastes (`encode_key` backwards, held by a round-trip test over every
  named key and chord), and the emulator's answers never reach the writer (`SessionManager::write_reply`). A mouse or
  focus report is not sent (there is no mouse intent yet).
- **`kill` closes, `leave` detaches.** `PtyProcess::leave` (default: `kill`) is what the quit calls (`kill_all`), so a
  local shell is still killed and a remote tab only detaches. `TabHost::shutdown` no longer kills tab by tab.
- **The first attach is a snapshot.** A new or restored tab has an empty screen, so it starts from `remote.snapshot`,
  then polls from its `seq`; `since: last_seq` is used for a reconnect within a run (`gap`: a snapshot). `last_seq` is
  still kept in `tabs.json`, but is no reason on its own to write it.
- **Replay without marks** is three parts: the snapshot is plain text (`plain_snapshot`), a catch-up has its OSC 777
  and 7501 taken out (`SignalStrip`), and the hub reads a remote tab's screen as blank, keeps no question watch and
  does not score its first 3 s of output (`AgentHub::on_remote_tab`). There is no in-band "replayed" marker.
- **`machines.json` by hand.** A small reader in `remote_link.rs` (`id`, `name`, `kind`, `ssh`, `user`, `flare`); PR 5's
  `machines.rs` registry takes it over. `MachineTag { id, name, kind: Option<Os> }`; a restored tab knows only the id
  until PR 7 reads names and logos. The ssh command builder is here already (the plan put it in PR 6).
- **One link per tab.** No ssh connection sharing yet; a second tab on the same machine starts a second ssh.
- **Kind == Shell audit.** Skipped for a remote tab: file link open, preview, thumbnail and menu path (`TabInfo.remote`),
  a dropped file, the live-preview chip, recent folders, `cd`, `live_pids` and the restore's stat. Kept: the Cwd report
  (the host's folder becomes the tab's folder), the tab menu (Close, Copy folder path), plugins' tab actions (their
  typing goes as intents), Ask agent.
- **Opening one.** No menu or command yet: under `--e2e` `FLARE_E2E_REMOTE_OPEN` opens one at start, and
  `FLARE_E2E_REMOTE_COMMAND` replaces ssh with a loopback `flare --serve-remote` of another profile.
- **Tests.** Unit: `remote_pty::tests`, `session::remote_tests`, `restore::remote_tests`, `model::remote_tests`,
  `remote_link::tests`, `tab_host::tests::a_remote_tab_never_starts_a_local_shell_and_the_quit_leaves_its_session`,
  `agent_hub::tests::a_remote_tabs_replay_raises_no_mark_and_live_output_does`. ui_tests:
  `a_remote_tab_shows_the_hosts_output_and_takes_typing`. E2E: `remoteViewer`.
- **Known limits.** Typing while the link is down is dropped. A `remote.open` whose answer is lost to a broken link is
  sent again, and may leave a second tab on the host. A remote tab shows no colours after a snapshot. A refused open
  leaves the tab with the reason in red and no session.

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

**Built (2026-11-12, `feat/remote-4`, stacked on `feat/remote-3`).** Rules R-new-14 to R-new-17 are rules 294 to 297.
Where the build differs from the lines above:
- **`remote.status`** answers `{tabs: [{tab, state, kind, session, folder, title, working_for?, question_text?,
  failed?}], seq}` for every shell tab, from `tab_facts` and the hub (`remote_glue::remote_status_tabs`); `kind` is the
  agent, the failure's kind is `failed`. One word a tab in the order of weight `question`, `failed`, `working`, `done`,
  `idle` (`flare_ctl::remote::TabState`). `seq` is the host's number of the answer (`RemoteHost::status_seq`): it grows
  when the answer changes, so it moves every poll while a tab works (`working_for`).
- **The viewer's poll** runs on the tab's link worker, between the output polls, every 1.5 s (`Timing::status_every`)
  and at once after each attach (`remote_status::StatusPoll`). A change of its own tab's entry goes into the tab's
  `RemoteLive` (`HostStatus`) and wakes the window (`RemoteSpawner::with_wake`); `AppCtl::sync_remote_status` hands it to
  `AgentHub::on_remote_status`. A host that refuses `remote.status` (a PR 3 build) keeps its link and is not asked again
  on it.
- **"Remote override"** is `Indicator::remote_mark` (`RemoteMark`): working and question are set as said (the tab gets an
  agent, the host's kind or Claude). Done and failed are never copied: when the host's word leaves working or question,
  the viewer raises its own Finished or Failed mark only if it showed the tab busy and nobody here looks (held and
  cleared by its own looking, rule 61); idle raises nothing. The hub applies a word only when it differs from the last
  one applied, so a status read again after a reconnect raises nothing. The question's words come from the status
  (`question_text` of a remote tab), so the viewer's toast carries them. `working_for` is not used on the viewer yet.
- **One toast.** The viewer's notifier is unchanged (one per episode); the host's `notify_update` leaves out every tab a
  remote viewer follows (`RemoteHost::is_followed`).
- **Keep-awake** is `flare_win::keep_awake` (`KeepAwake`, `Hold`, `SystemAwake`, `FakeAwake`): a counted hold, one
  thread on Windows (`SetThreadExecutionState`, declared from kernel32 rather than a new windows-sys feature), one
  IOKit assertion on macOS. `AppCtl::sync_keep_awake` holds while any followed tab is working with an agent, checked at
  every refresh, attach and release. Under `--e2e` the fake stands in.
- **`remote.convo {tab, since?}`** answers `{tab, entries: [{seq, role, text, tool?, at?}], reset, to}` for a followed
  tab. The host keeps a feed per tab (`remote_convo::ConvoFeeds` in the `RemoteHost`), not a `Follower` thread: the last
  200 entries read once (`said::read_last_until`, new, up to the byte a `Tail` then follows from) and the tail read at
  each call on the worker, so nothing runs while nobody asks. Numbers go on across a rewrite or a session change
  (`floor`), and `reset` says when the viewer must start over. The viewer's side is `ConvoMirror` (dedupe by number);
  nothing polls it yet: PR 7's fitted conversation view does.
- **Tests.** Unit: `flare_ctl::remote::tests` (`TabState`), `method::tests`, `flare_convo` `said` (`read_last_until`),
  `indicator::tests` (`remote_mark`), `keep_awake::tests`, `remote_status::tests`, `remote_convo::tests`,
  `remote_host::tests`, `remote_link::tests` (the status poll, an older host), `ctl_app::tests` (`remote_status_*`,
  `remote_convo_*`), `agent_hub::tests` (`a_remote_tabs_host_status_marks_it_and_a_reconnect_raises_nothing_twice`).
  ui_tests: `agent_flows::remote_flows` (keep-awake, the host's toast, the status words). E2E: `remoteStatus`.
- **Known limits.** The keep-awake and the IOKit assertion are not proven on real hardware (no real ssh, no sleep
  timer waited out): the unit tests hold the counting and the Windows call; the Mac assertion is made and released for
  real in macOS CI since 2026-11-13 (`pmset -g assertions`, rule 296). A host
  that says `idle` while the viewer's live signals say `working` (an agent the host's poll has not found yet) takes the
  viewer's work mark down until the next live signal or status.

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

**Built (2026-11-12, `feat/remote-5`, stacked on `feat/remote-4`).** Rules R-new-18 to R-new-20 are rules 299 to 301;
R-new-21 was not written (no session listing, below). Where the build differs from the lines above:
- **No resume list.** The owner's PR 7 picks drop the past-sessions list from the + menu (the person resumes Claude in
  the remote shell), and this PR's task is "command, agent tool, hidden start". So `remote.sessions`,
  `claude_session_list` and `ClaudeSessionSummary` are not built; `--resume <id>` and the tool's `resume` take an id the
  caller already has, and the host checks it with `valid_resume` at `remote.open` (PR 2).
- **One method, `open.remote` (`Method::OpenRemoteSession`), in a new `Namespace::Open`** (`ALL` 19 to 20, `Method::ALL`
  59 to 60). The agent grant enters it; no permission opens it (no mod), and the remote and bridge grants never do. There
  is no `RemoteOpenLocal` and no `cli` permission: `flare open` says `hello` with no token, which is the agent grant, so
  the command reaches exactly what the agent's tool reaches. Params `{machine, folder, resume?, front?}`; the command
  sends `front: true`, the agent's tool leaves it out, so an agent's open never takes the front tab (and the keys)
  from the person. Answered `{tab, machine, folder}` at once; the tab's link worker connects. `remote.open` now carries
  `resume` (`RemoteSpec.resume`).
- **`--start-hidden`, not `--background`.** `--background <colour>` is already the look flag. `--start-hidden` is a
  normal launch born parked (the e2e's window attributes, `select_backend_with(.., parked)`) and parked again by the same
  timer (`park_e2e_window_when_ready`), with nothing else of the e2e; its parked place is never saved; its first raise
  (`unpark_background`) gives back the normal style, the taskbar button and the saved place. A `--start-hidden` launch
  that meets a running Flare hands over nothing and raises nothing (`handoff`). macOS also gets `hold_off_app_nap`.
- **Where the start happens.** `flare_ctl::Dispatcher::remote_ready`, asked by the library after `remote.hello`'s
  version check and grant and before its answer; the bridge's `hidden_start::ensure_running` starts
  `<exe> --start-hidden [--user-data-dir] [--e2e]` detached (out of ssh's job where allowed, its own process group on a
  Mac) and waits 60 s for the pipe. `host_not_ready` (new code) answers the hello in one line; the link stays open. The
  viewer's hello wait is 90 s (`HELLO_WAIT`). `flare --serve-remote --no-start` (new) never starts one: the process
  tests use it.
- **Windows session 0.** sshd runs a login in session 0, where no window is ever seen and the single-instance lock is
  not the desktop's: a Flare started there could never be raised. So a serve process in session 0 starts nothing and
  answers `host_not_ready` ("open Flare there once"); the PC as a host needs Flare running in the person's session.
- **`machines.rs`** holds the registry `remote_link.rs` had (`Machine`, `parse_machines`, `read_machines`), plus
  `MachinesPort` (`list`, `get`), `FileMachines` with an atomic `save` (`flare_settings::atomic::write_atomic`) for PR 7,
  and `unknown_machine`'s one line.
- **Tests.** Unit: `flare_ctl` `grant::tests::the_agent_may_open_a_remote_session_but_never_type_into_one` (and the
  table tests), `method::tests`, `session::tests::a_host_that_is_not_ready_refuses_the_hello_in_one_line_and_takes_another`;
  `flare` `config::tests` (`open_on_*`, `start_hidden_*`), `main` (branch order), `cli_open::tests`, `hidden_start::tests`
  (running, started, never up, no desktop), `serve_remote::tests::the_hello_waits_for_a_flare_here_and_a_host_that_cannot_start_one_says_so`,
  `ctl_app::tests::open_remote_names_a_known_machine_and_opens_a_tab_the_agent_cannot_type_into`, `machines::tests`,
  `window::tests::a_background_start_is_parked_like_the_e2e_and_its_raise_undoes_the_style`,
  `appctl::tests::a_background_launch_that_meets_a_running_flare_raises_and_opens_nothing`; `flare-mcp`
  `open_remote_session_checks_its_arguments_and_routes_to_open_remote`. ui_tests:
  `term::flow::an_agents_remote_open_stays_behind_the_tab_in_front`. E2E: `remoteOpen`.
- **Known limits.** The hidden start over a real ssh login is not proven (no ssh to a real machine): whether a Mac's sshd
  login of the person at the screen can start the app (an accessory app until raised; whether the raise brings it to the
  Dock) and whether a Windows host refuses as designed are on the by-hand list. The e2e proves the chain on one PC: the
  command, the agent grant, the serve process's start of a `--start-hidden --e2e` host, its park and no focus.

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

**Built (2026-11-12, `feat/remote-6`, stacked on `feat/remote-5`).** Rules R-new-22 to R-new-24 are rules 303 to 305.
Where the build differs from the lines above:
- **`remote.fetch {tab, path, chunk?}`** answers `{tab, name, size, chunk, chunks, data}`: no `max_bytes` and no
  `mime` (the viewer classifies the copy by its name as a click does; the cap is the host's, 32 MiB, `too_large`).
  The tab must be followed; its folder comes from the window (`CtlOp::SessionOf`); the path is resolved with the
  click's own `resolve_term_path` from that folder and checked by `remote_files::resolve_fetch` on the worker (the
  file's real name inside the tab's folder or the host's temp folder, no device name, no share; `not_found`
  otherwise).
- **`remote.put {tab, name, chunk, chunks, data}`** answers `{tab, name, done, host_path?, paste?}`. Chunk 0's answer
  names the file as kept (`put_name`, then `-2`, `-3` when taken), and the next chunks send that name. The upload
  folder is per tab and per run, `<temp>/flare-remote/<run>/<tab>` (a host tab's id can come again after a restart),
  removed when the tab closes; a crashed run's folders go a day later. **The host quotes the path** (`paste`, with
  `quote_path` for the tab's own shell), not the viewer: the viewer does not know the host's shell (its `MachineTag`
  kind is unknown until PR 7), the host does. The viewer pastes the joined answers as one `paste` intent, sanitised
  again on the host (rule 1).
- **No `Link` in the window.** File work goes to the tab's link worker as an `Errand` (`flare_term::remote_pty`,
  `RemoteLive::send_errand`), done between the typing and the output poll, so a dropped file's path is pasted after
  what was typed before it; work asked while the link is down or after the session ended is refused at once.
- **Images.** `ClipboardPort::read_image` (new): Windows reads the registered PNG format, else `CF_DIB`/`CF_DIBV5` as a
  `.bmp` (`bmp_of_dib`), made a PNG with the `image` crate; macOS reads the pasteboard's PNG (`read_png`), and a
  TIFF-only picture is made a PNG by AppKit (2026-11-13, rule 1198). Copied files pasted on a remote tab go the same
  way as a drop.
- **`handle.rs` is unchanged.** The routing is above the handle: `TermUi` posts `TermMsg::RemoteLink` (a click) and
  `TermMsg::RemoteUpload` (a drop, or a paste of files or a picture) for a remote tab, and
  `appctl/remote_files_glue.rs` hands them to the link. Text still pastes through `TermHandle::paste` (rule 292).
- **The command builder** was built in PR 3; this PR adds its full table test and rule 305.
- **Tests.** Unit: `flare_ctl::remote::tests` (`put_name`, chunks), `method::tests`, `remote_files::tests`,
  `remote_link::tests` (fetch, put, caps, the link down, the command table), `ctl_app::tests` (`remote_fetch_*`,
  `remote_put_*`), `serve_remote::tests` (fetch retried, put never), `term_ui::tests` (a remote click, drop and paste),
  `remote_files_glue::tests`, `remote_pty::tests` (errands), `clipboard_port::tests` (`bmp_of_dib`, the stand-in's
  image) and, in macOS CI, `pasteboard::tests`. ui_tests: `term::flow::a_remote_file_link_*`,
  `a_file_dropped_on_a_remote_tab_*`, `an_image_pasted_on_a_remote_tab_*`. E2E: `remoteFiles`.
- **Known limits.** "Both directions" on one PC is the same code with the profiles' roles swapped, so no swapped copy of
  every remote scenario was added; the Mac to PC direction over real ssh is on the by-hand list
  (`docs/remote-sessions-manual-checks.md`, new). A dropped folder is not copied. The e2e has no clipboard-image
  request, so the image paste is held by the ui test only.

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

**Built (2026-11-12, `feat/remote-7`, stacked on `feat/remote-6`).** Rules R-new-25 and R-new-27 are rules 1192 and 1194
(numbered after main's last rule, 1189, and the PR 6 fixes 1190 and 1191); the fitted view's rule is 1193; R-new-26 was
dropped with the folder browser. Where the build differs from the lines above (the owner's picks win):
- **Settings > Machines** is `flare_settings::page::machines` (data: a table heading, one `BlockModel::Machine` a
  machine, the add row or `BlockModel::MachineAdd` in its place) drawn by `ui/settings/machines.slint`, with its door to
  the disk and the links in `crates/flare/src/machines_page.rs` (`MachinesPage`, `MachinesIo`; `RealMachinesIo` in the
  window, `SceneMachinesIo` in the settings scene, a fake in the tests). No `MachinesInfo.ssh_hosts: Vec<SshHost>`: the
  hosts are plain names. The ⋯ menu and the host drop-down reuse the Settings Select menu. The rail icon and the add
  row's plus are Flare's own icons (`icons::FLARE_ICONS`), since `SETTING_ICONS` is Prism Terminal's fixed set. The add
  row is in the row tables (`machines-add`), so Find a setting reaches the page.
- **`test_link`** is `remote_link::hello_over` (a link that says `remote.hello` and ends). The hello's answer now carries
  the host's `os` (`mac`, `pc`, `other`), which the page saves as the machine's `kind`. Adding a machine tries Flare on
  the host's PATH, then `/Applications/Flare.app/Contents/MacOS/flare`, double-quoted until the host says its system;
  a refusal stops the search. A version refusal's red line says which side to update from the two protocol numbers
  (`machines::failure_line`); the refusal names protocols, not Flare versions, so the line names protocols too. Every
  machine is checked once a run when the page first opens; Test checks it again.
- **The + menu** is the folder menu (the strip's + right-click and the sidebar's New tab arrow): one row per machine
  after the folders (`menus_ui::MACHINE_ROW` marks its pick), which opens a shell tab with `remote.open {folder: "~"}`;
  the host reads a lone `~` as its home folder (the viewer does not know it). The window reads `machines.json` once at
  start on a worker (`UiMsg::Machines`) and again after every save of the page.
- **Tab logos** are `TabRow.machine` (0 local, 1 Mac, 2 PC, 3 a remote machine of a system not known, which draws no
  logo); the vertical card draws it before the folder, the strip's `TabEnd` after the ring and before the ×; the
  tooltip starts with the machine's name. A restored tab takes its machine's name and kind once the machines are read
  (`AppCtl::tag_remote_tabs`).
- **"Hide here only"** is `TabHost::leave` and `SessionManager::leave` (the quit's detach for one tab).
- **The fitted conversation view** is on the viewer's remote tab only (as the plan says), not on the host's own tab.
  `remote.status` says who sizes each tab (`sized`); while the host does and the tab has a session, the link polls
  `remote.convo` into its `ConvoMirror` and hands the lines to the window (`RemoteLive`'s conversation); a click or a
  key takes the terminal back (`RemoteLive::retake_size`: the link sends `remote.resize` again) and the key goes on to
  the tab. `ui/remote_convo.slint` draws it over the terminal.
- **Tests.** Unit: `flare_settings` (`page::machines::tests`, the page, row, find and icon tables), `flare_ctl`
  (`session` and `client`: `os`), `flare_term::remote_pty::tests` (the conversation state), `flare` `machines::tests`,
  `machines_page::tests`, `ssh_config::tests`, `menus_ui::tests`, `tabs_ui::tests`, `remote_status::tests`,
  `ctl_app::tests::remote_open_reads_a_lone_tilde_as_the_hosts_home`, `config::tests`. ui_tests:
  `settings_machines::*` (the page at 100 and 225 percent, the table, the menu, the form and its red line),
  `remote_convo::*` (fitted at three widths and two scales, the click), `term::flow::a_machines_row_in_the_plus_menu_*`,
  `hide_here_only_*`, `a_remote_tab_the_host_sizes_shows_its_conversation_*`. Scene: `--scene settings --settings-page
  machines`. E2E: `remoteUi` (the + menu's machine row opens a tab on the loopback host with its logo; Hide here only
  leaves the host's session).
- **Known limits.** No real ssh was run: adding a machine over a real ssh (a Mac's login shell running
  `"flare" --serve-remote` and the Mac app's path, a PC's cmd), the page's first checks taking as long as a real
  login, and a key on the host taking the size back while the viewer shows the terminal are on the by-hand list. The
  host's own tab shows no fitted view while a viewer sizes it. A machine reached as another user, or with Flare
  elsewhere, still needs a hand edit of `machines.json` (the form has no user or path field, as picked).

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

Spike status after PR 3 (2026-11-12): still no ssh to any real machine. The viewer's whole path runs in `remoteViewer`
(two Flares, the link a loopback `flare --serve-remote` started as the viewer's child with piped stdio and no window),
and the ssh command line is held by a table test (argv items, sh and cmd quoting). Not proven without a real ssh and
left for the by-hand list: that Windows OpenSSH's `ssh.exe` started with no console (`CREATE_NO_WINDOW`) and
`BatchMode=yes` runs the remote command with clean stdio both ways; that a Mac's login shell and a PC's `cmd` run the
quoted command as built (paths with spaces); and how long a real ssh takes to come up, which the 30 s hello wait assumes.

Spike status after PR 4 (2026-11-12): still no ssh to any real machine. `remoteStatus` runs the status, the question
toast and the reconnect over the same loopback as `remoteViewer`. Not proven without real hardware and left for the
by-hand list: that a Mac host's IOKit assertion holds its idle sleep off for a session started over ssh and lets go
after (the code compiles in the macOS CI job only), and that a Windows host's `SetThreadExecutionState` keeps it awake
past its sleep timer while the viewer is on the other machine (`powercfg /requests` should list Flare while a followed
tab works).

Spike status after PR 5 (2026-11-12): still no ssh to any real machine. Spike 2 (can Flare start from an ssh session of
the logged-in user) cannot be proven here; the build tries the start on a Mac and answers `host_not_ready` when the pipe
never opens in 60 s, and on Windows it refuses in session 0 (sshd's logins) before trying, since a Flare there could
never be seen or raised. `remoteOpen` proves the rest of the chain on one PC (command, agent grant, the serve process's
hidden start, the park, no focus change). Left for the by-hand list: a Mac host started over ssh (does `open`-less
start of the binary reach the window server; does the accessory app get its Dock icon when raised), a PC host reached
over sshd with Flare closed (the one-line refusal), and whether `CREATE_BREAKAWAY_FROM_JOB` is allowed by sshd's job.

Spike status after PR 6 (2026-11-12): still no ssh to any real machine. Spike 3 (`LOCAL_PEERPID` and the per-user
socket for a serve process started by sshd on the Mac) cannot be proven without one: the serve child has no
`FLARE_CTL_PID`, so it takes the plain-name path, and the check is on the by-hand list. `remoteFiles` proves the file
path on one PC (a click fetched from the host into the viewer's panel, a drop copied to the host's upload folder and
its host path pasted). The by-hand list now exists: `docs/remote-sessions-manual-checks.md` (every spike and every
real-hardware check of PRs 1 to 6, in both directions).

Spike status after PR 7 (2026-11-12): still no ssh to any real machine. Settings > Machines checks and adds machines
over the same loopback the other scenarios use (`remoteUi`, and the page's tests over an in-memory stand-in). Not
proven without real hardware and left for the by-hand list: that `ssh <host> "flare" --serve-remote` finds Flare on a
Mac's or a PC's login PATH (and the Mac app's path when it does not), that `~/.ssh/config` hosts with an `Include` read
as ssh reads them, and how long the page's first checks take over a real login.

Mac parity audit (2026-11-13, `fix/mac-remote-parity`): every remote e2e scenario already ran in the macOS E2E job
(none is `mac_gate`d), the paste/drop routing and the put path's quoting (zsh, bash, fish, csh by the tab's shell id)
are the same code on both. Fixed for the Mac: the serve process made an `NSApplication` and checked in with
LaunchServices (rule 1196); a hidden start from ssh passed the login's `SSH_*` and a forwarded agent socket on and
could leave `TMPDIR` unset, and started a linked `flare` instead of the bundle's program (rule 1197); a TIFF-only
clipboard picture was refused (rule 1198); a Mac named itself "viewer" or nothing in `remote.hello` (rule 1199). The
process tests (`tests/serve_remote.rs`) and a real IOKit assertion test now run on macOS. Still by hand only: whether a
direct exec from an sshd login reaches the console user's window server (the alternative is `open -n -g -j`, which
LaunchServices runs in the GUI session; not taken without a real ssh to prove it), and Keychain access for Claude in a
Mac host started that way.

Open risks tracked across PRs: closed enum arrays (`Method::ALL`, `Namespace::ALL`, `Permission::ALL`) and their exhaustive tests; event lossiness (solved by polling and sequence numbers); stuck-handler cap of 32 on slow links (short polls only); two writers on one session; version skew between machines (refuse with one line); `tabs.changed` is reserved and unsent, so the viewer polls `remote.tabs` and `remote.status`.
