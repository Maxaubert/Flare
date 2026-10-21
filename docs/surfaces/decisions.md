# Surfaces decisions

Each entry: the date, the stage, the choice, the reason. Owner decisions are marked (owner).

- 2026-10-21, plan: build in Flare first, document for Prism Terminal, port after (owner).
- 2026-10-21, plan: the agent must be able to USE surfaces, not only show them, modelled on the iOS
  simulator MCP (describe, tap, type, screenshot) (owner).
- 2026-10-21, plan: video must play at full frame rate, so web content is a real WebView2 panel, not
  frames drawn into the grid (owner: "it can't just be a frame here and there").
- 2026-10-21, plan: one panel per tab in v1; more later.
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
