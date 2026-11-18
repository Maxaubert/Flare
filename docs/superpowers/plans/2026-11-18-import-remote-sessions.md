# Import remote sessions: implementation plan (2026-11-18)

Spec: `docs/superpowers/specs/2026-11-18-import-remote-sessions-design.md`.
Status: draft for Max's approval, together with the spec. Max's answers of 2026-11-18 are applied. Not started.

Four code PRs, in order. Each builds, passes its own tests and ships alone. No mockup (Max: just a toggle).

## Rule numbers

`docs/regression-rules.md` on origin/main (91bf9650) ends at rule 1285. Open PR #280 (`fix/remote-menu-browser`)
takes 1286 to 1288. This plan takes **1289 to 1296**. Check again before each PR: if another PR merged rules first,
renumber to the next free ones and fix the references here.

| Rule | Says | PR |
|---|---|---|
| 1289 | The host lists only its own shell tabs, each with `origin: here`, and answers `no_tab` to every `remote.*` op that names one of its viewer tabs (no mirror chains). A tab whose followers all attached with `ring: "both"` still notifies on the host (amends 295). | 1 |
| 1290 | The import plan: origin filter, one row per (machine, host tab), hidden and closing filters, in-flight skip, removal only on an answer, hidden pruning only on a real listing. | 2 |
| 1291 | The watch link runs `--serve-remote --no-start`, only while the setting is on (or an adopted row needs it), with backoff to 60 s; it never starts Flare on the host. Amends 290 and 305. | 3 |
| 1292 | An imported row attaches only when shown in front or clicked, never opens or resumes, and starts in the host-sizes state (rule 1193): no size is sent until the first click or key here. | 3 |
| 1293 | Close on an imported row is a remote close (rule 48 ask, a closing list keeps it out meanwhile); Hide here only remembers (machine, host tab) for every remote tab. | 3 |
| 1294 | The setting: default on; off removes imported rows by detach, never close, and leaves tabs the person or an agent opened. | 3 |
| 1295 | An unattached row is a tab with no shell to every consumer: no dictation, no plugin input, no close or install hold. A question on an imported row rings here once and on the host once, attached or not: the attach sends `ring: "both"`, and the episode already rung here is not rung again. | 3 |
| 1296 | An agent may list linked machines' host tabs (no session id, no question words) and add one as an unattached row behind the front tab (`front` refused with `tab`); it still cannot type, read, close, hide, front, attach or size. Amends 299. | 4 |

Existing rules to respect and cite: 48 (close question), 282 (last typist sizes), 288 (quit detaches), 290 and 305
(ssh argv, amended by 1291), 294 and 298 (status words, refusals pass), 295 (one notification per question), 296
(keep awake), 299 (agent opens remote, amended by 1296), 1192 (Close vs Hide here only), 1193 (fitted view while the
host sizes), 1210 (open id, kept typing), 1211 and 1212 (who sizes), 34 (blocking work on workers), 37 and 40
(tabs.json, unknown keys).

## Shared decisions (every PR)

- No `REMOTE_PROTOCOL` bump (rule 278 would refuse older links). Changes are new fields an older peer ignores. No new
  host param.
- No grant change. The agent stays in `open`; the remote grant stays in `remote`.
- Blocking work on workers (rule 34): watch links, the agent's fresh list asks, the close errand.
- Tests headless only. Fake connectors and links (`remote_link_tests.rs` style). No real ssh. e2e on the hidden
  desktop.
- Rust worktrees share one `CARGO_TARGET_DIR` (`<repo>\.claude\targets\shared`). Touch changed files before a build.

---

## PR 1: Host side (origin, no mirror attach)

Branch `feat/import-remote-1`. Ships alone: older viewers ignore it.

**Files.**
- `crates/flare/src/appctl/remote_glue.rs` (`CtlOp::RemoteTabs`, `CtlOp::RemoteAttach`, `remote_status_tabs`)
- `crates/flare/src/appctl/mods_glue.rs` (`tab_facts` stays as is for plugins; the remote answers filter on
  `model.remote_of(id).is_none()`)
- `crates/flare-ctl/src/remote.rs` (an `ORIGIN_HERE` const and the `ring` field name, if the remote constants live
  there)
- `crates/flare/src/appctl/surfaces7.rs` (`notify_update`: the rule 295 filter skips a tab whose followers all asked
  for `ring: "both"`)
- `docs/surfaces/protocol.md`, `docs/regression-rules.md` (1289), `docs/features.md`

**Changes.**
1. `remote.tabs` and `remote.status` leave out every tab with `remote.is_some()` and add `"origin": "here"`. Filter in
   `remote_glue.rs`, not in `tab_facts` (plugins' `tabs.list` and events read it too).
2. One check at the top of `ctl_remote`: an op that names a tab (attach, input, resize, release, close, and any other)
   answers `no_tab` (`closed()`) when `model.remote_of(tab).is_some()`. Today `RemoteInput` and `RemoteResize` check
   only `is_shell_tab`, which a viewer tab passes, and `RemoteClose` only that the tab exists.
3. `remote.attach` takes an optional `ring` (only `"both"` is known; anything else is ignored). The host remembers it
   per follower. `notify_update` leaves out a followed tab only while some follower did not ask for `both`.

**Tests first.**
- `remote_status_and_tabs_leave_out_this_hosts_remote_tabs_and_say_origin`.
- `every_remote_op_refuses_a_tab_that_is_itself_remote` (attach, input, resize, close).
- `a_tab_followed_with_ring_both_still_notifies_on_the_host`.
- `a_tab_followed_without_ring_both_stays_quiet_on_the_host` (rule 295 kept for tabs opened from a viewer).
- Keep green: every rule 294, 295, 296, 298 test (a followed tab is never a viewer tab, and a follower without
  `ring` keeps today's behaviour).
- Open check 2 below: note whether a host `RemotePty` tab had a handle and tail.

---

## PR 2: Import plan, row model, prefs functions (no wiring, no visible change)

Branch `feat/import-remote-2`. Pure code and data. Nothing calls it yet, and no row is drawn yet.

**Files.**
- `crates/flare/src/remote_import.rs` (new): `ImportPlan`, `import_plan`, `HiddenList`, `ClosingList`. CI runs
  `cargo clippy --workspace --all-targets -- -D warnings`, and nothing in the `flare` binary calls this module until
  PR 3, so it starts with `#![cfg_attr(not(test), allow(dead_code))]`, which PR 3 removes.
- `crates/flare-tabs/src/tabs.rs`, `model.rs`: `TabModel::import_remote_tab(tag, folder, host_tab, how)` with `how`
  `Imported` or `Adopted` (no title: a `Tab` has none, labels come from the folder), appended without changing
  `active_id` and without touching the used list (not `add_tab`, which always activates; not add-then-pick, which
  would put the new id second in the used list). `sync_mru` then adds it after the known tabs. A per-tab
  `attached: bool`; `saved()` leaves both kinds out of `tabs.json` and counts
  `active` over the kept tabs (an imported row in front saves the most recently used kept tab); `remove_imported(id)`
  that fronts the most recently used other tab when the row was in front.
- `crates/flare-settings/src/machine_prefs.rs`: `import_remote_on` (default true), `set_import_remote_on`,
  `import_hidden` / `set_import_hidden` (JSON, cap 200 per machine, bad JSON reads as empty).
- `docs/regression-rules.md` (1290), `docs/features.md`.

**Tests first.**
- `import_plan_keeps_origin_here_only_and_skips_an_older_host` (no `origin`: empty plan, flag "older host").
- `import_plan_makes_one_row_per_machine_and_host_tab` (shown here already: opened from here, restored, imported,
  adopted).
- `import_plan_drops_hidden_and_closing_tabs`.
- `import_plan_prunes_hidden_ids_only_on_a_real_listing_never_on_unavailable`.
- `import_plan_skips_a_machine_while_an_open_there_waits_for_its_host_tab` (`in_flight` is an input; PR 3's caller
  computes it).
- `import_plan_removes_only_unattached_imported_rows_and_never_on_a_refusal`.
- `import_plan_keeps_machines_apart` (three machines).
- `a_closing_entry_lapses_when_the_host_drops_it_or_after_two_minutes`.
- `an_imported_row_goes_behind_the_front_tab_and_is_not_saved` (flare-tabs).
- `an_imported_row_joins_the_end_of_the_used_list_so_ctrl_tab_goes_to_the_tab_used_before` (flare-tabs).
- `saved_counts_active_over_the_kept_tabs_and_an_imported_front_saves_the_last_used_kept_tab` (flare-tabs).
- `import_remote_defaults_on_and_the_hidden_list_is_capped_and_reads_bad_json_as_empty` (flare-settings).
- The macOS tab code is not a concern: `crates/flare-tabs/src/macos/` holds only `names.rs` (folder names in the +
  menu). Checked 2026-11-18.

---

## PR 3: Wiring (watcher, lazy attach, close, hide, the switch)

Branch `feat/import-remote-3`. The feature turns on here.

**Files.**
- `crates/flare/src/remote_link.rs`: `remote_command` / `ssh_argv` take a `no_start` flag (the watch link only).
  The size hold: `RemoteSpawner::spawn` seeds `Worker.size` with None when the spec says `host_sized_start`
  (today it is always `Some((spec.cols, spec.rows))`, sent after every attach and reconnect, line ~1235), and a
  worker flag `size_held` makes `pump` keep `out.size` in `self.size` without sending it (line ~1583). The hold ends
  on the first `take_retake` or the first typed intent: the size goes first, then the typing. It holds across
  reconnects until it ends.
- `crates/flare-term/src/remote_pty.rs`: `RemoteSpec` (defined here, not in `remote_link.rs`) gains
  `host_sized_start: bool` and `ring_both: bool`. The worker adds `ring: "both"` to every `remote.attach` it sends
  for such a spec, reconnects too.
- `crates/flare/src/remote_import.rs`: `Watcher` (one worker per machine; hello, `remote.status` every 3 s,
  `next_backoff` to 60 s, `unavailable` handed on as "no Flare there", errand `remote.close {tab}`), behind a
  `WatchPort` trait so tests swap a fake.
- `crates/flare/src/appctl/remote_glue.rs`: `UiMsg::ImportAnswer`; `apply_import` (no round before `restore_done`;
  a new row: `model.import_remote_tab`, then `hub.on_spawned(id, folder, None)` and `hub.on_remote_tab(id)`, since
  `on_remote_status` ignores a tab the hub does not know as remote; each unattached row's status goes through its
  own `RemoteLive::set_status`, so the existing `sync_remote_status` raises the marks, takes the folder and feeds
  `rconvo`); `attach_imported` (`deps.host.spawn_remote` with the row's `RemoteLive` and `host_sized_start`,
  `hub.on_remote_tab` again, `rconvo.heard` seeded from the last status; NOT `start_remote`, whose second
  `hub.on_spawned` drops the session and queues `DecorGone::AgentLeft`, and whose `remember_remote_root` adds the
  host folder to the recent folders); Close on an unattached row through the watcher plus the closing list;
  `hide_remote` reads the host tab id before `model.close` and adds it to the hidden list, for every remote tab;
  `set_machines` starts and stops watchers and drops removed machines' rows and hidden lists; the setting's on and
  off. Rows that go by themselves (off, machine removed, host tab gone) never use `close_now` (it calls
  `remember_closed` and `host.kill`, which sends `remote.close`): they detach or leave the model, then
  `forget_tab_surfaces`.
- `crates/flare/src/appctl.rs`: one hook at the top of `refresh()`, before its early return when there is no app
  (so headless tests reach it), calls `attach_imported` when the front tab is an unattached row and the window's
  `stepping` flag is off. `next_tab` (~line 5895) sets `stepping`; `commit_walk` (~line 5891, called by
  `chords.rs` on the Control release in both switch modes) clears it and refreshes. `TabSwitch::Order` holds no
  walk in the model, so the model's walk cannot be the gate. That covers every way a row comes forward: strip and
  sidebar clicks, Ctrl+Tab in both modes, Ctrl+1..9, a notification (`front_tab_of_toast`), the cog going back, a
  plugin sidebar item, and `close_tab` fronting its right neighbour. `apply_import` computes `in_flight` (a remote
  tab of that machine with no host tab whose link is not `Ended`) and skips every round before `restore_done`.
  Close on any remote tab (`request_close` / `close_now`) reads the host tab id first and feeds the closing list. `busy_agents` (line ~5294) and the
  install hold leave out unattached rows. Open check 1 answered: today a remote tab counts there whenever its host
  says `working` (`Indicator::remote_mark` puts it in `agent_ids` and `working`), so the filter is a task, not a
  check. Attached rows keep today's remote-tab handling.
- `crates/flare/src/appctl/surfaces7.rs`: `notify_update` rings for unattached rows from the watcher's status, once
  per episode (rule 206). On attach the episode carries over, so the link's first status does not ring the same
  question again. No rule 295 move for imported rows: their spec sets `ring_both`.
- Dictation route, `handle_of`, plugin `tabs.input`: an unattached row is "no shell" (verify each; guard where
  missing).
- `crates/flare-settings/src/rows.rs`: row `machines-import` (Switch, page `machines`, section `this-computer`, label
  "Import sessions started on other machines", sub "Show tabs opened on your linked machines.", key
  `flare.machines.importRemote`, keywords without "ssh", "remote", "mac" or "pc"); `ROW_ORDER` `[&str; 53]` to 54
  after `machines-sharing`; one line in the row=key table test (~line 1030).
- `crates/flare-settings/src/page/machines.rs`: This computer draws the sharing row, then the import row.
- `crates/flare-settings/src/actions.rs`: an arm `"machines-import" => machine_prefs::set_import_remote_on(p, on)` in
  `switch()` (line ~186, beside `"notify-ask"`). Line ~706 is a test that turns every plain switch off and skips
  `machines-sharing`; the import row joins it as a plain switch with no change there.
- `crates/flare-settings/src/find.rs`: `the_machines_page_is_found_by_ssh_mac_and_its_add_row` (line ~291) keeps
  "ssh", "add a machine" and "remote mac" finding `machines-add` first, and its "machines" list becomes
  `["machines-sharing", "machines-import", "machines-add"]`.
- `crates/flare/src/settings_ctl.rs`: the switch reads and writes `import_remote_on` and tells the window.
- `crates/flare/src/window.rs`, `headless.rs`: build the watcher with the real or the scene connector.
- e2e: `tools/e2e` scenario `remoteImport` (loopback, hidden desktop).
- `docs/regression-rules.md` (1291 to 1295; a pointer line in 290 and 305), `docs/features.md`, `docs/parity.md` if
  its checklist needs lines.

**Tests first.**
- `the_watch_link_runs_serve_remote_no_start_and_nothing_else_new` (extends the rule 305 table test).
- `the_watcher_polls_status_backs_off_when_down_and_stops_when_off` (fake connector, fake clock).
- `an_imported_tab_attaches_when_brought_forward_and_never_opens_or_resumes` (fake link: `remote.attach` yes,
  `remote.open` never, no `resume`).
- `an_imported_claude_tab_sends_no_size_on_attach_refit_switch_or_reconnect_and_one_before_the_first_key` (rules
  1193, 1211, 1212; a retake also ends the hold).
- `attaching_an_imported_row_keeps_its_session_and_decor_and_adds_no_recent_folder` (no second `on_spawned`).
- `an_imported_row_shows_marks_before_attach` (the hub knows it as remote from the import).
- `ctrl_tab_in_order_and_recent_modes_attaches_only_the_row_it_lands_on`;
  `a_close_that_fronts_an_imported_row_attaches_it`.
- `no_import_round_runs_before_the_restore_answered`; `an_open_that_ended_without_a_host_tab_does_not_block_imports`.
- `closing_a_followed_remote_tab_keeps_it_out_while_the_host_asks` (not only unattached rows).
- `the_setting_off_and_a_removed_machine_never_send_remote_close_or_remember_a_closed_tab`.
- `a_working_unattached_row_does_not_hold_the_window_close`.
- `an_imported_plain_shell_sends_its_size_on_attach_as_a_followed_tab_does`.
- `close_on_an_unattached_imported_row_sends_remote_close_through_the_watch_link`.
- `close_on_an_imported_tab_asks_on_the_host_while_an_agent_works` (rule 48, fake host).
- `a_closed_row_does_not_come_back_while_the_host_asks`.
- `hide_here_only_remembers_the_host_tab_and_the_next_poll_does_not_bring_it_back`, for an imported tab and for a
  tab opened from here.
- `turning_import_off_detaches_imported_rows_and_keeps_the_rest` (no `remote.close`; front moves to the mru; adopted
  rows stay).
- `a_machine_removed_from_settings_stops_its_watcher_and_drops_its_rows_and_hidden_list`.
- `a_question_on_an_imported_row_rings_here_and_on_the_host_attached_or_not` (both sides faked; one ring per machine).
- `attaching_an_imported_row_does_not_ring_its_question_again_here`.
- `an_imported_rows_attach_sends_ring_both_and_a_tab_opened_from_here_does_not` (reconnects too).
- `an_unattached_row_takes_no_dictation_no_plugin_input_and_holds_no_window_close`.
- `a_plugin_with_tabs_sees_imported_rows_with_their_session_ids` (Max: "Yes, show all").
- `ui_tests::settings_machines::the_import_switch_is_on_by_default_and_kept_across_a_restart`.
- `ui_tests::term::flow::an_import_adds_rows_behind_the_front_tab_and_a_click_attaches`.
- `the_machines_page_draws_sharing_then_import_in_this_computer`; update
  `this_computer_is_first_with_the_sharing_switch_and_its_fixed_subtext` and
  `the_machines_page_is_found_by_ssh_mac_and_its_add_row`.
- e2e `remoteImport`: a tab opened in the host Flare shows in the viewer, attaches on click, hides, stays hidden after
  two more polls.
- Keep green: rules 288, 294, 295, 298, 1192, 1193, 1210 to 1213 tests and the e2e `remoteUi`.

**Before merge.** Build and install on this PC and the Mac (global rule: install before merge). Max's by-hand check
from the spec, including "the Mac still sleeps with the setting on" (open check 4) and "a question in a Mac tab rings
on both machines, once each".

---

## PR 4: Agent list and adopt

Branch `feat/import-remote-4`. Needs PR 3 (it reads the watcher's answers and makes unattached rows).

**Files.**
- `crates/flare-ctl/src/method.rs`: `Method::OpenRemoteList` (`open.remote.list`, namespace `Open`), `ALL` count. Its
  timeout: `timeout()` goes by namespace and `Open` is `DEFAULT_TIMEOUT` (15 s, held by
  `open_remote_is_a_short_call_of_its_own_namespace`), so `REMOTE_CALL` needs an explicit arm for this method only.
  (The existing method is `Method::OpenRemoteSession`, name `open.remote`.)
- `crates/flare-ctl/src/grant.rs`: no rule change. Tests only.
- `crates/flare/src/ctl_app.rs`: `open_remote_list` (reads the cache; a fresh `--no-start` ask per machine at most
  once per 5 s, the machines asked in parallel from the call's worker, each bounded by ssh's 5 s connect timeout);
  `open_remote` takes `tab` (one plain word, only with `machine`; `folder`, `resume` and `front` are `bad_params`
  with it, because `open_remote` cannot tell the agent from `flare open` and a fronted row attaches). Add `tab` to
  its `only_fields` list.
- `crates/flare/src/appctl.rs`: `adopt_remote(machine, host_tab)`: nothing new if the tab is shown here; else an
  `Adopted` unattached row behind the front tab; off the hidden list; starts the machine's watcher if the setting
  is off.
- `crates/flare-mcp/src/lib.rs`: tool `list_remote_sessions`; `open_remote_session` gains `tab` (`machine` required,
  then `folder` or `tab`); one line in the server instructions.
- `crates/flare-mods/src/tools.rs`: `list_remote_sessions` in the built-in names test.
- `crates/flare/src/cli_open.rs`: no change. `flare open --tab` is a non-goal in v1: it could only add a row behind
  the front tab, since `front` is refused with `tab`.
- `docs/surfaces/protocol.md`, `docs/regression-rules.md` (1296, a pointer in 299), `docs/features.md`.

**Tests first.**
- `the_agent_may_list_and_adopt_remote_tabs_but_never_type_into_one` (grant: `open.remote.list` yes; `remote.*`,
  `shell.input`, `tabs.input` no; mod, remote and bridge grants no).
- `open_remote_list_names_known_machines_only_and_carries_no_session_or_question` (asserts the exact field set).
- `open_remote_list_asks_each_machine_at_most_once_per_five_seconds`.
- `an_agents_adopt_makes_an_unattached_row_behind_the_front_and_sends_no_attach_or_resize`.
- `open_remote_with_a_tab_never_fronts_or_attaches` (rule 299 kept for the folder form).
- `open_remote_refuses_tab_with_folder_resume_or_front_and_an_unknown_host_tab_is_not_found`.
- `open_remote_with_a_hidden_tab_takes_it_off_the_hidden_list`.
- `an_adopted_row_stays_when_the_setting_is_off_and_is_not_saved`.
- `list_remote_sessions_and_open_remote_session_tab_check_their_arguments_and_route` (flare-mcp).
- Keep green: `the_agent_may_open_a_remote_session_but_never_type_into_one`,
  `an_agents_remote_open_stays_behind_the_tab_in_front`.

---

## Docs (with the PRs above, or straight to main as docs-only)

- Amend `docs/superpowers/specs/2026-11-12-remote-sessions-design.md` non-goals: lift "More than two machines" to
  "machines Max does not own", with a pointer to this spec (2026-11-18).
- `docs/features.md`: one entry per PR.
- `README.md`: one line under remote sessions once PR 3 ships.

## Follow-ups (not in these PRs)

- `open_remote` (`ctl_app.rs` ~642) accepts `front: true` from any agent, though rule 299 says only `flare open`
  fronts. This feature does not change it (it refuses `front` only with `tab`). Fix later in its own PR.

## Open checks for the builder (verify before coding, note the answer in the PR)

1. Answered 2026-11-18: yes. `busy_agents` (`appctl.rs` ~5294) has no remote filter, and `Indicator::remote_mark`
   puts a remote tab whose host says `working` into `agent_ids` and `working`. PR 3 filters unattached rows out.
2. Does a host `RemotePty` tab have a `handle_of` and a tail? PR 1 refuses every op on it either way.
3. Answered 2026-11-18: `crates/flare-tabs/src/macos/` holds only `names.rs`; no tab logic differs there.
4. Does the always-on watch link (every 3 s, 60 s when down) keep a host awake or wake it (rule 296's owner
   decision)? By hand on the Mac. If yes: poll every 30 s while all of the host's tabs are idle.
5. Is `remote.status` small enough for a 3 s poll on a host with many tabs (it carries question words up to
   `QUESTION_MAX`)? If not, poll `remote.tabs` and ask `remote.status` only while a row is shown.
6. Confirmed while writing: `serve_remote::run` with `may_start` false (`--no-start`) relays as usual when a Flare
   runs, and answers `unavailable` when none does.
