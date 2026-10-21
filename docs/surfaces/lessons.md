# Surfaces lessons

Holes hit while building, what fixed them, and what the Prism Terminal port should do differently.
Newest last.

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
