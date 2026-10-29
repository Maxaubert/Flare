# Install when idle: spec and plan (2026-10-29)

Status: draft for Max's approval. One PR in Flare.

## Intent

Max runs agent sessions in FlareStable all day. Installing a new build means closing Stable, which cuts off any
session that is mid-answer. Today the agent builds, then runs `install-copy.ps1 -Stable`, which asks Stable to close
and waits; that is slow and lands at a bad moment. Max asked (2026-10-29) for installs to happen only when no session
is working, and for the install itself to be as quick as possible.

## Goals

- The slow part (the build) happens before anything is asked. The install itself is only a close, a swap and a restart.
- Flare installs a ready build **on its own only once no agent in any tab is working or waiting on a question**.
- Max can always say "now" or "not today", and sees that a build is waiting.
- Sessions come back after the restart exactly as after a normal close (the existing resume).

## Non-goals

- No change to the Velopack update path (public releases, off until the repo is public).
- No building inside Flare. The agent builds; Flare only installs what was staged.
- No auto-install of anything Max did not ask for: staging still needs his yes (see the agent rule below).

## Anti-goals

- A restart while an agent is mid-answer or waiting on a permission prompt.
- A forced close that loses the session resume.
- A half-copied install (a mix of old and new files) or one that cannot start, with no way back.
- A staged folder anyone could drop files into to get them run (only builds from Max's own repo, in his own profile).

## How it works

1. **Stage (agent, after Max's yes).** A new `tools/stage-install.ps1 [-Stable]` copies the release files
   (`flare.exe`, `flare-mcp.exe`, `flare-parakeet-host.exe`, LICENSE, notices, `claude-plugin\`) into
   `<install>\staged\` (next to `current\`) and writes `staged.json` last: `{commit, built_at, files: {name: sha256}}`.
   It does not touch the running app. A second stage replaces the first.
2. **Flare notices** `staged.json` at start and when the folder changes (a directory watch), checks every hash, and
   shows a chip in the tab bar: "Build ready, installs when idle" with a menu: Install now, Install when idle (default),
   Not today (hides it until the next start), Discard.
3. **Idle rule.** "Idle" means: `working_count() == 0`, no agent tab shows a question, no dictation running, no
   install question open, and this has held for 60 s without a break. Any new working state resets the 60 s.
4. **Install.** On idle (or Install now), a 10 s toast "Installing the new build and restarting" with Cancel.
   Then Flare:
   - renames each file in `current\` it replaces to `<name>.old` (Windows allows that for a running exe),
     copies the staged file in and checks its hash;
   - on any failure, puts every `.old` back and says "The build could not be installed; nothing changed";
   - quits through the normal close path with the close question pre-answered (as the update gate does), and
     starts the new exe with the same `--user-data-dir` (`handoff::restart_args`).
5. **After the restart.** The new Flare deletes `staged\` and the `.old` files, and shows "Installed <commit>" once.
   If the new exe dies within 20 s of start twice, the next start runs the `.old` files instead (a simple
   last-good guard) and says so.

## The agent's rule (memory, already saved)

Always ask Max before installing. Build first, then ask "Stage the build for <copy>? It installs when your sessions
are idle." On a yes, run `stage-install.ps1`. "Install now" stays Max's click, or a separate ask.

## Plan

1. `tools/stage-install.ps1` and the `staged.json` format; unit tests for reading and hash-checking it (S)
2. The idle rule as a pure function over the agent hub state, with tests (S)
3. The chip, its menu and the toast (Slint), ui tests; words in `flare-update::copy` style (M)
4. The swap with rollback, the restart, the post-start cleanup and the last-good guard; tests with a temp install dir (M)
5. Offscreen e2e: a staged build in a temp install, a fake working agent holds it, idle installs and restarts (M)
6. Docs (`features.md`, `docs/shipping.md` notes), PR, merge question.

## Left to the builder

- Exact chip placement and words, following the update chip.
- Whether the plain Flare copy gets it too (yes if it is free; same code, other install folder).
