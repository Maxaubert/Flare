# Remote sessions: a Claude tab that runs on the other machine (design and plan, 2026-11-12)

Status: planned, not started. Owner answers from the 2026-11-12 grilling session are marked **(owner)**;
everything else is Claude's call and can be overruled.

## Intent

Max has a Windows PC and a Mac Mini, linked both ways over SSH, both running an up-to-date Flare. He wants to
use Claude Code on one machine from the other as if it were local: a normal tab that shows in the tab list,
takes typing and dictation, shows working/done/question marks, but actually runs on the other machine. He also
wants to start such sessions from a menu, a command or an agent. "Our own cloud computer."

## Decisions

| Topic | Decision |
|---|---|
| Where a session lives | On the machine that runs it, owned by that machine's Flare. The other side only views and types. A dropped link or a sleeping viewer never kills it; the viewer reconnects. **(owner)** |
| Both screens | A remote session is a tab on both machines at once; either can type. **(owner)** |
| Ways to start one | New-tab menu "New tab on Mac/PC" with a folder picker; a terminal command (`flare open --on mac <folder>`); an agent tool; resuming one of that machine's past Claude sessions. All four. **(owner)** |
| The + menu | One row per linked machine, with that system's logo and the machine's name (for example the Apple logo and "Mac Mini"). It opens a shell tab on that machine, with no submenu: Max starts or resumes Claude from that shell himself. **(owner, 2026-11-12 mockup)** |
| Machines page look | No subtext under rows. Linked machines are a table: Machine (system logo and name), SSH host, Flare version, Status, and a ⋯ menu (Test, Remove). Adding is a "+ Add a machine" row that opens an inline form: SSH host drop-down, name field, Cancel, Add; a failed check shows one red line under it (for example "Flare there is 0.0.9, here 0.1.0. Update it there."). Logos, not text tags: Apple for a Mac, Windows for a PC. **(owner, 2026-11-12 mockup picks: Machine D, Add B)** |
| Tab marks | Vertical tabs: Clawd stays in the card's pane, the machine's logo sits before the folder path. Horizontal tabs: Clawd first, the logo at the right before the ×. **(owner, 2026-11-12 picks: Vertical 4, Horizontal 4)** |
| Direction | Both ways, one design and one code path. Built and tested PC to Mac first, then Mac to PC. **(owner)** |
| Size | The live terminal has one size: the machine you typed on last sets it. **(owner)** |
| The other screen | Shows Flare's own conversation view (`flare-convo`), fitted to its window, instead of a squeezed terminal; a click or a key takes over the live terminal at that window's size. **(owner, after asking why one size)** |
| Host Flare not running | It is started hidden (no window) over SSH; the tab is there when Flare is opened on that machine. **(owner)** |
| Sleep | The host stays awake while any of its remote sessions is working; normal sleep once all are idle. **(owner)** |
| File links | A link to a file on the other machine is fetched over SSH and shown in the side panel; Open saves a copy and opens it locally. **(owner)** |
| Closing the tab | Ends the session everywhere, like any tab. The right-click menu adds "Hide here only". **(owner)** |
| Tab list | Remote tabs sit among the others with a small Mac/PC tag. **(owner)** |
| Agent starts | An agent may start a session on the other machine without a prompt. **(owner)** |
| Setup | Settings > Machines: reads `~/.ssh/config`, Max picks the host, names it, and Flare tests the link. Mockup first (plugin/UI rule). **(owner)** |
| Transport | SSH only: Flare runs `ssh <host> <flare> --serve-remote` and speaks the existing control protocol (JSON lines, `flare-ctl` framing, any `Wire`) over its stdio. No new port, no listener. Claude's call. |
| Image paste and file drop | Copied to a per-session temp folder on the host first, then the host path is pasted. Claude's call. |
| Versions | Both sides state their protocol version on connect. A mismatch refuses with one line that names both versions. Claude's call. |
| Auth | Only what SSH already allows. The serve mode answers only its own stdio, under the logged-in user. Claude's call. |

## Non-goals (first version)

- More than two machines, or machines Max does not own.
- Any listener on a network port, any relay or cloud service.
- Running Claude on one machine against the other's files (sessions run where the files are).
- Remote plain shells beyond what a Claude tab needs (they may fall out for free; not a goal).

## Risks

- **Two Flares, one session.** The host's tab model must accept input and resizes from a second source
  without fighting its own window. The "last typist sets the size" rule keeps this simple.
- **Windows as host.** Windows OpenSSH lands in `cmd` by default; the serve command must not depend on the
  login shell. Hidden start on Windows must never take the foreground (tests-never-take-my-screen rule).
- **Mac as host.** A hidden start must not need a GUI login prompt; Flare must be startable from an SSH session
  of the logged-in user.
- **Reconnect.** Scrollback and the conversation view must be resent on reconnect without duplicating lines.
- **Latency.** Keystrokes go over SSH; a local echo is not attempted in v1.

## Plan

1. **Serve mode.** `flare --serve-remote`: a headless entry that connects to (or starts, hidden) the local
   Flare and bridges its control channel to stdio. New methods: `remote.hello` (versions, machine name),
   `remote.open` (folder, optional session to resume), `remote.attach`/`remote.detach`, `remote.input`,
   `remote.resize`, and an output stream (`remote.output` events with a sequence number for resume).
   Tests: protocol unit tests over an in-memory wire.
2. **Viewer tab.** A tab kind "remote" that draws the stream like a local PTY and sends keys, paste and
   resizes. Reconnect with the last sequence number. Mac/PC tag on the tab.
3. **Status.** The host forwards the tab's agent state (working, done, question, failed), title and badge
   over the link; the viewer shows them in the strip, the tab list, the badge count and notifications.
4. **Both screens.** The host also shows the session as an ordinary tab. Size follows the last typist; the
   idle screen shows the fitted `flare-convo` view and takes over on a click or key.
5. **Starting sessions.** New-tab menu entry and folder picker (browsing the host over the link), the
   `flare open --on <machine> <folder>` command, the `open_remote_session` MCP tool, and "resume a past
   session there" (the host lists its own Claude sessions).
6. **Settings > Machines.** Mockup first, then the page: hosts from `~/.ssh/config`, a name, a link test.
7. **Edges.** File links fetched over SSH into the side panel; image paste and file drop copied over; the
   host's keep-awake while a session works; hidden start on both systems; version refusal line.
8. **Mac to PC.** Turn the same path on with the PC as host; Windows-specific checks (cmd login shell,
   hidden start, no focus steal).
9. **Tests and docs.** Unit tests per step; an e2e scenario that runs both Flares on one machine over a
   loopback `ssh`-like stdio pipe (headless, no real SSH needed in CI); `docs/features.md` entries and
   regression rules; a by-hand check list for the real PC and Mac.

## Left to the builder

The exact method names and wire shapes, the hidden-start mechanics per system, how the conversation view
finds the remote transcript (streamed by the host vs. fetched), and the keep-awake API per system.
