//! Remote sessions on the host's side, through the real window controller (rules 1302 to 1305 on top of 294 to 296): a
//! `remote.open` makes a hidden session that no strip, sidebar, badge or notification of this window shows; only hidden
//! sessions answer the remote ops; a working one holds the system awake, followed or not; its question rings only on its
//! opener; `remote.close` ends it at once; the quit ask names it for its opener; its agent cannot put anything on this
//! screen. The keep-awake is a counter, the opener a call on the window's own channel.

use flare_settings::agent_prefs::set_notify_ask_on;
use flare_surfaces::notify::Toast;
use flare_win::keep_awake::FakeAwake;
use serde_json::Value;

use super::*;
use crate::ctl_app::{CtlOp, CtlRequest, CtlRequestBox};
use crate::remote_host::RemoteHost;

fn ask(f: &Fx, op: CtlOp) -> Result<Value, flare_ctl::CtlError> {
    let (tx, rx) = std::sync::mpsc::channel();
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Ctl(CtlRequestBox(CtlRequest {
            op,
            deadline: Instant::now() + Duration::from_secs(5),
            reply: tx,
        })));
    rx.recv_timeout(Duration::from_secs(5)) // sync-ok: test code only
        .expect("an answer")
}

/// The window serves remote sessions, its keep-awake a counter.
fn serving(f: &Fx) -> (Arc<RemoteHost>, FakeAwake) {
    let host = RemoteHost::with_key("k".into());
    let awake = FakeAwake::new();
    let mut c = f.ctl.borrow_mut();
    c.set_remote_host(Arc::clone(&host));
    c.set_keep_awake(Arc::new(awake.clone()));
    (host, awake)
}

/// A `remote.open` of `cwd` from the machine `opener` (None: a bridge that named none), its shell started.
fn open_from(f: &Fx, cwd: &str, opener: Option<&str>, open_id: Option<&str>) -> String {
    let v = ask(
        f,
        CtlOp::RemoteOpen {
            folder: cwd.into(),
            resume: None,
            open_id: open_id.map(str::to_string),
            opener: opener.map(str::to_string),
        },
    )
    .expect("the open");
    let id = v["tab"].as_str().expect("a tab").to_string();
    f.pump_until("the hidden session's shell", |f| {
        f.sp.made().iter().any(|m| m.spec().cwd == cwd)
    });
    id
}

fn hidden(f: &Fx, cwd: &str) -> String {
    open_from(f, cwd, Some("Mac Mini"), None)
}

/// A `remote.attach` of the hidden session `tab` on link `conn`. Its shell is made before its session is in the
/// manager (a worker starts it), so `that tab's shell is not running` is tried again for a while; it follows nothing.
fn attach(f: &Fx, tab: &str, conn: u64, viewer: Option<&str>) -> Value {
    let t = Instant::now();
    loop {
        let op = CtlOp::RemoteAttach {
            tab: tab.into(),
            conn,
            viewer: viewer.map(str::to_string),
        };
        match ask(f, op) {
            Ok(v) => return v,
            Err(e)
                if e.code == flare_ctl::code::UNAVAILABLE
                    && t.elapsed() < Duration::from_secs(5) =>
            {
                f.step(20);
            }
            Err(e) => panic!("attach: {e:?}"),
        }
    }
}

/// A Claude in the session `id` (shell in `cwd`), idle.
fn claude_in(f: &Fx, id: &str, cwd: &str) {
    f.say(cwd, &title(T_IDLE));
    f.pump_until("the agent", |f| f.ctl.borrow_mut().hub_mut().agent_here(id));
}

fn working(f: &Fx, id: &str) -> bool {
    f.ctl.borrow_mut().hub_mut().working(id)
}

fn until(f: &Fx, what: &str, mut cond: impl FnMut(&Fx) -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(5) {
        f.step(50);
        if cond(f) {
            return;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    panic!("timed out waiting for {what}");
}

fn hidden_ids(f: &Fx) -> Vec<String> {
    f.ctl
        .borrow()
        .model()
        .hidden()
        .iter()
        .map(|h| h.tab.id.clone())
        .collect()
}

/// Rule 1302: `remote.open` makes a hidden session with the opener's name (or "another computer" when the bridge named
/// none): no strip tab, no sidebar row, nothing in front moves, and `tabs.json` keeps it hidden.
#[test]
fn a_remote_open_makes_a_hidden_session_with_its_opener_name() {
    let f = Fx::new();
    let ids = f.tabs(&["C:\\work"]);
    serving(&f);
    let rows = f.rows();
    let id = hidden(&f, "C:\\there");
    assert_eq!(f.ids(), ids, "the strip is as it was");
    assert_eq!(f.rows(), rows, "no row for it");
    assert_eq!(f.front(), ids[0], "nothing in front moved");
    let c = f.ctl.borrow();
    let h = c.model().hidden_of(&id).expect("a hidden session");
    assert_eq!(
        (h.opener.as_str(), h.tab.cwd.as_str()),
        ("Mac Mini", "C:\\there")
    );
    let saved = c.model().saved();
    assert_eq!(saved.tabs.len(), 1);
    assert_eq!(saved.hidden.len(), 1);
    assert_eq!(saved.hidden[0].opener, "Mac Mini");
    drop(c);
    let unnamed = open_from(&f, "C:\\else", None, None);
    assert_eq!(
        f.ctl.borrow().model().hidden_of(&unnamed).unwrap().opener,
        "another computer"
    );
}

/// Rule 1210 with 1302: a `remote.open` sent again with the same open id is answered with the session the first one
/// opened; another id, or none, opens a session of its own; the strip never grows. Max 2026-11-19: a 51st is refused.
#[test]
fn a_remote_open_sent_again_with_its_open_id_opens_no_second_session() {
    let f = Fx::new();
    let strip = f.tabs(&["C:\\work"]);
    serving(&f);
    let first = open_from(&f, "C:\\there", Some("Mac Mini"), Some("viewer-1"));
    let again = ask(
        &f,
        CtlOp::RemoteOpen {
            folder: "C:\\there".into(),
            resume: None,
            open_id: Some("viewer-1".into()),
            opener: Some("Mac Mini".into()),
        },
    )
    .expect("the open again");
    assert_eq!(again["tab"], first.as_str(), "the same session again");
    assert_eq!(hidden_ids(&f).len(), 1, "one session for one open");
    assert_ne!(
        open_from(&f, "C:\\two", Some("Mac Mini"), Some("viewer-2")),
        first
    );
    assert_ne!(open_from(&f, "C:\\three", Some("Mac Mini"), None), first);
    assert_eq!(hidden_ids(&f).len(), 3);
    assert_eq!(f.ids(), strip);
    for i in 3..flare_tabs::restore::MAX_HIDDEN {
        let v = ask(
            &f,
            CtlOp::RemoteOpen {
                folder: format!("C:\\more{i}"),
                resume: None,
                open_id: None,
                opener: Some("Mac Mini".into()),
            },
        );
        assert!(v.is_ok(), "{i}");
    }
    let refused = ask(
        &f,
        CtlOp::RemoteOpen {
            folder: "C:\\fifty-one".into(),
            resume: None,
            open_id: None,
            opener: Some("Mac Mini".into()),
        },
    )
    .expect_err("the 51st");
    assert_eq!(refused.code, flare_ctl::code::BUSY);
    assert!(refused.message.contains("50"), "{}", refused.message);
    // A lost answer of an open made before the cap still gets its session.
    let again = ask(
        &f,
        CtlOp::RemoteOpen {
            folder: "C:\\there".into(),
            resume: None,
            open_id: Some("viewer-1".into()),
            opener: Some("Mac Mini".into()),
        },
    )
    .expect("a retried open is never refused");
    assert_eq!(again["tab"], first.as_str());
}

/// Rule 1303: `remote.status` and `remote.tabs` list only hidden sessions; a strip tab of this window and a remote
/// tab it shows are never listed.
#[test]
fn remote_status_and_tabs_list_only_hidden_sessions() {
    let f = Fx::new();
    let strip = f.tabs(&["C:\\work"]);
    serving(&f);
    let viewer = f
        .ctl
        .borrow_mut()
        .open_remote("mac", "/Users/max/p", None, false);
    let id = hidden(&f, "C:\\there");
    for op in [CtlOp::RemoteStatus, CtlOp::RemoteTabs] {
        let v = ask(&f, op).expect("the list");
        let tabs: Vec<&str> = v["tabs"]
            .as_array()
            .expect("tabs")
            .iter()
            .map(|t| t["tab"].as_str().unwrap())
            .collect();
        assert_eq!(tabs, [id.as_str()]);
        assert!(!tabs.contains(&strip[0].as_str()) && !tabs.contains(&viewer.as_str()));
    }
    let v = ask(&f, CtlOp::RemoteTabs).expect("tabs");
    assert_eq!(v["tabs"][0]["cwd"], "C:\\there");
    assert_eq!(v["tabs"][0]["title"], "there");
}

/// Rules 1303 and 1210: every remote op that names a tab answers `no_tab` for a strip tab of this window and for a
/// remote tab it shows (no mirror chains); the hidden session answers. The worker-side ops need a follow, which only an
/// attach of a hidden session makes.
#[test]
fn every_remote_op_refuses_a_tab_that_is_not_hidden() {
    let f = Fx::new();
    let strip = f.tabs(&["C:\\work"]);
    let (host, _) = serving(&f);
    let viewer = f
        .ctl
        .borrow_mut()
        .open_remote("mac", "/Users/max/p", None, false);
    let ops = |tab: &str| {
        vec![
            CtlOp::RemoteAttach {
                tab: tab.into(),
                conn: 9,
                viewer: None,
            },
            CtlOp::RemoteInput {
                tab: tab.into(),
                intents: vec![flare_term::handle::RemoteIntent::Text("x".into())],
            },
            CtlOp::RemoteResize {
                tab: tab.into(),
                cols: 61,
                rows: 19,
                viewer: None,
            },
            CtlOp::RemoteRelease { tab: tab.into() },
            CtlOp::RemoteClose { tab: tab.into() },
        ]
    };
    for tab in [strip[0].as_str(), viewer.as_str()] {
        for op in ops(tab) {
            let what = format!("{op:?}");
            let e = ask(&f, op).expect_err(&what);
            assert_eq!(e.code, flare_ctl::code::NO_TAB, "{what} on {tab}");
        }
        assert!(!host.is_followed(tab), "{tab} is never followed");
    }
    assert_eq!(f.ids().len(), 2, "nothing closed");
    let id = hidden(&f, "C:\\there");
    attach(&f, &id, 9, None);
    assert!(host.is_followed(&id));
    ask(
        &f,
        CtlOp::RemoteInput {
            tab: id.clone(),
            intents: vec![flare_term::handle::RemoteIntent::Text("x".into())],
        },
    )
    .expect("input");
}

/// Rule 1305: `remote.close` ends a hidden session at once with an agent at work, never asking on this screen, and it
/// never joins this window's reopen list (rule 1236).
#[test]
fn a_remote_close_ends_a_hidden_session_without_asking_on_the_host() {
    let f = Fx::new();
    let strip = f.tabs(&["C:\\work"]);
    let (host, _) = serving(&f);
    let id = hidden(&f, "C:\\there");
    claude_in(&f, &id, "C:\\there");
    f.say("C:\\there", W);
    until(&f, "the work", |f| working(f, &id));
    attach(&f, &id, 3, None);
    let v = ask(&f, CtlOp::RemoteClose { tab: id.clone() }).expect("the close");
    assert_eq!(v["closed"], true);
    assert!(f.ctl.borrow().ask_text().is_none(), "no ask here");
    assert!(hidden_ids(&f).is_empty());
    // The kill reaches the shell on a worker of its own.
    f.pump_until("its shell ended", |f| f.shell("C:\\there").killed());
    assert!(!host.is_followed(&id), "its follower hears no_tab");
    f.ctl.borrow_mut().reopen_closed_tab();
    f.pump();
    assert_eq!(f.ids(), strip, "Ctrl+Shift+T opens nothing");
    assert!(f.ctl.borrow().model().saved().hidden.is_empty());
}

/// Rules 1302 and 1304: a hidden session's question never rings on this machine, followed or not: no notification,
/// no taskbar badge, never a question tab here; the host's status still says it to the opener. A strip tab's question
/// still rings here.
#[test]
fn a_hidden_sessions_question_never_rings_on_the_host_followed_or_not() {
    let f = Fx::new();
    set_notify_ask_on(&f.prefs, true);
    let got: Rc<RefCell<Vec<Toast>>> = Rc::default();
    let sink = Rc::clone(&got);
    f.ctl
        .borrow_mut()
        .set_toast_sink(Rc::new(move |t: &Toast| sink.borrow_mut().push(t.clone())));
    let strip = f.tabs(&["C:\\work", "C:\\other"]);
    f.visit(1);
    serving(&f);
    let id = hidden(&f, "C:\\there");
    claude_in(&f, &id, "C:\\there");
    f.say("C:\\there", Q);
    until(&f, "the hidden question", |f| {
        let v = ask(f, CtlOp::RemoteStatus).expect("status");
        v["tabs"][0]["state"] == "question"
    });
    for _ in 0..20 {
        f.step(100);
    }
    assert!(
        got.borrow().is_empty(),
        "no notification: {:?}",
        got.borrow()
    );
    assert!(
        f.badge.last().is_none_or(|b| b.0.is_none()),
        "no badge: {:?}",
        f.badge.last()
    );
    assert!(f.ctl.borrow_mut().hub_mut().question_tabs().is_empty());
    // Followed now: still nothing here.
    attach(&f, &id, 4, None);
    for _ in 0..10 {
        f.step(100);
    }
    assert!(got.borrow().is_empty());
    // A strip tab's question still rings.
    claude(&f, "C:\\work");
    f.say("C:\\work", Q);
    until(&f, "the strip tab's notification", |_| {
        !got.borrow().is_empty()
    });
    assert_eq!(got.borrow()[0].tab, strip[0]);
}

/// Rule 294 with 1303: `remote.status` says each hidden session's state word from this window's own marks, with the
/// agent, the question's words only while it asks, and `sized: viewer`.
#[test]
fn remote_status_says_the_windows_own_marks_as_words() {
    let f = Fx::new();
    f.tabs(&["C:\\work"]);
    serving(&f);
    let id = hidden(&f, "C:\\there");
    claude_in(&f, &id, "C:\\there");
    f.say("C:\\there", W);
    until(&f, "the work", |f| working(f, &id));
    let v = ask(&f, CtlOp::RemoteStatus).expect("the status");
    let t = &v["tabs"][0];
    assert_eq!(t["tab"], id.as_str());
    assert_eq!(t["state"], "working");
    assert_eq!(t["kind"], "claude");
    assert_eq!(t["folder"], "C:\\there");
    assert_eq!(t["sized"], "viewer", "its opener sizes it from birth");
    assert!(t.get("question_text").is_none());
    f.say("C:\\there", Q);
    f.say("C:\\there", &paint_owned(&screens("run2-01-question-1")));
    until(&f, "the question's words", |f| {
        let v = ask(f, CtlOp::RemoteStatus).expect("the status");
        v["tabs"][0]["state"] == "question" && v["tabs"][0]["question_text"] == "Pick a colour?"
    });
    // The words go to the opener only: nothing is said to Sonara on this machine (rule 1304).
    for _ in 0..10 {
        f.step(100);
    }
    assert!(f.posts.lock().unwrap().is_empty(), "no question post here");
}

/// Rules 1302 and 280: a hidden session is born remote-sized; the opener's resize sizes it and is saved; its last
/// follower going leaves the size as it is (never this window's), and the session runs on.
#[test]
fn a_hidden_session_keeps_its_size_when_its_last_follower_goes() {
    let f = Fx::new();
    f.tabs(&["C:\\work"]);
    let (host, _) = serving(&f);
    let id = hidden(&f, "C:\\there");
    let size = |f: &Fx| f.ctl.borrow().handle_of(&id).expect("a handle").size();
    attach(&f, &id, 7, None);
    ask(
        &f,
        CtlOp::RemoteResize {
            tab: id.clone(),
            cols: 61,
            rows: 19,
            viewer: None,
        },
    )
    .expect("resize");
    assert_eq!(size(&f), (61, 19));
    assert_eq!(
        f.ctl.borrow().model().hidden_of(&id).unwrap().size,
        Some((61, 19)),
        "saved with it"
    );
    assert_eq!(host.gone(7), std::slice::from_ref(&id));
    ask(&f, CtlOp::RemoteRelease { tab: id.clone() }).expect("the release");
    assert_eq!(size(&f), (61, 19), "the opener's size stays");
    f.ctl.borrow_mut().on_grid_dims(120, 40);
    assert_eq!(size(&f), (61, 19), "this window's refit passes it by");
    let v = ask(&f, CtlOp::RemoteStatus).expect("status");
    assert_eq!(v["tabs"][0]["sized"], "viewer");
    assert!(!f.shell("C:\\there").killed(), "it runs on");
}

/// Rule 1302 (amends 296): a working hidden session holds the system awake with no follower at all; the end of the work
/// lets go, new work holds again, a strip tab's work holds nothing.
#[test]
fn a_working_hidden_session_holds_the_host_awake_with_no_follower() {
    let f = Fx::new();
    f.tabs(&["C:\\work"]);
    let (_host, awake) = serving(&f);
    claude(&f, "C:\\work");
    f.say("C:\\work", W);
    until(&f, "the strip tab's work", |f| f.working(0));
    assert_eq!(awake.held(), 0, "a strip tab's work holds nothing");
    let id = hidden(&f, "C:\\there");
    claude_in(&f, &id, "C:\\there");
    f.say("C:\\there", W);
    until(&f, "the hold", |_| awake.held() == 1);
    f.say("C:\\there", D);
    until(&f, "the hold let go", |_| awake.held() == 0);
    f.say("C:\\there", W);
    until(&f, "the hold again", |_| awake.held() == 1);
    assert!(f.ctl.borrow().holds_awake());
}

/// Rule 1302: the quit ask counts a hidden session at work and names it "<folder> (for <opener>)".
#[test]
fn the_quit_ask_names_a_working_hidden_session_for_its_opener() {
    let f = Fx::new();
    f.tabs(&["C:\\work"]);
    serving(&f);
    let id = hidden(&f, "C:\\there");
    claude_in(&f, &id, "C:\\there");
    f.say("C:\\there", W);
    until(&f, "the work", |f| working(f, &id));
    let d = f.ctl.borrow_mut().on_close_requested();
    assert!(matches!(d, crate::appctl::CloseDecision::KeepOpen));
    let (_, body, _) = f.ctl.borrow().ask_text().expect("the quit ask");
    // The body is markdown: its parentheses come escaped.
    assert!(
        body.replace('\\', "").contains("there (for Mac Mini)"),
        "{body}"
    );
}

/// Rule 1302 (Max 2026-11-19): a hidden session's agent cannot put anything on this screen: a panel, a text, a badge or
/// the browser answer `unavailable` naming the opener, and nothing in front moves. Its hooks' `panel.state`, its own
/// `tabs.list` and its folder still answer.
#[test]
fn a_hidden_tabs_agent_cannot_open_a_panel_or_front_the_window() {
    let f = Fx::new();
    let strip = f.tabs(&["C:\\work"]);
    serving(&f);
    let id = hidden(&f, "C:\\there");
    let refused = [
        CtlOp::ShowText {
            tab: id.clone(),
            text: "hi".into(),
            title: None,
        },
        CtlOp::BrowserOpen {
            tab: id.clone(),
            url: "https://example.com/".into(),
        },
        CtlOp::BadgeSet {
            tab: id.clone(),
            source: flare_tabs::decor::Source::Agent,
            key: "k".into(),
            icon: "star".into(),
            tip: "t".into(),
            tone: flare_tabs::decor::Tone::Ink,
            generation: 0,
            own: true,
        },
        CtlOp::PanelClose { tab: id.clone() },
    ];
    for op in refused {
        let what = format!("{op:?}");
        let e = ask(&f, op).expect_err(&what);
        assert_eq!(e.code, flare_ctl::code::UNAVAILABLE, "{what}");
        assert!(e.message.contains("shows on Mac Mini"), "{}", e.message);
    }
    assert_eq!(f.front(), strip[0], "nothing in front moved");
    assert!(f.ctl.borrow().panel_snap(&id).is_none_or(|s| !s.open));
    let st = ask(
        &f,
        CtlOp::PanelState {
            tab: id.clone(),
            cwd: Some("C:\\there\\sub".into()),
        },
    )
    .expect("its hooks' panel.state");
    assert_eq!(st["open"], false);
    assert_eq!(
        f.ctl.borrow().hub().agent_folder(&id),
        Some("C:\\there\\sub")
    );
    let own = ask(
        &f,
        CtlOp::Tabs {
            own: Some(id.clone()),
        },
    )
    .expect("its own tab");
    assert_eq!(own["tabs"][0]["id"], id.as_str());
    assert_eq!(own["tabs"][0]["active"], false);
    let cwd = ask(&f, CtlOp::TabCwd { tab: id.clone() }).expect("its folder");
    assert_eq!(cwd["cwd"], "C:\\there");
    // Its agent's `open.remote` would put a tab on this screen: refused too; `flare open --on` (no caller) is not.
    let e = ask(
        &f,
        CtlOp::OpenRemote {
            machine: "mac".into(),
            folder: "/Users/max".into(),
            resume: None,
            front: false,
            caller: Some(id.clone()),
        },
    )
    .expect_err("an open from the hidden session");
    assert_eq!(e.code, flare_ctl::code::UNAVAILABLE);
    assert!(e.message.contains("shows on Mac Mini"), "{}", e.message);
    assert_eq!(f.ids(), strip, "no tab opened here");
}

/// The ops a plugin may aim at a tab it named, on `tab`; `own` says whether it called with that tab's own token.
fn named_ops(tab: &str, own: bool) -> Vec<CtlOp> {
    let plugin = flare_tabs::decor::Source::Plugin("p".into());
    vec![
        CtlOp::BadgeSet {
            tab: tab.into(),
            source: plugin.clone(),
            key: "k".into(),
            icon: "star".into(),
            tip: "t".into(),
            tone: flare_tabs::decor::Tone::Ink,
            generation: 0,
            own,
        },
        CtlOp::ProgressSet {
            tab: tab.into(),
            source: plugin,
            key: "k".into(),
            value: Some(0.5),
            label: String::new(),
            generation: 0,
            own,
        },
        CtlOp::ShowMod {
            tab: tab.into(),
            mod_id: "p".into(),
            own,
            generation: 0,
        },
        CtlOp::BarShow {
            plugin: "p".into(),
            tab: tab.into(),
            generation: 0,
            own,
        },
        CtlOp::FillPage {
            tab: tab.into(),
            own,
        },
        CtlOp::Fill {
            plugin: "p".into(),
            plugin_name: "P".into(),
            tab: tab.into(),
            login_name: "x".into(),
            login_uris: vec!["https://example.com/".into()],
            values: flare_mods::fill::Values::default(),
            via_call: false,
            generation: 0,
            own,
        },
    ]
}

/// Rule 1306: a plugin that names a hidden session in a badge, progress, panel, bar or fill call gets the very answer a
/// tab that does not exist gets, byte for byte, so it learns neither that the session exists nor its opener's name.
/// Only a call made with the session's own token hears `unavailable` naming the opener.
#[test]
fn a_plugin_naming_a_hidden_tab_hears_what_an_unknown_tab_hears() {
    let f = Fx::new();
    let strip = f.tabs(&["C:\\work"]);
    serving(&f);
    let id = hidden(&f, "C:\\there");
    for (on_hidden, on_unknown) in named_ops(&id, false)
        .into_iter()
        .zip(named_ops("no-such-tab", false))
    {
        let what = format!("{on_hidden:?}");
        let e = ask(&f, on_hidden).expect_err(&what);
        let unknown = ask(&f, on_unknown).expect_err(&what);
        assert_eq!(e, unknown, "{what}");
        assert!(!e.message.contains("Mac Mini"), "{what}: {}", e.message);
    }
    // The first two reach the tab check first: the plain `no_tab`.
    for op in named_ops(&id, false).into_iter().take(2) {
        assert_eq!(
            ask(&f, op).unwrap_err(),
            flare_ctl::CtlError::new(flare_ctl::code::NO_TAB, "that tab is closed")
        );
    }
    // Its own token: `unavailable`, naming the opener.
    for op in named_ops(&id, true) {
        let what = format!("{op:?}");
        let e = ask(&f, op).expect_err(&what);
        assert_eq!(e.code, flare_ctl::code::UNAVAILABLE, "{what}");
        assert!(e.message.contains("shows on Mac Mini"), "{}", e.message);
    }
    assert!(!f.ctl.borrow().has_decor(&id));
    assert_eq!(f.ids(), strip);
}

/// Rule 1302: plugins never see a hidden session: `tabs.list` for a plugin with `tabs` (and the `tabs` and `agent`
/// events built from it) and `tabs.list` of every tab leave it out.
#[test]
fn plugins_tabs_list_and_events_leave_hidden_sessions_out() {
    let f = Fx::new();
    let strip = f.tabs(&["C:\\work"]);
    serving(&f);
    let id = hidden(&f, "C:\\there");
    claude_in(&f, &id, "C:\\there");
    let facts: Vec<String> = f
        .ctl
        .borrow()
        .tab_facts()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(facts, strip);
    let wide = ask(&f, CtlOp::TabsWide).expect("tabs.list wide");
    assert_eq!(wide["tabs"].as_array().unwrap().len(), 1);
    let all = ask(&f, CtlOp::Tabs { own: None }).expect("tabs.list");
    assert!(
        all["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["id"] != id.as_str())
    );
    assert!(
        f.ctl.borrow().dictation_tabs().iter().all(|t| t.tab != id),
        "dictation never routes words to it"
    );
}

/// Rule 1302: a hidden session whose shell ends goes: no session is kept with no shell, its followers hear `no_tab`,
/// and nothing of it is saved.
#[test]
fn a_hidden_session_whose_shell_ends_goes() {
    let f = Fx::new();
    let strip = f.tabs(&["C:\\work"]);
    let (host, _) = serving(&f);
    let id = hidden(&f, "C:\\there");
    attach(&f, &id, 2, Some("Mac Studio"));
    assert_eq!(
        f.ctl.borrow().model().hidden_of(&id).unwrap().opener,
        "Mac Studio",
        "the attach renames the opener"
    );
    f.shell("C:\\there").exit(0);
    f.pump_until("the session gone", |f| hidden_ids(f).is_empty());
    assert!(!host.is_followed(&id));
    assert_eq!(f.ids(), strip);
    assert!(f.ctl.borrow().model().saved().hidden.is_empty());
}

/// A Claude tab in `cwd` of the strip, idle.
fn claude(f: &Fx, cwd: &str) {
    f.say(cwd, &title(T_IDLE));
    f.pump_until("the agent", |f| {
        let id = f.ids()[f.cwds().iter().position(|c| c == cwd).unwrap()].clone();
        f.ctl.borrow_mut().hub_mut().agent_here(&id)
    });
}

/// Rule 1302: a hidden session whose shell never starts goes, so its opener hears `no_tab` instead of waiting for it.
#[test]
fn a_hidden_session_whose_shell_never_starts_goes() {
    let f = Fx::new();
    let strip = f.tabs(&["C:\\work"]);
    serving(&f);
    f.sp.fail_next_spawn();
    ask(
        &f,
        CtlOp::RemoteOpen {
            folder: "C:\\there".into(),
            resume: None,
            open_id: None,
            opener: Some("Mac Mini".into()),
        },
    )
    .expect("the open answers");
    f.pump_until("the session gone", |f| hidden_ids(f).is_empty());
    assert_eq!(f.ids(), strip);
}

/// Rules 1302 and 1286 through the window: a hidden session saved in `tabs.json` comes back hidden (no placeholder,
/// no strip row), at its opener's saved size and remote-sized, resuming its used Claude session; `remote.status` lists
/// it with its opener; and its question rings nowhere here.
#[test]
fn a_restored_hidden_session_comes_back_hidden_at_its_size_and_resumed() {
    let lookup = FixedSessions::new(&[SESSION_A]);
    let l: Arc<dyn SessionLookup> = lookup.clone();
    let f = Fx::with(
        Opts {
            saved: vec![("C:\\p", None)],
            hidden: vec![flare_tabs::restore::SavedHidden {
                cwd: "C:\\h".into(),
                agent: Some(AgentKind::Claude),
                session: Some(SESSION_A.into()),
                opener: "Mac Mini".into(),
                open_id: Some("o-1".into()),
                size: Some((61, 19)),
            }],
            lookup: Some(l),
            shells: Some(vec![resume_pwsh()]),
            ..Opts::default()
        },
        |_| {},
    );
    set_notify_ask_on(&f.prefs, true);
    let got: Rc<RefCell<Vec<Toast>>> = Rc::default();
    let sink = Rc::clone(&got);
    f.ctl
        .borrow_mut()
        .set_toast_sink(Rc::new(move |t: &Toast| sink.borrow_mut().push(t.clone())));
    serving(&f);
    assert_eq!(f.rows(), 1, "no placeholder for the hidden session");
    AppCtl::start_restore(&f.ctl);
    f.pump_until("the restore", |f| f.ctl.borrow().model().restore_done());
    // Wait for these two by folder: the home shell can be made first, so a count of two is not yet the hidden one.
    f.pump_until("both shells", |f| {
        let made = f.sp.made();
        ["C:\\p", "C:\\h"]
            .iter()
            .all(|c| made.iter().any(|m| m.spec().cwd == *c))
    });
    assert_eq!(f.rows(), 1, "still one row");
    let ids = hidden_ids(&f);
    assert_eq!(ids.len(), 1);
    let id = ids[0].clone();
    assert_eq!(
        env_of(&f.shell("C:\\h"), "FLARE_RESUME_ID").as_deref(),
        Some(SESSION_A),
        "its used session is resumed"
    );
    assert_eq!(
        f.ctl.borrow().handle_of(&id).expect("a handle").size(),
        (61, 19),
        "the opener's saved size"
    );
    let v = ask(&f, CtlOp::RemoteStatus).expect("status");
    assert_eq!(v["tabs"][0]["tab"], id.as_str());
    assert_eq!(v["tabs"][0]["sized"], "viewer");
    {
        let c = f.ctl.borrow();
        let h = c.model().hidden_of(&id).unwrap();
        assert_eq!(
            (h.opener.as_str(), h.open_id.as_deref()),
            ("Mac Mini", Some("o-1"))
        );
    }
    f.say("C:\\h", &title(T_IDLE));
    f.say("C:\\h", Q);
    until(&f, "the question", |f| {
        let v = ask(f, CtlOp::RemoteStatus).expect("status");
        v["tabs"][0]["state"] == "question"
    });
    for _ in 0..10 {
        f.step(100);
    }
    assert!(got.borrow().is_empty(), "no notification here");
    assert!(
        f.badge.last().is_none_or(|b| b.0.is_none()),
        "no badge here"
    );
}
