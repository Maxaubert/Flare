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

- [x] Spike 1, the PC as host: Windows OpenSSH starts the GUI-subsystem `flare.exe --serve-remote` with sshd's pipes as
  its stdio, and `remote.hello` answers (a tab opens on the Mac). Passed 2026-11-13 over real ssh from the Mac with
  `cmd /c "C:\Users\Admin\AppData\Local\StableFlare\current\FlareStable.exe" --serve-remote --user-data-dir=...`:
  hello, `remote.open {folder: "~"}`, attach, an `echo` typed and seen in `remote.output`, `remote.close`, and the serve
  log ends with "the link closed".
- [ ] Windows `ssh.exe` started with no console (`CREATE_NO_WINDOW`) and `BatchMode=yes` runs the command with clean
  stdio both ways, and no console window flashes on the PC.
- [ ] The Mac's login shell runs `'<path>' --serve-remote` as built, and the PC's login shell runs
  `cmd /c "<path>" --serve-remote` (rule 1207), both with a path that holds a space. Found 2026-11-13: the PC's sshd
  `DefaultShell` (`HKLM:\SOFTWARE\OpenSSH`) was PowerShell 7. PowerShell refuses the old `"<path>" --serve-remote`
  (`ParserError: Unexpected token 'serve-remote'`), and with `& "<path>"` it does not wait for the GUI exe, so sshd ends
  the session seconds after hello. `cmd /c` works under PowerShell (measured) and under cmd.
- [ ] Settings > Machines adds the PC with no path typed: the Stable install answers as
  `%LOCALAPPDATA%\StableFlare\current\FlareStable.exe` (not on the PATH), and its serve process relays to the Stable
  profile `%APPDATA%\FlareStable`, not the Dev copy's `%APPDATA%\Flare` (its log says which).
- [ ] Rule 1208 over real ssh: a PC account whose profile folder holds a space, with a PowerShell login shell, is added
  at `$env:LOCALAPPDATA\StableFlare\current\FlareStable.exe` and links (measured only by running the shells locally
  as sshd would, 2026-11-13).
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
- [ ] In that hidden Mac Flare's tab: `env | grep -E 'SSH_|TMPDIR'` shows no `SSH_CONNECTION` and a `/var/folders`
  `TMPDIR` (rule 1197); Claude started there signs in from the Keychain as in a Flare opened from the Dock (an ssh
  login is its own security session: if it cannot, note it).
- [ ] While the PC views the Mac, nothing new shows in the Mac's Dock or app switcher until a normal launch, and
  `lsappinfo list` names no second `flare` (the serve process is no app, rule 1196).
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
- [ ] A Mac clipboard picture that is only TIFF (Preview's Copy, Pages) arrives on the PC host as a PNG (rule 1198).

## Screens (PR 7)

- [ ] Settings > Machines on the PC: "+ Add a machine" lists the Mac's alias from `~/.ssh/config`; Add with a name links
  it (Flare found on the Mac's PATH, or at `/Applications/Flare.app/Contents/MacOS/flare`), the row shows the Apple
  logo, the Mac's Flare version and "Linked". The same on the Mac adding the PC (the Windows logo).
- [ ] With an older Flare on the other machine, Add shows one red line that says which side to update, and saves
  nothing; Test on a linked machine whose ssh is down shows "Not linked" and the reason as one grey line under its row (no tooltip on the status).
- [ ] The + menu (right-click on +, or the New tab arrow) lists the machine; its pick opens a shell tab in the other
  machine's home folder, with the logo on the tab (vertical: before the folder; horizontal: before the ×).
- [ ] "Hide here only" on that tab: gone here, still open on the other machine; Close on it ends it on both.
- [ ] A Claude session in a remote tab: type on the host's own Flare, and the viewer shows the conversation fitted to
  its window with "Live on <machine>"; a click or a key there brings the terminal back at the viewer's size, and the
  host's window gets its size back at the next key typed there.
- [ ] The host's own screen (rule 1211): with that Claude tab in front on the host, type in the viewer. The host's
  window shows the conversation fitted to its own width with "Live on <viewer's computer name>" above it, not the
  terminal at the viewer's size, and new answers appear in it within about 2 s. A key typed on the host, or a click on
  the view, brings the terminal back at the host window's size (the key reaches the shell), and the viewer then shows
  its own fitted view. A plain shell tab the viewer sizes keeps its terminal on the host.
- [ ] A size alone (rule 1212): type on the host last, then on the viewer only switch to that tab, resize the window,
  or drop and restore the link. The host keeps its live terminal (at the viewer's size); the view comes only once
  the viewer types.

## Link edge cases (rule 1210, 2026-11-13)

- [ ] A new remote tab opens with one ssh connect: the host's log shows one `--serve-remote` start per tab, even when
  the first `remote.attach` came before the new shell ran (it is asked again on the same link every 250 ms, up to
  10 s).
- [ ] Pull the network while a new remote tab is opening (between its open and the first output): when it is back, the
  host has one new tab, not two.
- [ ] Pull the network on a live remote tab, type `echo kept` (no Enter), plug it back within 30 s: `echo kept`
  appears at the host's prompt once the tab is back. Type for longer than 30 s while it is down: the tab shows one red
  line that some typing was not sent.

## Remote tab extras (rule 1213, 2026-11-13)

- [ ] Preview over ssh: in a remote tab run a dev server on the host (`npm run dev`, Vite on `localhost:5173`). The
  viewer shows the Preview chip; a click opens the side panel at `http://127.0.0.1:<port>/` with the host's page, and
  no console window flashes. A second click after the panel moved on uses the same tunnel (one `ssh -N` in the
  viewer's process list). `clear` in the tab, or closing it, ends that `ssh -N`.
- [ ] A host whose sshd does not forward (`AllowTcpForwarding no`): the click says in one line that the tunnel did not
  open, within 20 s, and the window never hangs.
- [ ] Recent folders: a remote tab opened with `flare open --on <machine> <folder>` (and one from the machine's row,
  once its shell reports its folder) is listed in the + menu with the machine's logo; its pick opens a remote tab
  there. Removing the machine in Settings > Machines takes its folders off the menu.
- [ ] Colours after a reconnect: with a coloured prompt or `ls --color` on the host's screen, drop the link until the
  output falls out of the ring (or restart the viewer): the restored tab shows the colours at once. Against a host on
  an older Flare the tab shows the text plain, as before.
- [ ] Folder drop: drop a project folder (with `.git`, `.env` and a link or junction inside) from Explorer or Finder on
  the remote tab: the host's quoted folder path is pasted, `ls -la` there shows the files with their names, dot files
  included, and no link. A folder over 200 MB or 5000 files copies nothing and says so in one line.

## Mac review fixes (rule 1215, 2026-11-13)

- [ ] ControlMaster: on a viewer whose `~/.ssh/config` has `ControlMaster auto`, `ControlPath ~/.ssh/cm-%r@%h:%p` and
  `ControlPersist 10m` for the host (and a link to it already open in a terminal), the Preview click opens the page,
  the `ssh -N` stays in the viewer's process list, and a second click uses the same tunnel instead of a new one.
- [ ] Package drop: drop `/System/Applications/Calculator.app` (or any `.app`) from Finder on a remote tab, and an
  `.app` from a PC viewer: the panel says in one line that packages are not copied, and nothing lands on the host.
- [ ] Exec bit: from a Mac viewer drop a folder holding `gradlew` (`chmod +x`) and `README`, and a lone `run.sh`
  (`chmod +x`), on a tab of the Mac host and of a Linux host if one is linked: `ls -l` there shows `gradlew` and
  `run.sh` runnable (`-rwxr-xr-x`) and `README` not; `./run.sh` runs. From the PC viewer the same files arrive
  not runnable. Against a host on an older Flare the files still land, not runnable.

## Machine pairing PR 1 (rules 1249 to 1251, 2026-11-14)

- [ ] The dialog on both machines: "+ Add a machine" opens the Add a machine dialog over the page, the drop-down lists
  the `~/.ssh/config` hosts in the code font, then "Type an address"; picking the other machine's alias and Add links
  it as before (row "Linked", its logo and version, the address column shows the alias).
- [ ] Type an address: on the PC pick "Type an address", type the Mac's IP address and the Mac's user name, Add: the
  machine is saved with `host` and `user` in `machines.json` (no `~/.ssh/config` entry needed when keys are already
  in place), and a tab opens on it from the + menu.
- [ ] Not answering: with the other machine asleep or its SSH server off, Test (or Add) answers "Not answering"
  within about 6 s (the 5 s connect timeout), not after 90 s.
- [ ] Not paired: with a key the other machine does not accept, the line reads "Not paired with this computer".
- [ ] Identity changed: change the host's key line in the viewer's known-hosts file (or point a machine's
  `known_hosts` at a file with another key): the line reads "Its identity changed. Pair again".
- [ ] Can't find it: a typed address that does not resolve (`no-such-host.local`) reads "Can't find it on the network".
- [ ] No SSH client (PC): with `C:\Windows\System32\OpenSSH\ssh.exe` missing (a test account or VM), the line reads
  "This PC is missing Windows' SSH client". Do not remove ssh from the everyday PC.
- [ ] Paired fields: a hand-written machine with `port`, `key` and `known_hosts` (a key path and a known-hosts path
  with a space in them) links over real ssh, and ssh asks nothing (`StrictHostKeyChecking=yes`, the key only).

## Machine pairing PR 2 (rules 1238 to 1241, 2026-11-14)

Run these on a test account or a machine you can put back; note what each machine had before (OpenSSH Server
installed or not, sshd running or not, Remote Login on or off).

- [ ] Windows, no SSH server: Computer sharing on shows one UAC prompt; after it (up to a minute while Windows adds
  the feature) the switch is on. `Get-Service sshd` is Running and Automatic; `(Get-NetFirewallRule -Name
  OpenSSH-Server-In-TCP).Profile` is `Private`; `Flare-Pairing-In` exists for Flare's exe, Private only;
  `%ProgramData%\ssh\sshd_config` has `PasswordAuthentication no` above the `Match Group administrators` block, and
  `ssh -o PreferredAuthentications=password localhost` is refused.
- [ ] Windows, SSH server already running: the switch is on after one UAC prompt; `sshd_config` and the sshd rule's
  profiles are as they were.
- [ ] Windows, SSH server installed but stopped (its rule on every profile): after the switch, sshd runs and
  `(Get-NetFirewallRule -Name OpenSSH-Server-In-TCP).Profile` is `Private`; `sshd_config` is as it was.
- [ ] Windows: a setup that fails after sshd started (for example a firewall policy that refuses new rules) leaves
  sshd Stopped with its old start type, and the switch off.
- [ ] Windows: No at the UAC prompt puts the switch back off, with no dialog.
- [ ] Windows, offline with no SSH server: the dialog says Windows could not add its SSH server, and the switch is off.
- [ ] Restart Flare with the switch on: it is still on, and no UAC prompt shows.
- [ ] Turn off on the machine where Flare turned sshd on: "Stop the SSH server too?"; Stop it shows one UAC prompt,
  then sshd is Stopped and Manual and `Flare-Pairing-In` is gone. Keep it leaves sshd running and shows no prompt.
- [ ] Mac with Remote Login on: the switch turns on with no prompt.
- [ ] Mac with Remote Login off: the password prompt shows; after OK, either the switch is on (Remote Login on in
  System Settings) or "Turn on Remote Login" shows. Open System Settings opens General > Sharing; turning Remote Login
  on there closes the dialog within about a second and the switch is on. Note the macOS version.
- [ ] Mac: Cancel at the password prompt puts the switch back off; Cancel in "Turn on Remote Login" too.
- [ ] Mac turn off with Stop it: Remote Login goes off (or the dialog says where to turn it off).
- [ ] Mac release build: the first launch after this PR shows no local network prompt yet (nothing browses until PR
  3); `plutil -p Flare.app/Contents/Info.plist` lists `NSLocalNetworkUsageDescription` and `NSBonjourServices`.