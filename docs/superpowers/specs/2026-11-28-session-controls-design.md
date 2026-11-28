# Claude session controls: end, start and restart Claude in all tabs (design)

Date: 2026-11-28. Owner request (Max): "some way for me to close all sessions without closing the tabs ... in one
place", then "there's also a start claude session in all tabs, and maybe other stuff too". He chose a Flare menu
item over an MCP command, and said to skip the mockup and reuse the existing menu rows and dialog styles.

## Where the rows live

Flare has no in-window app menu on Windows (`ui_tests/app.rs` holds "no app menu"), so the app-wide rows go in the
menu every tab already has: a tab's right-click menu.

- **This tab** (shell tabs only, after Copy folder path): "End Claude session" while Claude runs in the tab, or
  "Start Claude" while the shell is at its prompt with no agent. Neither row when neither holds.
- **Claude sessions** (a flyout, in its own group, on every tab's menu, Settings included), one level, the rows that
  have something to act on, each with its count:
  - "End all Claude sessions (N)"
  - "Start Claude in all tabs (N)"
  - "Restart all Claude sessions (N)"
  A row with N = 0 is left out; with all three out, the flyout is left out too. Nothing ever acts on zero tabs.

The macOS menu bar (`mac_menu.rs`) is not changed in this PR.

## Which tabs each row touches

A tab counts only when all of these hold: it is a shell tab of this window (not hidden, not remote), its shell is
live, and the process poll has answered for it at least once.

| Row | Tabs |
| --- | --- |
| End | Claude (`DetectedAgent::Claude`) is here. Codex and agy are never touched. |
| Start | No agent is here (none detected and none armed) and the shell is at its prompt: a prompt report (OSC 9;9 or OSC 7) came after the last key typed in the tab. |
| Restart | The End tabs. |

Remote tabs are left out of all three: their agent state is the host's words over the link, and the host shell's
prompt cannot be read here. Hidden sessions (run for another machine) are never counted.

## What is typed

All typing goes through one new typed intent, `TermHandle::session_key(SessionKey)` (rule 2's pattern: no free text
reaches a shell). Its words are fixed:

| Intent | Bytes |
| --- | --- |
| `SessionKey::Exit` | `/exit` |
| `SessionKey::Enter` | `\r` (300 ms after `/exit`, `SEND_ENTER_MS`, the tab checked again first) |
| `SessionKey::Interrupt` | `ESC` (Claude Code's own "esc to interrupt") |
| `SessionKey::Start { shell, resume }` | the start line of `flare_shells::resume::typed_start_line` plus `\r` |

The start line: `claude --resume <id>` when the tab's last Claude session is known and is a session id (a UUID,
`is_session_id`), else `claude`. In cmd the line is wrapped in the same `NoDefaultCurrentDirectoryInExePath` guard
the restore uses, so a `claude.cmd` in the project folder never runs. Flare has no configured agent command
(searched 2026-11-28: the restore and `claude:new` both use a plain `claude`), so none is read.

The last session: the hub keeps each tab's last Claude session id after the agent leaves (`last_sessions`), taken
from the turn's hooks (`+sid`) or the restore's resume id. `claude_session()` is unchanged.

## Ending one tab

A per-tab job, stepped on every drained batch of terminal events and on the controller's timer:

1. **Wait for idle.** Idle: Claude here, not working, no question up (the hub's question mark or a question box on
   the screen, `looks_like_question`). A question box is busy: an Enter there would answer it.
2. **Draft check.** If Claude's prompt box holds unsent text (`prompt_draft`), the tab is left alone: "/exit" would be
   appended to the draft and the Enter would send it. The tab is reported as kept.
3. **Type** `/exit`, then after 300 ms check the tab again (still open, still idle, still Claude) and press Enter.
4. **Gone.** The job ends when a new prompt report arrives after the Enter (the shell is back at its prompt), or after
   15 s (logged, given up). The poll is asked to look now.

The tab and its shell stay open in every case. Flare never kills the shell or closes the tab here.

## Busy agents: one question

When any End or Restart tab is busy (working or with a question up), one dialog asks first, in the dialog's existing
style:

- Title: "1 session is working" / "N sessions are working".
- Body: "End now stops their work first. Wait ends each one as soon as it is idle."
- Buttons: Cancel, Wait, End now. Wait is the default (it loses nothing).

Cancel does nothing at all, the idle tabs included. Wait starts every job; busy ones wait for idle with no time limit
(a closed tab or a new run drops the job). End now sends `SessionKey::Interrupt` to each busy tab once, then waits for
idle (60 s limit), then runs the draft check (an interrupted turn may put its prompt back in the box: then the tab is
kept) and ends as above. When no tab is busy, no dialog shows.

The dialog gets an optional third button (`alt-label`, empty for every other question, so they are unchanged), and
the new question takes its turn with the others: downloads and app-link questions wait while it is up.

## Starting one tab

At the prompt with no agent: `SessionKey::Start` with the tab's shell and last session. Nothing waits after it; the
poll is asked to look soon.

## Restart

End as above, and when the shell is back at its prompt (a prompt report after the Enter), type the start line with
the session id captured before the end. A tab whose end was given up, or kept for a draft, is not started.

## Out of scope / not decided here

- The macOS menu bar.
- Remote tabs (see above).
- Real Claude keystroke behaviour (`/exit`, ESC, `--resume`) is not provable headless; the tests drive the controller
  with fake shells and hook signals.
