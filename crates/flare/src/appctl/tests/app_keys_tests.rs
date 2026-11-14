//! The app keys of rule 1236 through the real controller: Ctrl+Shift+T reopens closed tabs in their places with their
//! agent resumed, Ctrl+Shift+N asks for a folder, Ctrl+Shift+R lists the linked machines, Ctrl+Shift+L opens the
//! browser panel with the address field holding the keys, Ctrl+Shift+E shows the folder in Explorer. A plain shell
//! keeps the same letters without Shift.

use flare_web::fake::FakeWeb;

use super::*;
use crate::menus_ui::NO_MACHINES;

fn shift_ctrl_key(key: &str) -> KeyEvent {
    KeyEvent::new(key, &format!("Key{key}")).ctrl().shift()
}

fn id_in(f: &Fx, cwd: &str) -> String {
    let i = f
        .cwds()
        .iter()
        .position(|c| c == cwd)
        .unwrap_or_else(|| panic!("no tab in {cwd}"));
    f.ids()[i].clone()
}

fn three_tabs() -> Fx {
    let f = fx_with(Opts {
        saved: vec![("C:\\a", None), ("C:\\b", None), ("C:\\c", None)],
        ..Opts::default()
    });
    f.start_restore_and_wait();
    f
}

#[test]
fn ctrl_shift_t_walks_back_through_the_closed_tabs_each_in_its_place() {
    let f = three_tabs();
    let b = id_in(&f, "C:\\b");
    f.ctl.borrow_mut().request_close(&b);
    let c = id_in(&f, "C:\\c");
    f.ctl.borrow_mut().request_close(&c);
    assert_eq!(f.cwds(), ["C:\\a"]);
    assert_eq!(f.ctl.borrow().closed_tabs().len(), 2);

    assert_eq!(f.key(shift_ctrl_key("T")), KeyOutcome::Consumed);
    assert_eq!(f.cwds(), ["C:\\a", "C:\\c"], "the last closed comes first");
    assert_eq!(f.key(shift_ctrl_key("T")), KeyOutcome::Consumed);
    assert_eq!(
        f.cwds(),
        ["C:\\a", "C:\\b", "C:\\c"],
        "back in its old place"
    );
    let front = f.ctl.borrow().front_id().unwrap();
    assert_eq!(front, id_in(&f, "C:\\b"), "the reopened tab is in front");
    let t = Instant::now();
    while f.made_with("C:\\b").len() < 2 && t.elapsed() < Duration::from_secs(15) {
        f.pump();
        std::thread::sleep(Duration::from_millis(5)); // sync-ok: test code waits for the spawn
    }
    let made: Vec<String> = f.sp.made().iter().map(|m| m.spec().cwd.clone()).collect();
    assert_eq!(
        f.made_with("C:\\b").len(),
        2,
        "a new shell for it: {made:?}"
    );

    // Nothing left to reopen: the key is still the app's, and nothing opens.
    assert_eq!(f.key(shift_ctrl_key("T")), KeyOutcome::Consumed);
    assert_eq!(f.cwds().len(), 3);
    // Ctrl+T without Shift is still a new tab.
    assert_eq!(
        f.key(KeyEvent::new("t", "KeyT").ctrl()),
        KeyOutcome::Consumed
    );
    assert_eq!(f.cwds().len(), 4);
}

#[test]
fn the_settings_tab_is_never_kept_as_a_closed_tab() {
    let f = three_tabs();
    f.ctl.borrow_mut().toggle_settings();
    let settings = f.ctl.borrow().front_id().unwrap();
    f.ctl.borrow_mut().close_now(&settings);
    assert!(f.ctl.borrow().closed_tabs().is_empty());
}

#[test]
fn a_closed_claude_tab_comes_back_with_its_session_and_codex_with_its_own_resume() {
    let lookup = FixedSessions::new(&[SESSION_A, SESSION_B]);
    let f = fx_with(Opts {
        saved: vec![
            ("C:\\p", None),
            ("C:\\c", Some(AgentKind::Claude)),
            ("C:\\x", Some(AgentKind::Codex)),
        ],
        lookup: lookup_of(&lookup),
        shells: Some(vec![resume_pwsh()]),
        ..Opts::default()
    });
    f.start_restore_and_wait();
    f.pump_until("the spawns", |f| {
        ["C:\\c", "C:\\x"]
            .iter()
            .all(|cwd| !f.made_with(cwd).is_empty())
    });
    let c = id_in(&f, "C:\\c");
    f.ctl.borrow_mut().close_now(&c);
    let x = id_in(&f, "C:\\x");
    f.ctl.borrow_mut().close_now(&x);
    let kept = f.ctl.borrow().closed_tabs().to_vec();
    assert_eq!(kept[0].agent, Some(AgentKind::Claude));
    assert_eq!(kept[1].agent, Some(AgentKind::Codex));

    // Codex first (closed last): `codex resume --last` in its folder.
    f.key(shift_ctrl_key("T"));
    f.pump_until("codex again", |f| f.made_with("C:\\x").len() == 2);
    let codex = &f.made_with("C:\\x")[1];
    assert!(env_of(codex, "FLARE_RESUME_ID").is_none());
    assert!(
        codex
            .spec()
            .args
            .last()
            .is_some_and(|a| a.ends_with("codex resume --last")),
        "{:?}",
        codex.spec().args
    );

    // Claude: the session it had (named by the resume, or found again as the newest no open tab holds).
    f.key(shift_ctrl_key("T"));
    f.pump_until("claude again", |f| f.made_with("C:\\c").len() == 2);
    let claude = &f.made_with("C:\\c")[1];
    assert_eq!(
        env_of(claude, "FLARE_RESUME_ID").as_deref(),
        Some(SESSION_A)
    );
    let back = id_in(&f, "C:\\c");
    assert_eq!(
        f.ctl.borrow().model().agent_kind(&back),
        Some(AgentKind::Claude),
        "it wears Claude's icon from the first frame"
    );
    assert_eq!(
        f.cwds(),
        ["C:\\p", "C:\\c", "C:\\x"],
        "both in their places"
    );
}

#[test]
fn a_claude_tab_whose_session_was_never_named_takes_the_newest_session_no_open_tab_holds() {
    let lookup = FixedSessions::new(&[SESSION_A, SESSION_B]);
    let f = fx_with(Opts {
        saved: vec![("C:\\c", Some(AgentKind::Claude))],
        lookup: lookup_of(&lookup),
        shells: Some(vec![resume_pwsh()]),
        ..Opts::default()
    });
    f.start_restore_and_wait();
    f.wait_made("C:\\c");
    // The open tab in C:\c resumed SESSION_A, the newest: the reopened tab must not take it too.
    f.ctl
        .borrow_mut()
        .closed
        .push(crate::appctl::app_keys_glue::ClosedTab {
            cwd: "C:\\d".into(),
            at: 0,
            agent: Some(AgentKind::Claude),
            session: None,
            machine: None,
        });
    f.key(shift_ctrl_key("T"));
    f.pump_until("the reopened tab", |f| !f.made_with("C:\\d").is_empty());
    assert_eq!(
        env_of(&f.made_with("C:\\d")[0], "FLARE_RESUME_ID").as_deref(),
        Some(SESSION_B)
    );
    assert!(lookup.asked().contains(&"C:\\d".to_string()));
    assert_eq!(f.cwds(), ["C:\\d", "C:\\c"], "back in its place");
}

#[test]
fn a_named_session_an_open_tab_took_since_is_not_resumed_twice() {
    let lookup = FixedSessions::new(&[SESSION_A, SESSION_B]);
    let f = fx_with(Opts {
        saved: vec![("C:\\c", Some(AgentKind::Claude))],
        lookup: lookup_of(&lookup),
        shells: Some(vec![resume_pwsh()]),
        ..Opts::default()
    });
    f.start_restore_and_wait();
    f.wait_made("C:\\c");
    // The closed tab had SESSION_A, but the open tab in C:\c resumed it since: two Claudes on one session never run.
    f.ctl
        .borrow_mut()
        .closed
        .push(crate::appctl::app_keys_glue::ClosedTab {
            cwd: "C:\\d".into(),
            at: 0,
            agent: Some(AgentKind::Claude),
            session: Some(SESSION_A.into()),
            machine: None,
        });
    f.key(shift_ctrl_key("T"));
    f.pump_until("the reopened tab", |f| !f.made_with("C:\\d").is_empty());
    assert_eq!(
        env_of(&f.made_with("C:\\d")[0], "FLARE_RESUME_ID").as_deref(),
        Some(SESSION_B)
    );
}

#[test]
fn ctrl_shift_n_asks_for_a_folder_and_opens_a_tab_there() {
    let f = fx_with(Opts {
        pick_answer: Some(PathBuf::from("C:\\picked")),
        ..Opts::default()
    });
    f.start_restore_and_wait();
    assert_eq!(f.key(shift_ctrl_key("N")), KeyOutcome::Consumed);
    f.pump_until("the picked tab", |f| f.cwds() == ["C:\\picked"]);
    assert_eq!(f.log.borrow().picks.len(), 1, "the picker asked once");
}

#[test]
fn the_folder_menu_shows_ctrl_shift_n_on_choose_a_folder() {
    let f = fx();
    f.start_restore_and_wait();
    f.ctl.borrow_mut().open_plus_menu(10.0, 10.0);
    let first = f.h.comp.get_menu_rows().row_data(0).unwrap();
    assert_eq!(first.label.as_str(), CHOOSE_FOLDER);
    assert_eq!(first.hint.as_str(), "Ctrl+Shift+N");
    // A pin clicked in the menu keeps the hint.
    f.ctl.borrow_mut().menu_trailing(1);
    let first = f.h.comp.get_menu_rows().row_data(0).unwrap();
    assert_eq!(first.hint.as_str(), "Ctrl+Shift+N");
}

fn mini() -> crate::machines::Machine {
    crate::machines::Machine {
        id: "mac".into(),
        name: "Mac Mini".into(),
        kind: Some(flare_tabs::tabs::Os::Mac),
        ssh: "mini".into(),
        user: None,
        flare: "flare".into(),
    }
}

fn menu_labels_now(f: &Fx) -> Vec<String> {
    let rows = f.h.comp.get_menu_rows();
    (0..rows.row_count())
        .filter_map(|i| rows.row_data(i))
        .map(|r| r.label.to_string())
        .collect()
}

#[test]
fn ctrl_shift_r_lists_the_linked_machines_and_a_pick_opens_a_tab_there() {
    let f = fx();
    f.start_restore_and_wait();
    // None linked: the menu says so, and the note does nothing.
    assert_eq!(f.key(shift_ctrl_key("R")), KeyOutcome::Consumed);
    assert!(f.h.comp.get_menu_open());
    assert_eq!(menu_labels_now(&f), [NO_MACHINES]);
    assert!(f.h.comp.get_menu_rows().row_data(0).unwrap().disabled);
    f.ctl.borrow_mut().close_menu();

    f.ctl.borrow_mut().on_msg(UiMsg::Machines(vec![mini()]));
    f.key(shift_ctrl_key("R"));
    assert_eq!(menu_labels_now(&f), ["Mac Mini"]);
    f.h.comp.invoke_menu_picked(0);
    let front = f.ctl.borrow().front_id().unwrap();
    let tag = f.ctl.borrow().model().remote_of(&front).cloned();
    assert_eq!(tag.map(|t| t.id), Some("mac".to_string()));
    assert!(!f.h.comp.get_menu_open());
}

#[test]
fn ctrl_shift_e_shows_the_folder_and_a_remote_tab_opens_nothing() {
    let f = three_tabs();
    assert_eq!(f.key(shift_ctrl_key("E")), KeyOutcome::Consumed);
    let t = Instant::now();
    while f.openers.records().is_empty() && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5)); // sync-ok: test code waits for the opener worker
    }
    let front_cwd = f.ctl.borrow().model().active().unwrap().cwd.clone();
    assert_eq!(f.openers.records(), [OpenedRecord::Folder(front_cwd)]);

    f.ctl.borrow_mut().on_msg(UiMsg::Machines(vec![mini()]));
    f.ctl
        .borrow_mut()
        .open_remote("mac", "/Users/max/proj", None, true);
    f.key(shift_ctrl_key("E"));
    std::thread::sleep(Duration::from_millis(50)); // sync-ok: test code gives a wrong opener time to show
    assert_eq!(f.openers.records().len(), 1, "a remote folder is not here");
}

#[test]
fn a_closed_remote_tab_comes_back_on_its_machine() {
    let f = three_tabs();
    f.ctl.borrow_mut().on_msg(UiMsg::Machines(vec![mini()]));
    let id = f
        .ctl
        .borrow_mut()
        .open_remote("mac", "/Users/max/proj", None, true);
    f.ctl.borrow_mut().close_now(&id);
    let kept = f.ctl.borrow().closed_tabs().last().cloned().unwrap();
    assert_eq!(kept.machine.as_deref(), Some("mac"));
    assert_eq!(kept.agent, None, "the host owns a remote session");
    f.key(shift_ctrl_key("T"));
    let front = f.ctl.borrow().front_id().unwrap();
    assert_eq!(
        f.ctl
            .borrow()
            .model()
            .remote_of(&front)
            .map(|t| t.id.clone()),
        Some("mac".to_string())
    );
    assert_eq!(
        f.ctl.borrow().model().active().unwrap().cwd,
        "/Users/max/proj"
    );
}

#[test]
fn ctrl_shift_l_opens_the_browser_panel_with_the_address_field_holding_the_keys() {
    let (f, _h, _) = live(Opts::default());
    f.ctl.borrow_mut().set_web_factory(Box::new(|sink| {
        let fake = FakeWeb::new(sink);
        Box::new(fake)
    }));
    let id = f.ctl.borrow().front_id().unwrap();
    assert_eq!(f.key(shift_ctrl_key("L")), KeyOutcome::Consumed);
    f.pump();
    f.h.advance(0);
    assert!(f.h.comp.get_panel_open(), "the panel shows");
    assert_eq!(
        f.ctl.borrow().panels.web_target(&id).as_deref(),
        Some("about:blank")
    );
    assert!(
        f.h.comp.get_panel_web_input_focused(),
        "the address field has the keys"
    );

    // Hidden, the page comes back as it was (it is not replaced by an empty one).
    f.ctl.borrow_mut().toggle_front_panel();
    f.pump();
    assert!(!f.h.comp.get_panel_open());
    f.key(shift_ctrl_key("L"));
    f.pump();
    f.h.advance(0);
    assert!(f.h.comp.get_panel_open());
    assert!(f.h.comp.get_panel_web_input_focused());
}

#[test]
fn the_plain_letters_stay_the_shells_in_a_plain_shell() {
    let (f, _h, _) = live(Opts::default());
    // Ctrl+T stays the app's new tab, as ever; the other four letters without Shift go to the shell.
    for key in ["n", "r", "l", "e"] {
        let upper = key.to_uppercase();
        let ev = KeyEvent::new(key, &format!("Key{upper}")).ctrl();
        assert_eq!(f.key(ev), KeyOutcome::Pty, "ctrl+{key}");
    }
    assert!(f.ctl.borrow().closed_tabs().is_empty());
    assert_eq!(f.cwds(), ["C:\\a"]);
}
