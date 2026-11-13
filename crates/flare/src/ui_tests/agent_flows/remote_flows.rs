//! Remote sessions PR 4 through the real window controller, on the host's side: a tab a remote viewer follows holds the
//! system awake while its agent works and lets go when the work stops or the viewer goes (rule 296); its question is
//! the viewer's to announce, never a notification here (rule 295); and `remote.status` says the window's own words
//! (rule 294). The keep-awake is a counter, the viewer a call on the window's own channel.

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

/// A Claude tab in `cwd`, idle.
fn claude(f: &Fx, cwd: &str) {
    f.say(cwd, &title(T_IDLE));
    f.pump_until("the agent", |f| {
        let id = f.ids()[f.cwds().iter().position(|c| c == cwd).unwrap()].clone();
        f.ctl.borrow_mut().hub_mut().agent_here(&id)
    });
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

/// Rule 296: work in a tab nobody follows holds nothing; a viewer's attach while it works holds the system awake; the
/// end of the work lets go, new work holds again, and the last viewer going lets go.
#[test]
fn a_followed_tab_holds_the_host_awake_while_it_works_and_lets_go_after() {
    let f = Fx::new();
    let ids = f.tabs(&["C:\\work", "C:\\other"]);
    f.visit(1);
    let (host, awake) = serving(&f);
    claude(&f, "C:\\work");
    f.say("C:\\work", W);
    until(&f, "the work", |f| f.working(0));
    assert_eq!(
        awake.held(),
        0,
        "nobody follows the tab: the system sleeps as usual"
    );
    ask(
        &f,
        CtlOp::RemoteAttach {
            tab: ids[0].clone(),
            conn: 7,
        },
    )
    .expect("the attach");
    assert_eq!(awake.held(), 1, "a followed tab at work holds the system");
    assert!(f.ctl.borrow().holds_awake());
    f.say("C:\\work", D);
    until(&f, "the hold let go", |_| awake.held() == 0);
    f.say("C:\\work", W);
    until(&f, "the hold again", |_| awake.held() == 1);
    // The viewer's link drops: the bridge forgets its follows and the window takes the size back.
    assert_eq!(host.gone(7), [ids[0].clone()]);
    ask(
        &f,
        CtlOp::RemoteRelease {
            tab: ids[0].clone(),
        },
    )
    .expect("the release");
    assert_eq!(awake.held(), 0, "no viewer, no hold");
    assert_eq!(awake.taken(), 2);
    assert!(f.working(0), "the session goes on");
}

/// Rule 295: a question in a tab a remote viewer follows is not notified on the host (the viewer says it); a question
/// in a tab nobody follows still is.
#[test]
fn a_question_in_a_followed_tab_is_the_viewers_to_notify() {
    let f = Fx::new();
    set_notify_ask_on(&f.prefs, true);
    let got: Rc<RefCell<Vec<Toast>>> = Rc::default();
    let sink = Rc::clone(&got);
    f.ctl
        .borrow_mut()
        .set_toast_sink(Rc::new(move |t: &Toast| sink.borrow_mut().push(t.clone())));
    let ids = f.tabs(&["C:\\work", "C:\\other", "C:\\third"]);
    f.visit(1);
    let (_host, _) = serving(&f);
    claude(&f, "C:\\work");
    claude(&f, "C:\\third");
    ask(
        &f,
        CtlOp::RemoteAttach {
            tab: ids[0].clone(),
            conn: 3,
        },
    )
    .expect("the attach");
    f.say("C:\\work", Q);
    f.say("C:\\third", Q);
    until(&f, "the unfollowed tab's notification", |_| {
        !got.borrow().is_empty()
    });
    for _ in 0..20 {
        f.step(100);
    }
    let all = got.borrow().clone();
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0].tab, ids[2]);
    // The followed tab still asks on the host's own strip.
    assert_eq!(
        f.ctl.borrow_mut().hub_mut().question_tabs(),
        [ids[0].clone(), ids[2].clone()]
    );
}

/// Rule 294: `remote.status` says each shell tab's state word from this window's own marks, with the agent, and the
/// question's words only while it asks.
#[test]
fn remote_status_says_the_windows_own_marks_as_words() {
    let f = Fx::new();
    let ids = f.tabs(&["C:\\work", "C:\\other"]);
    f.visit(1);
    serving(&f);
    claude(&f, "C:\\work");
    f.say("C:\\work", W);
    until(&f, "the work", |f| f.working(0));
    let v = ask(&f, CtlOp::RemoteStatus).expect("the status");
    let tabs = v["tabs"].as_array().expect("tabs");
    let work = tabs
        .iter()
        .find(|t| t["tab"] == ids[0].as_str())
        .expect("the tab");
    assert_eq!(work["state"], "working");
    assert_eq!(work["kind"], "claude");
    assert_eq!(work["folder"], "C:\\work");
    assert!(work.get("question_text").is_none());
    let other = tabs
        .iter()
        .find(|t| t["tab"] == ids[1].as_str())
        .expect("the other tab");
    assert_eq!(other["state"], "idle");
    f.say("C:\\work", Q);
    f.say("C:\\work", &paint_owned(&screens("run2-01-question-1")));
    until(&f, "the question's words", |f| {
        let v = ask(f, CtlOp::RemoteStatus).expect("the status");
        v["tabs"][0]["state"] == "question" && v["tabs"][0]["question_text"] == "Pick a colour?"
    });
}

/// Rule 1210: a `remote.open` sent again with the same open id (its answer was lost to a broken link) is answered with
/// the tab the first one opened, and no second tab opens; another id, or none, opens a tab of its own.
#[test]
fn a_remote_open_sent_again_with_its_open_id_opens_no_second_tab() {
    let f = Fx::new();
    f.tabs(&["C:\\work"]);
    serving(&f);
    let open = |oid: Option<&str>| {
        ask(
            &f,
            CtlOp::RemoteOpen {
                folder: "C:\\there".into(),
                resume: None,
                open_id: oid.map(str::to_string),
            },
        )
        .expect("the open")["tab"]
            .as_str()
            .expect("a tab")
            .to_string()
    };
    let before = f.ids().len();
    let first = open(Some("viewer-1"));
    assert_eq!(open(Some("viewer-1")), first, "the same tab again");
    assert_eq!(f.ids().len(), before + 1, "one tab for one open");
    assert_ne!(open(Some("viewer-2")), first);
    assert_ne!(open(None), first);
    assert_eq!(f.ids().len(), before + 3);
}

/// The host's agent tab in `cwd` with Claude session `session`, and its transcript in a Claude folder of the test's own.
fn host_agent_tab(
    f: &Fx,
    cwd: &str,
    session: &str,
    lines: &[String],
) -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    f.ctl
        .borrow_mut()
        .set_convo_claude_dir(Some(dir.path().to_path_buf()));
    let path = flare_convo::transcript_path_in(dir.path(), cwd, session).expect("a path");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap(); // sync-ok: test code only
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(&path, text).unwrap(); // sync-ok: test code only
    let id = f.tabs(&[cwd]).remove(0);
    f.say(
        cwd,
        &format!("\x1b]777;flare-agent;state=working;session={session}\x07"),
    );
    f.pump_until("the session", |f| {
        f.ctl.borrow().hub().claude_session(&id) == Some(session)
    });
    (id, dir)
}

/// A viewer's `remote.resize` of `tab`, from the machine `viewer`.
fn viewer_sizes(f: &Fx, tab: &str, viewer: Option<&str>) {
    ask(
        f,
        CtlOp::RemoteResize {
            tab: tab.to_string(),
            cols: 61,
            rows: 19,
            viewer: viewer.map(str::to_string),
        },
    )
    .expect("the resize");
}

/// A viewer's `remote.input` of one letter into `tab` (rule 1212: the viewer typed last).
fn viewer_types(f: &Fx, tab: &str) {
    ask(
        f,
        CtlOp::RemoteInput {
            tab: tab.to_string(),
            intents: vec![flare_term::handle::RemoteIntent::Text("m".into())],
        },
    )
    .expect("the input");
}

fn size_of(f: &Fx, tab: &str) -> (usize, usize) {
    f.ctl.borrow().handle_of(tab).expect("a handle").size()
}

/// Rule 1211, at every shipped scale: while a remote viewer sizes an agent tab of this host, the host's own window shows
/// the tab's conversation in the fitted view (the same view a viewer shows, rule 1193) with a line naming the viewer;
/// a key typed here takes the size back, the terminal returns at this window's size and the key reaches the shell; a
/// click on the view does the same. A plain shell a viewer sizes keeps its terminal.
#[test]
fn the_hosts_own_window_shows_the_fitted_view_while_a_viewer_sizes_an_agent_tab() {
    const SID: &str = "66666666-6666-4666-8666-666666666666";
    for s in SCALES {
        let f = Fx::with(
            Opts {
                scale: s,
                ..Opts::default()
            },
            |_| {},
        );
        serving(&f);
        let said = |role: &str, text: &str| {
            serde_json::json!({"type": role, "message": {"role": role, "content": text}})
                .to_string()
        };
        let (id, _claude) = host_agent_tab(
            &f,
            "C:\\work",
            SID,
            &[
                said("user", "Fix the build"),
                said("assistant", "Done: the test passes now."),
            ],
        );
        let own = size_of(&f, &id);
        assert!(!f.h.comp.get_rconvo_open(), "{s}: this window sizes it");
        // The Mac typed last: the tab takes its size, and this window shows the conversation instead of a squeezed
        // terminal, naming the Mac.
        viewer_sizes(&f, &id, Some("Mac Mini"));
        assert_eq!(size_of(&f, &id), (61, 19), "{s}");
        viewer_types(&f, &id);
        until(&f, "the host's fitted view", |f| f.h.comp.get_rconvo_open());
        assert_eq!(f.h.comp.get_rconvo_machine().as_str(), "Mac Mini", "{s}");
        let lines = f.h.comp.get_rconvo_lines();
        assert_eq!(lines.row_count(), 2, "{s}");
        assert_eq!(lines.row_data(0).unwrap().role.as_str(), "user");
        assert_eq!(
            lines.row_data(1).unwrap().text.as_str(),
            "Done: the test passes now."
        );
        assert!(f.h.comp.get_rconvo_at_foot(), "{s}: the newest line shows");
        let col = f.h.comp.get_rconvo_column_w();
        assert!(col > 0.0 && col < 1000.0, "{s}: column {col}");
        // A key typed here wins: the view goes, the terminal is back at this window's size, the key reaches the shell.
        let shell = f.shell("C:\\work");
        assert_eq!(
            f.key(KeyEvent::new("y", "KeyY").text("y")),
            KeyOutcome::Pty,
            "{s}"
        );
        assert!(!f.h.comp.get_rconvo_open(), "{s}");
        let st = ask(&f, CtlOp::RemoteStatus).expect("the status");
        assert_eq!(st["tabs"][0]["sized"], "host", "{s}");
        assert_eq!(size_of(&f, &id), own, "{s}: this window's size again");
        f.pump_until("the key on the shell", |_| {
            String::from_utf8_lossy(&shell.written()).contains('y')
        });
        // The viewer types again; a click on the view takes it back the same way.
        viewer_sizes(&f, &id, Some("Mac Mini"));
        viewer_types(&f, &id);
        until(&f, "the view again", |f| f.h.comp.get_rconvo_open());
        f.h.click(500.0, 400.0);
        f.pump();
        assert!(!f.h.comp.get_rconvo_open(), "{s}: the click took it back");
        assert_eq!(size_of(&f, &id), own, "{s}");
        // The viewer goes: the size is this window's and nothing shows.
        viewer_sizes(&f, &id, None);
        viewer_types(&f, &id);
        until(&f, "the view, no machine named", |f| {
            f.h.comp.get_rconvo_open()
        });
        assert_eq!(f.h.comp.get_rconvo_machine().as_str(), "", "{s}");
        ask(&f, CtlOp::RemoteRelease { tab: id.clone() }).expect("the release");
        assert!(!f.h.comp.get_rconvo_open(), "{s}");
        assert_eq!(size_of(&f, &id), own, "{s}");
        // A plain shell a viewer sizes keeps its terminal at the viewer's size.
        let plain = f.tabs(&["C:\\plain"]).remove(0);
        f.visit(1);
        viewer_sizes(&f, &plain, Some("Mac Mini"));
        viewer_types(&f, &plain);
        for _ in 0..5 {
            f.step(HOST_EVERY);
        }
        assert!(!f.h.comp.get_rconvo_open(), "{s}: a plain shell");
        assert_eq!(size_of(&f, &plain), (61, 19), "{s}");
        // Back on the agent tab, sized by the viewer: its view; another tab in front shows its own terminal.
        viewer_sizes(&f, &id, Some("Mac Mini"));
        viewer_types(&f, &id);
        f.visit(0);
        until(&f, "the agent tab's view", |f| f.h.comp.get_rconvo_open());
        f.visit(1);
        assert!(!f.h.comp.get_rconvo_open(), "{s}");
    }
}

/// Rule 1212: a viewer's size alone never puts the host's fitted view up. An attach, a reconnect after a release, a
/// refit or a tab switch on the viewer sends `remote.resize` while this window typed last: the tab takes the viewer's
/// size and this window keeps its live terminal. The view stands only once the viewer types, and a key typed here
/// takes it back for good: the viewer's next size alone leaves the terminal here.
#[test]
fn a_viewers_size_alone_never_puts_the_hosts_fitted_view_up() {
    const SID: &str = "77777777-7777-4777-8777-777777777777";
    let f = Fx::new();
    serving(&f);
    let said = |role: &str, text: &str| {
        serde_json::json!({"type": role, "message": {"role": role, "content": text}}).to_string()
    };
    let (id, _claude) = host_agent_tab(
        &f,
        "C:\\work",
        SID,
        &[said("user", "Fix the build"), said("assistant", "Done.")],
    );
    let quiet = |f: &Fx, what: &str| {
        for _ in 0..5 {
            f.step(HOST_EVERY);
        }
        assert!(!f.h.comp.get_rconvo_open(), "{what}");
    };
    // The viewer attaches, refits and comes back after a release: sizes only.
    viewer_sizes(&f, &id, Some("Mac Mini"));
    assert_eq!(size_of(&f, &id), (61, 19), "the viewer's size");
    quiet(&f, "an attach is not typing");
    viewer_sizes(&f, &id, Some("Mac Mini"));
    quiet(&f, "a refit is not typing");
    ask(&f, CtlOp::RemoteRelease { tab: id.clone() }).expect("the release");
    viewer_sizes(&f, &id, Some("Mac Mini"));
    quiet(&f, "a reconnect is not typing");
    // The viewer types: the view stands.
    viewer_types(&f, &id);
    until(&f, "the host's fitted view", |f| f.h.comp.get_rconvo_open());
    // A key here takes it back, and the viewer's next refit alone leaves the terminal here.
    assert_eq!(f.key(KeyEvent::new("y", "KeyY").text("y")), KeyOutcome::Pty);
    assert!(!f.h.comp.get_rconvo_open());
    viewer_sizes(&f, &id, Some("Mac Mini"));
    quiet(&f, "this window typed last");
}

/// The host's read interval, for the steps of the window clock.
const HOST_EVERY: u64 = crate::appctl::HOST_CONVO_EVERY_MS;
