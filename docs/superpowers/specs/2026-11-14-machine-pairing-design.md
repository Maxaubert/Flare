# Machine pairing: link another computer without leaving Flare

Status: draft for Max's approval (2026-11-14). Research: `Documents\Claude\research\flare\2026-11-14-machine-pairing.md`.

## Why

Remote sessions work, but a new user cannot link a machine from inside Flare. Today they must, outside Flare:
turn on an SSH server, make an SSH key, copy it to the other machine (on a Windows admin account into
`C:\ProgramData\ssh\administrators_authorized_keys` with special permissions), add a host to `~/.ssh/config`,
and run `ssh <host>` once in a terminal to accept the host key. When any step is wrong, Flare shows one line,
"No Flare answered over SSH on <host>", because it throws ssh's own message away.

Goal (Max, 2026-11-14): Flare handles all of it. Accepting a system permission is fine. Going into settings to find
things or copying codes by hand is not.

## Goals

- Link a fresh pair of computers on the same network with: one switch on the machine to reach, one click on the
  other, and a 6-digit code typed once. Plus the system prompts the OS insists on.
- No file edits, no terminal, no `~/.ssh/config` needed. The user's own SSH files stay untouched, except the one
  line Flare adds to `authorized_keys` on the machine being reached.
- When a link fails, say what is wrong in plain words and offer the fix as a button.
- People who already use SSH keep working exactly as today (config hosts still listed and still work).

## Non-goals

- Reaching a machine outside the home network. That is issue #233 (Tailscale, jump host), later. Pairing is built
  so that a later "pair over Tailscale" only changes how the two machines find each other.
- Installing Flare on the other machine. The user installs it there; pairing starts once both have Flare.
- Replacing SSH with Flare's own connection. SSH stays the transport (decided in the remote sessions design).
- Linux hosts (they work through `~/.ssh/config` as today; no setup switch).

## Anti-goals (a failure even if it works)

- A pairing code that can be guessed by someone else on the network, or a link that anyone on the network can use.
- Flare silently weakening the machine: an SSH server open to every network, or password logins turned on.
- A switch that turns on, but the user then has to visit System Settings with no guidance and no auto-continue.

## The flow, as the user sees it

**Revised after Max's review (2026-11-14), this replaces the flow below.**
- Settings > Machines, section **This computer**: one switch row, **Computer sharing**, subtext "Allows you to control
  sessions from other devices" (fixed; it never changes to report state). Turning it on runs the OS setup (the system
  prompt; on the Mac the fallback dialog "Turn on Remote Login" with Open System Settings, which continues by itself).
  While it is on, the computer announces itself on the network.
- **+ Add a machine** opens a Flare dialog (the Dialog look): a Machine drop-down listing computers with sharing on
  (found on the network), then the SSH config hosts, then "Type an address". Picking a nearby computer makes that
  computer show a Flare dialog "<name> wants to connect" with a 6-digit code and Decline; the user types the code
  into "Code shown on <name>" and clicks Add. Wrong code: one red line "Wrong code" in the dialog.
- Both computers show the same **Linked machines** table (Machine, Address, Flare, Status, menu). Problems are the
  grey line under a row, as today ("Computer sharing is off there", "Not answering", "Not paired with this computer").
The older text below is kept for the setup and protocol details.

### On the machine to reach (the host)

Settings > Machines gets a new section at the top: **This computer**.

- A switch: **Let my other computers open tabs here**. Off by default.
- Turning it on runs the setup for that OS (below). When it is done, the section shows:
  "Ready. On your other computer, open Settings > Machines and add this one." and a **Pair** button.
- **Pair** shows a 6-digit code and the computer's name for 5 minutes ("Code for Max's PC: 482 913").
  The section also shows the address in small text (for the fallback, below).
- Turning the switch off removes the keys Flare added and, if Flare turned the SSH server on, offers to turn it off.

### On the other machine (the viewer)

**+ Add a machine** now shows two kinds of rows:

1. **Nearby**: computers on the network with the switch on, found automatically ("Max's PC, Windows, Flare 0.4").
2. **From your SSH config**: the aliases as today (for people who already use SSH).

Picking a nearby computer asks for the code shown on it. The user types it, clicks **Pair**, and the row turns
"Linked" with the logo and Flare version. Nothing else to do.

If nothing shows under Nearby (the network blocks discovery), **Enter an address** lets the user type the address
shown on the host and the code.

### Setup per OS (when the switch turns on)

**Windows host.** One UAC prompt ("Flare wants to make changes"). Flare runs one elevated script that:
installs the OpenSSH Server feature if missing (downloads from Windows Update, can take a minute, a progress line
shows), starts `sshd` and sets it to start automatically, makes sure the inbound firewall rule exists for the
**Private** network profile only, and adds a firewall rule for Flare's pairing port (Private only). It turns off
password logins in `sshd_config` only if Flare installed the server itself (a server the user already had is left
as it was). If the feature cannot download (offline, blocked by policy), the line says so.

**Mac host.** macOS does not let an app turn on Remote Login by itself. Flare:

1. Checks whether port 22 answers. If yes, the switch is on with no prompt.
2. Otherwise asks for the admin password (the standard macOS dialog) and runs `systemsetup -setremotelogin on`.
3. If macOS refuses because Flare lacks Full Disk Access (likely on recent macOS), Flare opens
   System Settings at Sharing and shows one line: "Turn on Remote Login, then come back." Flare watches port 22 and
   continues by itself the moment it opens. No hunting: the page opens at the right place.

(Asking for Full Disk Access just to flip one switch is a bigger permission than the toggle itself, so Flare does not
ask for it.)

## What happens underneath (pairing)

- Flare makes its own SSH key the first time it is needed: `id_flare_ed25519` in Flare's profile, not in `~/.ssh`.
- While **Pair** is showing a code, the host listens on a TCP port on the local network and announces itself with
  mDNS as `_flare-pair._tcp` (crate `mdns-sd`). Outside that window there is no listener and no announcement.
  The "found nearby" list on the viewer uses a lighter, always-on announcement only while the switch is on
  (name, OS, Flare version, no secrets).
- The viewer connects and both run **SPAKE2** (crate `spake2`, RFC 9382) with the 6-digit code. The code never
  crosses the network. A wrong code ends the attempt; after 3 wrong codes the window closes and a new code is
  needed. An attacker on the network gets 3 guesses in a million per code.
- Over the encrypted channel the viewer sends its public key and its computer name. The host adds the key to the
  right `authorized_keys` file with a `flare-pair:<viewer name>` comment (on a Windows admin account that file needs
  admin rights: a second UAC prompt at pairing time; a standard account needs none).
- The host replies with its SSH host key, its user name, its addresses and its Flare path. The viewer stores the host
  key in Flare's own known-hosts file, so ssh never asks "are you sure" and never trusts a stranger later.
- The viewer saves the machine with the new fields below, then links over SSH exactly as today.

## Changes to how Flare runs ssh

`machines.json` gains optional fields: `host` (address), `port`, `user`, `key` (Flare's key file) and
`known_hosts` (Flare's file). A paired machine runs:
`ssh -o BatchMode=yes -o IdentitiesOnly=yes -i <key> -o UserKnownHostsFile=<file> -o StrictHostKeyChecking=yes -p <port> -l <user> <host> ...`.
A machine from `~/.ssh/config` runs exactly as today. All values are still single argv items (rule 290/305 stay).

Add `-o ConnectTimeout=5` so a dead host does not keep Add spinning.

## Plain-words errors (also helps the ~/.ssh/config path)

Flare stops discarding ssh's stderr and sorts it into a few cases, each with one line and, where possible, a button:

| ssh says | Flare shows | Button |
|---|---|---|
| Connection refused / timed out | "Max's Mac doesn't answer. Is it on, and is 'Let my other computers open tabs here' on?" | Try again |
| Permission denied (publickey) | "Max's Mac doesn't know this computer yet." | Pair again |
| Host key verification failed / changed | "Max's Mac's identity changed since you paired. Pair again to trust it." | Pair again |
| Could not resolve hostname | "Can't find <host> on the network." | Enter an address |
| ssh not found (Windows client missing) | "This PC is missing Windows' SSH client." | Install it (UAC) |

## Permissions the user may see, all from the OS

- Windows host: UAC when turning the switch on; UAC again when pairing on an admin account.
- Windows: possibly the firewall "allow access" dialog the first time, if the rule could not be added.
- Mac: the admin password dialog, or the Sharing page opened at the right place; the "find devices on your local
  network" prompt the first time (Flare's Info.plist gets `NSLocalNetworkUsageDescription` and
  `NSBonjourServices = _flare-pair._tcp`).

## Delivery, in three PRs

1. **Errors and fields**: keep ssh's message, the plain-words table, `ConnectTimeout`, the new `machines.json`
   fields, and **Enter an address** (address + user) in the Add form. Useful on its own.
2. **This computer switch**: the Windows and Mac setup, the status line, turning it off cleanly.
3. **Pairing**: Flare's key, mDNS, the SPAKE2 exchange, the code UI on both sides, the known-hosts pin.

Each PR: rules in `docs/regression-rules.md`, unit tests for everything pure (the SPAKE2 exchange over an
in-memory pipe, error sorting, argv building), and UI tests for the page. The OS setup scripts are tested by dry run
(the script text and the elevation call), never run by tests. Real checks between Max's PC and Mac go in
`docs/remote-sessions-manual-checks.md`.

## Left to the builder

- Exact copy of the lines and buttons (keep them short, no tooltips on labelled controls).
- Port numbers (pairing port picked at random, sent in the mDNS record).
- How "nearby" rows look while being found (a spinner or a quiet "Looking..." line).

## Open questions for Max

None blocking. Two calls I made, which Max may overrule:
- A code to **type** (not two numbers to compare): it works when the two screens are in different rooms.
- Flare does **not** ask for Full Disk Access on the Mac; it opens Sharing at the right place instead.
