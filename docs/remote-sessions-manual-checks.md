# Remote sessions: checks by hand on the real PC and Mac (2026-11-12)

Every automated test of remote sessions runs on one machine with a loopback `flare --serve-remote` in place of ssh
(`docs/e2e.md`, the `remote*` scenarios). What only a real ssh between two machines can show is listed here. Plan:
`docs/superpowers/plans/2026-11-12-remote-sessions.md` ("Order, ship gates and risks", spike status per PR).

Run them after PR 6 merges, and again after PR 7, in both directions: the PC viewing the Mac, then the Mac viewing
the PC. Note the date, both Flare versions and the result of each line in the PR or an issue.

## Setup

1. Both machines run the same Flare version (a different one must refuse in one line naming both versions, rule 278).
2. `ssh <host>` works from each machine to the other with keys and no prompt (`BatchMode=yes`).
3. Each viewer's profile has `machines.json` with the other machine: `id`, `name`, `kind` (`mac` or `pc`), `ssh`,
   `user` when needed, and `flare`, the full path of the other machine's Flare (on the Mac
   `/Applications/Flare.app/Contents/MacOS/flare`; on the PC `C:\Users\<you>\AppData\Local\Flare\current\flare.exe`).

## The link (PRs 1 to 3)

- [ ] Spike 1, the PC as host: Windows OpenSSH starts the GUI-subsystem `flare.exe --serve-remote` with sshd's pipes as
  its stdio, and `remote.hello` answers (a tab opens on the Mac).
- [ ] Windows `ssh.exe` started with no console (`CREATE_NO_WINDOW`) and `BatchMode=yes` runs the command with clean
  stdio both ways, and no console window flashes on the PC.
- [ ] The Mac's login shell runs `'<path>' --serve-remote` as built, and the PC's `cmd` runs `"<path>" --serve-remote`,
  both with a path that holds a space.
- [ ] How long a real ssh takes to come up; the 90 s hello wait covers it with room.
- [ ] Spike 3: the serve process started by sshd on the Mac reaches the running Flare's per-user socket
  (`LOCAL_PEERPID` checks pass; the serve child has no `FLARE_CTL_PID`, so it uses the plain-name path).
- [ ] A dropped network (Wi-Fi off for a minute) reconnects with no line twice; closing the tab closes it on the host.

## Status and sleep (PR 4)

- [ ] The Mac as host: a session followed from the PC keeps the Mac awake while it works (`pmset -g assertions` lists
  Flare's `PreventUserIdleSystemSleep`), and the Mac sleeps as usual once the work stops.
- [ ] The PC as host: `powercfg /requests` lists Flare (SYSTEM) while a followed tab works, and nothing after.
- [ ] One notification per question: on the viewer only.

## Starting sessions (PR 5)

- [ ] Spike 2: with Flare closed on the Mac, opening a tab from the PC starts it hidden over ssh; the tab works; a
  normal launch on the Mac then brings the window forward with its Dock icon.
- [ ] With Flare closed on the PC, opening a tab from the Mac answers one line (`host_not_ready`: open Flare there
  once), and connects once Flare runs on the PC.
- [ ] Whether sshd's job on the PC allows `CREATE_BREAKAWAY_FROM_JOB` (a hidden start there outlives the ssh login).
- [ ] `flare open --on <machine> <folder>` from a terminal opens the tab in front.

## Files (PR 6)

- [ ] On the PC viewing the Mac: Claude in the remote tab writes a file link to a picture and to a `.md` file in the
  session's folder; a click shows each in the PC's side panel, from a copy in the PC's temp folder.
- [ ] The same on the Mac viewing the PC (paths with `\`, a drive letter, a folder with a space).
- [ ] A link to a file outside the tab's folder (for example `~/.ssh/config`) says "No such file" and fetches nothing.
- [ ] A link to a `.pdf` opens the copy in the viewer's PDF reader; a link to a script or a program shows it in
  Explorer or Finder and never runs it.
- [ ] A screenshot pasted into the remote Claude (Ctrl+V on the PC, Cmd+V on the Mac) arrives as an image in Claude on
  the host (`[Image #1]`), from the host's temp folder.
- [ ] A file dropped from Explorer or Finder on the remote tab pastes the host's quoted path; Claude reads it.
- [ ] Closing the remote tab removes its upload folder on the host (`<temp>/flare-remote/<run>/<tab>`) and its copies
  on the viewer (`<temp>/flare-remote-copies/<run>/<tab>`).
- [ ] A Mac clipboard picture that is only TIFF (rare: some apps copy only TIFF) is refused with one line, not sent.
