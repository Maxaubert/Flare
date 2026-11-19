# Remote sessions show only on the machine that opened them

Status: draft for Max's approval (2026-11-19). Plan: `docs/superpowers/plans/2026-11-19-remote-session-on-opener.md`.
Replaces: `docs/superpowers/specs/2026-11-18-import-remote-sessions-design.md` (imports). Its PRs #281, #282, #284 and
#286 were closed on 2026-11-19. Parts reused from them are named where they are used.

Builds on: `docs/superpowers/specs/2026-11-12-remote-sessions-design.md` (remote tabs) and
`docs/superpowers/specs/2026-11-14-machine-pairing-design.md` (linking machines).

## Intent

Max's words (2026-11-19): "from now on you can open a session on another machine but its only ever on one machine, no
imports since that gets messy. on the machine its running on, it will be in the background, you wont see it, only on
the machine that started it remotely."

Why: imports made one session show on two screens. Two screens meant two sizes, two rings, a fitted conversation
view on each side, and many edge cases (closed PRs #281 to #286). One session on one screen removes all of that.

Words used here:
- **Opener**: the machine whose Flare opened the session (+ menu, machine picker, or an agent's `open.remote`).
- **Host**: the machine that runs the session.
- **Hidden session**: a tab the host runs for an opener. It has a shell and an agent like any tab, but the host's
  window never shows it.

## Goals

- A session opened from another machine runs on the host as a hidden session. The host shows no tab, no sidebar row,
  no conversation view, no mark, no badge and no ring for it.
- The opener shows it as a remote tab, as today.
- Only one machine ever shows a session. Nothing is imported.
- A hidden session survives a link drop, the opener's quit or restart, and a Flare restart on the host. The opener
  finds it again on its own.
- On the host, Settings > Machines lists the hidden sessions, grouped by opener, with **Show here** and **End** per
  session (Max chose this, per the brief).
- A question in a hidden session rings only on the opener (Max chose this, per the brief).

## Non-goals

- Imports, views, a view switcher, or any list of another machine's sessions on the opener.
- The fitted conversation view for remote tabs (rules 1193, 1211, 1212). Only one screen shows a session, so the
  other screen needs no view. Removed in PR 3.
- Moving a session from the opener's screen to the host's screen without a click on the host. Show here is the only
  way.
- Panels, the browser panel or notice lines of a hidden session's agent shown on the opener. See "Agents and plugins".
- A new network listener or relay. SSH stays the only transport.

## Anti-goals (a failure even if it works)

- A hidden session shows up in the host's strip, sidebar, Ctrl+Tab walk, Ctrl+1..9, taskbar badge or notifications.
- A question rings on the host, or rings on neither machine once the opener is back.
- The host's window refits, resizes or redraws a hidden session at the host's window size while it is hidden.
- The opener's Close ends a session with an ask on the host's screen, where nobody sits.
- A Flare update on the host silently ends every remote tab on the opener.
- A third machine attaches to a hidden session or to the host's own tabs through any UI.
- Show here leaves the session on both screens.

## What happens today (read on main 5a078137, 2026-11-19)

- `remote.open` on the host (`appctl/remote_glue.rs`, `CtlOp::RemoteOpen`) calls `model.open_tab(folder)`. That is a
  normal tab in the host's strip, spawned with the resume id, with marks, ring and badge as any tab.
- While the opener follows the tab, rule 295 keeps the host's notifier quiet for it (`RemoteHost::is_followed`). When
  the opener quits or its link drops, nobody follows it, so the host rings again.
- Rule 296 holds the host awake only while a followed tab works.
- When the last follower goes, the size goes back to the host window (`CtlOp::RemoteRelease`,
  `TabHost::take_size_back`).
- `remote.close` calls `request_close`: the rule 48 ask shows in the host's window. The opener asks first as well,
  but only after its hub saw the remote tab working or asking (see the next point).
- `remote.status` and `remote.tabs` list every shell tab of the host, its viewer tabs too. `remote.attach`, `input`
  and `resize` check only `is_shell_tab`, so a viewer tab can be followed (the mirror chain closed PR 1 found).
- The opener asks before a Close only once its hub has seen the remote tab working or asking: `remote_mark` puts the
  tab in `agent_ids` only for `working` and `question`, and `start_remote` sets no agent in the model. So an idle
  Claude tab restored on the opener closes there with no ask. The host asks instead: its own `agent_present` knows the
  Claude tab.
- **Host restart.** The host's `tabs.json` saves the tab as a normal local tab. The restore makes new tab ids
  (`TabModel::settle`, `make_id`). The tab comes back visible on the host, with its Claude session resumed (rule
  1286). The opener's saved remote tab attaches with the old `host_tab`, gets `no_tab` and ends as a shell that exits
  (`remote_link.rs`, `Worker::attach`). So today the session survives on the host but the opener loses it.
- `open_id` (rule 1210) is made once per link worker and is not saved on either side.

## Behaviour per case

### Open

1. The person picks a machine in the + menu (or an agent calls `open.remote`, or `flare open --on`). Nothing changes on
   the opener: a remote tab opens as today, in front for a person and behind the front tab for an agent (rule 299).
2. The opener's link sends `remote.open {folder, resume, open_id}` as today. The `open_id` is now made when the tab
   is made and saved in `tabs.json` with the tab (it was per worker).
3. The host makes a hidden session, never a strip tab. It keeps with it: the opener's name, the `open_id`, and the
   time it was opened. The opener's name is the machine name the link's `remote.hello` gave (`ctl_app::viewer_of`,
   today passed only to `RemoteResize`). An older bridge names no machine: the name is "another computer".
4. The hidden session is sized by the opener from birth (`SizeOwner::Remote`). The host's refits never touch it.
   `remote.status` says `sized: viewer` for it.
5. The opener attaches, sends its size, takes a snapshot and polls status, as today.

### Show on the opener

The opener shows the tab as any remote tab today: live terminal at its own size, its marks from the host's status
(rule 294), its question rung here (rule 295), Preview chip and file copies as today (rules 303, 304, 1213).

### Close on the opener

- The opener asks first while an agent is there (rule 48). New in PR 1: the opener's `agent_present` is also true when
  the host's last status names an agent (`kind` not null), idle or not. Today it is true only after the hub saw the tab
  working or asking, so an idle Claude tab would close with no ask on either machine once the host stops asking.
- The host closes a hidden session at once on `remote.close`. It never shows the rule 48 ask for a hidden session,
  since nobody sees the host's screen for it.
- Ctrl+Shift+T on the opener reopens it as today: a new remote tab in that folder on that machine (rule 1236), which
  is a new hidden session.

### Opener quits, restarts, or its link drops

- The quit and a dropped link only detach (rule 288). The hidden session runs on.
- The hidden session keeps the opener's last size. The last follower going no longer gives the size to the host
  window for a hidden session.
- A question meanwhile does not ring on the host. It shows on the opener when the link is back: the first status after
  an attach is always news (rule 294), so the opener raises the mark and rings once (rule 206).
- A working hidden session holds the host awake, followed or not (amends rule 296). Nobody on the host can see it, and
  the opener cannot reach a sleeping host.
- The opener's restore attaches to `host_tab` from `tabs.json` (rule 288), as today.

### Host Flare restart (quit, update, crash, reboot)

- The host's quit ask (`busy_agents`) and the install hold count working hidden sessions. Their row names the opener:
  "<folder> (for <opener>)". So an update never silently ends one mid-answer.
- The host's `tabs.json` saves each hidden session with `hidden: {opener, open_id}` beside the usual fields (session id
  as rule 1286). An older Flare reads the unknown key as a local tab (rule 40), so a downgrade shows them in the strip.
- The restore brings them back hidden: new tab ids as for every tab, the Claude session resumed (rule 1286), a plain
  shell started fresh in its folder.
- The opener's link gets `no_tab` for the old id. Before it ends the tab, it sends one `remote.find {open_id}`. The
  host answers `{tab}` when a hidden session holds that open id. The opener then sets the new `host_tab`, takes a
  fresh snapshot and goes on. Its kept typing is dropped, as on any `no_tab` (rule 1210), and the tab shows one line:
  "Flare restarted on <machine>. The session goes on."
- An older host answers `unknown_method`: the tab ends as today.
- A host restart while the opener is away is the same: the opener's next attach finds it.

### Link down

As today: the opener reconnects with backoff (1 s, doubling to 30 s) and keeps typing for 30 s (rule 1210). The host
does nothing new.

### Show here (on the host)

- Settings > Machines on the host lists hidden sessions (see "Settings"). **Show here** turns one into a normal local
  tab: it joins the end of the strip and comes to the front. The window takes its size (`take_size_back`). From then
  on it rings, marks and saves as any local tab.
- Every follower is dropped at once (`RemoteHost::forget_tab`). The opener's next call gets `no_tab`. Its
  `remote.find` answers `{shown: true}` (the host keeps the open id on the tab in memory). The opener ends the tab with
  one red line: "This session now shows on <machine>." Kept typing is dropped.
- No ask: the session goes on, only its screen moves.

### End (on the host)

- **End** on a row closes the hidden session with the usual ask (rule 48) while an agent works, shown in the host's
  window, since the person is there now.
- The opener's next call gets `no_tab`. `remote.find` answers `no_tab`. The opener ends the tab with one red line:
  "This session was ended on <machine>."
- End does not add the session to the host's Ctrl+Shift+T list. It never showed there.

### Questions

- A hidden session's question rings only on the opener. The host's notifier, sounds, taskbar badge and attention
  count leave hidden sessions out, followed or not (amends rule 295).
- The host's hub still tracks the question, its words and the state word, because `remote.status` answers from them.
- After Show here the session is a local tab and rings on the host only.

### Agents

- `open.remote`, the MCP tool `open_remote_session` and `flare open --on` work as today (rule 299). The session is
  hidden on the host.
- No list tool. `list_remote_sessions` and `open.remote {tab}` from closed PR #286 are not built.
- An agent inside a hidden session talks to the host's Flare with its own tab token, as today. `tabs.list` gives it its
  own tab, as for any agent.
- Calls from a hidden session's agent that would show something on the host's screen (panels, `show_*` tools, the
  browser panel, `notice.show`, tab badge and progress) answer `unavailable` with "this session shows on <opener>;
  its panels cannot show there yet". The agent can then say it in words. Nothing from a hidden tab may raise, focus or
  front the host window or change its front tab.

### Plugins

- A plugin on the host with `tabs` does not see hidden sessions: `tabs.list` (`tab_facts`) and the `tabs` and `agent`
  events leave them out. They are not the host's tabs to show.
- A plugin on the opener sees the remote tab as today.
- Dictation on the host never routes words to a hidden session.

## No mirror chains, and one screen per session

Reused from closed PR #281 (draft rule 1289), made stricter:

- `remote.status` and `remote.tabs` list only hidden sessions. The host's own visible tabs and its viewer tabs (remote
  tabs it shows) are never listed. The opener needs only its own entry; it finds it by tab id.
- Every `remote.*` op that names a tab (`attach`, `input`, `resize`, `release`, `close`, and through follows `output`,
  `snapshot`, `fetch`, `put`, `convo`) answers `no_tab` for a tab that is not a hidden session. So:
  - a viewer tab on the host is never followed from a third machine (no mirror chains);
  - a session started on a machine shows only on that machine, as Max asked;
  - after Show here the opener's link ends by itself.
- Any linked machine may attach to any hidden session if it knows the id. No UI offers that: the opener knows only its
  own ids. Tying a session to the opener's name was rejected: a Mac's host name can change (rule 1199), which would
  strand the session.

## Settings > Machines on the host

- A new section under the machine table, shown only while hidden sessions run: one group per opener name, headed
  "N sessions running for <opener>". Each row: the folder name, the agent's state word (working, question, idle), and
  "open there now" or "not connected" (whether a link follows it).
- Two labelled buttons per row: **Show here** and **End**. Labelled, so no tooltips (rule 1201).
- The rows update when a session starts, ends or changes state.
- A mockup and Max's yes come before the code (global rule). See the plan, PR 2.

## Kept, amended and retired rules

| Rule | Today | After |
|---|---|---|
| 279, 281, 283, 284, 285, 286, 289, 290, 291, 292, 293, 300 to 305 | Remote transport, typing, snapshot, ssh | Kept as they are. |
| 280 | Detach or dead link never kills; last follower gives the size back | Kept, but a hidden session keeps its size (new 1302). |
| 282 | Remote resize owns the size until a local key | Kept. A hidden session is remote-owned from birth; Show here takes it back. |
| 287, 288 | Remote tab model, quit detaches, restore attaches | Kept. Amended by 1302 (the host saves and restores hidden sessions hidden) and 1306 (the opener saves `open_id` and finds its session after a host restart). |
| 294, 298 | Status words, refusals pass | Kept. `remote.status` lists only hidden sessions (1303). |
| 295 | One ring across both machines while followed | Amended by 1304: a hidden session never rings on the host, followed or not. |
| 296 | Awake while a followed tab works | Amended by 1302: a hidden session that works holds the host awake, followed or not. |
| 297 | Conversation entries cross as numbered entries | Retired in PR 3 with its only users, the fitted views. |
| 299 | An agent may open a remote session | Kept. |
| 1192 | Close ends everywhere; Hide here only detaches | Close kept (the ask moves to the opener only, 1305). Hide here only removed in PR 3 if Max agrees (it would leave a session nobody sees). |
| 1193 | Viewer's fitted conversation view while the host sizes | Retired in PR 3. The host never sizes a hidden session, so the view never stands. |
| 1204, 1210 | Start races, open id, kept typing | Kept. 1210's open id is now saved per tab (1306). |
| 1211, 1212 | Host's own fitted view while a viewer sizes | Retired in PR 3. A hidden session is never in the host's front. |
| 1213, 1279 | Preview tunnel, folder from status | Kept. |
| Draft 1289 (closed #281) | Mirror chains, `ring: both` | The refusal part lives on in 1303. `ring: both` and `origin` are dropped. |

## Mixed versions and old tabs

| Case | What happens |
|---|---|
| New opener, older host | The host shows the session in its strip as today. No `remote.find`: a host restart ends the tab as today. After PR 3 the opener has no fitted view, so a key typed on the host squeezes the opener's terminal. Accepted: Max updates both machines. |
| Older opener, new host | The session is hidden on the host. The older opener works as today. Its "Hide here only" leaves a hidden session that only Settings on the host can show or end. |
| Remote tabs opened before the update | On the host they are visible local tabs (nothing marks them). After PR 1 the host answers `no_tab` for them, so the opener's tab ends once, with no line (it has no saved `open_id`). The session stays on the host's strip. A host update restarts the host's Flare anyway, which ends these tabs today too. |

## Protocol changes (`REMOTE_PROTOCOL` stays the same)

| Where | Change | Older peer |
|---|---|---|
| `remote.open` | Makes a hidden session. Same params. | An older opener gets the same answer. |
| `remote.status`, `remote.tabs` | List hidden sessions only; a hidden session says `sized: viewer` | Older opener reads its own entry by id, as before. |
| Every op that names a tab | `no_tab` for a tab that is not a hidden session | Older opener ends the tab, as on any `no_tab`. |
| `remote.close` | Closes a hidden session at once (no host ask) | An older opener asks only if it saw the tab working or asking, so an idle Claude tab closes unasked. Accepted: Max updates both. |
| `remote.find {open_id}` | New method in `remote`. Answers `{tab}`, `{shown: true}` or `no_tab`. Relayed by the bridge. | Older host: `unknown_method`, the tab ends as today. |

## Security notes

- A paired peer has SSH to the account, so it could already run anything there. After this change the remote grant
  reaches less: no host tab that is not a hidden session, no viewer tab. `remote.find` answers only a tab id for an
  open id the caller already holds. No grant change.
- An agent gains nothing. The agent on the opener opens sessions as before (rule 299). An agent in a hidden session
  loses its panels while hidden.
- A plugin on the host sees less than today: no hidden session in `tabs.list`.

## Failure modes

| What happens | What Flare does |
|---|---|
| Opener off for days | The hidden session runs on. Settings on the host lists it as "not connected". Show here or End are the way out. |
| Opener's host name changed | Nothing breaks: the opener finds its tab by id and open id, not by name. Settings shows the old name until it reattaches; the name is updated at the next attach. |
| Host restart while a hidden session works | The quit ask names it. After the restart the Claude session resumes (rule 1286); the opener finds it (1306). |
| Host crash | Same as a restart, without the ask. The session saved last is resumed. |
| Show here while the opener types | The opener's tab ends at its next call with its line. Typing kept for the outage is dropped (rule 1210). |
| End while the opener is away | The opener's restore gets `no_tab`, then `remote.find` says `no_tab`: the tab ends with the "ended" line. |
| Host's Flare started hidden by ssh (rule 301) with only hidden sessions | The window has no tab of its own. Its first raise opens as Flare does with an empty strip. Checked by hand. |
| `remote.find` refused for another reason (busy, timeout) | The opener tries once more on its next reconnect, then ends the tab with the "ended" line. |
| Agent in a hidden session opens a panel | `unavailable` with the message above; nothing shows on either screen. |

## Test plan

All headless. No real ssh. No visible window (global rule, 2026-10-28). Per PR in the plan.

## Open checks for Max's by-hand test (PC and Mac)

1. On the PC, open a Claude session on the Mac. The Mac's strip and sidebar show nothing new. The PC shows the tab.
2. Ask Claude a question that waits for an answer. Only the PC rings. The Mac shows no badge and no notification.
3. Quit Flare on the PC while the Mac session works. The Mac does not ring when it asks. Start the PC again: the tab
   comes back and rings once for the question.
4. Leave the Mac idle while the hidden session works with the PC quit. Check the Mac does not sleep, and does sleep
   once the work ends.
5. Quit Flare on the Mac (the quit ask names the session "for <PC>"), start it again. The PC tab shows the line
   "Flare restarted on Mac Mini" and goes on in the same conversation.
6. Update Flare on the Mac with a hidden session open. Same as 5.
7. On the Mac, Settings > Machines lists "1 session running for <PC>". Click Show here: the tab appears on the Mac in
   front, at the Mac's size; the PC tab ends with "This session now shows on Mac Mini."
8. Open another one and click End on the Mac while Claude works: the ask shows on the Mac; after yes the PC tab ends
   with "This session was ended on Mac Mini."
9. Close a remote tab on the PC while Claude works: the PC asks, the Mac never shows an ask.
10. Ask Claude in a hidden session to show an image: it says it cannot show panels there. The Mac shows nothing.
11. The same from the Mac to the PC (both directions).
