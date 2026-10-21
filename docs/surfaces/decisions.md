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
  cannot hold a connection past its timeout (15 s, 60 s for `browser.*`). Timed-out handlers are counted;
  past 32 new calls answer `unavailable`.
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
