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
