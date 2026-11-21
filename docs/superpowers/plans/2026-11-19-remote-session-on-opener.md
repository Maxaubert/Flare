# Remote sessions shown only on the opener: implementation plan (2026-11-19)

Spec: `docs/superpowers/specs/2026-11-19-remote-session-on-opener-design.md`.
Status: approved by Max 2026-11-19. PR 1 built (branch `feat/remote-on-opener-1`); PR 2 needs its mockup first.
Replaces: `docs/superpowers/plans/2026-11-18-import-remote-sessions.md` (PRs #281, #282, #284, #286 closed).

Three code PRs, in order. Each builds and passes its own tests. PR 1 and PR 2 are installed and released together:
with PR 1 alone, a hidden session whose opener never comes back, or whose host restarted, has no way out (no
`remote.find`, no Settings list). So PR 1 merges only after PR 2 is ready, and no release goes out between them.
PR 3 ships alone. PR 2 has a mockup step first.

## Rule numbers

1286 to 1301 are taken or reserved (1297 #283, 1298 to 1300 #280, 1301 #287). This plan takes **1302 to 1309**.
Check `docs/regression-rules.md` again before each PR and renumber if another PR took them first.

| Rule | Says | PR |
|---|---|---|
| 1302 | A tab opened from another machine runs hidden on its host: no strip tab, sidebar row, Ctrl+Tab or Ctrl+1..9 stop, front pick, badge, attention count, plugin tab or dictation target. It is remote-sized from birth and keeps its size when the last follower goes. A working one holds the host awake, followed or not (amends 280, 296). The quit ask and the install hold name it "for <opener>". It is saved with `hidden: {opener, open_id}` and restored hidden (amends 287). | 1 |
| 1303 | The remote ops answer only for hidden sessions: `remote.status` and `remote.tabs` list only them, and every op that names any other tab (host's own or a viewer tab) is `no_tab`. One screen per session, no mirror chains. | 1 |
| 1304 | A hidden session's question rings only on its opener: the host's notifier, sounds and badge leave it out, followed or not (amends 295). | 1 |
| 1305 | `remote.close` ends a hidden session at once on the host, never asking there; the opener asks first while an agent is there (amends 1192 for the host side). | 1 |
| 1306 | The opener finds its session after a host restart: it saves its `open_id`, and on `no_tab` asks `remote.find` once and attaches to the new id with a snapshot (amends 288, 1210). | 2 |
| 1307 | Settings > Machines on the host lists hidden sessions by opener with Show here (a local tab in front, followers dropped) and End (rule 48 ask on the host, not in the reopen list). | 2 |
| 1308 | A remote tab whose session moved or ended on its host says so in one red line, from `remote.find`'s answer. | 2 |
| 1309 | The fitted conversation views and `remote.convo` are gone (retires 297, 1193, 1211, 1212). ("Hide here only" went in PR 1, Max 2026-11-19, as an amend note on 1192.) | 3 |

Existing rules to respect and cite: 34 (blocking work on workers), 37 and 40 (tabs.json, unknown keys), 48 (close
ask), 206 (one ring per episode), 278 (no protocol bump), 279 to 305 (remote sessions), 1192, 1199 (Mac host name),
1201 (no tooltip on labelled buttons), 1210, 1236 (Ctrl+Shift+T), 1286 (resume only a used session).

## Shared decisions (every PR)

- No `REMOTE_PROTOCOL` bump (rule 278). One new method, `remote.find` (PR 2). Correction: the bridge relays by namespace
  (`grant.rs`), there is no relayed list; `remote.find` touches `method.rs` (the enum, `ALL`, `name`, `parse`,
  `namespace`, `timeout`, its tests), the `ctl_app.rs` dispatch and `serve_remote.rs` `may_retry` (find is idempotent). No
  grant change.
- Hidden sessions live in a list of their own in the tab model (`TabModel::hidden`, not in `state.tabs`). The 22
  `state.tabs` sites in `appctl` (strip, sidebar, MRU, Ctrl+Tab, front picks, `busy_agents`, `tab_facts` and the rest)
  then skip them. Sites that must see them read the new list: the remote ops, `remote.status`, keep-awake, the quit
  ask, the install hold, save and restore, and Settings. The hub, `TabHost`, the agent hooks and the model's `agents`
  and `sessions` maps key on the tab id and keep working. Id-keyed model calls that read `state.tabs` today must learn
  the hidden list: `set_cwd` (OSC 7 and the folder `remote.status` names), the label behind `title` (`labels()` reads
  `state.tabs`), the shell-exit path (`HostEvent::Exited` in `appctl.rs`: a hidden session whose shell ends is dropped
  and its followers hear `no_tab`), `is_shell_tab` in the rule 1210 `open_id` check, and `request_close`/`close_now`.
  PR 1 lists every one it touched in its body.
- Blocking work on workers (rule 34).
- Tests headless only. Fake connectors and links (`remote_link_tests.rs` style). No real ssh. e2e on the hidden desktop.
- Rust worktrees share one `CARGO_TARGET_DIR` (`<repo>\.claude\targets\shared`). Touch changed files before a build.
- Each PR: build and install on this PC and confirm the version before the merge question (global rule). The by-hand
  checks in the spec need both machines: run them after PR 2.

---

## PR 1: Host side, hidden sessions

Branch `feat/remote-on-opener-1`. Released together with PR 2 (see the top). An older opener keeps working (spec,
"Mixed versions").

**Files.**
- `crates/flare-tabs/src/model.rs`, `tabs.rs`: `TabModel::open_hidden(cwd, opener) -> id`, `hidden()`, `hidden_of(id)`,
  `close_hidden(id)`. A `Hidden { tab: Tab, opener: String, open_id: Option<String>, since_ms }` entry. `saved()` adds
  hidden sessions after the shell tabs (`active` counts shell tabs only); `settle` restores them into `hidden`, never
  into `state.tabs`. In PR 1 so a host restart never loses one (today the tab is saved as a local tab).
- `crates/flare-tabs/src/restore.rs`: `SavedTab.hidden: Option<SavedHidden { opener, open_id }>`. Unknown keys still
  read as before (rule 40). `remote.open`'s `open_id` (rule 1210) is kept with the session.
- `crates/flare/src/appctl.rs` (`restored`): spawn a restored hidden session like a local one (resume as rule 1286),
  hidden and remote-sized.
- `crates/flare/src/ctl_app.rs`: `remote.open` hands `viewer_of(ctx)` to `CtlOp::RemoteOpen` as `opener`.
- `crates/flare/src/appctl/remote_glue.rs`: `CtlOp::RemoteOpen` calls `open_hidden`; one check at the top of
  `ctl_remote` (as closed PR #281 did, reversed): an op that names a tab that is not hidden answers `closed()`;
  `RemoteClose` on a hidden tab closes at once (`close_hidden_now`); `remote_status_tabs` and `RemoteTabs` list only
  hidden sessions, each `sized: viewer`; `sync_keep_awake` counts working hidden sessions, followed or not;
  `RemoteRelease` keeps the size for a hidden tab.
- `crates/flare/src/remote_host.rs`: no change expected. `forget_tab` is reused by PR 2's Show here.
- `crates/flare/src/tab_host.rs`: spawn a hidden tab remote-sized (`SizeOwner::Remote`), and skip `take_size_back`
  for it.
- `crates/flare/src/appctl/surfaces7.rs` (`notify_update`) and `appctl.rs` (`update_badge`): leave hidden ids out.
  `AgentHub::attention` takes the set of hidden ids to skip.
- `crates/flare/src/appctl.rs`: `busy_agents` and the install hold add working hidden sessions, labelled
  "<folder> (for <opener>)"; the tab-token ops that show something on the host (panel, browser panel, `notice.show`,
  `show_*`, tab badge and progress) answer `unavailable` for a hidden tab.
- `crates/flare/src/appctl/mods_glue.rs`: `tab_facts` already reads `state.tabs`; add a test that it never lists a
  hidden session.
- Docs: `docs/regression-rules.md` (1302 to 1305; amend notes on 280, 295, 296, 1192), `docs/surfaces/protocol.md`
  (`remote.open`, `remote.status`, `remote.tabs`, `no_tab`, `remote.close`), `docs/features.md` (remote sessions).
  Mark the 2026-11-18 import spec and plan "Superseded by 2026-11-19".

**Tests first.**
- `flare-tabs`: `a_hidden_session_is_not_in_the_strip_the_walk_or_the_front_pick`, `closing_the_front_tab_never_fronts_a_hidden_session`.
- `remote_glue` / `ui_tests/agent_flows/remote_flows.rs`:
  - `a_remote_open_makes_a_hidden_session_with_its_opener_name` (and "another computer" when the bridge names none).
  - `remote_status_and_tabs_list_only_hidden_sessions` (a local tab and a viewer tab are left out).
  - `every_remote_op_refuses_a_tab_that_is_not_hidden` (attach, input, resize, release, close; a local tab and a
    viewer tab).
  - `a_remote_close_ends_a_hidden_session_without_asking_on_the_host` (agent working).
  - `a_hidden_sessions_question_never_rings_on_the_host_followed_or_not` and badge count 0.
  - `a_hidden_session_keeps_its_size_when_its_last_follower_goes` and says `sized: viewer`.
  - `a_working_hidden_session_holds_the_host_awake_with_no_follower`.
  - `the_quit_ask_names_a_working_hidden_session_for_its_opener`.
  - `a_hidden_tabs_agent_cannot_open_a_panel_or_front_the_window`.
  - `plugins_tabs_list_and_events_leave_hidden_sessions_out`.
- `restore.rs` and `flare-tabs`: `a_hidden_session_saves_and_restores_hidden_with_its_opener_and_open_id`;
  `an_older_tabs_json_reads_as_before`; `a_restored_hidden_session_resumes_its_used_claude_session` (rule 1286).
- Opener side: `the_opener_asks_before_closing_an_idle_remote_claude_tab` (the host's status names `kind`, the hub never
  saw work). Files: `crates/flare/src/appctl.rs` (`agent_present`), `crates/flare/src/agent_hub.rs` (keep the
  remote tab's last `kind`).
- e2e `remoteUi` (`tools/e2e/src/scenarios/remote.rs`): after the open, the host's strip has the same tab count as
  before. Update `docs/e2e.md`.
- Keep green: rules 280, 282, 288, 294, 298, 1210 tests. Tests that relied on a remote-opened tab showing in the
  host's strip are changed to read `hidden()`; list each one in the PR body.

---

### PR 1 as built: corrections to the plan above (2026-11-19)

Where the code disagreed with the plan, PR 1 follows the code:
- **Save format.** `SavedTabs.hidden: Vec<SavedHidden>` (not `SavedTab.hidden`): `parse_tabs` moves hidden entries to
  their own list, so `peek_state`, the slots and `settle_restore_from` never see them; `active` counts strip tabs only.
  The file keeps the plan's shape (`hidden: {opener, open_id, cols, rows}` on a tab after the strip). `Restored.hidden`
  carries them; `plan_restore_with` stats their folders and shares the saved and taken sessions with the strip tabs;
  `TabModel::settle` adds them hidden and returns their spawns last; `AppCtl::restored` spawns them with
  `TabHost::spawn_hidden` at the saved size.
- **Birth size.** `remote.open` gets no `cols`/`rows`: a new opener sending them would get `bad_params` from an older
  host (`only_fields`). A hidden session is remote-owned from birth instead (`TabHost::spawn_hidden`, at the window's
  grid size until the opener's first `remote.resize`, which follows the attach), never handed back
  (`take_size_back` and a local key pass it by), and its last size is saved.
- **The remote gate.** Only `remote.attach` makes a follow, and it refuses any tab that is not hidden, so the ops the
  worker answers (`output`, `snapshot`, `fetch`, `put`, `convo`, `detach`) refuse them too. `RemoteRelease` and
  `RemoteClose` check it as well. `remote.status`/`remote.tabs` build from `hidden_rows` (`tab_facts` stays the strip's).
- **Token ops from a hidden tab.** `is_shell_tab` stays strip-only. One guard ahead of every UI-thread handler
  (`AppCtl::hidden_refusal`) answers `unavailable` for panels, `show_*`, the browser, mini-apps, mods, fill, badge,
  progress and the session bar; `PanelState` (the hooks), `tabs.list` of its own tab, `TabCwd` and `SessionOf` answer
  for it (`session_tab_folder`). `notice.show` and `open.remote` carry no tab: left as they are (see PR 2).
- **Lifecycle sites that read only `state.tabs`.** `sync_hub`, `sync_model_agents` and `prune_restoring` read the hidden
  list too; the hub knows hidden ids (`on_hidden_tab`) and leaves them out of `question_tabs`, `attention`, `send_tabs`
  and the screen question post; the bell flash and the dev chip skip them; `HostEvent::Exited` drops a hidden session.
- **Close.** `close_hidden_now` (no `remember_closed`, no ask). `request_close` stays strip-only (no End on the host,
  see Max's update below).
- **Opener ask.** `AgentHub::remote_agent` keeps the host's `kind` before `on_remote_status` returns early;
  `AppCtl::close_asks` uses it (not `agent_present`, which the install ask also reads).
- **Opener name.** `RemoteAttach` carries `viewer_of` and renames the session's opener.
- **Cap.** 50 hidden sessions per machine (`MAX_HIDDEN`), `busy` on the 51st, after the open-id dedupe.
- **"Hide here only"** removed in PR 1 (Max 2026-11-19), with `TabHost::leave`.
- **Tests removed.** The two host fitted view flows (rules 1211, 1212) cannot hold once only hidden sessions are sized
  by a viewer; the code goes in PR 3.

PR 2 must also pick up: `SavedRemote.open_id` and `RemoteLive::new` / the `remote_link.rs` worker spawn carrying the
tab's open id (the opener side is untouched in PR 1); `remote.find` as corrected above (it reads `Hidden::open_id`).
PR 1 refuses `open.remote` from a hidden session's agent (`CtlOp::OpenRemote.caller`); `notice.show` is a plugin's
call only, never an agent tab token's, so it needs nothing.

## PR 2: The opener finds its session again, Settings list, Show here and End

Branch `feat/remote-on-opener-2`. Needs PR 1.

**Step 0, before code: mockup.** The Settings > Machines section "N sessions running for <opener>" with its rows and
the Show here and End buttons. Ask Max which design skill and image tool (global rule), send the screenshot in chat,
and wait for his yes.

**Files.**
- `crates/flare-tabs/src/restore.rs`: `SavedRemote.open_id: Option<String>` (the opener's side). Unknown keys still
  read as before (rule 40).
- `crates/flare-term/src/remote_pty.rs` (`RemoteLive`/`RemoteSpec`): carry the tab's `open_id`, made when the tab is
  made, restored from `tabs.json`.
- `crates/flare/src/remote_link.rs`: on `no_tab` at attach or in the pump, send `remote.find {open_id}` once (a
  `--no-start` is not needed, the link is up). `{tab}`: set the host tab, fresh snapshot, line "Flare restarted on
  <machine>. The session goes on." `{shown: true}`: line "This session now shows on <machine>.", end. `no_tab`: line
  "This session was ended on <machine>.", end. `unknown_method`: end as today. Kept typing is dropped in every case.
- `crates/flare-ctl/src/method.rs`, `crates/flare/src/ctl_app.rs`, `crates/flare/src/serve_remote.rs`: `remote.find`
  (`Method::RemoteFind`, `only_fields ["open_id"]`, `CtlOp::RemoteFind`), relayed by the bridge.
- `crates/flare/src/appctl/remote_glue.rs`: `RemoteFind`; `show_hidden_here(id)` (move to the strip end, front it,
  `take_size_back`, `RemoteHost::forget_tab`, keep the open id on the tab in memory for `{shown: true}`);
  `end_hidden(id)` (`request_close` path with the rule 48 ask, no `remember_closed`).
- `crates/flare-settings/src/page/machines.rs`, `rows.rs`, `actions.rs`, `find.rs`: the section and its two actions.
  Keywords: sessions running background show end. The rows read a snapshot the window hands the page.
- Docs: `docs/regression-rules.md` (1306 to 1308; amend notes on 287, 288, 1210), `docs/surfaces/protocol.md`
  (`remote.find`, the `tabs.json` keys), `docs/features.md`, `docs/settings.md` if it lists the Machines page rows.

**Tests first.**
- `restore.rs`: `a_remote_tab_saves_and_restores_its_open_id`; `an_older_tabs_json_reads_as_before`.
- `remote_link_tests.rs` (fake host): `a_no_tab_after_a_host_restart_finds_the_session_by_open_id_and_takes_a_snapshot`;
  `a_shown_answer_ends_the_tab_with_its_line`; `an_ended_answer_ends_the_tab_with_its_line`;
  `an_older_host_without_find_ends_the_tab_as_before`; `kept_typing_is_never_sent_to_the_found_tab`.
- `ctl_app.rs`: `remote_find_answers_only_for_a_known_open_id` (bad params, length cap as `OPEN_ID_MAX`).
- `ui_tests/settings_machines.rs`: the section shows only with hidden sessions, groups by opener, counts right, has no
  tooltip on its buttons.
- `ui_tests` flows: `show_here_makes_a_front_local_tab_at_the_window_size_and_drops_its_followers`;
  `end_asks_while_an_agent_works_and_never_joins_the_reopen_list`.
- e2e `remoteUi`: restart the host Flare in the scenario; the viewer's tab goes on with the restart line.

---

## PR 3: Remove what one screen no longer needs

Branch `feat/remote-on-opener-3`. Needs PR 1.

**Files.**
- Remove: `RemoteConvoView`, `HostConvoView`, `host_view_stands`, `host_convo_*`, `HOST_CONVO_EVERY_MS`,
  `UiMsg::HostConvo` (`appctl/remote_glue.rs`, `appctl.rs`); `poll_convo`, the retake and `want_convo`
  (`remote_link.rs`, `remote_pty.rs`); `TabHost::note_viewer_typed` and `viewer_typed_last` (`tab_host.rs`); the view
  in `crates/flare/ui/app.slint` and `remote_convo.slint`; `crates/flare/src/remote_convo.rs`;
  `crates/flare/src/ui_tests/remote_convo.rs`; `Method::RemoteConvo` and its bridge relay. Grep for other callers
  first; keep anything another feature uses.
- "Hide here only" was removed in PR 1 (Max agreed 2026-11-19).
- Docs: `docs/regression-rules.md` (1309; mark 297, 1193, 1211, 1212 "Retired by 1309"; amend 1192),
  `docs/surfaces/protocol.md`, `docs/features.md`.

**Tests first.**
- `a_remote_tab_never_shows_a_conversation_view` (the host's status says `sized: host`: the terminal stays).
- (`a_remote_tabs_menu_has_close_and_no_hide_here_only` is in PR 1.)
- Keep green: every remaining remote test, the e2e `remoteUi`.

---

## Open decisions for Max (before PR 1)

1. Remove "Hide here only" on the opener (recommended: it leaves a session nobody sees)?
2. A hidden session's agent: panels refused with a clear message (recommended), or kept quietly for a later Show here?
3. A working hidden session holds the host awake even with the opener gone (recommended)?
4. Remote tabs opened before the update end once on the opener (they stay on the host's strip). Fine?
5. For PR 2's mockup: which design skill and image tool?

## Update, Max, 2026-11-19 (after the mockups)

Max: "just have it show which PCs are connected, no session things. If you start a session on the Mac it stays only
there. Only sessions you start remotely on the PC or remotely on the Mac show, but in the normal tab view."

- There is NO list of hidden sessions in Settings. No "Running for other machines" section, no Show here, no End on the
  host. Settings > Machines stays as it is: the machines and whether they are connected.
- A session started on a machine shows only there, as a normal tab. A session opened on another machine shows only on
  the opener, as a normal tab in its strip. The host shows it nowhere.
- Close on the opener ends it on the host. A hidden session whose opener never comes back keeps running until the
  host's Flare quits (it is saved and restored like any hidden session until then). The cap of 50 hidden sessions per
  machine stays.
- PR 2 keeps `remote.find` (the opener finds its session again after a host restart) and drops the Settings list, Show
  here and End. The "now shows on" line goes; "was ended on" stays only for a session that ended on the host by
  itself (its shell exited).
