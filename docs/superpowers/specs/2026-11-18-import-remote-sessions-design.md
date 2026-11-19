# Import remote sessions: tabs started on another machine show up here

Status: draft for Max's approval (2026-11-18). Max's answers of 2026-11-18 are applied. Plan: `docs/superpowers/plans/2026-11-18-import-remote-sessions.md`.
Research: `Documents\Claude\research\flare\2026-11-18-views-research.md` (read-only check of origin/main 91bf9650).
The "views" idea in that brief is dropped. This spec replaces it.

Builds on: `docs/superpowers/specs/2026-11-12-remote-sessions-design.md` (remote tabs) and
`docs/superpowers/specs/2026-11-14-machine-pairing-design.md` (linking machines).

## Intent

Max works on a PC and a Mac Mini. Both run Flare and are linked. Today a tab opened in Flare on the Mac never shows
on the PC. Only tabs opened from the PC (local tabs, and remote tabs opened from here) show on the PC.

Max wants one setting. When it is on, the sessions started in Flare on his other machines also show here, as remote
tabs. When it is off, Flare works as today.

Why: he moves between the two desks. He wants to pick up a Claude session from either screen without reopening it
by hand.

## Goals

- One switch, "Import sessions started on other machines", on Settings > Machines. On by default.
- With it on: every session that Flare on a linked machine runs as its own tab shows here as a remote tab row
  within one poll (about 3 s).
- A row costs no session link until it is shown. It attaches (never resumes) when it comes to the front or is
  clicked.
- Close and "Hide here only" work as they do on remote tabs today. A hidden tab stays hidden.
- Marks (working, question, failed, done) and badges show on imported rows as on followed remote tabs. A question
  on an imported row rings on both machines: on the host, and here, attached or not (Max, 2026-11-18). It never
  rings twice on the same machine.
- An agent may list the sessions on linked machines and add one here, but never type into it or close it.
- Works for any number of linked machines. Tested with two.

## Non-goals

- Views, a view switcher, or a filtered tab strip (dropped, 2026-11-18).
- Importing sessions that are not Flare tabs on the host (a Claude run in another terminal app).
- Importing from machines that are not in `machines.json`.
- Mirroring imported tabs back. The host never lists its own remote (viewer) tabs.
- Saving imported tabs in `tabs.json`. They come back from the first poll after a start.
- A new network listener or relay. SSH stays the only transport.

## Anti-goals (a failure even if it works)

- The setting on, and Flare starts a hidden Flare on every linked machine just to look.
- Fronting an imported tab on the PC resizes the terminal Max is typing in on the Mac.
- A question on an imported row rings twice on the same machine, or on neither machine.
- Closing an imported row by mistake ends the Mac session without the usual ask.
- A hidden tab comes back on the next poll.
- A tab followed from here also shows as a second, imported row.
- The agent gains a way to type into, read, or close a session on another machine.

## Max's decisions (2026-11-18, AskUserQuestion)

| Topic | Decision |
|---|---|
| Views | No views. One setting: "Import sessions started on other machines". |
| Default | On. Confirmed by Max (2026-11-18). |
| On | Sessions started in Flare on another linked machine appear here as remote tabs. They show as rows at once and connect (attach, never resume) only when shown in front or clicked. |
| Off | Today's behaviour: only sessions started from this machine (local tabs and remote tabs opened from here). |
| Close an imported tab | Same as remote tabs today. Close ends it on the host with the usual ask (rule 48). "Hide here only" detaches and leaves it running. |
| Hidden tabs | Not re-imported on the next poll. Hidden host tabs are remembered per machine. |
| Agents | "Yes, list and open." Agents may list and open sessions on other machines. Confirmed by Max (2026-11-18). |
| Agent scope | As planned (Max, 2026-11-18). List the Flare tabs of all linked machines: title, folder, state word, agent kind, shown here. Add an imported row behind the front tab, never in front. Never type, read, close, hide, attach or resize. `front` with `tab` stays refused. |
| Questions | An imported session's question rings on both machines once imported (Max, 2026-11-18). The host keeps ringing. This PC rings too, attached or not. No machine rings twice for one question. |
| Plugins | A plugin approved with `tabs` sees imported rows like local tabs, including the host's Claude session ids. Max: "Yes, show all" (2026-11-18). |
| Mockups | None. Just a toggle. |

Max's own words in the request: "have it just be a setting to automatically import sessions started on other
machines, if it's not enabled you only see ones you start from this machine".

## Builder's calls (Claude's, can be overruled)

1. **No mirror chains.** The host never lists its own remote (viewer) tabs in `remote.status` or `remote.tabs`, and
   answers `no_tab` to every `remote.*` call that names one (attach, input, resize, close and the rest that reach
   `ctl_remote`). Today `remote.input` and `remote.resize` check only `is_shell_tab`, which is true for a viewer tab.
2. **One row per host tab.** Rows are keyed by (machine id, host tab id). A tab already shown here (opened from here,
   restored, or imported) is never imported again.
3. **Polling.** One light "watch link" per linked machine (named in `machines.json`), only while the setting is on
   or an agent's adopted row of that machine is in the strip. It asks `remote.status`
   every 3 s. When the machine does not answer, it waits longer each time (3 s, 6 s, 12 s, up to 60 s).
   Deviation from the task text: it polls `remote.status`, not `remote.tabs`. `remote.status` already carries the
   state word, agent kind and question words that the marks need, so one call does both jobs.
4. **The watch link never starts Flare on the host.** It runs `--serve-remote --no-start`. A host with no Flare
   running has no sessions to import, and stays as it is.
5. **Questions ring on both machines (Max's decision, 2026-11-18).** An imported row, attached or not, shows the
   host's marks (strip, sidebar, taskbar badge), and its question rings here too. The host keeps ringing as well.
   Rule 295 does not move the ring for an imported row: the attach asks the host to keep its own ring (see "Ring on
   both machines" below). Each machine rings once per question episode (rule 206). An attach does not ring a
   question here again when the watcher's status already rang it. A remote tab opened from here keeps rule 295 as
   today (one ring, here).
6. **No resize on attach.** An imported tab attaches in the "host sizes it" state of rule 1193. A Claude tab shows
   its fitted conversation view. Its first click or key here takes the terminal (`retake_size`, as today), and only
   then is its size sent. Window refits, tab switches and reconnects send no size while the host sizes it. A plain
   shell tab (no Claude session, so no conversation view) sends its size on attach as a followed tab does today. That
   resizes the host's shell when the row comes to the front here (a click, a key jump, or a close that fronts its
   neighbour), and the host window takes the size back on its next key (rule 282). Ctrl+Tab passing over a row (in
   either switch mode) does not attach it (see "Attach on show").
7. **A session that ends on the host removes its row.** An unattached row goes on the next good poll that does not
   list it. An attached row ends as a remote tab does today when the host tab goes (`no_tab`).
8. **N machines.** The import keeps state per machine. Tested with two (PC and Mac, both directions).
9. **What counts as a session.** Every shell tab of the host's own (plain shells too), not only Claude tabs. Same as
   what a person sees in the host's strip.
10. **Where rows go.** At the end of the strip, in the host's order, behind the front tab. An import never changes the
    front tab.
11. **Not saved.** Imported rows are not written to `tabs.json` (the restore attaches every saved remote tab at
    once, rule 288). After a restart the first poll brings them back. An agent's adopt (below) makes the same kind
    of unattached row, also not saved.
12. **A closed row does not come back while the host asks.** Close removes the row here at once, but the host tab
    lives until its ask (rule 48) is answered. (machine, host tab) goes on a short "closing" list. The import skips
    it until the host stops listing it, or for 2 minutes. If the person on the host answers "keep", the tab comes
    back after that.
13. **Turning the setting off.** Unattached imported rows go at once. Attached imported rows let go of the host
    (detach, as "Hide here only", never `remote.close`) and leave. If one was in front, the most recently used other
    tab comes to the front. Nothing is added to the hidden list. An agent's adopted rows stay.

## Findings (read-only check, 2026-11-18)

- Today `open_remote` (`crates/flare/src/ctl_app.rs` ~642) accepts `front: true` from any agent. Rule 299 says only
  `flare open` fronts a remote tab, but the code does not check the caller. This feature does not change that. It
  only refuses `front` together with the new `tab`. Listed as a follow-up.

## How it works

### Host side (the machine that runs the session)

- `remote.status` and `remote.tabs` answer only the host's own shell tabs (`remote.is_none()`). Each entry gains
  `"origin": "here"`. A viewer imports only entries that carry it. An older host has no `origin`, so nothing is
  imported from it (one log line per link, no error shown).
- Every `remote.*` op in `ctl_remote` that names a tab (attach, input, resize, release, close) answers `no_tab` when
  that tab is itself a remote tab here (`model.remote_of(tab).is_some()`). The rest answer only for a followed tab:
  `remote.output` and `remote.snapshot` through `host.followed`, `remote.convo` and `remote.fetch` through
  `is_followed` (checked 2026-11-18; `remote.put` to check in PR 1). A viewer tab can no longer become followed.
- Polling changes nothing on the host: it does not follow, size, notify differently or hold the host awake.
- **Ring on both machines.** `remote.attach` gains one optional field, `ring: "both"`. An imported row's attach sends
  it. The host's notifier then keeps notifying for that tab as if nobody followed it, while every follower of the tab
  asked for `both`. A follower without it (a tab opened from here) keeps rule 295 as today: the host stays quiet. An
  older host ignores the field, so after attach the question rings only here (rule 295 as before). Accepted.
- No new method, no new namespace and no grant change on the host. The only new param is the optional `ring` above.
  The remote grant reaches what it reached before.

### Viewer side (this Flare)

- **Watcher.** `crates/flare/src/remote_import.rs` (new). One worker per machine named in `machines.json`, while the
  setting is on, or while an agent's adopted row of that machine is in the strip. It holds one SSH link
  (`SshConnector`, `--no-start`), sends `remote.hello`, then `remote.status` every 3 s. It hands the window each answer. Backoff with `next_backoff` when the
  link fails. It also runs one errand: `remote.close {tab}` for an unattached row's Close.
- **Plan (pure).** `import_plan(machine, answer, shown, hidden, in_flight) -> ImportPlan { add, update, remove }`.
  It keeps entries with `origin == "here"`. It drops entries already shown here and hidden ones. It drops the whole
  round for a machine while a tab here on that machine waits for `remote.open` to answer (its `RemoteLive` has no host
  tab id yet and its link has not `Ended`, so the new host tab could be imported twice; a tab whose open was refused
  and ended does not block imports for ever). No round runs before the restore answered (`restore_done`), so a saved
  remote tab and its import never both appear. It removes unattached rows the answer no longer lists.
  It prunes hidden ids only on a real listing. An `unavailable` answer (a host with no Flare running) removes
  unattached rows but never prunes the hidden list, since a starting or busy host may answer it in passing. It skips
  ids on the "closing" list (builder's call 12).
- **Rows.** `TabModel::import_remote_tab(tag, folder, host_tab, how)` adds a remote tab row at the end of the strip
  without making it the front tab (`add_tab` always activates, so it is not used). A `Tab` has no title field (labels
  come from the folder), so the host's title is kept only in the watcher's last answer, for the agent's list. The row
  never touches the used list: `sync_mru` adds it after the tabs already known, so a Ctrl+Tab "most recent" walk
  still goes to the tab used before, never to the newest import. Its `RemoteLive` has the host tab id set. It
  carries `how` (`Imported` or `Adopted`) and `attached: false`, and starts with no session. At import the hub
  learns it as a remote tab (`hub.on_spawned` with no resume, then `hub.on_remote_tab`): `on_remote_status` returns
  at once for a tab the hub does not know as remote, so without this no mark would show.
- **Attach on show.** One hook in `refresh()`, where the front tab change is already seen (`crumbed_front`), attaches
  an unattached row that is now in front, whatever fronted it: a click, Ctrl+1..9, a notification, the cog going
  back, or a close that fronts its neighbour (`close_tab` picks the right one). While Ctrl is held after a Ctrl+Tab,
  the hook waits, in both switch modes: "most recent" holds a walk, and "in order" holds none (`TabModel::step` with
  `TabSwitch::Order` picks at once), so the window keeps its own `stepping` flag, set by `next_tab` and cleared by
  `commit_walk` (the chord code calls it on the Control release in both modes). The row the keys land on attaches
  then; the rows passed over do not. The attach calls
  `deps.host.spawn_remote` with the row's own `RemoteLive` (host tab set, seq 0) and `hub.on_remote_tab` again (the
  snapshot is not new work). The attach sends `ring: "both"`, so the host keeps its own ring (builder's call 5). The
  question episode the watcher already rang here carries over, so the attach does not ring it again. It does NOT
  call `start_remote`: that runs `hub.on_spawned` again, which drops the
  session the poll gave and queues `DecorGone::AgentLeft`, and `remember_remote_root`, which would add the host's
  folder to the recent folders on every click. The worker attaches (no `remote.open`, no resume), takes a snapshot
  (`fresh`), and starts its own `remote.status` poll as today. From then on the tab is a followed remote tab.
- **Host-sized start (builder's call 6).** The watcher's last status of the tab decides it: `sized: host` and a
  session means the spec gets `host_sized_start`. The worker then starts with no size to send (`Worker.size` is
  seeded from the pty today and sent after every attach, reconnects too), keeps the newest size the window asks for,
  and sends nothing from `pump` until the first `take_retake` or the first typed intent here, then the size, then the
  typing. The window seeds `rconvo.heard(tab, true, true)` from the same status at attach, so the fitted view can
  stand (and a click on it calls `retake_size`) before the link's own first status. Anything else attaches as a
  followed tab does today.
- **Before attach.** An unattached row has no handle. Every consumer treats it as a tab with no running shell:
  dictation routing skips it, plugin `tabs.input` refuses it, `handle_of` is None. Its statuses go through its own
  `RemoteLive::set_status` (the watcher hands them to the window, which sets them), so the existing
  `sync_remote_status` raises the marks, takes the host's folder (rule 1279) and feeds `rconvo`. The strip, sidebar
  and taskbar badge show them. Once attached, the watcher stops feeding the row; its link's own poll does. The
  notifier rings for unattached rows from the watcher's status, once per episode, as for any tab (builder's call 5).
  The host rings for them too: nothing follows them there. `busy_agents` (the window close) and the install hold leave
  them out: today a remote tab's working status puts it in `agent_ids` and `working` (`Indicator::remote_mark`), so
  without a filter every working Mac tab would hold the PC's quit (no shell runs here, and the quit only detaches,
  rule 288). Attached rows keep today's remote-tab handling.
- **Close.** Close on an imported row is Close on a remote tab: `remote.close`, and the host asks first while an agent
  works there (rule 48). An unattached row sends it through its machine's watch link. Every remote tab's Close
  (imported, adopted, attached or opened from here) puts (machine, host tab) on the closing list, its host tab id
  read before `close_now` drops the `RemoteLive`: a followed tab the host still asks about is listed with
  `origin: here` and would come back on the next poll. As for any remote tab,
  Ctrl+Shift+T then opens a new shell in that folder on the host (rule 1236), never a resume.
- **Hide here only.** Detaches (an attached row) or just drops the row (an unattached one), and adds
  (machine, host tab id) to the hidden list. The host tab id is read before `model.close`, which drops the
  `RemoteLive`. This also applies to a remote tab opened from here: once detached, the host lists it as its own tab,
  so it must be hidden too or it comes back as an import.
- **Rows that go by themselves** (setting off, machine removed, host tab gone) never go through `close_now`: that
  calls `remember_closed` and `host.kill`, which sends `remote.close`. They detach (attached) or just leave the model
  (unattached), with `forget_tab_surfaces`.
- **How a hidden tab comes back.** Only through the agent's adopt (below), or after the host restarts Flare (new
  ids). Turning the setting off and on again does not bring it back. The hidden list is pruned when the host no
  longer lists the tab. Builder's call: no "show hidden" menu in v1. If Max wants one, it is a later row in the
  + menu.

### Agent side

- New method `open.remote.list` (`Method::OpenRemoteList`) in the `open` namespace. The agent grant already enters
  `open` (rule 299), so `grant.rs` does not change.
- It answers, for each machine in `machines.json`: `machine`, `name`, `reachable`, and `tabs`, each with `tab` (host
  tab id), `folder`, `title`, `state` (one word), `agent` (kind or null) and `here` (`shown`, `hidden` or `none`).
  It never answers a Claude session id, question words, screen text or conversation.
- It reads the watcher's last answers. When the setting is off, or an answer is older than 5 s, it asks each machine
  once over a short `--no-start` link on the call's worker (rule 34). At most one fresh ask per machine per 5 s.
- `open.remote` gains an optional `tab` (a host tab id, one plain word). With `tab` it takes `machine` and `tab` only
  (no `folder`, no `resume`, no `front`: each is `bad_params`). `front` is refused because `open_remote` does not know
  its caller: today any agent may send `front: true` (rule 299's "only the command" is a convention, the check takes
  it from anyone), and a fronted row attaches. It never opens or resumes anything on the host. It adds that host tab
  here as an unattached row, as an import does, behind the front tab. It is not saved in `tabs.json` and it does not
  attach. So an agent never resizes a host terminal, never changes where a question rings and never starts a session
  link. Only the person fronting the row attaches it. The row is marked `adopted`, so it stays when the setting is
  off. If the tab is already shown here, nothing new is made. A hidden tab is taken off the hidden list. A host tab
  that is not listed, or not `origin: here`, is `not_found`.
- MCP: a new tool `list_remote_sessions` (no arguments) and an optional `tab` on `open_remote_session` (then
  `folder` is not needed). Both tool names join the built-in list in `flare-mods/src/tools.rs`.

## Protocol changes (all additive, `REMOTE_PROTOCOL` stays the same)

| Where | Change | Older peer |
|---|---|---|
| `remote.status`, `remote.tabs` answer | Own shell tabs only; each entry has `origin: "here"` | Older viewer ignores the field. Older host: no field, so nothing is imported. |
| `remote.attach` | Optional `ring: "both"`: the host keeps notifying for that tab (amends rule 295 for imported rows only) | Older host ignores it: the question rings only on the viewer after attach. |
| `remote.attach`, `remote.input`, `remote.resize`, `remote.close` and every other op that names a tab | `no_tab` for a host tab that is itself remote | No viewer ever named one on purpose (it was never listed). |
| ssh command | The watch link adds the fixed word `--no-start` after `--serve-remote` (amends rules 290, 305) | A host older than remote sessions PR 5 refuses the flag: the link fails, nothing imported. |
| `open.remote` | Optional `tab`; `folder` not needed with it; `folder`, `resume` and `front` refused with it | Pipe callers only (agent, `flare open`). |
| `open.remote.list` | New method in `open` | Agent and app only. |

## Security notes

**A paired peer (the other machine's Flare) before and after.** Before: with a paired key it has SSH to the account,
so it can already list, attach to, type into and close any shell tab of the host (`remote.*`). After: the same, with
less. It can no longer list, attach to, type into, size or close the host's own viewer tabs. No new method or
namespace on the host side. The one new param, `ring: "both"` on `remote.attach`, only keeps the host's own
notification on. It gives the peer nothing new to read or do.

What this feature does change for Max: with the setting on, every Flare tab on a linked machine shows on the other
screen and is one click from typing. That was already possible; now it is visible. The setting being off makes the
PC look as it does today, but does not take any access away from a peer. Removing a machine on Settings > Machines
does.

**An agent before and after.** Before: it could open a new remote tab (`open.remote`), resuming a session id it knew,
and could not see what runs on other machines. After, it also can:
- List the host tabs of linked machines: tab id, folder, title, one state word, agent kind, and whether shown here.
- Add one existing host tab here as an unattached row, behind the front tab. The row does not link, attach or size
  anything on the host until the person clicks it.

It still cannot: type into any remote tab (no `remote.*`, `shell.input` or `tabs.input`), read its screen or
conversation, close or hide it, front or attach it (`front` is refused with `tab`), see session ids or question
words, name a machine not in `machines.json`, or attach to a host's viewer tab. `flare open` and any other no-token
pipe caller get the same as the agent (they share the agent grant). The list is rate bound (one fresh ask per machine
per 5 s) and the adopt is bound by the existing `open.remote` checks.

Note for Max: this is more than the agent sees of this machine. On this machine `tabs.list` gives the agent its own
tab only, because tab titles carry folders, commands and task names (`ctl_app.rs`, `Method::TabsList`). The new list
gives it the titles and folders of every Flare tab on the linked machines. Max accepted this scope (2026-11-18).

**A plugin with `tabs` sees more.** `tabs.list` gives the agent and a mod their own tab only. Only a plugin approved
with `tabs` gets every tab (`CtlOp::TabsWide`, `tab_facts`), and its `tabs` and `agent` events. With the setting on
it also sees every imported row: the other machines' folders, titles, state words and, through `TabFact.session`,
the host's Claude session id of each row (the poll carries it). Today it sees that only for remote tabs opened or
followed from here. This new exposure comes with the setting, not with the agent methods. **Accepted by Max**
(2026-11-18, AskUserQuestion: "Yes, show all"). Plugins see imported rows like local tabs, session ids included.

A plugin approved with `tabs` and `input` (full control, rule 2) can type into an attached imported tab as into any
remote tab today. An unattached row refuses (no handle).

## Persistence

- `flare.machines.importRemote`: bool, default on. In `crates/flare-settings/src/machine_prefs.rs`. Row
  `machines-import` on Settings > Machines, section "This computer", after "Computer sharing". Label "Import
  sessions started on other machines", sub "Show tabs opened on your linked machines." (7 words), icon an existing
  one (`share` or `tabs`, picked in the build). Keywords: import sessions tabs linked other computers show. No
  "ssh", "remote", "mac" or "pc": `find.rs` holds that "ssh" and "remote mac" find Add a machine first, and this row
  sorts before it.
- `flare.machines.importHidden`: JSON object, machine id to a list of host tab ids. At most 200 ids per machine (the
  oldest drop). Ids are pruned only when a real listing no longer names them (never on `unavailable`). A machine removed from `machines.json` drops
  its list.
- Host tab ids look like `t<clock>-<n>` and are new after a host restart (the restore makes new ids). So a hidden tab
  comes back after Flare restarts on the host. Its session there is a new process then. Accepted.
- Imported rows are not saved in `tabs.json` (builder's call 11). `TabModel::saved()` counts its `active` index over
  the tabs it keeps, not over every shell tab as today. When an imported row is in front, the saved front is the most
  recently used kept tab (0 when there is none).

## Failure modes

| What happens | What Flare does |
|---|---|
| Machine off or asleep | Rows stay as they were. Backoff up to 60 s. A click attaches as today, with the existing reconnect line. |
| Flare not running on the host | `--no-start` link: relayed calls are `unavailable` (checked: `serve_remote::run` with `may_start` false relays as usual when Flare runs). Unattached rows go. The hidden list is not pruned. |
| Older host (no `origin`) | Nothing imported from it. One log line per link. |
| Closed here, host still asking | The "closing" list keeps it out for up to 2 minutes (builder's call 12). |
| Always-on watch link and host sleep | Not verified. A poll every 3 s (60 s when down) may keep a host awake or wake it. Rule 296 says the host sleeps when idle. On the by-hand list; if it does, poll slower while the host's tabs are idle. |
| Host refuses a poll (busy, timeout) | Logged; next poll asks again. Rows are not removed on a refusal, only on a good answer. |
| `remote.open` in flight here | That machine's round is skipped, so no duplicate. |
| Host tab ended | Unattached row goes on the next good poll. Attached row ends on `no_tab` as today. |
| Close on an unattached row, link down | One try over the watch link when it comes back, as `close_without_a_link`. The row goes here at once. |
| Setting turned off while a tab is attached and in front | It detaches and leaves; the most recently used other tab comes forward. |
| Machine removed from Settings | Its watcher stops, its rows go (detach), its hidden list goes. |
| Two viewers import the same host tab | Both show it. Both may ring for a question, as two followers do today. |
| Many host tabs | A row costs nothing until shown. Links grow only with tabs actually opened. |

## Test plan

All headless. No real SSH, no visible window (global rule, 2026-10-28).

- **Unit, pure.** `import_plan`: origin filter, dedupe by (machine, host tab), hidden filter, in-flight skip, removal
  only on a good answer, pruning, N machines. Hidden list store: cap, prune, bad JSON reads as empty.
- **Unit, host.** `remote_status_tabs` leaves out remote tabs and marks `origin`. `remote.attach`, `remote.input`,
  `remote.resize` and `remote.close` on a remote tab are `no_tab`. A poll changes no follow, size, notification or
  keep-awake state. A tab followed with `ring: "both"` still notifies on the host. A tab followed without it stays
  quiet on the host (rule 295 kept).
- **Unit, ctl.** `open.remote.list` and `open.remote {tab}` for the agent grant: fields, no session id, `not_found`,
  rate bound, refused for mod, remote and bridge grants. `tab` with `folder`, `resume` or `front` is `bad_params`.
- **Unit, MCP.** `list_remote_sessions` and `open_remote_session {tab}` argument checks and routing.
- **Unit, link.** Watch link argv carries `--no-start` and nothing else new. Backoff steps. An imported Claude tab attaches
  in the host-sizes state: no resize after attach, none on a refit, a tab switch or a reconnect, one on the first
  click (retake) or the first typed key, sent before that key.
- **Unit, model (flare-tabs).** An import is not the front tab and joins the end of the used list; `saved()` leaves
  imported and adopted rows out and counts `active` over the kept tabs.
- **Unit, window (appctl).** Ctrl+Tab in either mode attaches only the row it lands on; no import round before
  `restore_done`; an open that ended without a host tab does not block a machine's imports.
- **ui_tests (headless Slint).** The switch row on Settings > Machines, default on, kept across a restart. An import
  adds rows behind the front tab. A click attaches. Close asks while an agent works. Hide here only remembers. Off
  removes rows. A question on an imported row rings once here and once on the host, before and after attach.
  Attaching a row whose question already rang here does not ring it again.
  An agent's adopt makes an unattached row and sends no `remote.attach` or `remote.resize`. A closed row
  does not come back while the host asks.
- **Fakes.** A fake `Connector` with in-memory links (as `remote_link_tests.rs` does). A fake host answering
  `remote.status` from a script.
- **e2e.** One scenario `remoteImport` with two Flares on one machine over the existing loopback (`FLARE_E2E` remote
  loopback, hidden desktop). A tab opened in the host shows in the viewer, attaches on click, hides, stays hidden.
- **By hand (Max, PC and Mac).** Open a tab on the Mac; see it on the PC within a few seconds; click it; type; check
  the Mac terminal kept its size until typing; hide it; close another one and answer the ask on the Mac. Leave both
  idle with the setting on and check the Mac still sleeps.
